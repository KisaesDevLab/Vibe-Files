//! Saved searches (P03 step 6), shared by the whole firm (QUESTIONS Q-27):
//! `<cabinet>/_Vibe/saved_searches.json`. Browse lists them as virtual folders.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::search::Query;

pub const FILE: &str = "saved_searches.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedSearch {
    pub id: String,
    pub name: String,
    pub query: Query,
    pub created: String,
    pub device: String,
}

pub fn path(root: &Path) -> PathBuf {
    vf_fs::clients::master_dir(root).join(FILE)
}

/// Every saved search, in the order they were saved. A missing or unreadable file is empty.
pub fn load(root: &Path) -> Vec<SavedSearch> {
    std::fs::read_to_string(path(root))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn write(root: &Path, all: &[SavedSearch]) -> Result<(), String> {
    let p = path(root);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("cannot create {}: {e}", d.display()))?;
    }
    vf_fs::cabinet::write_replace(&p, &serde_json::to_vec_pretty(all).expect("serializes"))
}

/// Saves `query` as `name` (a search with that name is replaced). Returns its id.
pub fn add(root: &Path, name: &str, query: &Query, device: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Give the search a name.".into());
    }
    let mut all = load(root);
    let id = all
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(name))
        .map(|s| s.id.clone())
        .unwrap_or_else(vf_core::new_id);
    all.retain(|s| s.id != id);
    all.push(SavedSearch {
        id: id.clone(),
        name: name.to_string(),
        query: query.clone(),
        created: vf_core::now_rfc3339(),
        device: device.into(),
    });
    write(root, &all)?;
    Ok(id)
}

pub fn remove(root: &Path, id: &str) -> Result<(), String> {
    let mut all = load(root);
    all.retain(|s| s.id != id);
    write(root, &all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::Year;

    #[test]
    fn saved_searches_round_trip_and_replace_by_name() {
        let t = tempfile::tempdir().unwrap();
        assert!(load(t.path()).is_empty());
        let q = Query {
            text: "1099".into(),
            year: Some(Year::Year(2025)),
            ..Query::default()
        };
        let a = add(t.path(), "1099s 2025", &q, "dev").unwrap();
        add(
            t.path(),
            "Permanent",
            &Query {
                year: Some(Year::Permanent),
                ..Query::default()
            },
            "dev",
        )
        .unwrap();
        let again = add(
            t.path(),
            "1099S 2025",
            &Query {
                text: "1099-MISC".into(),
                ..q.clone()
            },
            "dev",
        )
        .unwrap();
        assert_eq!(a, again, "same name replaces");
        let all = load(t.path());
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].query.text, "1099-MISC");
        assert!(add(t.path(), "  ", &q, "dev").is_err());
        remove(t.path(), &a).unwrap();
        assert_eq!(load(t.path()).len(), 1);
    }
}
