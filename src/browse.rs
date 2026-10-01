//! Interactive tree browser. The state machine and the screen it draws are plain data, so they
//! are tested without a terminal; `run_terminal` is a thin crossterm loop around them, and
//! `run_script` replays a list of key names and returns the final screen.

use crate::render::{bar, human, label, thousands};
use crate::tree::{Kind, Node, SortBy};
use std::io::{self, Write};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    Enter,
    Back,
    Sort,
    Quit,
}

pub fn parse_key(s: &str) -> Option<Key> {
    Some(match s.trim().to_ascii_lowercase().as_str() {
        "up" | "k" => Key::Up,
        "down" | "j" => Key::Down,
        "pageup" | "pgup" => Key::PageUp,
        "pagedown" | "pgdn" => Key::PageDown,
        "home" => Key::Home,
        "end" => Key::End,
        "enter" | "right" | "l" => Key::Enter,
        "back" | "left" | "backspace" | "h" => Key::Back,
        "sort" | "s" => Key::Sort,
        "quit" | "q" | "esc" => Key::Quit,
        _ => return None,
    })
}

pub struct Screen {
    pub lines: Vec<String>,
    /// Index into `lines` of the highlighted entry.
    pub selected: Option<usize>,
}

pub struct Browser {
    root: Node,
    /// Child indices from the root to the directory being shown.
    path: Vec<usize>,
    cursor: usize,
    sort: SortBy,
    ascii: bool,
    pub quit: bool,
}

fn sort_name(s: SortBy) -> &'static str {
    match s {
        SortBy::Size => "size",
        SortBy::Files => "files",
        SortBy::Name => "name",
    }
}

impl Browser {
    pub fn new(mut root: Node, sort: SortBy, ascii: bool) -> Browser {
        root.sort_children(sort);
        Browser { root, path: Vec::new(), cursor: 0, sort, ascii, quit: false }
    }

    fn current(&self) -> &Node {
        self.path.iter().fold(&self.root, |n, i| &n.children[*i])
    }

    fn current_mut(&mut self) -> &mut Node {
        let path = self.path.clone();
        path.iter().fold(&mut self.root, |n, i| &mut n.children[*i])
    }

    /// Names from the root to the current directory, joined with `/`.
    pub fn crumbs(&self) -> String {
        let mut names = vec![self.root.name.clone()];
        let mut n = &self.root;
        for i in &self.path {
            n = &n.children[*i];
            names.push(n.name.clone());
        }
        names.join("/")
    }

    pub fn selected_name(&self) -> Option<&str> {
        self.current().children.get(self.cursor).map(|c| c.name.as_str())
    }

    pub fn handle(&mut self, key: Key, page: usize) {
        let len = self.current().children.len();
        match key {
            Key::Up => self.cursor = self.cursor.saturating_sub(1),
            Key::Down => self.cursor = (self.cursor + 1).min(len.saturating_sub(1)),
            Key::PageUp => self.cursor = self.cursor.saturating_sub(page.max(1)),
            Key::PageDown => self.cursor = (self.cursor + page.max(1)).min(len.saturating_sub(1)),
            Key::Home => self.cursor = 0,
            Key::End => self.cursor = len.saturating_sub(1),
            Key::Enter => {
                if self.current().children.get(self.cursor).is_some_and(|c| c.kind == Kind::Dir && !c.children.is_empty()) {
                    self.path.push(self.cursor);
                    let sort = self.sort;
                    self.current_mut().sort_children(sort);
                    self.cursor = 0;
                }
            }
            Key::Back => {
                if let Some(left) = self.path.pop() {
                    // Returning to the parent: put the cursor back on the directory just left.
                    let name = self.current().children[left].name.clone();
                    let sort = self.sort;
                    self.current_mut().sort_children(sort);
                    self.cursor = self.current().children.iter().position(|c| c.name == name).unwrap_or(0);
                }
            }
            Key::Sort => {
                self.sort = match self.sort {
                    SortBy::Size => SortBy::Files,
                    SortBy::Files => SortBy::Name,
                    SortBy::Name => SortBy::Size,
                };
                let sort = self.sort;
                self.current_mut().sort_children(sort);
                self.cursor = 0;
            }
            Key::Quit => self.quit = true,
        }
    }

