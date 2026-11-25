//! The firm's partners, who sign agreements and letters by name (ADDENDUM-annual-agreements.md
//! A7, A15, owner 2026-11-25): `<cabinet>/_Vibe/partners.json`, one list for every PC. A client
//! defaults to the partner its client CSV's Partner column names (by the partner's code, such as
//! initials, or by name); an agreement can name another.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const FILE: &str = "partners.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Partner {
    /// As printed under "/s/" (`{Partner name}`).
    pub name: String,
    /// `{Partner title}`, such as "Partner" or "Managing Member".
    #[serde(default)]
    pub title: String,
    /// What the client CSV's Partner column holds for this partner (initials, a code), if not
    /// the name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub code: String,
}

fn path(root: &Path) -> PathBuf {
    root.join(vf_fs::cabinet::VIBE_DIR).join(FILE)
}

/// The partners, as saved (none: an empty list).
pub fn load(root: &Path) -> Vec<Partner> {
    std::fs::read(path(root))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Saves the list (names trimmed; a row with no name left out; two of one name refused).
pub fn save(root: &Path, list: &[Partner]) -> Result<Vec<Partner>, String> {
    let mut out: Vec<Partner> = Vec::new();
    for p in list {
        let p = Partner {
            name: p.name.trim().to_string(),
            title: p.title.trim().to_string(),
            code: p.code.trim().to_string(),
        };
        if p.name.is_empty() {
            continue;
        }
        if out.iter().any(|x| x.name.eq_ignore_ascii_case(&p.name)) {
            return Err(format!("{} is in the list twice.", p.name));
        }
        out.push(p);
    }
    vf_fs::lock::with(root, "partners", "the partners", || {
        let bytes = serde_json::to_vec_pretty(&out).expect("serializes");
        vf_fs::cabinet::write_replace(&path(root), &bytes)
    })?;
    Ok(out)
}

/// The partner a client's Partner value names (by code or name), else the first in the list.
pub fn default_for<'a>(list: &'a [Partner], partner: Option<&str>) -> Option<&'a Partner> {
    partner
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .and_then(|p| {
            list.iter().find(|x| {
                (!x.code.is_empty() && x.code.eq_ignore_ascii_case(p))
                    || x.name.eq_ignore_ascii_case(p)
            })
        })
        .or_else(|| list.first())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_and_finds_the_clients_partner() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::create_dir_all(root.join("_Vibe")).unwrap();
        assert!(load(root).is_empty());
        let list = vec![
            Partner {
                name: " Morgan Example ".into(),
                title: "Partner".into(),
                code: "ME".into(),
            },
            Partner {
                name: "Riley Sample".into(),
                title: "Managing Partner".into(),
                code: String::new(),
            },
            Partner::default(),
        ];
        let saved = save(root, &list).unwrap();
        assert_eq!(saved.len(), 2);
        assert_eq!(load(root), saved);
        assert_eq!(
            default_for(&saved, Some("me")).unwrap().name,
            "Morgan Example"
        );
        assert_eq!(
            default_for(&saved, Some("riley sample")).unwrap().title,
            "Managing Partner"
        );
        assert_eq!(
            default_for(&saved, Some("XX")).unwrap().name,
            "Morgan Example"
        );
        assert_eq!(default_for(&saved, None).unwrap().name, "Morgan Example");
        assert!(default_for(&[], Some("ME")).is_none());
        let mut twice = saved.clone();
        twice.push(saved[0].clone());
        assert!(save(root, &twice).is_err());
    }
}
