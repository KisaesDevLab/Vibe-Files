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
    /// Client uuid → the last 4 digits of its SSN / EIN (Q-27; never the full number), from
    /// the client CSV's taxpayer ID column. Form recognition (P08) matches on it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tin_last4: BTreeMap<String, String>,
    /// Client uuid → the firm's partner for the client (owner 2026-11-05). Set here it wins over
    /// a client CSV column named Partner.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub partners: BTreeMap<String, String>,
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

/// Puts every client with a folder in the catalog into Filer's client list (`clients.json`)
/// when it is missing there: a cabinet whose clients came from their folders (rebuild, or a
/// cabinet made before the list) otherwise gives Intake no clients to pick (owner 2026-11-04).
/// Returns how many were added.
pub fn ensure_listed(conn: &vf_core::db::Connection, root: &Path) -> Result<usize, String> {
    let mut st = conn
        .prepare(
            "SELECT id, ext_id, name FROM client
             WHERE deleted_at IS NULL AND folder_rel IS NOT NULL AND ext_id <> ''
             ORDER BY name",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<(String, String, String)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    if rows.is_empty() {
        return Ok(0);
    }
    let mut list = load(root)?
        .list
        .unwrap_or_else(crate::newclient::empty_list);
    let have: std::collections::HashSet<String> =
        list.clients.iter().map(|c| c.id.to_uppercase()).collect();
    let mut added = 0;
    for (id, ext, name) in rows {
        if have.contains(&ext.to_uppercase()) {
            continue;
        }
        let Ok(uuid) = Uuid::parse_str(&id) else {
            continue;
        };
        list.uuids.insert(ext.clone(), uuid);
        list.clients.push(Client {
            id: ext,
            uuid,
            name,
            extra: BTreeMap::new(),
        });
        added += 1;
    }
    if added > 0 {
        save_list(root, &list)?;
    }
    Ok(added)
}

pub fn save_rels(root: &Path, rels: &Relationships) -> Result<(), String> {
    relationships::save(&master_dir(root), rels)
        .map_err(|e| format!("saving relationships.json: {e}"))
}

pub fn save_meta(root: &Path, meta: &ClientMeta) -> Result<(), String> {
    crate::cabinet::write_replace(
        &master_dir(root).join(SIDECAR_FILE),
        &serde_json::to_vec_pretty(meta).expect("serializes"),
    )
    .map_err(|e| format!("saving {SIDECAR_FILE}: {e}"))
}

/// Imports the client CSV with `map` and saves it as the master list. Every client keeps the
/// uuid it already has: from `known` (the catalog's `ext_id → id`, which includes folders bound
/// by sentinel) first, then from the current list.
/// The last 4 digits of a taxpayer ID (`123-45-6789` → `6789`, `XX-XXX1234` → `1234`), or
/// `None` when it has fewer than 4 digits. Only these 4 are ever kept.
pub fn last4(value: &str) -> Option<String> {
    let digits: Vec<char> = value.chars().filter(|c| c.is_ascii_digit()).collect();
    (digits.len() >= 4).then(|| digits[digits.len() - 4..].iter().collect())
}

pub fn import_csv(
    root: &Path,
    csv: &Path,
    map: &ColumnMap,
    known: &BTreeMap<String, Uuid>,
) -> Result<ClientList, String> {
    import_csv_tin(root, csv, map, known, None)
}

/// Makes `csv` and `map` Intake Setup's client CSV (Filer's shared `config.json`), so Browse's
/// import and Intake's re-import read the same file (owner 2026-11-12).
pub fn use_csv_in_intake(root: &Path, csv: &Path, map: &ColumnMap) -> Result<(), String> {
    let dir = master_dir(root);
    let (mut settings, savable) = filer_core::config::Settings::load_or_default(&dir);
    if !savable {
        return Err("Intake's config.json could not be read".into());
    }
    if settings.csv_path.as_deref() == Some(csv) && settings.column_map == *map {
        return Ok(());
    }
    settings.csv_path = Some(csv.to_path_buf());
    settings.column_map = map.clone();
    settings.save(&dir).map_err(|e| e.to_string())
}

