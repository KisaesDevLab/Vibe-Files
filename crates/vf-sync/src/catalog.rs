//! The catalog side of sync: what changed since the last exchange, segments built from the
//! catalog, and the base (`sync_base`, `sync_segment`) recorded after an exchange.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use vf_core::db::{Connection, OptionalExtension, params};

use crate::Result;
use crate::crypto::Keys;
use crate::layout::{DocEntry, VersionEntry};

/// Cabinet segment id in `sync_segment` and in pending counts.
pub const CABINET: &str = "";

/// A document as the catalog has it.
#[derive(Debug, Clone, PartialEq)]
pub struct Doc {
    pub id: String,
    pub client_id: Option<String>,
    pub rel_path: String,
    pub hash: String,
    pub size: i64,
    pub rev: i64,
    pub deleted: bool,
}

pub fn doc(conn: &Connection, id: &str) -> Result<Option<Doc>> {
    Ok(conn
        .query_row(
            // A document not fingerprinted yet (PA-60) has an empty hash here: it equals no
            // entry's, so its file is read before anything is decided about it.
            "SELECT id, client_id, rel_path, coalesce(hash, ''), size, rev, deleted_at IS NOT NULL
             FROM document WHERE id = ?1",
            [id],
            |r| {
                Ok(Doc {
                    id: r.get(0)?,
                    client_id: r.get(1)?,
                    rel_path: r.get(2)?,
                    hash: r.get(3)?,
                    size: r.get(4)?,
                    rev: r.get(5)?,
                    deleted: r.get(6)?,
                })
            },
        )
        .optional()?)
}

/// What was last exchanged for a document.
#[derive(Debug, Clone, PartialEq)]
pub struct Base {
    pub client_id: Option<String>,
    pub rel_path: String,
    pub hash: String,
    pub rev: i64,
    pub deleted: bool,
}

pub fn base(conn: &Connection, id: &str) -> Result<Option<Base>> {
    Ok(conn
        .query_row(
            "SELECT client_id, rel_path, hash, rev, deleted FROM sync_base WHERE doc_id = ?1",
            [id],
            |r| {
                Ok(Base {
                    client_id: r.get(0)?,
                    rel_path: r.get(1)?,
                    hash: r.get(2)?,
                    rev: r.get(3)?,
                    deleted: r.get(4)?,
                })
            },
        )
        .optional()?)
}

/// Records the document's current state as exchanged.
pub fn set_base(conn: &Connection, id: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO sync_base (doc_id, client_id, rel_path, hash, rev, deleted)
         SELECT id, client_id, rel_path, hash, rev, deleted_at IS NOT NULL FROM document
         WHERE id = ?1 AND hash IS NOT NULL
         ON CONFLICT(doc_id) DO UPDATE SET client_id = excluded.client_id,
           rel_path = excluded.rel_path, hash = excluded.hash, rev = excluded.rev,
           deleted = excluded.deleted",
        [id],
    )?;
    Ok(())
}

/// Records what was sent for a document (not the live row: it may have changed meanwhile, and
/// then stays pending).
pub fn set_base_values(
    conn: &Connection,
    id: &str,
    client: Option<&str>,
    rel_path: &str,
    hash: &str,
    rev: i64,
    deleted: bool,
) -> Result<()> {
    conn.execute(
        "INSERT INTO sync_base (doc_id, client_id, rel_path, hash, rev, deleted)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(doc_id) DO UPDATE SET client_id = excluded.client_id,
           rel_path = excluded.rel_path, hash = excluded.hash, rev = excluded.rev,
           deleted = excluded.deleted",
        params![id, client, rel_path, hash, rev, deleted],
    )?;
    Ok(())
}

const DIRTY: &str = "
    FROM document d LEFT JOIN sync_base b ON b.doc_id = d.id
    WHERE ((b.doc_id IS NULL AND d.deleted_at IS NULL)
       OR (b.doc_id IS NOT NULL AND (b.rev != d.rev OR b.hash != d.hash
           OR b.rel_path != d.rel_path OR b.deleted != (d.deleted_at IS NOT NULL)
           OR coalesce(b.client_id, '') != coalesce(d.client_id, ''))))
      AND lower(d.rel_path) != '!fcinfo.ini' AND lower(d.rel_path) NOT LIKE '%/!fcinfo.ini'";

