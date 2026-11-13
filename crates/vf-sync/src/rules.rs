//! Which documents go to B2 (owner 2026-11-12, PA-53; a list of rules since PA-63, owner
//! 2026-11-13). B2 is for sharing through the portal first, so little goes up by itself: a
//! document never sent goes up when **any** rule of the firm's list matches it. A rule names
//! folders (any when none), doc types (any when none), which tax years, and whether documents
//! with no tax year count. Per client, Always send / Never send win over the list. A document
//! pushed or shared by hand goes up whatever the list says (`push::push_docs`). Anything
//! already in B2 keeps syncing (an edit, a move, a deletion), so the bucket never shows an old
//! version as current.
//!
//! The list lives in `clients.vf.json` (`b2_rules`; every PC and the bucket carry it). A
//! cabinet that never saved one uses the rule from before PA-63, written as rules:
//! `b2_years_back` (absent: [`DEFAULT_YEARS_BACK`]; 0: every year) and the Permanent folders.

use std::collections::HashMap;
use std::path::Path;

use chrono::Datelike;
use vf_fs::clients::{B2Rule, B2Settings, B2Years};

/// Tax years before this year that still go up when nothing is set.
pub const DEFAULT_YEARS_BACK: u16 = 3;

/// A client's own choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientChoice {
    Always,
    Never,
}

impl ClientChoice {
    pub fn key(self) -> &'static str {
        match self {
            ClientChoice::Always => "always",
            ClientChoice::Never => "never",
        }
    }

    pub fn from_key(k: &str) -> Option<Self> {
        match k {
            "always" => Some(ClientChoice::Always),
            "never" => Some(ClientChoice::Never),
            _ => None,
        }
    }
}

/// Which documents with a tax year a rule takes, for this year.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Years {
    Any,
    /// None with a tax year (a rule for the permanent file).
    None,
    /// This tax year and later.
    From(i32),
}

/// One rule, ready to apply: folders in lower case with forward slashes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// A document matches when its path holds one of these folder paths (`income tax`,
    /// `permanent/setup`), at any depth; none listed: any folder.
    pub folders: Vec<String>,
    /// Doc type ids; none listed: any, and documents with no doc type too.
    pub doc_types: Vec<String>,
    pub years: Years,
    /// Documents with no tax year match too.
    pub no_year: bool,
}

/// The rules as read from the cabinet.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rules {
    /// A never-sent document goes up when any of these matches. Empty: nothing goes up by
    /// itself.
    pub rules: Vec<Rule>,
    /// Client uuid → its own choice.
    pub clients: HashMap<String, ClientChoice>,
}

/// A folder as a rule compares it: forward slashes, no slash at either end, lower case.
fn folder_key(f: &str) -> String {
    f.replace('\\', "/").trim_matches('/').trim().to_lowercase()
}

/// The rule from before PA-63 as a list: tax years `back` before this one (0: every document),
/// and documents with no tax year in a Permanent folder.
pub fn legacy_rules(back: u16) -> Vec<B2Rule> {
    if back == 0 {
        return vec![B2Rule {
            name: "Everything".into(),
            years: B2Years::Any,
            no_year: true,
            ..B2Rule::default()
        }];
    }
    vec![
        B2Rule {
            name: "Recent tax years".into(),
            years: B2Years::Back(back),
            ..B2Rule::default()
        },
        B2Rule {
            name: "Permanent files".into(),
            folders: vec!["Permanent".into()],
            years: B2Years::None,
            no_year: true,
            ..B2Rule::default()
        },
    ]
}

/// The firm's list as saved, or the earlier rule written as a list when none was saved yet.
pub fn listed(b2: &B2Settings) -> Vec<B2Rule> {
    match &b2.rules {
        Some(r) => r.clone(),
        None => legacy_rules(b2.years_back.unwrap_or(DEFAULT_YEARS_BACK)),
    }
}

impl Rules {
    /// Every document goes up (no rule).
    pub fn all() -> Self {
        Rules {
            rules: vec![Rule {
                folders: vec![],
                doc_types: vec![],
                years: Years::Any,
                no_year: true,
            }],
            clients: HashMap::new(),
        }
    }

    /// The cabinet's rules (`clients.vf.json` only). Unreadable: an error, never "send
    /// everything" (QA 2026-11-12: a passing read error would have sent every held document,
    /// for good).
    pub fn load(root: &Path) -> Result<Self, String> {
        vf_fs::clients::load_b2(root).map(|b| Self::from_settings(&b, chrono::Local::now().year()))
    }

    /// [`Rules::load`], or the default rule when the file cannot be read (counts on screen).
    pub fn load_or_default(root: &Path) -> Self {
        Self::load(root).unwrap_or_else(|_| {
            Self::from_settings(&Default::default(), chrono::Local::now().year())
        })
    }