/// A Windows-1252 byte for `c`, when the code page has one.
fn to_1252(c: char) -> Option<u8> {
    const HIGH: [char; 32] = [
        '\u{20AC}', '\u{0081}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008D}',
        '\u{017D}', '\u{008F}', '\u{0090}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}',
        '\u{2022}', '\u{2013}', '\u{2014}', '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}',
        '\u{0153}', '\u{009D}', '\u{017E}', '\u{0178}',
    ];
    let n = c as u32;
    if n < 0x80 || (0xA0..=0xFF).contains(&n) {
        return Some(n as u8);
    }
    HIGH.iter().position(|&h| h == c).map(|i| 0x80 + i as u8)
}

/// One CSV field, quoted when it holds a comma, a quote, a line break or edge spaces.
fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) || s.trim() != s {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Adds a client made in Vibe Files (+ New client) to the client CSV the list was imported
/// from (owner 2026-11-12), so a later re-import keeps it: its ID, name and partner in the
/// mapped columns, the others blank, in the file's own encoding and line endings. `None`: no
/// CSV (a list made in Vibe Files only) or the ID is in it already.
pub fn append_to_csv(
    root: &Path,
    id: &str,
    name: &str,
    partner: Option<&str>,
) -> Result<Option<PathBuf>, String> {
    // The CSV Intake › Setup names (and re-imports): none after "Stop using the CSV".
    let (settings, _) = filer_core::config::Settings::load_or_default(&master_dir(root));
    let Some(csv) = settings.csv_path.clone() else {
        return Ok(None);
    };
    if csv.as_os_str().is_empty() {
        return Ok(None);
    }
    if !csv.is_file() {
        return Err(format!("{} cannot be reached from this PC", csv.display()));
    }
    let map = &settings.column_map;
    let table = clients::read_csv(&csv).map_err(|e| format!("{}: {e}", csv.display()))?;
    let (Some(id_h), Some(name_h)) = (map.id.as_deref(), map.name.as_deref()) else {
        return Err("the client list has no ID and name columns mapped".into());
    };
    let col = |h: &str| table.headers.iter().position(|x| x == h);
    let id_col = col(id_h).ok_or_else(|| format!("no column {id_h} in {}", csv.display()))?;
    if table.rows.iter().any(|r| {
        r.get(id_col)
            .is_some_and(|v| v.trim().eq_ignore_ascii_case(id.trim()))
    }) {
        return Ok(None);
    }
    let fields: Vec<String> = table
        .headers
        .iter()
        .map(|h| {
            if h == id_h {
                id.trim().to_string()
            } else if h == name_h {
                name.trim().to_string()
            } else if map.partner.as_deref() == Some(h.as_str()) {
                partner.unwrap_or("").trim().to_string()
            } else {
                String::new()
            }
        })
        .collect();
    let line: String = fields
        .iter()
        .map(|f| csv_field(f))
        .collect::<Vec<_>>()
        .join(",");
    let raw = std::fs::read(&csv).map_err(|e| format!("{}: {e}", csv.display()))?;
    let crlf = raw.windows(2).any(|w| w == b"\r\n") || !raw.contains(&b'\n');
    let eol: &[u8] = if crlf { b"\r\n" } else { b"\n" };
    let mut bytes: Vec<u8> = vec![];
    if !raw.is_empty() && !raw.ends_with(b"\n") {
        bytes.extend_from_slice(eol);
    }
    match table.encoding {
        clients::Encoding::Windows1252 => {
            for c in line.chars() {
                bytes.push(to_1252(c).ok_or_else(|| {
                    format!("\"{c}\" cannot be written in this CSV's Windows-1252 text")
                })?);
            }
        }
        _ => bytes.extend_from_slice(line.as_bytes()),
    }
    bytes.extend_from_slice(eol);
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&csv)
        .and_then(|mut f| f.write_all(&bytes).and_then(|()| f.sync_all()))
        .map_err(|e| format!("{}: {e}", csv.display()))?;
    Ok(Some(csv))
}

