//! Help articles (PA-25, owner 2026-11-04): Markdown files in `crates/vf-app/help/`, compiled
//! into the exe, listed here by section. The Help window (`ui::help`) draws the small Markdown
//! subset `help/README.md` describes; this module reads it and searches the articles.

use crate::app::Tab;

/// Sections of the article list, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Start,
    Browse,
    Intake,
    Search,
    Viewer,
    Sync,
    Admin,
    Trouble,
}

impl Section {
    pub const ALL: [Section; 8] = [
        Section::Start,
        Section::Browse,
        Section::Intake,
        Section::Search,
        Section::Viewer,
        Section::Sync,
        Section::Admin,
        Section::Trouble,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Section::Start => "Getting started",
            Section::Browse => "Browse",
            Section::Intake => "Intake and scanning",
            Section::Search => "Search",
            Section::Viewer => "Viewer",
            Section::Sync => "Sync",
            Section::Admin => "Admin",
            Section::Trouble => "When something goes wrong",
        }
    }
}

pub struct Article {
    pub id: &'static str,
    pub section: Section,
    pub text: &'static str,
}

impl Article {
    /// The `# Title` line.
    pub fn title(&self) -> &'static str {
        self.text
            .lines()
            .next()
            .and_then(|l| l.strip_prefix("# "))
            .map_or(self.id, str::trim)
    }

    /// The first paragraph after the title (search results show it).
    pub fn summary(&self) -> String {
        self.text
            .lines()
            .skip(1)
            .skip_while(|l| l.trim().is_empty())
            .take_while(|l| !l.trim().is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

macro_rules! articles {
    ($($section:ident: $($id:literal),+;)+) => {
        pub const ARTICLES: &[Article] = &[
            $($(Article {
                id: $id,
                section: Section::$section,
                text: include_str!(concat!("../help/", $id, ".md")),
            },)+)+
        ];
    };
}

articles! {
    Start: "welcome", "first-run", "screen-tour", "shortcuts";
    Browse: "browse", "documents", "moving-renaming", "new-files", "browse-scan-print", "undo";
    Intake: "intake", "intake-rules", "just-filed", "scanning", "cover-sheets", "print-capture",
        "several-pcs";
    Search: "search", "ocr";
    Viewer: "viewer", "annotations", "forms", "ten-key", "page-tools", "printing";
    Sync: "sync", "sync-key", "sync-conflicts", "portal";
    Admin: "admin", "admin-password", "doc-types", "folder-templates", "clients", "folder-import", "merge-cabinets", "retention",
        "trash", "audit", "devices", "maintenance";
    Trouble: "troubleshooting", "data-and-backups";
}

pub fn article(id: &str) -> Option<&'static Article> {
    ARTICLES.iter().find(|a| a.id == id)
}

/// The article F1 and the Help button open: the one about the tab shown (no cabinet open:
/// opening one).
pub fn for_tab(tab: Option<Tab>) -> &'static str {
    match tab {
        None => "first-run",
        Some(Tab::Browse) => "browse",
        Some(Tab::Intake) => "intake",
        Some(Tab::Search) => "search",
        Some(Tab::Viewer) => "viewer",
        Some(Tab::Sync) => "sync",
        Some(Tab::Admin) => "admin",
    }
}

/// Articles with every word of `query` in their title or text, title matches first.
pub fn search(query: &str) -> Vec<&'static Article> {
    let words: Vec<String> = query
        .split_whitespace()
        .map(str::to_lowercase)
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() {
        return vec![];
    }
    let mut hits: Vec<(usize, usize, &Article)> = ARTICLES
        .iter()
        .enumerate()
        .filter_map(|(i, a)| {
            let title = a.title().to_lowercase();
            let text = a.text.to_lowercase();
            words.iter().all(|w| text.contains(w.as_str())).then(|| {
                let in_title = words.iter().filter(|w| title.contains(w.as_str())).count();
                (usize::MAX - in_title, i, a)
            })
        })
        .collect();
    hits.sort_by_key(|(rank, i, _)| (*rank, *i));
    hits.into_iter().map(|(_, _, a)| a).collect()
}

