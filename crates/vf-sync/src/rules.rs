//! Which documents go to B2 (owner 2026-11-12, PA-53): to cut what is synced, a document never
//! sent goes up only when its tax year is recent (this year and the few before), or it has no
//! tax year and is in a Permanent folder; per client, Always send / Never send win. Anything
//! already in B2 keeps syncing (an edit, a move, a deletion), so the bucket never shows an old
//! version as current. The rule lives in `clients.vf.json` (every PC and the bucket carry it):
//! `b2_years_back` (absent: [`DEFAULT_YEARS_BACK`]; 0: every year) and `b2_clients`.

use std::collections::HashMap;
use std::path::Path;

use chrono::Datelike;

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

/// The rule as read from the cabinet.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rules {
    /// The oldest tax year sent; `None`: every year.
    pub from_year: Option<i32>,
    /// Client uuid → its own choice.
    pub clients: HashMap<String, ClientChoice>,
}

impl Rules {
    /// Every document goes up (no rule).
    pub fn all() -> Self {
        Rules::default()
    }

    /// The cabinet's rule (`clients.vf.json` only). Unreadable: an error, never "send
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

    pub fn from_settings(b2: &vf_fs::clients::B2Settings, this_year: i32) -> Self {
        let back = b2.years_back.unwrap_or(DEFAULT_YEARS_BACK);
        Rules {
            from_year: (back > 0).then(|| this_year - i32::from(back)),
            clients: b2
                .clients
                .iter()
                .filter_map(|(k, v)| Some((k.clone(), ClientChoice::from_key(v)?)))
                .collect(),
        }
    }

    pub fn from_meta(meta: &vf_fs::clients::ClientMeta, this_year: i32) -> Self {
        Self::from_settings(
            &vf_fs::clients::B2Settings {
                years_back: meta.b2_years_back,
                clients: meta.b2_clients.clone(),
            },
            this_year,
        )
    }

    /// The rule as SQL over a `document d` (never-sent documents it sends), the same as
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
        let mut c = match self.from_year {
            None => "1".to_string(),
            Some(y) => format!(
                "((d.tax_year IS NOT NULL AND d.tax_year >= {y}) OR (d.tax_year IS NULL AND \
                 ('/' || lower(replace(d.rel_path, char(92), '/')) || '/') LIKE '%/permanent/%'))"
            ),
        };
        let never = ids(ClientChoice::Never);
        if !never.is_empty() {
            c = format!("(coalesce(d.client_id, '') NOT IN ({never}) AND {c})");
        }
        let always = ids(ClientChoice::Always);
        if !always.is_empty() {
            c = format!("(d.client_id IN ({always}) OR {c})");
        }
        // Never NULL (a document with no tax year or no client): `NOT` of it must mean held.
        format!("coalesce({c}, 0)")
    }

    /// Whether a document goes to B2. `rel_path`: cabinet-relative; `in_bucket`: sent before
    /// (then it always keeps syncing).
    pub fn sends(
        &self,
        client: Option<&str>,
        rel_path: &str,
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
        let Some(from) = self.from_year else {
            return true;
        };
        match tax_year {
            Some(y) => y >= from,
            // No tax year: a Permanent folder's (as the Needs tax year view counts them).
            None => {
                format!("/{}/", rel_path.replace('\\', "/").to_lowercase()).contains("/permanent/")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_goes_to_b2() {
        let mut meta = vf_fs::clients::ClientMeta::default();
        let r = Rules::from_meta(&meta, 2026);
        assert_eq!(r.from_year, Some(2023), "this year and the three before");
        let c = Some("c1");
        assert!(r.sends(c, "Acme (1001)/Income Tax/2023/a.pdf", Some(2023), false));
        assert!(!r.sends(c, "Acme (1001)/Income Tax/2022/a.pdf", Some(2022), false));
        assert!(
            r.sends(c, "Acme (1001)/Income Tax/2022/a.pdf", Some(2022), true),
            "sent before"
        );
        assert!(r.sends(c, "Acme (1001)/Permanent/Setup/EIN.pdf", None, false));
        assert!(!r.sends(c, "Acme (1001)/Other/notes.pdf", None, false));
        assert!(!r.sends(None, "loose.pdf", None, false));

        meta.b2_clients.insert("c1".into(), "always".into());
        meta.b2_clients.insert("c2".into(), "never".into());
        let r = Rules::from_meta(&meta, 2026);
        assert!(r.sends(c, "Acme (1001)/Income Tax/2010/a.pdf", Some(2010), false));
        assert!(!r.sends(Some("c2"), "B (2)/Income Tax/2026/a.pdf", Some(2026), false));
        assert!(r.sends(Some("c2"), "B (2)/Income Tax/2026/a.pdf", Some(2026), true));

        meta.b2_years_back = Some(0);
        let r = Rules::from_meta(&meta, 2026);
        assert!(
            r.sends(Some("c3"), "C (3)/Other/x.pdf", None, false),
            "0: every year"
        );
    }
}
