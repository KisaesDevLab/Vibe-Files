//! Letter templates for tax return delivery (ADDENDUM-tax-return-delivery.md §12.2, owner
//! 2026-11-24; T4, T5, T21): engagement letters, §7216 consents and package addenda that Vibe
//! Files fills in for a client and sends to the portal to be signed.
//!
//! A template is a list of blocks (heading, paragraph, list, table, signature). Text carries a
//! small markup: `**bold**`, `*italic*` and `{Field}` merge fields. A block can be marked
//! "only for" return types or packages, so one template serves 1040, 1120, 1120-S and 1065.
//! [`layout`] lays a merged letter out on Letter pages and answers the PDF with the page and
//! rectangle of each signature and date, which the portal draws the signatures into.

pub mod layout;
pub mod partners;
pub mod store;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use layout::{Laid, SignBlock, layout};

/// The merge fields a template can use (§12.2), in the order Insert field lists them.
/// Annual agreements (ADDENDUM-annual-agreements.md §3.6) add the partner, the agreement's year,
/// its services and fee lines, and `{Terms and Conditions}`: a paragraph of only that field is
/// replaced by the Terms and Conditions template's text ([`with_terms`]).
pub const FIELDS: &[&str] = &[
    "Client name",
    "Client ID",
    "Signer first names",
    "Taxpayer name",
    "Spouse name",
    "Signer name",
    "Signer title",
    "Tax year",
    "Next year",
    "Return type",
    "Covered returns",
    "Fee",
    "Fee options",
    "Package name",
    "Package fee",
    "Date",
    "Firm name",
    "Partner name",
    "Partner title",
    "Agreement year",
    "Services",
    "Fee lines",
    "One-time fee",
    "Monthly fee",
    TERMS_FIELD,
];

/// The field that stands for the Terms and Conditions (A5: appended to each letter, covered by
/// its signature).
pub const TERMS_FIELD: &str = "Terms and Conditions";

/// Template kinds: a letter, a package's addendum, the Terms and Conditions (one per firm,
/// appended where a letter says `{Terms and Conditions}`), an engagement closure letter (read,
/// not signed).
pub const LETTER: &str = "letter";
pub const ADDENDUM: &str = "addendum";
pub const TERMS: &str = "terms";
pub const CLOSURE: &str = "closure";

/// Who signs at a signature block (§4).
pub const ROLES: [&str; 3] = ["taxpayer", "spouse", "officer"];

