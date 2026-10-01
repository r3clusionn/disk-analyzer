//! Parallel directory scan.
//!
//! Each directory is read on one thread, its subdirectories are handed to the rayon pool, and the
//! results are joined into a [`Node`] tree. Three things are handled on purpose:
//!
//! - **Hard links** are identified by (volume, file index) and each file is counted once.
//! - **Symlinks and junctions** are not followed unless asked, and when they are, a directory that
//!   is its own ancestor is detected and skipped, so loops terminate.
//! - **Unreadable entries** are counted and the first few messages are kept; the scan carries on.

use crate::tree::{dedupe_hard_links, FileId, Node};
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone, Debug)]
pub struct Options {
    /// Follow symlinks and junctions to directories (loops are detected).
    pub follow: bool,
    /// Count allocated bytes instead of file length (compression, sparse files, block rounding).
    pub disk: bool,
    /// Detect hard links so each file counts once. On Windows this opens each file briefly.
    pub hard_links: bool,
    /// Worker threads; 0 means one per CPU.
    pub threads: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options { follow: false, disk: false, hard_links: true, threads: 0 }
    }
}

#[derive(Debug)]
pub struct Scan {
    pub root: Node,
    /// The first few errors, as `path: message`.
    pub errors: Vec<String>,
    /// Paths skipped because following them would loop.
    pub loops: Vec<String>,
    /// Extra paths of hard-linked files whose bytes were counted once elsewhere.
    pub shared_links: u64,
}

const MAX_MESSAGES: usize = 20;

struct Ctx<'a> {
    opts: &'a Options,
    errors: Mutex<Vec<String>>,
    loops: Mutex<Vec<String>>,
}

impl Ctx<'_> {
    fn error(&self, path: &Path, e: &dyn std::fmt::Display) {
        let mut v = self.errors.lock().unwrap();
        if v.len() < MAX_MESSAGES {
            v.push(format!("{}: {e}", path.display()));
        }
    }
}

pub fn scan(root: &Path, opts: &Options) -> std::io::Result<Scan> {
    let meta = fs::metadata(root)?;
    let name = root.file_name().map_or_else(|| root.display().to_string(), |n| n.to_string_lossy().into_owned());
    let ctx = Ctx { opts, errors: Mutex::new(Vec::new()), loops: Mutex::new(Vec::new()) };
    let run = || {
        if meta.is_dir() {
            let ancestors = if opts.follow { dir_id(root).into_iter().collect() } else { Vec::new() };
            walk(root, name.clone(), &ctx, &ancestors)
        } else {
            Node::file(name.clone(), file_size(root, &meta, opts.disk), None)
        }
    };
    let mut node = if opts.threads == 0 {
        run()
    } else {
        rayon::ThreadPoolBuilder::new().num_threads(opts.threads).build().map_err(std::io::Error::other)?.install(run)
    };
    let shared_links = if opts.hard_links { dedupe_hard_links(&mut node) } else { 0 };
    // A directory's `files` already counts every path, shared or not.
    Ok(Scan { root: node, errors: ctx.errors.into_inner().unwrap(), loops: ctx.loops.into_inner().unwrap(), shared_links })
}

fn walk(dir: &Path, name: String, ctx: &Ctx, ancestors: &[FileId]) -> Node {
    let mut node = Node::dir(name);
    let rd = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) => {
            ctx.error(dir, &e);
            node.errors += 1;
            return node;
        }
    };
    let mut subdirs: Vec<(PathBuf, String, Vec<FileId>)> = Vec::new();
    for entry in rd {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                ctx.error(dir, &e);
                node.errors += 1;
                continue;
            }
        };
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let md = match entry.metadata() {
            Ok(m) => m,
            Err(e) => {
                ctx.error(&path, &e);
                node.errors += 1;
                continue;
            }
        };
        let is_link = is_link(&entry, &md);
        if is_link && !ctx.opts.follow {
            node.children.push(Node::link(name));
            continue;
        }
        // With --follow a link is resolved to its target's type; otherwise `md` is the entry itself.
        let md = if is_link {
            match fs::metadata(&path) {
                Ok(m) => m,
                Err(e) => {
                    ctx.error(&path, &e);
                    node.errors += 1;
                    node.children.push(Node::link(name));
                    continue;
                }
            }
        } else {
            md
        };
        if md.is_dir() {
            if ctx.opts.follow {
                match dir_id(&path) {
                    Some(id) if ancestors.contains(&id) => {
                        ctx.loops.lock().unwrap().push(path.display().to_string());
                        node.children.push(Node::link(name));
                        continue;
                    }
                    Some(id) => {
                        let mut a = ancestors.to_vec();
                        a.push(id);
                        subdirs.push((path, name, a));
                        continue;
                    }
                    None => {}
                }
            }
            subdirs.push((path, name, ancestors.to_vec()));
        } else {
            let id = if ctx.opts.hard_links { hard_link_id(&path, &md) } else { None };
            node.children.push(Node::file(name, file_size(&path, &md, ctx.opts.disk), id));
        }
    }
    let dirs: Vec<Node> = subdirs.par_iter().map(|(p, n, a)| walk(p, n.clone(), ctx, a)).collect();
    node.children.extend(dirs);
    node.finish();
    node
}