/// A piece of a line of text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Span {
    Text(String),
    Bold(String),
    Code(String),
    /// A link to another article: (text, article id).
    Link(String, String),
}

/// A block of an article.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Title(String),
    Heading(String),
    Sub(String),
    Para(Vec<Span>),
    Bullets(Vec<Vec<Span>>),
    Numbered(Vec<Vec<Span>>),
    Note(Vec<Span>),
}

/// Reads one line's inline marks: `**bold**`, `` `code` ``, `[text](help:id)`.
pub fn spans(line: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut rest = line;
    let flush = |text: &mut String, out: &mut Vec<Span>| {
        if !text.is_empty() {
            out.push(Span::Text(std::mem::take(text)));
        }
    };
    while let Some(c) = rest.chars().next() {
        if let Some(r) = rest.strip_prefix("**")
            && let Some(end) = r.find("**")
        {
            flush(&mut text, &mut out);
            out.push(Span::Bold(r[..end].to_string()));
            rest = &r[end + 2..];
            continue;
        }
        if let Some(r) = rest.strip_prefix('`')
            && let Some(end) = r.find('`')
        {
            flush(&mut text, &mut out);
            out.push(Span::Code(r[..end].to_string()));
            rest = &r[end + 1..];
            continue;
        }
        if let Some(r) = rest.strip_prefix('[')
            && let Some(close) = r.find("](help:")
            && let Some(end) = r[close..].find(')')
        {
            flush(&mut text, &mut out);
            let label = r[..close].to_string();
            let id = r[close + 7..close + end].to_string();
            out.push(Span::Link(label, id));
            rest = &r[close + end + 1..];
            continue;
        }
        text.push(c);
        rest = &rest[c.len_utf8()..];
    }
    flush(&mut text, &mut out);
    out
}

/// A numbered-list line: the text after `1. `.
fn numbered(line: &str) -> Option<&str> {
    let (n, rest) = line.split_once(". ")?;
    (!n.is_empty() && n.chars().all(|c| c.is_ascii_digit())).then_some(rest)
}