/// The words shown under a signature line for a role.
pub fn role_words(role: &str) -> &'static str {
    match role {
        "taxpayer" => "Taxpayer",
        "spouse" => "Spouse",
        "officer" => "Officer or partner",
        _ => "Signer",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Block {
    Heading {
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        only_for: Vec<String>,
    },
    Paragraph {
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        only_for: Vec<String>,
    },
    List {
        items: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        only_for: Vec<String>,
    },
    Table {
        rows: Vec<Vec<String>>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        only_for: Vec<String>,
    },
    Signature {
        role: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        only_for: Vec<String>,
    },
}

impl Block {
    pub fn only_for(&self) -> &[String] {
        match self {
            Block::Heading { only_for, .. }
            | Block::Paragraph { only_for, .. }
            | Block::List { only_for, .. }
            | Block::Table { only_for, .. }
            | Block::Signature { only_for, .. } => only_for,
        }
    }

    /// Kept for a letter of this return type and package (an empty "only for" is for all).
    /// `return_type` may name several tags joined by `|`, as `"1040|Individual"` or, for an
    /// agreement, `"Individual|Bookkeeping|Payroll"`: a block for any of them is kept.
    pub fn applies(&self, return_type: &str, package: Option<&str>) -> bool {
        let o = self.only_for();
        o.is_empty()
            || o.iter().any(|x| {
                return_type
                    .split('|')
                    .any(|r| !r.trim().is_empty() && x.eq_ignore_ascii_case(r.trim()))
                    || package.is_some_and(|p| x.eq_ignore_ascii_case(p))
            })
    }

    /// The texts a block holds (for searching it).
    pub fn texts(&self) -> Vec<&str> {
        match self {
            Block::Heading { text, .. } | Block::Paragraph { text, .. } => vec![text.as_str()],
            Block::List { items, .. } => items.iter().map(String::as_str).collect(),
            Block::Table { rows, .. } => rows.iter().flatten().map(String::as_str).collect(),
            Block::Signature { .. } => vec![],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Template {
    /// Stable id (file name in `_Vibe/letters`).
    pub id: String,
    /// Raised on every save; a delivery records the version it used.
    #[serde(default)]
    pub version: u32,
    /// The name the client sees.
    pub name: String,
    /// The return types it is offered for (empty: all).
    #[serde(default)]
    pub return_types: Vec<String>,
    /// `letter`, or `addendum` (a package's addendum, signed with the selection; T20).
    #[serde(default = "letter_kind")]
    pub kind: String,
    /// For an addendum: the package it belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    /// The doc type the signed letter is filed as.
    #[serde(default)]
    pub doc_type: String,
    #[serde(default)]
    pub letterhead: bool,
    /// Ticked by default in a new delivery.
    #[serde(default)]
    pub default_ticked: bool,
    /// Seeded text: marked "Sample — review before use" until the firm saves its own.
    #[serde(default)]
    pub sample: bool,
    pub blocks: Vec<Block>,
}

fn letter_kind() -> String {
    "letter".into()
}

impl Template {
    pub fn offered_for(&self, return_type: &str) -> bool {
        self.return_types.is_empty()
            || self
                .return_types
                .iter()
                .any(|r| r.eq_ignore_ascii_case(return_type))
    }

    /// A letter a client signs (not the Terms and Conditions, not a closure letter).
    pub fn signed(&self) -> bool {
        self.kind != TERMS && self.kind != CLOSURE
    }

    /// The fill-ins still in the letter as laid out for this return type and package (A17):
    /// `[X]`, `[State]`, `[Month & Date, 20XX]`… A bracket holding no capital letter or digit,
    /// as in "widow[er]", is ordinary text.
    pub fn fill_ins(&self, return_type: &str, package: Option<&str>) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for b in self
            .blocks
            .iter()
            .filter(|b| b.applies(return_type, package))
        {
            for s in b.texts() {
                for f in fill_ins_in(s) {
                    if !out.contains(&f) {
                        out.push(f);
                    }
                }
            }
        }
        out
    }

    /// Every fill-in in the template, whatever return type, signer or package a block is only
    /// for (the editor's warning; review 2026-11-25: blocks "only for" one were missed).
    pub fn all_fill_ins(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for b in &self.blocks {
            for s in b.texts() {
                for f in fill_ins_in(s) {
                    if !out.contains(&f) {
                        out.push(f);
                    }
                }
            }
        }
        out
    }

    /// The signer roles this letter asks for, for a return with or without a spouse.
    pub fn roles(&self, return_type: &str, package: Option<&str>, has_spouse: bool) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for b in &self.blocks {
            if let Block::Signature { role, .. } = b
                && b.applies(return_type, package)
                && (role != "spouse" || has_spouse)
                && !out.contains(role)
            {
                out.push(role.clone());
            }
        }
        out
    }
}

/// The `[…]` fill-ins in a text (see [`Template::fill_ins`]).
pub fn fill_ins_in(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find('[') {
        let after = &rest[i + 1..];
        match after.find(['[', ']']) {
            Some(j) if after.as_bytes()[j] == b']' => {
                let inner = &after[..j];
                if inner
                    .chars()
                    .any(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
                {
                    out.push(format!("[{inner}]"));
                }
                rest = &after[j + 1..];
            }
            Some(j) => rest = &after[j..],
            None => break,
        }
    }
    out
}

/// The letter with its `{Terms and Conditions}` paragraph replaced by the Terms and Conditions
/// template's blocks (its signature blocks left out), or removed when there is none.
pub fn with_terms(t: &Template, terms: Option<&Template>) -> Template {
    let marker = format!("{{{TERMS_FIELD}}}");
    let mut out = t.clone();
    out.blocks = Vec::with_capacity(t.blocks.len());
    for b in &t.blocks {
        match b {
            Block::Paragraph { text, .. } if text.trim() == marker => {
                if let Some(terms) = terms {
                    out.blocks.extend(
                        terms
                            .blocks
                            .iter()
                            .filter(|x| !matches!(x, Block::Signature { .. }))
                            .cloned(),
                    );
                }
            }
            other => out.blocks.push(other.clone()),
        }
    }
    out
}

/// A run of text in one style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
}

/// The merge fields' values for one client and delivery.
pub type Fields = BTreeMap<String, String>;

/// `{Field}` replaced by its value; a field with no value is left as written, so a missing
/// value shows in Preview instead of silently vanishing.
pub fn merge(text: &str, fields: &Fields) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('}') {
            Some(j) if j > 0 && !after[..j].contains('{') => {
                let name = &after[..j];
                match fields.get(name).filter(|v| !v.is_empty()) {
                    Some(v) => out.push_str(v),
                    None => {
                        out.push('{');
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = &after[j + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `**bold**` and `*italic*` read into spans (after merging).
pub fn spans(text: &str) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    let (mut bold, mut italic) = (false, false);
    let mut cur = String::new();
    let mut chars = text.chars().peekable();
    let flush = |cur: &mut String, out: &mut Vec<Span>, bold: bool, italic: bool| {
        if !cur.is_empty() {
            out.push(Span {
                text: std::mem::take(cur),
                bold,
                italic,
            });
        }
    };
    while let Some(c) = chars.next() {
        if c == '*' {
            if chars.peek() == Some(&'*') {
                chars.next();
                flush(&mut cur, &mut out, bold, italic);
                bold = !bold;
            } else {
                flush(&mut cur, &mut out, bold, italic);
                italic = !italic;
            }
        } else {
            cur.push(c);
        }
    }
    flush(&mut cur, &mut out, bold, italic);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_and_markup() {
        let mut f = Fields::new();
        f.insert("Client name".into(), "Jordan Sample".into());
        f.insert("Tax year".into(), "2025".into());
        assert_eq!(
            merge("Dear {Client name}, your {Tax year} return ({Fee}).", &f),
            "Dear Jordan Sample, your 2025 return ({Fee})."
        );
        assert_eq!(merge("a { b } {", &f), "a { b } {");
        let s = spans("We **will** prepare *your* return.");
        assert_eq!(
            s.iter()
                .map(|x| (x.text.as_str(), x.bold, x.italic))
                .collect::<Vec<_>>(),
            [
                ("We ", false, false),
                ("will", true, false),
                (" prepare ", false, false),
                ("your", false, true),
                (" return.", false, false)
            ]
        );
    }

    #[test]
    fn only_for_and_roles() {
        let t = Template {
            id: "x".into(),
            version: 1,
            name: "Engagement letter".into(),
            return_types: vec![],
            kind: "letter".into(),
            package: None,
            doc_type: String::new(),
            letterhead: true,
            default_ticked: true,
            sample: false,
            blocks: vec![
                Block::Paragraph {
                    text: "For a 1040.".into(),
                    only_for: vec!["1040".into()],
                },
                Block::Signature {
                    role: "taxpayer".into(),
                    only_for: vec!["1040".into()],
                },
                Block::Signature {
                    role: "spouse".into(),
                    only_for: vec!["1040".into()],
                },
                Block::Signature {
                    role: "officer".into(),
                    only_for: vec!["1120".into(), "1120-S".into(), "1065".into()],
                },
            ],
        };
        assert_eq!(t.roles("1040", None, true), ["taxpayer", "spouse"]);
        assert_eq!(t.roles("1040", None, false), ["taxpayer"]);
        assert_eq!(t.roles("1065", None, false), ["officer"]);
        assert!(!t.blocks[0].applies("1120-S", None));
        assert!(t.blocks[0].applies("Individual|1040", None));
        assert!(!t.blocks[0].applies("Entity|Bookkeeping", None));
    }

    #[test]
    fn fill_ins_and_terms() {
        assert_eq!(
            fill_ins_in("widow[er], [X]% by [Month & Date, 20XX] in [State]; [a [B]"),
            ["[X]", "[Month & Date, 20XX]", "[State]", "[B]"]
        );
        assert!(fill_ins_in("no [lowercase] here").is_empty());
        let mut t = Template {
            id: "x".into(),
            version: 1,
            name: "Letter".into(),
            return_types: vec![],
            kind: LETTER.into(),
            package: None,
            doc_type: String::new(),
            letterhead: true,
            default_ticked: true,
            sample: false,
            blocks: vec![
                Block::Paragraph {
                    text: "Fee [X]%.".into(),
                    only_for: vec!["Entity".into()],
                },
                Block::Paragraph {
                    text: "{Terms and Conditions}".into(),
                    only_for: vec![],
                },
                Block::Signature {
                    role: "officer".into(),
                    only_for: vec![],
                },
            ],
        };
        assert_eq!(t.fill_ins("1120|Entity", None), ["[X]"]);
        assert!(t.fill_ins("1040|Individual", None).is_empty());
        // The editor sees it whatever the block is only for.
        assert_eq!(t.all_fill_ins(), ["[X]"]);
        let mut terms = t.clone();
        terms.kind = TERMS.into();
        terms.blocks = vec![Block::Heading {
            text: "Billing and payment terms".into(),
            only_for: vec![],
        }];
        let laid = with_terms(&t, Some(&terms));
        assert_eq!(laid.blocks.len(), 3);
        assert!(
            matches!(&laid.blocks[1], Block::Heading { text, .. } if text.starts_with("Billing"))
        );
        t.blocks.remove(0);
        assert_eq!(with_terms(&t, None).blocks.len(), 1);
    }
}