/// [`import_csv`], also keeping the last 4 digits of `tin_column` (a taxpayer ID column) per
/// client in `clients.vf.json`. The full numbers are read from the CSV and dropped at once;
/// Filer's client list never gets the column.
pub fn import_csv_tin(
    root: &Path,
    csv: &Path,
    map: &ColumnMap,
    known: &BTreeMap<String, Uuid>,
    tin_column: Option<&str>,
) -> Result<ClientList, String> {
    let tins = tin_extras(map);
    if !tins.is_empty() {
        return Err(format!(
            "Leave out {}: taxpayer ID columns are not copied into the client list (full TINs stay in the documents only).",
            tins.join(", ")
        ));
    }
    let table = clients::read_csv(csv).map_err(|e| e.to_string())?;
    // By the values too: an ID, name or kept column holding SSNs / EINs is refused.
    let col = |h: &str| table.headers.iter().position(|x| x == h);
    for h in map
        .id
        .iter()
        .chain(map.name.iter())
        .chain(map.extras.iter())
        .chain(map.partner.iter())
    {
        if let Some(i) = col(h)
            && values_look_like_tins(
                table
                    .rows
                    .iter()
                    .filter_map(|r| r.get(i).map(String::as_str)),
            )
        {
            return Err(format!(
                "The column {h} holds taxpayer IDs (SSNs or EINs): it cannot be the client ID, the name, the partner or a kept column. Full TINs stay in the documents only."
            ));
        }
    }
    let mut list = clients::apply_mapping(&table, map).map_err(|e| e.to_string())?;
    let mut all = load(root)?.list.map(|l| l.uuids).unwrap_or_default();
    all.extend(known.iter().map(|(k, v)| (k.clone(), *v)));
    list.assign_uuids(&all);
    save_list(root, &list)?;
    if let Some(tc) = tin_column {
        let col = |h: &str| table.headers.iter().position(|x| x == h);
        let tin_col = col(tc).ok_or_else(|| format!("no column {tc}"))?;
        let id_col = map
            .id
            .as_deref()
            .and_then(col)
            .ok_or("no client ID column")?;
        let mut meta = load(root)?.meta;
        for row in &table.rows {
            let (Some(id), Some(tin)) = (row.get(id_col), row.get(tin_col)) else {
                continue;
            };
            let Some(c) = list.match_by_id(id.trim()) else {
                continue;
            };
            let key = c.uuid.to_string();
            match last4(tin) {
                Some(l4) => {
                    meta.tin_last4.insert(key, l4);
                }
                None => {
                    meta.tin_last4.remove(&key);
                }
            }
        }
        save_meta(root, &meta)?;
    }
    // Re-read so the list's lookup index is built.
    load(root)?
        .list
        .ok_or_else(|| "clients.json vanished after saving".into())
}

/// Sets (or clears, `None` or blank) a client's partner in the sidecar. Run
/// `rebuild::sync_clients` after.
pub fn set_partner(root: &Path, client_id: &str, partner: Option<&str>) -> Result<(), String> {
    let mut m = load(root)?;
    match partner.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => {
            m.meta.partners.insert(client_id.into(), p.into());
        }
        None => {
            m.meta.partners.remove(client_id);
        }
    }
    save_meta(root, &m.meta)
}

/// A client's partner: set in Vibe Files, else the client CSV's Partner column.
pub fn partner_of(meta: &ClientMeta, client: &str, listed: Option<&Client>) -> Option<String> {
    meta.partners.get(client).cloned().or_else(|| {
        listed?
            .extra
            .iter()
            .find(|(k, _)| k.trim().eq_ignore_ascii_case("partner"))
            .map(|(_, v)| v.trim().to_string())
            .filter(|v| !v.is_empty())
    })
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
    h == "ss"
        || [
            "ssn",
            "ein",
            "tin",
            "itin",
            "fein",
            "taxid",
            "socialsecurity",
            "socsec",
            "taxpayerid",
            "taxidentification",
            "fedid",
            "federalid",
            "employerid",
        ]
        .iter()
        .any(|k| h == *k || h.starts_with(k) || h.ends_with(k))
}

/// Whether `v` is shaped like a full SSN or EIN (`123-45-6789`, `12-3456789`, 9 digits).
pub fn is_tin_value(v: &str) -> bool {
    let t = v.trim();
    let digits = t.chars().filter(|c| c.is_ascii_digit()).count();
    let others_ok = t
        .chars()
        .all(|c| c.is_ascii_digit() || c == '-' || c == ' ');
    others_ok && digits == 9 && t.len() <= 11
}