/// Reads an article into blocks.
pub fn parse(md: &str) -> Vec<Block> {
    let mut out: Vec<Block> = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let end_para = |para: &mut Vec<&str>, out: &mut Vec<Block>| {
        if !para.is_empty() {
            out.push(Block::Para(spans(&para.join(" "))));
            para.clear();
        }
    };
    for raw in md.lines() {
        let line = raw.trim_end();
        let t = line.trim_start();
        if t.is_empty() {
            end_para(&mut para, &mut out);
            continue;
        }
        if let Some(h) = t.strip_prefix("### ") {
            end_para(&mut para, &mut out);
            out.push(Block::Sub(h.trim().into()));
        } else if let Some(h) = t.strip_prefix("## ") {
            end_para(&mut para, &mut out);
            out.push(Block::Heading(h.trim().into()));
        } else if let Some(h) = t.strip_prefix("# ") {
            end_para(&mut para, &mut out);
            out.push(Block::Title(h.trim().into()));
        } else if let Some(item) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            end_para(&mut para, &mut out);
            match out.last_mut() {
                Some(Block::Bullets(items)) => items.push(spans(item)),
                _ => out.push(Block::Bullets(vec![spans(item)])),
            }
        } else if let Some(item) = numbered(t) {
            end_para(&mut para, &mut out);
            match out.last_mut() {
                Some(Block::Numbered(items)) => items.push(spans(item)),
                _ => out.push(Block::Numbered(vec![spans(item)])),
            }
        } else if let Some(n) = t.strip_prefix("> ").or_else(|| t.strip_prefix('>')) {
            end_para(&mut para, &mut out);
            match out.last_mut() {
                Some(Block::Note(s)) => {
                    s.push(Span::Text(" ".into()));
                    s.extend(spans(n.trim()));
                }
                _ => out.push(Block::Note(spans(n.trim()))),
            }
        } else if !para.is_empty() || !line.starts_with("  ") {
            para.push(t);
        } else {
            // A continuation line of the item above.
            match out.last_mut() {
                Some(Block::Bullets(items) | Block::Numbered(items)) => {
                    if let Some(last) = items.last_mut() {
                        last.push(Span::Text(" ".into()));
                        last.extend(spans(t));
                    }
                }
                _ => para.push(t),
            }
        }
    }
    end_para(&mut para, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_marks() {
        assert_eq!(
            spans("Press **Save tape**, type `2025` or see [the tape](help:ten-key)."),
            [
                Span::Text("Press ".into()),
                Span::Bold("Save tape".into()),
                Span::Text(", type ".into()),
                Span::Code("2025".into()),
                Span::Text(" or see ".into()),
                Span::Link("the tape".into(), "ten-key".into()),
                Span::Text(".".into()),
            ]
        );
        assert_eq!(spans("a ** b"), [Span::Text("a ** b".into())]);
    }

    #[test]
    fn blocks() {
        let b = parse("# T\n\nOne\ntwo.\n\n## H\n- a\n- b\n  more\n\n1. x\n2. y\n> tip\n");
        assert_eq!(b[0], Block::Title("T".into()));
        assert_eq!(b[1], Block::Para(vec![Span::Text("One two.".into())]));
        assert_eq!(b[2], Block::Heading("H".into()));
        assert_eq!(
            b[3],
            Block::Bullets(vec![
                vec![Span::Text("a".into())],
                vec![
                    Span::Text("b".into()),
                    Span::Text(" ".into()),
                    Span::Text("more".into())
                ],
            ])
        );
        assert!(matches!(&b[4], Block::Numbered(v) if v.len() == 2));
        assert_eq!(b[5], Block::Note(vec![Span::Text("tip".into())]));
    }

    /// Every article: a title, words in range, only the Markdown the window draws, links that
    /// lead somewhere, no real-looking SSN or EIN.
    #[test]
    fn articles_are_well_formed() {
        let ssn = regex::Regex::new(r"\b\d{3}-\d{2}-\d{4}\b").unwrap();
        let ein = regex::Regex::new(r"\b\d{2}-\d{7}\b").unwrap();
        let mut ids = std::collections::HashSet::new();
        for a in ARTICLES {
            assert!(ids.insert(a.id), "{} listed twice", a.id);
            assert!(a.text.starts_with("# "), "{}: no title line", a.id);
            assert!(!a.summary().is_empty(), "{}: no summary paragraph", a.id);
            let words = a.text.split_whitespace().count();
            assert!((100..=1000).contains(&words), "{}: {words} words", a.id);
            for (n, line) in a.text.lines().enumerate() {
                let t = line.trim_start();
                for bad in ["|", "![", "<", "####", "```"] {
                    assert!(
                        !t.starts_with(bad),
                        "{} line {}: {bad} is not drawn",
                        a.id,
                        n + 1
                    );
                }
                assert!(
                    !(line.starts_with("  ") && (t.starts_with("- ") || numbered(t).is_some())),
                    "{} line {}: nested list",
                    a.id,
                    n + 1
                );
            }
            assert!(!ssn.is_match(a.text), "{}: SSN-like number", a.id);
            assert!(!ein.is_match(a.text), "{}: EIN-like number", a.id);
            for b in parse(a.text) {
                let lines: Vec<Vec<Span>> = match b {
                    Block::Para(s) | Block::Note(s) => vec![s],
                    Block::Bullets(v) | Block::Numbered(v) => v,
                    _ => vec![],
                };
                for s in lines.into_iter().flatten() {
                    if let Span::Link(_, id) = s {
                        assert!(article(&id).is_some(), "{}: link to {id}", a.id);
                    }
                }
            }
            assert!(
                !a.text.contains("](http") && !a.text.contains("](#"),
                "{}: only help: links",
                a.id
            );
        }
        for t in Tab::ALL {
            assert!(article(for_tab(Some(t))).is_some());
        }
        assert!(article(for_tab(None)).is_some());
    }

    #[test]
    fn search_finds_by_words() {
        let hits = search("ten-key tape");
        assert_eq!(hits.first().map(|a| a.id), Some("ten-key"));
        assert!(search("").is_empty());
        assert!(search("zzqqxx").is_empty());
    }
}
