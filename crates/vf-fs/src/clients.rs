//! The cabinet's client master on disk (P01 step 3).
//!
//! `<cabinet>/_Vibe/` is the Filer Desktop data folder for master data, so the Filer engine
//! mounted in P02 reads and writes the same files:
//!
//! - `clients.json` — Filer's client list (CSV import cache with the stable `ID → uuid` map),
//!   read and written only through `filer_core::clients::{load_cache, save_cache}`.
//! - `relationships.json` — Filer's relationships, through `filer_core::relationships`.
//! - `template.json` — the cabinet folder template, through `filer_core::template`.
//!
//! Never add fields to those three: Filer's own `save` would drop them. What Vibe Files adds
//! (entity type, share %) lives in the sidecar `clients.vf.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use filer_core::clients::{self, Client, ClientList};
use filer_core::config::ColumnMap;
use filer_core::relationships::{self, Relationships};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::cabinet::VIBE_DIR;

pub const SIDECAR_FILE: &str = "clients.vf.json";

/// The entity-type pick list (catalog `client.entity_type`).
pub const ENTITY_TYPES: [(&str, &str); 9] = [
    ("individual", "Individual"),
    ("sole_prop", "Sole proprietor"),
    ("partnership", "Partnership"),
    ("s_corp", "S corporation"),
    ("c_corp", "C corporation"),
    ("llc", "LLC (disregarded)"),
    ("trust", "Trust"),
    ("estate", "Estate"),
    ("nonprofit", "Nonprofit"),
];

/// The folder Filer's data functions are pointed at.
pub fn master_dir(root: &Path) -> PathBuf {
    root.join(VIBE_DIR)
}

/// One relationship's share, keyed like Filer's relationship (CSV IDs + role).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Share {
    pub parent_id: String,
    pub child_id: String,
    pub role: String,
    pub share_pct: f64,
}

/// `clients.vf.json`: what Vibe Files keeps beside Filer's files.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClientMeta {
    /// Client uuid (sentinel `client_id`) → entity type key from [`ENTITY_TYPES`].
    #[serde(default)]
    pub entity_types: BTreeMap<String, String>,
    #[serde(default)]
    pub shares: Vec<Share>,
}

impl ClientMeta {
    pub fn share(&self, parent: &str, child: &str, role: &str) -> Option<f64> {
        self.shares
            .iter()
            .find(|s| s.parent_id == parent && s.child_id == child && same_role(&s.role, role))
            .map(|s| s.share_pct)
    }

    /// Sets or clears (None) the share for one relationship.
    pub fn set_share(&mut self, parent: &str, child: &str, role: &str, pct: Option<f64>) {
        self.shares.retain(|s| {
            !(s.parent_id == parent && s.child_id == child && same_role(&s.role, role))
        });
        if let Some(p) = pct {
            self.shares.push(Share {
                parent_id: parent.into(),
                child_id: child.into(),
                role: role.trim().into(),
                share_pct: p,
            });
        }
    }
}

