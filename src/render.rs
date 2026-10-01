//! Text rendering of a scanned tree.

use crate::tree::{Kind, Node, SortBy};
use std::fmt::Write;

#[derive(Clone, Debug)]
pub struct View {
    /// Levels below the root to print (0 prints only the root).
    pub depth: usize,
    /// Entries printed per directory; the rest are summarized on one line.
    pub top: usize,
    pub sort: SortBy,
    /// Entries smaller than this many bytes are folded into the summary line.
    pub min_size: u64,
    pub ascii: bool,
}

impl Default for View {
    fn default() -> Self {
        View { depth: 2, top: 10, sort: SortBy::Size, min_size: 0, ascii: false }
    }
}

pub fn human(n: u64) -> String {
    const U: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let (mut v, mut i) = (n as f64, 0);
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{n} B") } else { format!("{v:.1} {}", U[i]) }
}

pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `1 file`, `2,048 files`.
pub fn files_label(n: u64) -> String {
    if n == 1 { "1 file".into() } else { format!("{} files", thousands(n)) }
}

/// A bar `width` cells wide filled to `frac`, in eighths when Unicode is allowed.
pub fn bar(frac: f64, width: usize, ascii: bool) -> String {
    let frac = frac.clamp(0.0, 1.0);
    if ascii {
        let n = (frac * width as f64).round() as usize;
        return format!("{}{}", "#".repeat(n), ".".repeat(width - n));
    }
    const PARTIAL: [char; 8] = [' ', '\u{258f}', '\u{258e}', '\u{258d}', '\u{258c}', '\u{258b}', '\u{258a}', '\u{2589}'];
    let eighths = (frac * width as f64 * 8.0).round() as usize;
    let (full, part) = (eighths / 8, eighths % 8);
    let mut s = "\u{2588}".repeat(full);
    if full < width {
        s.push(PARTIAL[part]);
        s.push_str(&" ".repeat(width - full - 1));
    }
    s
}

fn sorted(n: &Node, by: SortBy) -> Vec<&Node> {
    let mut v: Vec<&Node> = n.children.iter().collect();
    match by {
        SortBy::Size => v.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name))),
        SortBy::Files => v.sort_by(|a, b| b.files.cmp(&a.files).then_with(|| a.name.cmp(&b.name))),
        SortBy::Name => v.sort_by(|a, b| a.name.cmp(&b.name)),
    }
    v
}

pub fn label(n: &Node) -> String {
    match n.kind {
        Kind::Dir => format!("{}/", n.name),
        Kind::Link => format!("{} (link, not followed)", n.name),
        Kind::File if n.shared => format!("{} (hard link, counted elsewhere)", n.name),
        Kind::File => n.name.clone(),
    }
}

pub fn render_tree(root: &Node, view: &View) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{:>10}  {}{}", human(root.size), label(root), if root.kind == Kind::Dir { format!("  ({})", files_label(root.files)) } else { String::new() });
    node_lines(root, view, 1, &mut out);
    out
}

fn node_lines(parent: &Node, view: &View, level: usize, out: &mut String) {
    if level > view.depth || parent.kind != Kind::Dir {
        return;
    }
    let kids = sorted(parent, view.sort);
    let shown: Vec<&&Node> = kids.iter().filter(|c| c.size >= view.min_size).take(view.top).collect();
    let (mut hidden_n, mut hidden_size) = (0u64, 0u64);
    for c in &kids {
        if !shown.iter().any(|s| std::ptr::eq(**s, *c)) {
            hidden_n += 1;
            hidden_size += c.size;
        }
    }
    let total = parent.size.max(1) as f64;
    for c in shown {
        let pct = c.size as f64 * 100.0 / total;
        let _ = writeln!(
            out,
            "{:>10} {:>5.1}% {}  {}{}{}",
            human(c.size),
            pct,
            bar(c.size as f64 / total, 12, view.ascii),
            "  ".repeat(level - 1),
            label(c),
            if c.kind == Kind::Dir { format!("  ({})", files_label(c.files)) } else { String::new() }
        );
        node_lines(c, view, level + 1, out);
    }
    if hidden_n > 0 {
        let _ = writeln!(out, "{:>10} {:>5.1}%  {}  ... {} more entries", human(hidden_size), hidden_size as f64 * 100.0 / total, "  ".repeat(level - 1), thousands(hidden_n));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Node {
        let mut root = Node::dir("root".into());
        let mut big = Node::dir("big".into());
        big.children = vec![Node::file("x.bin".into(), 700, None), Node::file("y.bin".into(), 100, None)];
        big.finish();
        root.children = vec![big, Node::file("small.txt".into(), 100, None), Node::file("tiny.txt".into(), 5, None)];
        root.finish();
        root
    }

    #[test]
    fn number_formatting() {
        assert_eq!(human(0), "0 B");
        assert_eq!(human(1023), "1023 B");
        assert_eq!(human(1536), "1.5 KiB");
        assert_eq!(human(5 * 1024 * 1024 * 1024), "5.0 GiB");
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!((files_label(1).as_str(), files_label(2).as_str()), ("1 file", "2 files"));
    }

    #[test]
    fn bars_have_constant_width() {
        for f in [0.0, 0.01, 0.33, 0.5, 0.999, 1.0, 2.0, -1.0] {
            assert_eq!(bar(f, 10, false).chars().count(), 10, "{f}");
            assert_eq!(bar(f, 10, true).chars().count(), 10, "{f}");
        }
        assert_eq!(bar(0.5, 10, true), "#####.....");
    }

    #[test]
    fn tree_output_is_sorted_with_percentages() {
        let out = render_tree(&tree(), &View { ascii: true, ..View::default() });
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].contains("905 B") && lines[0].contains("root/") && lines[0].contains("(4 files)"), "{out}");
        assert!(lines[1].contains("800 B") && lines[1].contains("88.4%") && lines[1].contains("big/"), "{out}");
        assert!(lines[2].contains("700 B") && lines[2].contains("x.bin"), "{out}");
        assert!(lines[2].contains("87.5%"), "percent is of the parent");
    }

    #[test]
    fn top_depth_and_min_size_fold_the_rest() {
        let out = render_tree(&tree(), &View { top: 1, depth: 1, ascii: true, ..View::default() });
        assert!(out.contains("big/") && !out.contains("x.bin") && out.contains("... 2 more entries"), "{out}");
        let out = render_tree(&tree(), &View { min_size: 50, depth: 1, ascii: true, ..View::default() });
        assert!(!out.contains("tiny.txt") && out.contains("... 1 more entries"), "{out}");
        let out = render_tree(&tree(), &View { depth: 0, ..View::default() });
        assert_eq!(out.lines().count(), 1);
    }
}
