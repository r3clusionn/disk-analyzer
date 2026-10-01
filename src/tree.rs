//! The scanned tree and what can be done with it that needs no filesystem: sorting, totals,
//! hard link accounting and listing the largest files.

use serde::Serialize;
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    File,
    Dir,
    /// A symlink or junction that was not followed. Its own size is counted, its target is not.
    Link,
}

/// Identity of a file independent of its path: (volume or device, file index or inode).
pub type FileId = (u64, u64);

#[derive(Clone, Debug, Serialize)]
pub struct Node {
    pub name: String,
    pub kind: Kind,
    /// Bytes: the file itself, or everything below a directory.
    pub size: u64,
    /// Files below a directory (1 for a file).
    pub files: u64,
    /// Entries that could not be read in or below this node.
    pub errors: u64,
    /// For a file with several hard links: true when this path's bytes are counted elsewhere.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub shared: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,
    #[serde(skip)]
    pub link_id: Option<FileId>,
}

impl Node {
    pub fn file(name: String, size: u64, link_id: Option<FileId>) -> Node {
        Node { name, kind: Kind::File, size, files: 1, errors: 0, shared: false, children: Vec::new(), link_id }
    }

    pub fn link(name: String) -> Node {
        Node { name, kind: Kind::Link, size: 0, files: 0, errors: 0, shared: false, children: Vec::new(), link_id: None }
    }

    pub fn dir(name: String) -> Node {
        Node { name, kind: Kind::Dir, size: 0, files: 0, errors: 0, shared: false, children: Vec::new(), link_id: None }
    }

    /// Rolls the children's totals into this directory (adding to any errors of its own) and
    /// orders them by name, so a scan produces the same tree however the threads interleave.
    /// Call once, after all children are added.
    pub fn finish(&mut self) {
        self.children.sort_by(|a, b| a.name.cmp(&b.name));
        if self.kind == Kind::Dir {
            self.size = self.children.iter().map(|c| c.size).sum();
            self.files = self.children.iter().map(|c| c.files).sum();
            self.errors += self.children.iter().map(|c| c.errors).sum::<u64>();
        }
    }

    pub fn sort_children(&mut self, by: SortBy) {
        match by {
            SortBy::Size => self.children.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name))),
            SortBy::Files => self.children.sort_by(|a, b| b.files.cmp(&a.files).then_with(|| a.name.cmp(&b.name))),
            SortBy::Name => self.children.sort_by(|a, b| a.name.cmp(&b.name)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortBy {
    Size,
    Files,
    Name,
}

/// A copy limited to `depth` levels with at most `top` children per directory, for output.
/// Children smaller than `min_size` are dropped, and the order follows `by`.
pub fn pruned(n: &Node, depth: usize, top: usize, min_size: u64, by: SortBy) -> Node {
    let mut copy = Node { children: Vec::new(), ..n.clone() };
    if depth > 0 {
        let mut kids: Vec<&Node> = n.children.iter().filter(|c| c.size >= min_size).collect();
        match by {
            SortBy::Size => kids.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name))),
            SortBy::Files => kids.sort_by(|a, b| b.files.cmp(&a.files).then_with(|| a.name.cmp(&b.name))),
            SortBy::Name => kids.sort_by(|a, b| a.name.cmp(&b.name)),
        }
        copy.children = kids.into_iter().take(top).map(|c| pruned(c, depth - 1, top, min_size, by)).collect();
    }
    copy
}

/// Counts each hard-linked file once. Paths are visited in tree order (children sorted by name),
/// the first path to reach a file id keeps its bytes and later ones are marked `shared` with size
/// zero, then every directory total is recomputed. Deterministic regardless of scan order.
pub fn dedupe_hard_links(root: &mut Node) -> u64 {
    fn walk(n: &mut Node, seen: &mut HashSet<FileId>, dups: &mut u64) {
        if n.kind == Kind::File {
            if let Some(id) = n.link_id {
                if !seen.insert(id) {
                    n.size = 0;
                    n.shared = true;
                    *dups += 1;
                }
            }
            return;
        }
        for c in &mut n.children {
            walk(c, seen, dups);
        }
        n.size = n.children.iter().map(|c| c.size).sum();
    }
    let (mut seen, mut dups) = (HashSet::new(), 0);
    walk(root, &mut seen, &mut dups);
    dups
}