#[cfg(windows)]
fn is_link(entry: &fs::DirEntry, md: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    let _ = entry;
    // FILE_ATTRIBUTE_REPARSE_POINT covers symlinks, junctions and cloud placeholders.
    md.file_attributes() & 0x400 != 0 && (md.file_type().is_symlink() || md.is_dir())
}

#[cfg(not(windows))]
fn is_link(entry: &fs::DirEntry, md: &fs::Metadata) -> bool {
    let _ = md;
    entry.file_type().is_ok_and(|t| t.is_symlink())
}

#[cfg(windows)]
fn file_size(path: &Path, md: &fs::Metadata, disk: bool) -> u64 {
    if disk {
        if let Some(n) = win::allocated_size(path) {
            return n;
        }
    }
    md.len()
}

#[cfg(unix)]
fn file_size(_path: &Path, md: &fs::Metadata, disk: bool) -> u64 {
    use std::os::unix::fs::MetadataExt;
    if disk { md.blocks() * 512 } else { md.len() }
}

#[cfg(windows)]
fn hard_link_id(path: &Path, _md: &fs::Metadata) -> Option<FileId> {
    let info = win::file_info(path)?;
    (info.links > 1).then_some((info.volume as u64, info.index))
}

#[cfg(unix)]
fn hard_link_id(_path: &Path, md: &fs::Metadata) -> Option<FileId> {
    use std::os::unix::fs::MetadataExt;
    (md.nlink() > 1).then_some((md.dev(), md.ino()))
}

#[cfg(windows)]
fn dir_id(path: &Path) -> Option<FileId> {
    win::file_info(path).map(|i| (i.volume as u64, i.index))
}

#[cfg(unix)]
fn dir_id(path: &Path) -> Option<FileId> {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path).ok().map(|m| (m.dev(), m.ino()))
}