    pub fn from_settings(b2: &B2Settings, this_year: i32) -> Self {
        Rules {
            rules: listed(b2)
                .iter()
                .map(|r| Rule {
                    folders: r
                        .folders
                        .iter()
                        .map(|f| folder_key(f))
                        .filter(|f| !f.is_empty())
                        .collect(),
                    doc_types: r.doc_types.clone(),
                    years: match r.years {
                        B2Years::Any => Years::Any,
                        B2Years::None => Years::None,
                        B2Years::Back(n) => Years::From(this_year - i32::from(n)),
                    },
                    no_year: r.no_year,
                })
                .collect(),
            clients: b2
                .clients
                .iter()
                .filter_map(|(k, v)| Some((k.clone(), ClientChoice::from_key(v)?)))
                .collect(),
        }
    }

    pub fn from_meta(meta: &vf_fs::clients::ClientMeta, this_year: i32) -> Self {
        Self::from_settings(&B2Settings::of(meta), this_year)
    }

    /// The rules as SQL over a `document d` (never-sent documents they send), the same as
    /// [`Rules::sends`] with `in_bucket` false: the counts are made in the catalog, not by
    /// reading every held document (QA 2026-11-12).
    pub fn sendable_sql(&self) -> String {
        let ids = |want: ClientChoice| -> String {
            let mut v: Vec<String> = self
                .clients
                .iter()
                .filter(|(id, c)| {
                    **c == want && id.chars().all(|ch| ch.is_ascii_hexdigit() || ch == '-')
                })
                .map(|(id, _)| format!("'{id}'"))
                .collect();
            v.sort();
            v.join(",")
        };
        let mut c = if self.rules.is_empty() {
            "0".to_string()
        } else {
            let any: Vec<String> = self.rules.iter().map(rule_sql).collect();
            format!("({})", any.join(" OR "))
        };
        let never = ids(ClientChoice::Never);
        if !never.is_empty() {
            c = format!("(coalesce(d.client_id, '') NOT IN ({never}) AND {c})");
        }
        let always = ids(ClientChoice::Always);
        if !always.is_empty() {
            c = format!("(d.client_id IN ({always}) OR {c})");
        }
        // Never NULL (a document with no tax year, doc type or client): `NOT` of it must mean
        // held.
        format!("coalesce({c}, 0)")
    }

    /// Whether a document goes to B2. `rel_path`: cabinet-relative; `in_bucket`: sent before
    /// (then it always keeps syncing).
    pub fn sends(
        &self,
        client: Option<&str>,
        rel_path: &str,
        doc_type: Option<&str>,
        tax_year: Option<i32>,
        in_bucket: bool,
    ) -> bool {
        if in_bucket {
            return true;
        }
        match client.and_then(|c| self.clients.get(c)) {
            Some(ClientChoice::Always) => return true,
            Some(ClientChoice::Never) => return false,
            None => {}
        }
        let path = format!("/{}/", rel_path.replace('\\', "/").to_lowercase());
        self.rules.iter().any(|r| {
            let year = match (tax_year, r.years) {
                (None, _) => r.no_year,
                (Some(_), Years::Any) => true,
                (Some(_), Years::None) => false,
                (Some(y), Years::From(from)) => y >= from,
            };
            year && (r.folders.is_empty()
                || r.folders.iter().any(|f| path.contains(&format!("/{f}/"))))
                && (r.doc_types.is_empty()
                    || doc_type.is_some_and(|t| r.doc_types.iter().any(|x| x == t)))
        })
    }
}