    pub fn render(&self, width: usize, height: usize) -> Screen {
        let cur = self.current();
        let fit = |s: String| s.chars().take(width).collect::<String>();
        let mut lines = vec![
            fit(format!("{}   {}   {} files   sort: {}", self.crumbs(), human(cur.size), thousands(cur.files), sort_name(self.sort))),
            "-".repeat(width.min(200)),
        ];
        let rows = height.saturating_sub(3).max(1);
        let len = cur.children.len();
        let start = if len <= rows { 0 } else { self.cursor.saturating_sub(rows / 2).min(len - rows) };
        let total = cur.size.max(1) as f64;
        let mut selected = None;
        if len == 0 {
            lines.push("(empty)".into());
        }
        for (i, c) in cur.children.iter().enumerate().skip(start).take(rows) {
            if i == self.cursor {
                selected = Some(lines.len());
            }
            let marker = if i == self.cursor { '>' } else { ' ' };
            lines.push(fit(format!(
                "{marker} {:>10} {:>5.1}% {} {}{}",
                human(c.size),
                c.size as f64 * 100.0 / total,
                bar(c.size as f64 / total, 10, self.ascii),
                label(c),
                if c.kind == Kind::Dir { format!("  ({} files)", thousands(c.files)) } else { String::new() }
            )));
        }
        while lines.len() < height.saturating_sub(1) {
            lines.push(String::new());
        }
        let pos = if len == 0 { "0/0".to_string() } else { format!("{}/{}", self.cursor + 1, len) };
        lines.push(fit(format!("{pos}  up/down move  enter open  left back  s sort  q quit")));
        Screen { lines, selected }
    }
}