/// Whether a column's values are taxpayer IDs: a third or more of its non-empty values look
/// like full SSNs / EINs.
pub fn values_look_like_tins<'a>(values: impl Iterator<Item = &'a str>) -> bool {
    let (mut n, mut tin) = (0usize, 0usize);
    for v in values.filter(|v| !v.trim().is_empty()) {
        n += 1;
        if is_tin_value(v) {
            tin += 1;
        }
    }
    n > 0 && tin * 3 >= n
}

/// Removes taxpayer-ID columns from a client list (by header, or by values that look like full
/// TINs) wherever they came from: Filer's own CSV wizard (Intake › Setup) keeps whatever the
/// person ticks. Returns the columns removed (only their headers).
pub fn strip_tins(list: &mut ClientList) -> Vec<String> {
    let mut keys: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for c in &list.clients {
        keys.extend(c.extra.keys().cloned());
    }
    let bad: Vec<String> = keys
        .into_iter()
        .filter(|k| {
            looks_like_tin(k)
                || values_look_like_tins(
                    list.clients
                        .iter()
                        .filter_map(|c| c.extra.get(k).map(String::as_str)),
                )
        })
        .collect();
    if !bad.is_empty() {
        for c in &mut list.clients {
            for k in &bad {
                c.extra.remove(k);
            }
        }
        list.column_map.extras.retain(|e| !bad.contains(e));
    }
    bad
}

/// [`strip_tins`] on the cabinet's `clients.json`, saving it when anything was removed. Run when
/// a cabinet opens and before the catalog or the bucket gets the list.
pub fn strip_tins_on_disk(root: &Path) -> Result<Vec<String>, String> {
    let Some(mut list) = load(root)?.list else {
        return Ok(vec![]);
    };
    let bad = strip_tins(&mut list);
    if !bad.is_empty() {
        save_list(root, &list)?;
    }
    Ok(bad)
}