/// A text as an SQL string literal.
fn quoted(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// One rule as SQL over `document d`.
fn rule_sql(r: &Rule) -> String {
    let mut all: Vec<String> = vec![];
    all.push(match (r.years, r.no_year) {
        (Years::Any, true) => "1".into(),
        (Years::Any, false) => "d.tax_year IS NOT NULL".into(),
        (Years::None, true) => "d.tax_year IS NULL".into(),
        (Years::None, false) => "0".into(),
        (Years::From(y), true) => format!("(d.tax_year IS NULL OR d.tax_year >= {y})"),
        (Years::From(y), false) => format!("(d.tax_year IS NOT NULL AND d.tax_year >= {y})"),
    });
    if !r.folders.is_empty() {
        let any: Vec<String> = r
            .folders
            .iter()
            .map(|f| {
                // LIKE's own wildcards in a folder name are plain characters.
                let like = f.replace('!', "!!").replace('%', "!%").replace('_', "!_");
                format!(
                    "('/' || lower(replace(d.rel_path, char(92), '/')) || '/') LIKE {} ESCAPE '!'",
                    quoted(&format!("%/{like}/%"))
                )
            })
            .collect();
        all.push(format!("({})", any.join(" OR ")));
    }
    if !r.doc_types.is_empty() {
        let ids: Vec<String> = r.doc_types.iter().map(|t| quoted(t)).collect();
        all.push(format!("d.doc_type_id IN ({})", ids.join(",")));
    }
    format!("({})", all.join(" AND "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules_of(meta: &vf_fs::clients::ClientMeta) -> Rules {
        Rules::from_meta(meta, 2026)
    }

    /// Before any list is saved: the rule from before PA-63, as two rules.
    #[test]
    fn the_earlier_rule_is_two_rules() {
        let mut meta = vf_fs::clients::ClientMeta::default();
        let r = rules_of(&meta);
        assert_eq!(r.rules.len(), 2);
        assert_eq!(
            r.rules[0].years,
            Years::From(2023),
            "this year and three before"
        );
        let c = Some("c1");
        assert!(r.sends(
            c,
            "Acme (1001)/Income Tax/2023/a.pdf",
            None,
            Some(2023),
            false
        ));
        assert!(!r.sends(
            c,
            "Acme (1001)/Income Tax/2022/a.pdf",
            None,
            Some(2022),
            false
        ));
        assert!(
            r.sends(
                c,
                "Acme (1001)/Income Tax/2022/a.pdf",
                None,
                Some(2022),
                true
            ),
            "sent before"
        );
        assert!(r.sends(c, "Acme (1001)/Permanent/Setup/EIN.pdf", None, None, false));
        assert!(!r.sends(c, "Acme (1001)/Other/notes.pdf", None, None, false));
        assert!(!r.sends(None, "loose.pdf", None, None, false));
        assert!(
            !r.sends(
                c,
                "Acme (1001)/Permanent/Setup/old.pdf",
                None,
                Some(2010),
                false
            ),
            "an old year in Permanent is still an old year"
        );

        meta.b2_clients.insert("c1".into(), "always".into());
        meta.b2_clients.insert("c2".into(), "never".into());
        let r = rules_of(&meta);
        assert!(r.sends(
            c,
            "Acme (1001)/Income Tax/2010/a.pdf",
            None,
            Some(2010),
            false
        ));
        assert!(!r.sends(
            Some("c2"),
            "B (2)/Income Tax/2026/a.pdf",
            None,
            Some(2026),
            false
        ));
        assert!(r.sends(
            Some("c2"),
            "B (2)/Income Tax/2026/a.pdf",
            None,
            Some(2026),
            true
        ));

        meta.b2_years_back = Some(0);
        let r = rules_of(&meta);
        assert!(
            r.sends(Some("c3"), "C (3)/Other/x.pdf", None, None, false),
            "0: every year"
        );
    }

    /// PA-63: a saved list replaces the earlier rule; any rule matching sends; folders match
    /// at any depth and in any case; doc types and years narrow; an empty list sends nothing.
    #[test]
    fn a_list_of_rules() {
        let mut meta = vf_fs::clients::ClientMeta {
            b2_years_back: Some(0),
            ..Default::default()
        };
        meta.b2_rules = Some(vec![
            B2Rule {
                name: "Returns".into(),
                folders: vec!["Income Tax".into()],
                doc_types: vec!["ret".into()],
                years: B2Years::Back(2),
                no_year: false,
            },
            B2Rule {
                name: "Setup".into(),
                folders: vec!["\\Permanent\\Setup\\".into()],
                doc_types: vec![],
                years: B2Years::None,
                no_year: true,
            },
        ]);
        let r = rules_of(&meta);
        let c = Some("c1");
        let ret = Some("ret");
        assert!(r.sends(c, "A (1)/Income Tax/2025/1040.pdf", ret, Some(2025), false));
        assert!(r.sends(c, "A (1)/INCOME TAX/2024/1040.pdf", ret, Some(2024), false));
        assert!(
            !r.sends(c, "A (1)/Income Tax/2023/1040.pdf", ret, Some(2023), false),
            "too old"
        );
        assert!(!r.sends(
            c,
            "A (1)/Income Tax/2025/w2.pdf",
            Some("w2"),
            Some(2025),
            false
        ));
        assert!(
            !r.sends(c, "A (1)/Income Tax/2025/x.pdf", None, Some(2025), false),
            "no doc type"
        );
        assert!(
            !r.sends(c, "A (1)/Payroll/2025/1040.pdf", ret, Some(2025), false),
            "other folder"
        );
        assert!(
            !r.sends(c, "A (1)/Income Tax/1040.pdf", ret, None, false),
            "no year, not ticked"
        );
        assert!(r.sends(c, "A (1)/Permanent/Setup/EIN.pdf", None, None, false));
        assert!(!r.sends(c, "A (1)/Permanent/Stock Basis/b.pdf", None, None, false));
        assert!(!r.sends(c, "A (1)/Permanent/Setup/2019.pdf", None, Some(2019), false));
        // The years-back number of the earlier rule no longer counts.
        assert!(!r.sends(c, "A (1)/Other/x.pdf", None, Some(2026), false));

        meta.b2_rules = Some(vec![]);
        let r = rules_of(&meta);
        assert!(r.rules.is_empty());
        assert!(!r.sends(c, "A (1)/Income Tax/2026/1040.pdf", ret, Some(2026), false));
        assert_eq!(r.sendable_sql(), "coalesce(0, 0)");
        meta.b2_clients.insert("c1".into(), "always".into());
        assert!(rules_of(&meta).sends(c, "A (1)/Other/x.pdf", None, None, false));
    }
}