/// A conflict copy (this PC's edit of a document in B2, kept beside the other PC's): it goes
/// up whatever the B2 rule says, like the document it came from (QA 2026-11-12).
const COPY: &str = "d.id IN (SELECT copy_doc_id FROM sync_conflict WHERE copy_doc_id IS NOT NULL)";

/// Segments (client ids; [`CABINET`] for the cabinet) with documents changed since the last
/// exchange, with how many each. A document that moved between clients counts for both. A
/// document `rules` holds back (never sent, outside the rule) is not counted (PA-53).
pub fn pending(conn: &Connection, rules: &crate::rules::Rules) -> Result<BTreeMap<String, usize>> {
    let mut out: BTreeMap<String, usize> = BTreeMap::new();
    let mut st = conn.prepare(&format!(
        "SELECT coalesce(d.client_id, ''), coalesce(b.client_id, d.client_id, '') {DIRTY}
           AND (b.doc_id IS NOT NULL OR {COPY} OR {})",
        rules.sendable_sql()
    ))?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    for r in rows {
        let (now, was) = r?;
        *out.entry(now.clone()).or_default() += 1;
        if was != now {
            *out.entry(was).or_default() += 1;
        }
    }
    // Bound clients whose folder or sentinel changed (or never sent).
    let mut st = conn.prepare(
        "SELECT c.id FROM client c LEFT JOIN sync_segment s ON s.client_id = c.id
         WHERE c.deleted_at IS NULL AND c.folder_rel IS NOT NULL
           AND (s.client_id IS NULL OR coalesce(s.folder_rel, '') != c.folder_rel
                OR coalesce(s.sentinel_hash, '') != coalesce(c.sentinel_hash, ''))",
    )?;
    for id in st.query_map([], |r| r.get::<_, String>(0))? {
        out.entry(id?).or_insert(0);
    }
    Ok(out)
}