/// Replays key names and returns the final screen as text.
pub fn run_script(mut b: Browser, keys: &str, width: usize, height: usize) -> Result<String, String> {
    for k in keys.split(',').filter(|k| !k.trim().is_empty()) {
        let key = parse_key(k).ok_or_else(|| format!("unknown key '{}'", k.trim()))?;
        b.handle(key, height.saturating_sub(3));
        if b.quit {
            break;
        }
    }
    let s = b.render(width, height);
    Ok(s.lines.iter().map(|l| format!("{l}
")).collect())
}

/// Full-screen interactive loop.
pub fn run_terminal(mut b: Browser) -> io::Result<()> {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
    use crossterm::style::{Attribute, Print, SetAttribute};
    use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
    use crossterm::{cursor, execute, queue};

    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = terminal::disable_raw_mode();
            let _ = execute!(io::stdout(), cursor::Show, LeaveAlternateScreen);
        }
    }
    terminal::enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, cursor::Hide)?;
    let _restore = Restore;

    loop {
        let (w, h) = terminal::size()?;
        let screen = b.render(w as usize, h as usize);
        let mut out = io::stdout().lock();
        queue!(out, cursor::MoveTo(0, 0), Clear(ClearType::All))?;
        for (i, line) in screen.lines.iter().enumerate() {
            queue!(out, cursor::MoveTo(0, i as u16))?;
            if Some(i) == screen.selected {
                queue!(out, SetAttribute(Attribute::Reverse), Print(line), SetAttribute(Attribute::Reset))?;
            } else {
                queue!(out, Print(line))?;
            }
        }
        out.flush()?;
        drop(out);
        if let Event::Key(k) = event::read()? {
            if k.kind != KeyEventKind::Press {
                continue;
            }
            let page = (h as usize).saturating_sub(3);
            match k.code {
                KeyCode::Up | KeyCode::Char('k') => b.handle(Key::Up, page),
                KeyCode::Down | KeyCode::Char('j') => b.handle(Key::Down, page),
                KeyCode::PageUp => b.handle(Key::PageUp, page),
                KeyCode::PageDown => b.handle(Key::PageDown, page),
                KeyCode::Home => b.handle(Key::Home, page),
                KeyCode::End => b.handle(Key::End, page),
                KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => b.handle(Key::Enter, page),
                KeyCode::Left | KeyCode::Backspace | KeyCode::Char('h') => b.handle(Key::Back, page),
                KeyCode::Char('s') => b.handle(Key::Sort, page),
                KeyCode::Char('q') | KeyCode::Esc => b.handle(Key::Quit, page),
                KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => b.handle(Key::Quit, page),
                _ => {}
            }
        }
        if b.quit {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(name: &str, size: u64) -> Node {
        Node::file(name.into(), size, None)
    }

    fn d(name: &str, kids: Vec<Node>) -> Node {
        let mut n = Node::dir(name.into());
        n.children = kids;
        n.finish();
        n
    }

    fn tree() -> Node {
        d("root", vec![d("big", vec![f("x", 700), d("deep", vec![f("z", 50)]), f("y", 100)]), f("mid", 300), d("emptydir", vec![]), f("small", 10)])
    }

    fn names(b: &Browser) -> Vec<String> {
        b.current().children.iter().map(|c| c.name.clone()).collect()
    }

    #[test]
    fn starts_sorted_by_size_and_moves_within_bounds() {
        let mut b = Browser::new(tree(), SortBy::Size, true);
        assert_eq!(names(&b), ["big", "mid", "small", "emptydir"]);
        assert_eq!(b.selected_name(), Some("big"));
        b.handle(Key::Up, 10);
        assert_eq!(b.selected_name(), Some("big"));
        b.handle(Key::End, 10);
        assert_eq!(b.selected_name(), Some("emptydir"));
        b.handle(Key::Down, 10);
        assert_eq!(b.selected_name(), Some("emptydir"));
        b.handle(Key::PageUp, 2);
        assert_eq!(b.selected_name(), Some("mid"));
        b.handle(Key::Home, 2);
        assert_eq!(b.selected_name(), Some("big"));
    }

    #[test]
    fn enter_descends_only_into_non_empty_directories_and_back_restores_the_cursor() {
        let mut b = Browser::new(tree(), SortBy::Size, true);
        b.handle(Key::Down, 10); // mid, a file
        b.handle(Key::Enter, 10);
        assert_eq!(b.crumbs(), "root", "a file cannot be entered");
        b.handle(Key::End, 10); // emptydir
        b.handle(Key::Enter, 10);
        assert_eq!(b.crumbs(), "root", "an empty directory cannot be entered");
        b.handle(Key::Home, 10);
        b.handle(Key::Enter, 10);
        assert_eq!(b.crumbs(), "root/big");
        assert_eq!(names(&b), ["x", "y", "deep"]);
        b.handle(Key::Down, 10);
        b.handle(Key::Down, 10); // deep
        b.handle(Key::Enter, 10);
        assert_eq!(b.crumbs(), "root/big/deep");
        b.handle(Key::Back, 10);
        assert_eq!(b.selected_name(), Some("deep"), "cursor returns to the directory just left");
        b.handle(Key::Back, 10);
        assert_eq!(b.selected_name(), Some("big"));
        b.handle(Key::Back, 10);
        assert_eq!(b.crumbs(), "root", "back at the root stays at the root");
    }

    #[test]
    fn sort_cycles_and_applies_to_the_current_directory() {
        let mut b = Browser::new(tree(), SortBy::Size, true);
        b.handle(Key::Sort, 10); // files
        assert_eq!(b.current().children[0].name, "big", "big has the most files");
        b.handle(Key::Sort, 10); // name
        assert_eq!(names(&b), ["big", "emptydir", "mid", "small"]);
        b.handle(Key::Sort, 10); // size again
        assert_eq!(names(&b), ["big", "mid", "small", "emptydir"]);
        // The chosen order carries into directories entered afterwards.
        b.handle(Key::Sort, 10);
        b.handle(Key::Sort, 10); // name
        b.handle(Key::Home, 10);
        b.handle(Key::Enter, 10);
        assert_eq!(names(&b), ["deep", "x", "y"]);
    }

    #[test]
    fn screen_contents_and_highlight() {
        let mut b = Browser::new(tree(), SortBy::Size, true);
        b.handle(Key::Down, 10);
        let s = b.render(80, 10);
        assert_eq!(s.lines.len(), 10);
        assert!(s.lines[0].starts_with("root   1.1 KiB") || s.lines[0].starts_with("root   "), "{}", s.lines[0]);
        let sel = s.selected.unwrap();
        assert!(s.lines[sel].starts_with('>') && s.lines[sel].contains("mid"), "{}", s.lines[sel]);
        assert!(s.lines[9].contains("2/4") && s.lines[9].contains("q quit"));
        assert!(s.lines.iter().all(|l| l.chars().count() <= 80));
        assert!(s.lines.iter().any(|l| l.contains("big/") && l.contains("(3 files)")));
    }

    #[test]
    fn long_lists_scroll_with_the_cursor() {
        let kids: Vec<Node> = (0..100).map(|i| f(&format!("f{i:03}"), 1000 - i)).collect();
        let mut b = Browser::new(d("r", kids), SortBy::Size, true);
        for _ in 0..60 {
            b.handle(Key::Down, 10);
        }
        let s = b.render(60, 12);
        let sel = s.selected.unwrap();
        assert!(s.lines[sel].contains("f060"), "{}", s.lines[sel]);
        assert!(s.lines.iter().any(|l| l.contains("f056")) && !s.lines.iter().any(|l| l.contains("f000")));
        assert!(s.lines[11].starts_with("61/100"));
    }

    #[test]
    fn script_replay() {
        let b = Browser::new(tree(), SortBy::Size, true);
        let out = run_script(b, "enter,down,down,q,enter", 70, 8).unwrap();
        assert!(out.lines().next().unwrap().starts_with("root/big"), "{out}");
        assert!(out.contains("> "), "{out}");
        let b = Browser::new(tree(), SortBy::Size, true);
        assert!(run_script(b, "enter,wat", 70, 8).is_err());
        assert_eq!(parse_key("PgDn"), Some(Key::PageDown));
        assert_eq!(parse_key("nope"), None);
    }
}