fn same_role(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// Everything in the client master.
#[derive(Debug, Clone, Default)]
pub struct Master {
    /// `None` until a CSV is imported or a client is added.
    pub list: Option<ClientList>,
    pub rels: Relationships,
    pub meta: ClientMeta,
}

impl Master {
    pub fn clients(&self) -> &[Client] {
        self.list
            .as_ref()
            .map(|l| l.clients.as_slice())
            .unwrap_or(&[])
    }
}

/// Reads the master. An unreadable file is an error (never silently replaced by an empty one,
/// which a later save would write over it).
pub fn load(root: &Path) -> Result<Master, String> {
    let dir = master_dir(root);
    let list = clients::load_cache(&dir).map_err(|e| {
        format!(
            "{} could not be read: {e}",
            clients::cache_path(&dir).display()
        )
    })?;
    let (rels, _savable, err) = relationships::load(&dir);
    if let Some(e) = err {
        return Err(e);
    }
    let meta = match std::fs::read_to_string(dir.join(SIDECAR_FILE)) {
        Ok(t) => serde_json::from_str(&t)
            .map_err(|e| format!("{SIDECAR_FILE} could not be read: {e}"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ClientMeta::default(),
        Err(e) => return Err(format!("{SIDECAR_FILE} could not be read: {e}")),
    };
    Ok(Master { list, rels, meta })
}

pub fn save_list(root: &Path, list: &ClientList) -> Result<(), String> {
    clients::save_cache(&master_dir(root), list).map_err(|e| format!("saving clients.json: {e}"))
}

pub fn save_rels(root: &Path, rels: &Relationships) -> Result<(), String> {
    relationships::save(&master_dir(root), rels)
        .map_err(|e| format!("saving relationships.json: {e}"))
}

pub fn save_meta(root: &Path, meta: &ClientMeta) -> Result<(), String> {
    let dir = master_dir(root);
    let p = dir.join(SIDECAR_FILE);
    let tmp = dir.join(format!("{SIDECAR_FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(meta).expect("serializes"))
        .and_then(|()| std::fs::rename(&tmp, &p))
        .map_err(|e| format!("saving {SIDECAR_FILE}: {e}"))
}

/// Imports the client CSV with `map` and saves it as the master list. Every client keeps the
/// uuid it already has: from `known` (the catalog's `ext_id → id`, which includes folders bound
/// by sentinel) first, then from the current list.
pub fn import_csv(
    root: &Path,
    csv: &Path,
    map: &ColumnMap,
    known: &BTreeMap<String, Uuid>,
) -> Result<ClientList, String> {
    let tins = tin_extras(map);
    if !tins.is_empty() {
        return Err(format!(
            "Leave out {}: taxpayer ID columns are not copied into the client list (full TINs stay in the documents only).",
            tins.join(", ")
        ));
    }
    let table = clients::read_csv(csv).map_err(|e| e.to_string())?;
    let mut list = clients::apply_mapping(&table, map).map_err(|e| e.to_string())?;
    let mut all = load(root)?.list.map(|l| l.uuids).unwrap_or_default();
    all.extend(known.iter().map(|(k, v)| (k.clone(), *v)));
    list.assign_uuids(&all);
    save_list(root, &list)?;
    // Re-read so the list's lookup index is built.
    load(root)?
        .list
        .ok_or_else(|| "clients.json vanished after saving".into())
}

/// Sets (or clears) a client's entity type in the sidecar. Run `rebuild::sync_clients` after.
pub fn set_entity_type(root: &Path, client_id: &str, entity: Option<&str>) -> Result<(), String> {
    let mut m = load(root)?;
    match entity.filter(|e| !e.is_empty()) {
        Some(e) => {
            if !ENTITY_TYPES.iter().any(|(k, _)| *k == e) {
                return Err(format!("unknown entity type {e}"));
            }
            m.meta.entity_types.insert(client_id.into(), e.into());
        }
        None => {
            m.meta.entity_types.remove(client_id);
        }
    }
    save_meta(root, &m.meta)
}

/// Adds a relationship (Filer's checks: not to itself, a role, no duplicate) and its share.
/// Both IDs must be clients the catalog knows (bound or in the list). Run `sync_clients` after.
pub fn add_relationship(
    root: &Path,
    rel: relationships::Relationship,
    share_pct: Option<f64>,
) -> Result<(), String> {
    if let Some(p) = share_pct
        && !(0.0..=100.0).contains(&p)
    {
        return Err("The share must be between 0 and 100 %.".into());
    }
    let mut m = load(root)?;
    let (p, c, role) = (
        rel.parent_id.trim().to_owned(),
        rel.child_id.trim().to_owned(),
        rel.role.trim().to_owned(),
    );
    m.rels.add(rel).map_err(|e| e.to_string())?;
    m.meta.set_share(&p, &c, &role, share_pct);
    save_rels(root, &m.rels)?;
    save_meta(root, &m.meta)
}

/// Removes one relationship and its share. Run `sync_clients` after (it tombstones the row).
pub fn remove_relationship(
    root: &Path,
    parent: &str,
    child: &str,
    role: &str,
) -> Result<(), String> {
    let mut m = load(root)?;
    let before = m.rels.records.len();
    m.rels
        .records
        .retain(|r| !(r.parent_id == parent && r.child_id == child && same_role(&r.role, role)));
    if m.rels.records.len() == before {
        return Err("That relationship is not in the list.".into());
    }
    m.meta.set_share(parent, child, role, None);
    save_rels(root, &m.rels)?;
    save_meta(root, &m.meta)
}

/// Top-level folders with no sentinel that Filer's scan matches to a master-list client (by the
/// ID in the folder name, or by a close name). Binding them is the folder import (P10).
pub fn unbound_matches(root: &Path, list: &ClientList) -> Result<Vec<(String, String)>, String> {
    use filer_intake::sentinel::{Progress, ScanInput, Status, scan_root};
    let s = filer_core::config::Settings::default();
    let input = ScanInput {
        root: root.to_owned(),
        clients: std::sync::Arc::new(list.clone()),
        id_pattern: s.id_pattern,
        threshold: s.fuzzy_suggest,
        ignored: vec![],
    };
    Ok(scan_root(&input, &Progress::default())?
        .into_iter()
        .filter(|b| matches!(b.status, Status::UnboundNamed | Status::UnboundFuzzy))
        .filter_map(|b| Some((b.name(), b.client_id?)))
        .collect())
}

/// Headers that look like a taxpayer ID column. Mapping one as an extra would copy full TINs into
/// `clients.json` and the catalog (CLAUDE.md §Security: no full TINs outside the PDF).
pub fn looks_like_tin(header: &str) -> bool {
    let h: String = header
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    [
        "ssn",
        "ein",
        "tin",
        "itin",
        "fein",
        "taxid",
        "socialsecurity",
        "taxpayerid",
    ]
    .iter()
    .any(|k| h == *k || h.starts_with(k) || h.ends_with(k))
}

/// Extras in `map` that look like taxpayer IDs.
pub fn tin_extras(map: &ColumnMap) -> Vec<String> {
    map.extras
        .iter()
        .filter(|h| looks_like_tin(h))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_csv(dir: &Path, text: &str) -> PathBuf {
        let p = dir.join("clients.csv");
        std::fs::write(&p, text).unwrap();
        p
    }

    fn map(extras: &[&str]) -> ColumnMap {
        ColumnMap {
            id: Some("ClientID".into()),
            name: Some("Name".into()),
            extras: extras.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn cabinet() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(master_dir(t.path())).unwrap();
        t
    }

    #[test]
    fn import_keeps_known_uuids_and_reads_back_through_filer() {
        let cab = cabinet();
        let csv = write_csv(
            cab.path(),
            "ClientID,Name,Partner\n1001,Acme LLC,KW\n1002,Baker Trust,AB\n",
        );
        let bound = Uuid::new_v4();
        let known = BTreeMap::from([("1001".to_string(), bound)]);
        let list = import_csv(cab.path(), &csv, &map(&["Partner"]), &known).unwrap();
        assert_eq!(list.match_by_id("1001").unwrap().uuid, bound);
        let baker = list.match_by_id("1002").unwrap().uuid;
        assert!(!baker.is_nil());
        assert_eq!(list.match_by_id("1002").unwrap().extra["Partner"], "AB");

        // A second import (CSV changed) keeps both uuids.
        let csv = write_csv(
            cab.path(),
            "ClientID,Name,Partner\n1002,Baker Trust,AB\n1003,Cole,KW\n",
        );
        let list = import_csv(cab.path(), &csv, &map(&["Partner"]), &BTreeMap::new()).unwrap();
        assert_eq!(list.match_by_id("1002").unwrap().uuid, baker);
        assert_eq!(
            list.uuids["1001"], bound,
            "an ID that left the CSV keeps its uuid"
        );

        // Filer reads the file Vibe wrote.
        let filer = clients::load_cache(&master_dir(cab.path()))
            .unwrap()
            .unwrap();
        assert_eq!(filer.clients, list.clients);
    }

    #[test]
    fn relationships_round_trip_through_filer_format() {
        let cab = cabinet();
        let mut rels = Relationships::default();
        rels.add(relationships::Relationship {
            parent_id: "1001".into(),
            child_id: "1002".into(),
            role: "shareholder".into(),
            note: "via K-1".into(),
        })
        .unwrap();
        save_rels(cab.path(), &rels).unwrap();
        let mut meta = ClientMeta::default();
        meta.set_share("1001", "1002", "Shareholder", Some(40.0));
        save_meta(cab.path(), &meta).unwrap();

        // Filer's loader sees exactly the records; the share is only in the sidecar.
        let (filer, savable, err) = relationships::load(&master_dir(cab.path()));
        assert!(savable && err.is_none());
        assert_eq!(filer, rels);
        let raw = std::fs::read_to_string(relationships::path(&master_dir(cab.path()))).unwrap();
        assert!(!raw.contains("share_pct"), "no Vibe fields in Filer's file");

        let m = load(cab.path()).unwrap();
        assert_eq!(m.rels, rels);
        assert_eq!(m.meta.share("1001", "1002", "shareholder"), Some(40.0));

        // A file Filer Desktop wrote (its own save) loads too.
        let other = tempfile::tempdir().unwrap();
        relationships::save(other.path(), &rels).unwrap();
        std::fs::copy(
            relationships::path(other.path()),
            relationships::path(&master_dir(cab.path())),
        )
        .unwrap();
        assert_eq!(load(cab.path()).unwrap().rels, rels);
    }

    #[test]
    fn unreadable_master_is_an_error_not_empty() {
        let cab = cabinet();
        std::fs::write(relationships::path(&master_dir(cab.path())), "{not json").unwrap();
        assert!(load(cab.path()).is_err());
    }

    #[test]
    fn share_set_and_clear() {
        let mut m = ClientMeta::default();
        m.set_share("1", "2", "partner", Some(25.0));
        m.set_share("1", "2", "Partner", Some(30.0));
        assert_eq!(m.shares.len(), 1);
        assert_eq!(m.share("1", "2", "partner"), Some(30.0));
        m.set_share("1", "2", "partner", None);
        assert!(m.shares.is_empty());
    }

    #[test]
    fn tin_headers_are_flagged() {
        for h in ["SSN", "EIN", "Taxpayer ID", "Spouse SSN", "FEIN", "Tax ID"] {
            assert!(looks_like_tin(h), "{h}");
        }
        for h in ["Partner", "Entity Type", "Preparer", "City", "Phone"] {
            assert!(!looks_like_tin(h), "{h}");
        }
        assert_eq!(tin_extras(&map(&["Partner", "SSN"])), ["SSN"]);
    }

    fn rel(p: &str, c: &str, role: &str) -> relationships::Relationship {
        relationships::Relationship {
            parent_id: p.into(),
            child_id: c.into(),
            role: role.into(),
            note: String::new(),
        }
    }

    #[test]
    fn add_and_remove_relationships_with_filer_checks() {
        let cab = cabinet();
        add_relationship(cab.path(), rel("1001", "1002", "shareholder"), Some(60.0)).unwrap();
        let e = add_relationship(cab.path(), rel("1001", "1002", "Shareholder"), None).unwrap_err();
        assert!(e.contains("already exists"), "{e}");
        assert!(add_relationship(cab.path(), rel("1001", "1001", "partner"), None).is_err());
        assert!(add_relationship(cab.path(), rel("1001", "1003", " "), None).is_err());
        assert!(add_relationship(cab.path(), rel("1001", "1003", "partner"), Some(120.0)).is_err());
        let m = load(cab.path()).unwrap();
        assert_eq!(m.rels.len(), 1);
        assert_eq!(m.meta.share("1001", "1002", "shareholder"), Some(60.0));

        remove_relationship(cab.path(), "1001", "1002", "SHAREHOLDER").unwrap();
        let m = load(cab.path()).unwrap();
        assert_eq!(m.rels.len(), 0);
        assert!(m.meta.shares.is_empty());
        assert!(remove_relationship(cab.path(), "1001", "1002", "shareholder").is_err());
    }

    #[test]
    fn entity_type_set_clear_and_checked() {
        let cab = cabinet();
        set_entity_type(cab.path(), "u-1", Some("s_corp")).unwrap();
        assert_eq!(load(cab.path()).unwrap().meta.entity_types["u-1"], "s_corp");
        assert!(set_entity_type(cab.path(), "u-1", Some("pirate ship")).is_err());
        set_entity_type(cab.path(), "u-1", None).unwrap();
        assert!(load(cab.path()).unwrap().meta.entity_types.is_empty());
    }

    #[test]
    fn unbound_folders_matching_the_list_are_reported() {
        let cab = cabinet();
        std::fs::create_dir(cab.path().join("Acme LLC (1001)")).unwrap();
        std::fs::create_dir(cab.path().join("Random Stuff")).unwrap();
        let csv = write_csv(
            cab.path(),
            "ClientID,Name\n1001,Acme LLC\n1002,Baker Trust\n",
        );
        let list = import_csv(cab.path(), &csv, &map(&[]), &BTreeMap::new()).unwrap();
        assert_eq!(
            unbound_matches(cab.path(), &list).unwrap(),
            [("Acme LLC (1001)".to_string(), "1001".to_string())]
        );
    }

    #[test]
    fn import_refuses_a_tin_extra() {
        let cab = cabinet();
        let csv = write_csv(
            cab.path(),
            "ClientID,Name,SSN
1001,Acme,000-00-0001
",
        );
        let e = import_csv(cab.path(), &csv, &map(&["SSN"]), &BTreeMap::new()).unwrap_err();
        assert!(e.contains("SSN"), "{e}");
        assert!(!clients::cache_path(&master_dir(cab.path())).exists());
    }
}