#[cfg(windows)]
mod win {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, GetCompressedFileSizeW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, INVALID_FILE_SIZE, OPEN_EXISTING,
    };

    pub struct Info {
        pub links: u32,
        pub volume: u32,
        pub index: u64,
    }

    /// Wide, NUL-terminated path with the `\\?\` prefix so paths over 260 characters work.
    fn wide(path: &Path) -> Vec<u16> {
        let mut w: Vec<u16> = Vec::new();
        let s = path.as_os_str();
        let text = s.to_string_lossy();
        if path.is_absolute() && !text.starts_with(r"\\?\") && !text.starts_with(r"\\") {
            w.extend(r"\\?\".encode_utf16());
        }
        // The extended-length form does not normalise separators, so make them all backslashes.
        w.extend(s.encode_wide().map(|c| if c == u16::from(b'/') { u16::from(b'\\') } else { c }));
        w.push(0);
        w
    }

    /// Link count and file identity. Needs only attribute access, so it works on files that are
    /// locked or unreadable.
    pub fn file_info(path: &Path) -> Option<Info> {
        let w = wide(path);
        unsafe {
            let h = CreateFileW(w.as_ptr(), 0x80, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, std::ptr::null(), OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT, std::ptr::null_mut());
            if h == INVALID_HANDLE_VALUE {
                return None;
            }
            let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
            let ok = GetFileInformationByHandle(h, &mut info);
            CloseHandle(h);
            (ok != 0).then_some(Info { links: info.nNumberOfLinks, volume: info.dwVolumeSerialNumber, index: ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64 })
        }
    }

    /// Bytes actually allocated for a compressed or sparse file; the plain length otherwise.
    pub fn allocated_size(path: &Path) -> Option<u64> {
        let w = wide(path);
        unsafe {
            let mut high = 0u32;
            let low = GetCompressedFileSizeW(w.as_ptr(), &mut high);
            if low == INVALID_FILE_SIZE && windows_sys::Win32::Foundation::GetLastError() != 0 {
                return None;
            }
            Some(((high as u64) << 32) | low as u64)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::Kind;
    use std::io::Write;

    fn write(p: &Path, n: usize) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::File::create(p).unwrap().write_all(&vec![b'x'; n]).unwrap();
    }

    #[test]
    fn sizes_and_counts_add_up() {
        let t = tempfile::tempdir().unwrap();
        write(&t.path().join("a/one.txt"), 100);
        write(&t.path().join("a/b/two.txt"), 2000);
        write(&t.path().join("three.bin"), 30);
        let s = scan(t.path(), &Options::default()).unwrap();
        assert_eq!((s.root.size, s.root.files, s.root.errors), (2130, 3, 0));
        let a = s.root.children.iter().find(|c| c.name == "a").unwrap();
        assert_eq!((a.size, a.files), (2100, 2));
        assert!(s.errors.is_empty());
    }

    #[test]
    fn single_file_and_missing_path() {
        let t = tempfile::tempdir().unwrap();
        write(&t.path().join("f.dat"), 123);
        let s = scan(&t.path().join("f.dat"), &Options::default()).unwrap();
        assert_eq!((s.root.kind, s.root.size, s.root.files), (Kind::File, 123, 1));
        assert!(scan(&t.path().join("nope"), &Options::default()).is_err());
    }

    #[test]
    fn same_result_for_any_thread_count() {
        let t = tempfile::tempdir().unwrap();
        for i in 0..40 {
            write(&t.path().join(format!("d{}/e{}/f{}.txt", i % 5, i % 3, i)), 10 + i);
        }
        let one = scan(t.path(), &Options { threads: 1, ..Options::default() }).unwrap();
        let many = scan(t.path(), &Options { threads: 8, ..Options::default() }).unwrap();
        assert_eq!(serde_json::to_string(&one.root).unwrap(), serde_json::to_string(&many.root).unwrap());
    }

    #[test]
    fn hard_links_are_counted_once() {
        let t = tempfile::tempdir().unwrap();
        write(&t.path().join("a/original.bin"), 5000);
        if fs::hard_link(t.path().join("a/original.bin"), t.path().join("b_link.bin")).is_err() {
            eprintln!("skipped: no hard link support here");
            return;
        }
        write(&t.path().join("c/other.bin"), 10);
        let s = scan(t.path(), &Options::default()).unwrap();
        assert_eq!(s.root.size, 5010, "5000 once, not twice");
        assert_eq!(s.shared_links, 1);
        assert_eq!(s.root.files, 3, "paths are all counted as files");
        let off = scan(t.path(), &Options { hard_links: false, ..Options::default() }).unwrap();
        assert_eq!(off.root.size, 10_010);
    }

    #[test]
    fn symlinks_are_not_followed_by_default() {
        let t = tempfile::tempdir().unwrap();
        write(&t.path().join("real/file.bin"), 1000);
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(t.path().join("real"), t.path().join("alias")).is_ok();
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(t.path().join("real"), t.path().join("alias")).is_ok();
        if !made {
            eprintln!("skipped: this account cannot create symlinks");
            return;
        }
        let s = scan(t.path(), &Options::default()).unwrap();
        assert_eq!(s.root.size, 1000, "the target is counted once, through its real path");
        assert!(s.root.children.iter().any(|c| c.name == "alias" && c.kind == Kind::Link));
        let followed = scan(t.path(), &Options { follow: true, ..Options::default() }).unwrap();
        assert_eq!(followed.root.size, 2000, "following counts the target again through the link");
    }

    #[test]
    fn symlink_loops_terminate_when_following() {
        let t = tempfile::tempdir().unwrap();
        write(&t.path().join("a/file.bin"), 77);
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(t.path(), t.path().join("a/back")).is_ok();
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(t.path(), t.path().join("a/back")).is_ok();
        if !made {
            eprintln!("skipped: this account cannot create symlinks");
            return;
        }
        let s = scan(t.path(), &Options { follow: true, ..Options::default() }).unwrap();
        assert_eq!(s.root.size, 77);
        assert_eq!(s.loops.len(), 1, "{:?}", s.loops);
    }

    #[test]
    fn unreadable_directories_are_counted_not_fatal() {
        // A path that disappears between listing and reading behaves the same way; here the
        // simplest portable trigger is a directory entry removed while scanning is not possible,
        // so check the accounting through the error path of a nonexistent child instead.
        let t = tempfile::tempdir().unwrap();
        write(&t.path().join("ok/file.bin"), 10);
        let ctx = Ctx { opts: &Options::default(), errors: Mutex::new(Vec::new()), loops: Mutex::new(Vec::new()) };
        let n = walk(&t.path().join("does-not-exist"), "x".into(), &ctx, &[]);
        assert_eq!((n.errors, n.size), (1, 0));
        assert_eq!(ctx.errors.lock().unwrap().len(), 1);
    }
}
