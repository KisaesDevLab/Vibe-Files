//! Safeguards appendix (PHASES P07 step 5): one PDF for the firm's WISP with the cabinet
//! inventory, encryption status, workstations, retention schedule and legal holds, the last
//! integrity sweep and audit-chain check, backup / sync, and the disposition runs in the
//! period.
//!
//! Workstations register themselves in the cabinet (`_Vibe/devices/<id>.json`) each time they
//! open it: every workstation keeps its own catalog (Q-26), so the cabinet is the one place
//! that knows them all.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use vf_core::db::Connection;
use vf_pdf::report::{Column, Section};

use crate::cabinet;

pub const DEVICES_DIR: &str = "devices";

/// A workstation that has opened the cabinet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    /// The computer's name (`COMPUTERNAME`).
    pub name: String,
    pub first_seen: String,
    pub last_seen: String,
    #[serde(default)]
    pub version: String,
    /// A Remote Desktop server in server mode (PA-36): it works in the cabinet folder itself,
    /// on the share, as the other servers do (PA-41).
    #[serde(default)]
    pub server: bool,
}

fn devices_dir(root: &Path) -> PathBuf {
    root.join(cabinet::VIBE_DIR).join(DEVICES_DIR)
}

/// Records this workstation in the cabinet (first seen kept, last seen now). `server`: it runs
/// in server mode.
pub fn register_device(root: &Path, device_id: &str, server: bool) -> Result<(), String> {
    let dir = devices_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let file = dir.join(format!("{device_id}.json"));
    let now = vf_core::now_rfc3339();
    let first_seen = std::fs::read_to_string(&file)
        .ok()
        .and_then(|t| serde_json::from_str::<Device>(&t).ok())
        .map(|d| d.first_seen)
        .unwrap_or_else(|| now.clone());
    let d = Device {
        id: device_id.into(),
        name: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown".into()),
        first_seen,
        last_seen: now,
        version: env!("CARGO_PKG_VERSION").into(),
        server,
    };
    cabinet::write_replace(&file, &serde_json::to_vec_pretty(&d).expect("serializes"))
}

/// Every workstation that has opened the cabinet, by name.
pub fn devices(root: &Path) -> Vec<Device> {
    let mut out: Vec<Device> = std::fs::read_dir(devices_dir(root))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|t| serde_json::from_str(&t).ok())
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Takes a workstation that is gone off the list (Admin › Devices, PA-24). It comes back if it
/// opens the cabinet again. Audited `device_removed`.
pub fn remove_device(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    gone: &str,
) -> Result<(), String> {
    if gone == device_id {
        return Err("This PC cannot remove itself.".into());
    }
    let file = devices_dir(root).join(format!("{gone}.json"));
    let name = std::fs::read_to_string(&file)
        .ok()
        .and_then(|t| serde_json::from_str::<Device>(&t).ok())
        .map(|d| d.name)
        .ok_or("That workstation is not on the list.")?;
    std::fs::remove_file(&file).map_err(|e| e.to_string())?;
    vf_core::tx::Transaction::new("device_removed")
        .audit(vf_core::audit::AuditEntry::new(
            "device_removed",
            None,
            serde_json::json!({ "device": gone, "name": name }),
        ))
        .commit(
            conn,
            &mut vf_core::tx::NoFileOps,
            &vf_core::tx::Actor::device(device_id),
        )
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// What the app adds that the catalog cannot say.
pub struct Inputs<'a> {
    pub cabinet_name: &'a str,
    /// RFC 3339 start of the period the disposition runs are listed for.
    pub period_from: &'a str,
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap_or(0)
}

fn size(bytes: i64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else {
        format!("{:.0} KB", b / 1e3)
    }
}

