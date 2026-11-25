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
pub mod store;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use layout::{Laid, SignBlock, layout};

/// The merge fields a template can use (§12.2), in the order Insert field lists them.
pub const FIELDS: [&str; 17] = [
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
];

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
    pub fn applies(&self, return_type: &str, package: Option<&str>) -> bool {
        let o = self.only_for();
        o.is_empty()
            || o.iter().any(|x| {
                x.eq_ignore_ascii_case(return_type)
                    || package.is_some_and(|p| x.eq_ignore_ascii_case(p))
            })
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
    }
}