/// What a push would upload now (documents, bytes): changed or never-sent documents the rules
/// send whose content is not in the bucket yet; `clients`: of these clients only. For the
/// progress a push shows (PA-63); a document not read yet counts by its listed size.
pub fn to_send(
    conn: &Connection,
    rules: &crate::rules::Rules,
    clients: Option<&[String]>,
) -> Result<(u64, u64)> {
    let only = match clients {
        None => String::new(),
        Some(ids) => {
            let list: Vec<String> = ids
                .iter()
                .filter(|id| id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
                .map(|id| format!("'{id}'"))
                .collect();
            format!(" AND d.client_id IN ({})", list.join(","))
        }
    };
    let (n, bytes): (i64, i64) = conn.query_row(
        &format!(
            "SELECT count(*), coalesce(sum(d.size), 0) {DIRTY}
               AND d.deleted_at IS NULL
               AND (b.doc_id IS NOT NULL OR {COPY} OR {})
               AND (d.hash IS NULL OR d.hash NOT IN (SELECT hash FROM sync_object)){only}",
            rules.sendable_sql()
        ),
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok((n.max(0) as u64, bytes.max(0) as u64))
}

/// What the rule holds back now: documents never sent that it leaves out (count, bytes).
pub fn held(conn: &Connection, rules: &crate::rules::Rules) -> Result<(usize, u64)> {
    let (n, bytes): (i64, i64) = conn.query_row(
        &format!(
            "SELECT count(*), coalesce(sum(d.size), 0)
             FROM document d LEFT JOIN sync_base b ON b.doc_id = d.id
             WHERE b.doc_id IS NULL AND d.deleted_at IS NULL AND NOT {COPY}
               AND NOT {}",
            rules.sendable_sql()
        ),
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok((n.max(0) as usize, bytes.max(0) as u64))
}

/// The documents sent before (they have an exchange base): they keep syncing whatever the
/// rule says (PA-53).
pub fn sent_ids(conn: &Connection) -> Result<std::collections::HashSet<String>> {
    // Conflict copies too: they go up like the document they came from.
    let mut st = conn.prepare(
        "SELECT doc_id FROM sync_base
         UNION SELECT copy_doc_id FROM sync_conflict WHERE copy_doc_id IS NOT NULL",
    )?;
    let ids = st
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<_, _>>()?;
    Ok(ids)
}

/// A client's folder (cabinet-relative) and sentinel path.
pub fn client_folder(conn: &Connection, client_id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT folder_rel FROM client WHERE id = ?1 AND deleted_at IS NULL",
            [client_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

/// A document not fingerprinted yet: (id, cabinet path, doc type, tax year).
pub type Unread = (String, String, Option<String>, Option<i32>);

/// The live documents of a segment not fingerprinted yet (PA-60). An entry needs its content's fingerprint, so a push reads the ones it is going
/// to send first ([`entries`] leaves the others out; they were never sent).
pub fn unread(conn: &Connection, client: &str) -> Result<Vec<Unread>> {
    let which = if client.is_empty() {
        "client_id IS NULL AND ?1 = ''"
    } else {
        "client_id = ?1"
    };
    let mut st = conn.prepare(&format!(
        "SELECT id, rel_path, doc_type_id, tax_year FROM document
         WHERE {which} AND deleted_at IS NULL AND hash IS NULL ORDER BY rel_path"
    ))?;
    let rows = st.query_map([client], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// The documents of a segment as entries (live ones, and tombstones of documents ever sent).
pub fn entries(
    conn: &Connection,
    keys: &Keys,
    client: &str,
    folder: &str,
) -> Result<Vec<DocEntry>> {
    // By the client index (PA-34): `coalesce(client_id, '')` read every document per client.
    let which = if client.is_empty() {
        "d.client_id IS NULL AND ?1 = ''"
    } else {
        "d.client_id = ?1"
    };
    let mut st = conn.prepare(&format!(
        "SELECT d.id, d.rel_path, d.hash, d.size, d.mime, d.doc_type_id, d.tax_year, d.status,
                d.source, d.filed_by, d.filed_at, d.device_id, d.rev, d.deleted_at,
                d.tombstone_reason
         FROM document d LEFT JOIN sync_base b ON b.doc_id = d.id
         WHERE {which} AND (d.deleted_at IS NULL OR b.doc_id IS NOT NULL)
           AND d.hash IS NOT NULL
         ORDER BY d.rel_path, d.id"
    ))?;
    let strip = |rel: &str| -> String {
        if folder.is_empty() {
            rel.to_string()
        } else {
            rel.strip_prefix(folder)
                .and_then(|r| r.strip_prefix('/'))
                .unwrap_or(rel)
                .to_string()
        }
    };
    let mut out: Vec<DocEntry> = st
        .query_map([client], |r| {
            Ok(DocEntry {
                doc_id: r.get(0)?,
                rel_path: r.get(1)?,
                hash: r.get(2)?,
                object: String::new(),
                size: r.get(3)?,
                mime: r.get(4)?,
                doc_type_id: r.get(5)?,
                tax_year: r.get(6)?,
                status: r.get(7)?,
                source: r.get(8)?,
                filed_by: r.get(9)?,
                filed_at: r.get(10)?,
                device_id: r.get(11)?,
                rev: r.get(12)?,
                tags: vec![],
                versions: vec![],
                deleted_at: r.get(13)?,
                tombstone_reason: r.get(14)?,
            })
        })?
        .collect::<std::result::Result<_, _>>()?;
    // A client's notes file (PA-76) is not a document: a row an earlier build made for one is
    // never sent, least of all as a deletion (another PC would move the file to its trash).
    out.retain(|e| !vf_fs::fcinfo::is_note_path(&e.rel_path));
    let mut tags = conn.prepare("SELECT tag FROM document_tag WHERE doc_id = ?1 ORDER BY tag")?;
    let mut vers = conn.prepare(
        "SELECT hash, size, created, created_by, device_id, reason FROM version
         WHERE doc_id = ?1 ORDER BY created, hash",
    )?;
    for e in &mut out {
        e.rel_path = strip(&e.rel_path);
        e.object = keys.object_name(&e.hash);
        e.tags = tags
            .query_map([&e.doc_id], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        e.versions = vers
            .query_map([&e.doc_id], |r| {
                Ok(VersionEntry {
                    hash: r.get(0)?,
                    object: String::new(),
                    size: r.get(1)?,
                    created: r.get(2)?,
                    created_by: r.get(3)?,
                    device_id: r.get(4)?,
                    reason: r.get(5)?,
                })
            })?
            .collect::<std::result::Result<_, _>>()?;
        for v in &mut e.versions {
            v.object = keys.object_name(&v.hash);
        }
    }
    Ok(out)
}

/// Folders under a client folder with nothing in them (so a restore keeps the template tree).
pub fn empty_dirs(root: &Path, folder: &str) -> Vec<String> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) -> bool {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return false;
        };
        let mut any = false;
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                if name == vf_fs::cabinet::VIBE_DIR {
                    continue;
                }
                if !walk(&e.path(), base, out) {
                    out.push(
                        e.path()
                            .strip_prefix(base)
                            .unwrap_or(&e.path())
                            .to_string_lossy()
                            .replace('\\', "/"),
                    );
                }
                any = true;
            } else {
                any = true;
            }
        }
        any
    }
    let base = root.join(folder);
    let mut out = vec![];
    walk(&base, &base, &mut out);
    out.sort();
    out
}

pub fn segment_rev(conn: &Connection, client: &str) -> Result<u64> {
    Ok(conn
        .query_row(
            "SELECT rev FROM sync_segment WHERE client_id = ?1",
            [client],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .unwrap_or(0) as u64)
}

pub fn set_segment(
    conn: &Connection,
    client: &str,
    rev: u64,
    folder: Option<&str>,
    sentinel_hash: Option<&str>,
    files_hash: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO sync_segment (client_id, rev, folder_rel, sentinel_hash, files_hash)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(client_id) DO UPDATE SET rev = excluded.rev, folder_rel = excluded.folder_rel,
           sentinel_hash = excluded.sentinel_hash,
           files_hash = coalesce(excluded.files_hash, sync_segment.files_hash)",
        params![client, rev as i64, folder, sentinel_hash, files_hash],
    )?;
    Ok(())
}

pub fn files_base(conn: &Connection) -> Result<BTreeMap<String, String>> {
    let t: Option<String> = conn
        .query_row(
            "SELECT files_hash FROM sync_segment WHERE client_id = ''",
            [],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    Ok(t.and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default())
}

pub fn last_rev(conn: &Connection, device: &str) -> Result<u64> {
    Ok(conn
        .query_row(
            "SELECT last_manifest_rev FROM sync_state WHERE device_id = ?1",
            [device],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .unwrap_or(0) as u64)
}

pub fn set_state(conn: &Connection, device: &str, rev: u64, what: &str) -> Result<()> {
    let col = if what == "push" {
        "last_push"
    } else {
        "last_pull"
    };
    conn.execute(
        &format!(
            "INSERT INTO sync_state (device_id, last_manifest_rev, {col}) VALUES (?1, ?2, ?3)
             ON CONFLICT(device_id) DO UPDATE SET last_manifest_rev = excluded.last_manifest_rev,
               {col} = excluded.{col}"
        ),
        params![device, rev as i64, vf_core::now_rfc3339()],
    )?;
    Ok(())
}

/// Contents known to be in the bucket.
pub fn uploaded(conn: &Connection) -> Result<BTreeSet<String>> {
    let mut st = conn.prepare("SELECT hash FROM sync_object")?;
    let v = st
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    Ok(v)
}

/// Gives the document `old` the id `new` (a document both workstations have on disk, which the
/// bucket knows by another id), with everything that points at it.
pub fn rekey(conn: &Connection, old: &str, new: &str) -> Result<()> {
    let cols: Vec<String> = {
        let mut st = conn.prepare("SELECT name FROM pragma_table_info('document')")?;
        st.query_map([], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?
    };
    let others: Vec<&String> = cols.iter().filter(|c| *c != "id").collect();
    let list = others
        .iter()
        .map(|c| c.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let tx = vf_core::db::begin_write(conn)?;
    tx.execute(
        "UPDATE document SET deleted_at = coalesce(deleted_at, 'rekey') WHERE id = ?1",
        [old],
    )?;
    tx.execute(
        &format!("INSERT INTO document (id, {list}) SELECT ?2, {list} FROM document WHERE id = ?1"),
        [old, new],
    )?;
    tx.execute(
        "UPDATE document SET deleted_at = NULL WHERE id = ?1 AND deleted_at = 'rekey'",
        [new],
    )?;
    for t in [
        "document_tag",
        "version",
        "lock",
        "disposition",
        "document_extract",
        "sync_pending",
        "sync_base",
        "legal_hold",
    ] {
        tx.execute(
            &format!("UPDATE {t} SET doc_id = ?2 WHERE doc_id = ?1"),
            [old, new],
        )?;
    }
    tx.execute("DELETE FROM document WHERE id = ?1", [old])?;
    tx.commit()?;
    Ok(())
}