/// The `n` largest files below `root`, as (size, path relative to root's parent).
pub fn largest_files(root: &Node, n: usize) -> Vec<(u64, String)> {
    fn walk(node: &Node, path: &mut Vec<String>, out: &mut Vec<(u64, String)>, n: usize) {
        path.push(node.name.clone());
        if node.kind == Kind::File {
            if node.size > 0 {
                out.push((node.size, path.join("/")));
                if out.len() > 4 * n.max(16) {
                    out.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
                    out.truncate(n);
                }
            }
        } else {
            for c in &node.children {
                walk(c, path, out, n);
            }
        }
        path.pop();
    }
    let mut out = Vec::new();
    walk(root, &mut Vec::new(), &mut out, n);
    out.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    out.truncate(n);
    out
}

/// Total bytes and file count per extension (lowercase, without the dot; empty for none).
pub fn by_extension(root: &Node) -> Vec<(String, u64, u64)> {
    use std::collections::HashMap;
    fn walk(n: &Node, m: &mut HashMap<String, (u64, u64)>) {
        if n.kind == Kind::File {
            let ext = match n.name.rsplit_once('.') {
                Some((stem, e)) if !stem.is_empty() => e.to_ascii_lowercase(),
                _ => String::new(),
            };
            let e = m.entry(ext).or_default();
            e.0 += n.size;
            e.1 += 1;
        }
        for c in &n.children {
            walk(c, m);
        }
    }
    let mut m = HashMap::new();
    walk(root, &mut m);
    let mut v: Vec<(String, u64, u64)> = m.into_iter().map(|(k, (s, c))| (k, s, c)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str, kids: Vec<Node>) -> Node {
        let mut d = Node::dir(name.into());
        d.children = kids;
        d.finish();
        d
    }

    fn f(name: &str, size: u64) -> Node {
        Node::file(name.into(), size, None)
    }

    #[test]
    fn totals_and_name_order() {
        let d = dir("r", vec![f("b", 5), dir("a", vec![f("x", 10), f("y", 20)]), f("c", 1)]);
        assert_eq!((d.size, d.files), (36, 4));
        assert_eq!(d.children.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
        assert_eq!(d.children[0].size, 30);
    }

    #[test]
    fn sorting_modes() {
        let mut d = dir("r", vec![f("small", 1), f("big", 100), dir("many", vec![f("1", 2), f("2", 2), f("3", 2)])]);
        d.sort_children(SortBy::Size);
        assert_eq!(d.children[0].name, "big");
        d.sort_children(SortBy::Files);
        assert_eq!(d.children[0].name, "many");
        d.sort_children(SortBy::Name);
        assert_eq!(d.children[0].name, "big");
    }

    #[test]
    fn hard_links_count_once_in_tree_order() {
        let id = Some((1, 77));
        let mut d = dir(
            "r",
            vec![
                dir("a", vec![Node::file("one".into(), 100, id)]),
                dir("b", vec![Node::file("two".into(), 100, id), f("other", 7)]),
            ],
        );
        assert_eq!(d.size, 207);
        let dups = dedupe_hard_links(&mut d);
        assert_eq!(dups, 1);
        assert_eq!(d.size, 107);
        assert_eq!(d.children[0].size, 100, "the first path in name order keeps the bytes");
        assert_eq!(d.children[1].size, 7);
        let two = d.children[1].children.iter().find(|c| c.name == "two").unwrap();
        assert!(two.shared && two.size == 0);
        // Counting again changes nothing.
        assert_eq!(dedupe_hard_links(&mut d), 1);
        assert_eq!(d.size, 107);
    }

    #[test]
    fn largest_files_and_extensions() {
        let d = dir("r", vec![f("a.rs", 5), f("b.RS", 7), f("c.md", 100), f(".hidden", 3), dir("s", vec![f("d.rs", 1)])]);
        let top = largest_files(&d, 2);
        assert_eq!(top, vec![(100, "r/c.md".to_string()), (7, "r/b.RS".to_string())]);
        let ext = by_extension(&d);
        assert_eq!(ext[0], ("md".to_string(), 100, 1));
        assert_eq!(ext[1], ("rs".to_string(), 13, 3));
        assert_eq!(ext[2], (String::new(), 3, 1), "a leading dot alone is a name, not an extension");
    }
}