/// The appendix as PDF bytes.
pub fn appendix(conn: &Connection, root: &Path, inp: &Inputs) -> Result<Vec<u8>, String> {
    let now = vf_core::now_rfc3339();
    let mut sections = Vec::new();

    // 1. Inventory.
    let clients = count(conn, "SELECT count(*) FROM client WHERE deleted_at IS NULL");
    let docs = count(
        conn,
        "SELECT count(*) FROM document WHERE deleted_at IS NULL",
    );
    let bytes = count(
        conn,
        "SELECT coalesce(sum(size), 0) FROM document WHERE deleted_at IS NULL",
    );
    let versions = count(conn, "SELECT count(DISTINCT hash) FROM version");
    let per_client: Vec<Vec<String>> = {
        let mut st = conn
            .prepare(
                "SELECT c.name || ' (' || c.ext_id || ')', coalesce(c.folder_rel, ''),
                        count(d.id), coalesce(sum(d.size), 0)
                 FROM client c LEFT JOIN document d
                   ON d.client_id = c.id AND d.deleted_at IS NULL
                 WHERE c.deleted_at IS NULL GROUP BY c.id ORDER BY c.name",
            )
            .map_err(|e| e.to_string())?;
        let rows = st
            .query_map([], |r| {
                Ok(vec![
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?.to_string(),
                    size(r.get(3)?),
                ])
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?
    };
    sections.push(Section {
        heading: "1. Cabinet inventory",
        lines: vec![
            format!("Cabinet: {}", root.display()),
            format!(
                "{clients} clients · {docs} documents · {} · {versions} earlier versions kept",
                size(bytes)
            ),
        ],
        table: Some((
            vec![
                Column {
                    heading: "client",
                    width: 40,
                },
                Column {
                    heading: "folder",
                    width: 50,
                },
                Column {
                    heading: "documents",
                    width: 10,
                },
                Column {
                    heading: "size",
                    width: 10,
                },
            ],
            per_client,
        )),
    });

    // 2. Encryption.
    sections.push(Section {
        heading: "2. Encryption status",
        lines: vec![
            "Documents are plain files in the cabinet folder; protect the share and its disk (e.g. BitLocker) and limit who can open it.".into(),
            "Each workstation's catalog (<data>\\catalog.db) is plain SQLite by the firm's decision (2026-11-03): protect the workstation's disk the same way.".into(),
            if root
                .join(crate::cabinet::VIBE_DIR)
                .join(crate::cabinet::KEYS_DIR)
                .join("sync.json")
                .is_file()
            {
                "Off-site copies (B2 sync) are encrypted on the workstation with the firm's key (XChaCha20-Poly1305) before upload; the key is held only by the firm (sync passphrase).".into()
            } else {
                "B2 sync is not set up for this cabinet: no off-site copy is made by Vibe Files.".into()
            },
        ],
        table: None,
    });

    // 3. Workstations.
    let devs = devices(root);
    sections.push(Section {
        heading: "3. Workstations",
        lines: vec![format!(
            "{} workstations have opened this cabinet (no user accounts in v1; actions are recorded with the Windows login).",
            devs.len()
        )],
        table: Some((
            vec![
                Column { heading: "computer", width: 24 },
                Column { heading: "device id", width: 38 },
                Column { heading: "first seen", width: 20 },
                Column { heading: "last seen", width: 20 },
                Column { heading: "version", width: 10 },
            ],
            devs.iter()
                .map(|d| {
                    vec![
                        d.name.clone(),
                        d.id.clone(),
                        d.first_seen.chars().take(19).collect(),
                        d.last_seen.chars().take(19).collect(),
                        d.version.clone(),
                    ]
                })
                .collect(),
        )),
    });

    // 4. Retention and holds.
    let classes = crate::retention::load(root)?.unwrap_or_default();
    let types = crate::doctypes::load(root)?.unwrap_or_default();
    let holds = crate::holds::load(root)?;
    let active: Vec<&crate::holds::Hold> = holds.iter().filter(|h| h.active()).collect();
    let mut lines = vec![format!(
        "{} legal holds active; nothing they cover is disposed of or emptied from the trash.",
        active.len()
    )];
    for h in &active {
        lines.push(format!(
            "Hold since {}: {} ({})",
            h.since.get(..10).unwrap_or(""),
            h.reason,
            if h.client_id.is_some() {
                "a client".to_string()
            } else {
                format!("{} documents", h.docs.len())
            }
        ));
    }
    let unclassified = types.iter().filter(|t| t.retention.is_none()).count();
    if unclassified > 0 {
        lines.push(format!(
            "{unclassified} doc types have no retention class: their documents are never disposed."
        ));
    }
    sections.push(Section {
        heading: "4. Retention schedule and legal holds",
        lines,
        table: Some((
            vec![
                Column {
                    heading: "class",
                    width: 28,
                },
                Column {
                    heading: "kept",
                    width: 56,
                },
                Column {
                    heading: "doc types",
                    width: 10,
                },
            ],
            classes
                .iter()
                .map(|c| {
                    vec![
                        c.name.clone(),
                        c.describe(),
                        types
                            .iter()
                            .filter(|t| t.retention.as_deref() == Some(c.id.as_str()))
                            .count()
                            .to_string(),
                    ]
                })
                .collect(),
        )),
    });

    // 5. Integrity.
    let sweep = crate::sweep::last_sweep(conn)
        .map(|t| {
            format!(
                "Last integrity sweep (files re-hashed against the catalog): {}",
                &t[..19.min(t.len())]
            )
        })
        .unwrap_or_else(|| "No integrity sweep has run on this workstation yet.".into());
    let chain = match vf_core::audit::verify(conn) {
        Ok(r) => match r.broken {
            None => format!(
                "Audit trail verified when this appendix was made ({}): {} rows, intact.",
                &now[..19],
                r.rows
            ),
            Some((seq, why)) => format!("Audit trail does NOT verify at row {seq}: {why}."),
        },
        Err(e) => format!("Audit trail could not be checked: {e}"),
    };
    sections.push(Section {
        heading: "5. Integrity",
        lines: vec![sweep, chain],
        table: None,
    });

    // 6. Backup / sync.
    let last_push: Option<String> = conn
        .query_row("SELECT max(last_push) FROM sync_state", [], |r| r.get(0))
        .ok()
        .flatten();
    sections.push(Section {
        heading: "6. Backup and sync",
        lines: vec![match last_push {
            Some(t) => format!("Last sync to B2: {t}"),
            None => "No sync to B2 has run from this workstation; if sync is not set up, back up the cabinet share with the firm's own backup.".into(),
        }],
        table: None,
    });

    // 7. Disposition runs.
    let runs = crate::disposition::runs(conn, Some(inp.period_from))?;
    sections.push(Section {
        heading: "7. Disposition runs in the period",
        lines: vec![if runs.is_empty() {
            format!(
                "No disposition runs since {}.",
                &inp.period_from[..10.min(inp.period_from.len())]
            )
        } else {
            format!(
                "{} runs from this workstation since {}; every workstation's certificates are filed in _Vibe\\disposition ({} there).",
                runs.len(),
                &inp.period_from[..10.min(inp.period_from.len())],
                std::fs::read_dir(root.join(crate::cabinet::VIBE_DIR).join("disposition"))
                    .map(|d| d.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "pdf")).count())
                    .unwrap_or(0)
            )
        }],
        table: Some((
            vec![
                Column {
                    heading: "when",
                    width: 20,
                },
                Column {
                    heading: "by",
                    width: 24,
                },
                Column {
                    heading: "documents",
                    width: 10,
                },
                Column {
                    heading: "certificate",
                    width: 70,
                },
            ],
            runs.iter()
                .map(|r| {
                    vec![
                        r.ts.chars().take(19).collect(),
                        r.by_user.clone().unwrap_or_default(),
                        r.documents.to_string(),
                        r.certificate.clone(),
                    ]
                })
                .collect(),
        )),
    });

    let subtitle = format!(
        "{} · made {} UTC · period from {} · for the firm's Written Information Security Program",
        inp.cabinet_name,
        &now[..19],
        &inp.period_from[..10.min(inp.period_from.len())]
    );
    Ok(vf_pdf::report::document(
        "Vibe Files safeguards appendix",
        &subtitle,
        &sections,
    ))
}