/// Extras in `map` that look like taxpayer IDs.
pub fn tin_extras(map: &ColumnMap) -> Vec<String> {
    // (headers only; the import also checks the values)
    map.extras
        .iter()
        .chain(map.partner.iter())
        .filter(|h| looks_like_tin(h))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Clients known only from their folders reach Filer's list, once (owner 2026-11-04).
    #[test]
    fn clients_with_folders_are_put_in_filers_list() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("cab");
        std::fs::create_dir_all(master_dir(&root)).unwrap();
        let conn = vf_core::db::open(&t.path().join("c.db"), vf_core::db::Storage::Local).unwrap();
        conn.execute_batch(
            "INSERT INTO client (id, ext_id, name, folder_rel, device_id, updated_at) VALUES
               ('0190a1b2-0000-7000-8000-000000000001', 'INVE1001', 'Invented Family Trust',
                'Invented Family Trust (INVE1001)', 'd', 't'),
               ('0190a1b2-0000-7000-8000-000000000002', 'NOFO1002', 'No Folder Co', NULL, 'd', 't');",
        )
        .unwrap();
        assert!(load(&root).unwrap().list.is_none());
        assert_eq!(ensure_listed(&conn, &root).unwrap(), 1);
        let list = load(&root).unwrap().list.unwrap();
        assert_eq!(list.clients.len(), 1);
        assert_eq!(list.clients[0].id, "INVE1001");
        assert_eq!(
            list.clients[0].uuid.to_string(),
            "0190a1b2-0000-7000-8000-000000000001"
        );
        assert_eq!(ensure_listed(&conn, &root).unwrap(), 0, "once");
    }

    fn write_csv(dir: &Path, text: &str) -> PathBuf {
        let p = dir.join("clients.csv");
        std::fs::write(&p, text).unwrap();
        p
    }

    fn map(extras: &[&str]) -> ColumnMap {
        ColumnMap {
            partner: None,
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

    /// QA P1/P2: TIN columns are refused by header or values, wherever they are mapped, and
    /// stripped from a list written by Filer's own wizard.
    #[test]
    fn taxpayer_ids_never_enter_the_client_list() {
        for h in [
            "SSN",
            "Fed ID",
            "Federal ID Number",
            "SS #",
            "Soc Sec No",
            "EIN",
        ] {
            assert!(looks_like_tin(h), "{h}");
        }
        assert!(!looks_like_tin("Partner") && !looks_like_tin("Status"));
        assert!(
            is_tin_value("900-12-4321") && is_tin_value("41-1234567") && is_tin_value("900124321")
        );
        assert!(!is_tin_value("1001") && !is_tin_value("ANDE1042"));

        let cab = cabinet();
        // Taxpayer IDs as the client ID column: refused.
        let csv = write_csv(
            cab.path(),
            "ClientID,Name
900-12-4321,Acme
900-12-4322,Baker
",
        );
        assert!(import_csv(cab.path(), &csv, &map(&[]), &Default::default()).is_err());
        // A kept column of TINs with an innocent header: refused by its values.
        let csv = write_csv(
            cab.path(),
            "ClientID,Name,Number
1001,Acme,900-12-4321
1002,Baker,41-1234567
",
        );
        assert!(import_csv(cab.path(), &csv, &map(&["Number"]), &Default::default()).is_err());

        // A list as Filer's own wizard writes it, with an SSN column kept: stripped.
        let csv = write_csv(
            cab.path(),
            "ClientID,Name,Partner
1001,Acme,KW
",
        );
        let mut list =
            import_csv(cab.path(), &csv, &map(&["Partner"]), &Default::default()).unwrap();
        for c in &mut list.clients {
            c.extra.insert("Fed ID".into(), "41-1234567".into());
            c.extra.insert("Ref".into(), "900-12-4321".into());
        }
        save_list(cab.path(), &list).unwrap();
        let bad = strip_tins_on_disk(cab.path()).unwrap();
        assert_eq!(bad, ["Fed ID", "Ref"]);
        let text = std::fs::read_to_string(master_dir(cab.path()).join("clients.json")).unwrap();
        assert!(
            !text.contains("1234567") && !text.contains("12-4321"),
            "{text}"
        );
        assert!(text.contains("Partner"));
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
        // The Partner column too (Filer V35).
        let as_partner = ColumnMap {
            partner: Some("Tax ID".into()),
            ..map(&[])
        };
        assert_eq!(tin_extras(&as_partner), ["Tax ID"]);
    }

    /// The New client dialog adds the client to the client CSV (owner 2026-11-12): mapped
    /// columns filled, others blank, quoted where needed, in the file's encoding and line
    /// endings; a re-import then keeps it.
    #[test]
    fn a_new_client_is_added_to_the_csv() {
        let cab = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(master_dir(cab.path())).unwrap();
        let csv = cab.path().join("c.csv");
        // Windows-1252 (é is one byte), CRLF, no line break at the end.
        let mut raw = b"IDNUM,Name,Preparer,City
1001,Caf"
            .to_vec();
        raw.push(0xE9);
        raw.extend_from_slice(b" LLC,KW,Ames");
        std::fs::write(&csv, &raw).unwrap();
        let m = ColumnMap {
            id: Some("IDNUM".into()),
            name: Some("Name".into()),
            extras: vec![],
            partner: Some("Preparer".into()),
        };
        import_csv(cab.path(), &csv, &m, &Default::default()).unwrap();
        // Not Intake's CSV yet: nothing written.
        assert_eq!(append_to_csv(cab.path(), "2002", "x", None).unwrap(), None);
        use_csv_in_intake(cab.path(), &csv, &m).unwrap();

        let got = append_to_csv(cab.path(), "2002", "Smith, José", Some("AB")).unwrap();
        assert_eq!(got.as_deref(), Some(csv.as_path()));
        let bytes = std::fs::read(&csv).unwrap();
        let mut want = raw.clone();
        want.extend_from_slice(
            b"
2002,\"Smith, Jos",
        );
        want.push(0xE9);
        want.extend_from_slice(
            b"\",AB,
",
        );
        assert_eq!(bytes, want);
        // Already there: nothing added.
        assert_eq!(append_to_csv(cab.path(), "2002", "x", None).unwrap(), None);
        // A character the code page lacks is refused, the file untouched.
        let e = append_to_csv(cab.path(), "3003", "Zhang 张", None).unwrap_err();
        assert!(e.contains("Windows-1252"), "{e}");
        assert_eq!(std::fs::read(&csv).unwrap(), want);
        // Re-imported: the client is in the list, with its partner.
        let list = import_csv(cab.path(), &csv, &m, &Default::default()).unwrap();
        let c = list.match_by_id("2002").unwrap();
        assert_eq!(c.name, "Smith, José");
        assert_eq!(c.extra["Partner"], "AB");

        // UTF-8 with LF.
        std::fs::write(
            &csv,
            "IDNUM,Name
1001,Acme
",
        )
        .unwrap();
        let m = ColumnMap { partner: None, ..m };
        import_csv(cab.path(), &csv, &m, &Default::default()).unwrap();
        use_csv_in_intake(cab.path(), &csv, &m).unwrap();
        append_to_csv(cab.path(), "4004", "Zhang 张", Some("AB")).unwrap();
        assert_eq!(
            std::fs::read_to_string(&csv).unwrap(),
            "IDNUM,Name
1001,Acme
4004,Zhang 张
"
        );
        // After "Stop using the CSV" (Intake names none): the CSV is left alone.
        let dir = master_dir(cab.path());
        let (mut s, _) = filer_core::config::Settings::load_or_default(&dir);
        s.csv_path = None;
        s.save(&dir).unwrap();
        assert_eq!(
            append_to_csv(cab.path(), "5005", "Lee", None).unwrap(),
            None
        );
        assert!(!std::fs::read_to_string(&csv).unwrap().contains("5005"));
        // A CSV named but out of reach (a drive this PC lacks): an error, so the dialog warns.
        s.csv_path = Some(cab.path().join("gone.csv"));
        s.save(&dir).unwrap();
        let e = append_to_csv(cab.path(), "5005", "Lee", None).unwrap_err();
        assert!(e.contains("cannot be reached"), "{e}");
    }

    /// A Partner column holding SSNs is refused, whatever its header.
    #[test]
    fn a_partner_column_of_taxpayer_ids_is_refused() {
        let cab = tempfile::tempdir().unwrap();
        let csv = cab.path().join("c.csv");
        std::fs::write(
            &csv,
            "ID,Name,Preparer
1001,Acme,123-45-6789
1002,Baker,987-65-4321
",
        )
        .unwrap();
        let m = ColumnMap {
            partner: Some("Preparer".into()),
            ..map(&[])
        };
        let e = import_csv(cab.path(), &csv, &m, &Default::default()).unwrap_err();
        assert!(e.contains("holds taxpayer IDs"), "{e}");
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

    /// P08 step 0 (Q-27): a taxpayer ID column keeps only its last 4 digits, per client, in
    /// clients.vf.json and the catalog; the full numbers are nowhere in the cabinet's files.
    #[test]
    fn tin_column_keeps_the_last_4_only() {
        let cab = cabinet();
        let csv = write_csv(
            cab.path(),
            "ClientID,Name,SSN
1001,Acme,900-12-3456
1002,Baker Trust,98-7654321
1003,No TIN,
",
        );
        let list =
            import_csv_tin(cab.path(), &csv, &map(&[]), &BTreeMap::new(), Some("SSN")).unwrap();
        let meta = load(cab.path()).unwrap().meta;
        let uuid = |id: &str| list.match_by_id(id).unwrap().uuid.to_string();
        assert_eq!(
            meta.tin_last4.get(&uuid("1001")).map(String::as_str),
            Some("3456")
        );
        assert_eq!(
            meta.tin_last4.get(&uuid("1002")).map(String::as_str),
            Some("4321")
        );
        assert!(!meta.tin_last4.contains_key(&uuid("1003")));
        // Nothing in the cabinet's files holds a full TIN.
        let mut files = Vec::new();
        fn walk(d: &Path, out: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                if e.path().is_dir() {
                    walk(&e.path(), out);
                } else {
                    out.push(e.path());
                }
            }
        }
        walk(&master_dir(cab.path()), &mut files);
        for f in files {
            let t = std::fs::read_to_string(&f).unwrap_or_default();
            for full in ["900-12-3456", "900123456", "98-7654321", "987654321"] {
                assert!(!t.contains(full), "{} holds {full}", f.display());
            }
        }
        assert_eq!(last4("XXX-XX-1234").as_deref(), Some("1234"));
        assert_eq!(last4("12"), None);
    }
}