#[cfg(test)]
mod tests {
    use vf_core::db::Storage;

    use super::*;

    /// PHASES P07 done-when 4: the appendix renders with every section populated.
    #[test]
    fn appendix_has_every_section_populated() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("cab");
        std::fs::create_dir_all(root.join("_Vibe")).unwrap();
        crate::doctypes::seed_if_missing(&root).unwrap();
        crate::retention::seed_if_missing(&root).unwrap();
        let mut conn = vf_core::db::open(&t.path().join("c.db"), Storage::Local).unwrap();
        crate::retention::sync(&mut conn, &root, "dev", "seed").unwrap();
        conn.execute_batch(
            "INSERT INTO client (id, ext_id, name, folder_rel, device_id, updated_at)
               VALUES ('c1', 'BELL2002', 'Bell Ranch', 'Bell Ranch (BELL2002)', 'd', 't');
             INSERT INTO document (id, client_id, rel_path, hash, size, filed_at, device_id, updated_at)
               VALUES ('a', 'c1', 'Bell Ranch (BELL2002)/w2.pdf', 'h', 120000, 't', 'd', 't');
             INSERT INTO disposition_run (id, ts, by_user, device_id, documents, certificate)
               VALUES ('r1', '2026-10-01T10:00:00Z', 'OFFICE\\ann', 'dev', 3,
                       '_Vibe/disposition/Certificate of destruction 2026-10-01 r1.pdf');",
        )
        .unwrap();
        crate::holds::place(
            &mut conn,
            &root,
            "dev",
            crate::holds::Target::Client("c1".into()),
            "IRS exam",
        )
        .unwrap();
        crate::sweep::set_last_sweep(&conn, "2026-11-01T08:00:00Z").unwrap();
        register_device(&root, "dev-1", false).unwrap();
        let first = devices(&root)[0].first_seen.clone();
        register_device(&root, "dev-1", false).unwrap();
        assert_eq!(devices(&root).len(), 1);
        assert_eq!(devices(&root)[0].first_seen, first, "first seen kept");

        let bytes = appendix(
            &conn,
            &root,
            &Inputs {
                cabinet_name: "Firm",
                period_from: "2026-01-01T00:00:00Z",
            },
        )
        .unwrap();
        let d = lopdf::Document::load_mem(&bytes).unwrap();
        let text: String = (1..=d.get_pages().len() as u32)
            .map(|p| d.extract_text(&[p]).unwrap())
            .collect();
        for needle in [
            "1. Cabinet inventory",
            "Bell Ranch (BELL2002)",
            "2. Encryption status",
            "3. Workstations",
            "dev-1",
            "4. Retention schedule and legal holds",
            "Returns & workpapers",
            "1 legal holds active",
            "5. Integrity",
            "Last integrity sweep",
            "intact",
            "6. Backup and sync",
            "7. Disposition runs in the period",
            "Certificate of destruction 2026-10-01",
        ] {
            assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
        }
    }
}
