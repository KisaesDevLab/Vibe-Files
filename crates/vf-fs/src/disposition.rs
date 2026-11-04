//! Disposition (PHASES P07 steps 2–3).
//!
//! - [`eligible`]: documents whose retention has ended (their doc type's class,
//!   `vf_core::retention`), by client and class, leaving out documents under a legal hold
//!   (the catalog's rows and the cabinet's hold file), already approved, or deferred.
//!   "Monthly" is the review cadence the Admin page supports, not a scheduler.
//! - [`approve`]: destroyed after a [`WAIT_DAYS`] wait. [`defer`]: shown again after
//!   [`DEFER_MONTHS`] months.
//! - [`destroy_due`]: for each approved document whose wait is over and that no hold covers
//!   now, every copy of its content is overwritten and deleted: the file, its versions (when no
//!   other document uses the same content), copies left in the trash and Filer's working
//!   folders, its OCR text and index rows. The row stays as a tombstone (`disposed`), a
//!   certificate of destruction PDF is filed in `_Vibe/disposition/`, and the audit trail
//!   links to it.
//!
//! Decisions live in this workstation's catalog: run disposition from one workstation, as
//! Intake (STATE). The holds are re-read from the cabinet just before anything is destroyed.

use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::json;
use time::Date;
use vf_core::audit::AuditEntry;
use vf_core::db::Connection;
use vf_core::retention::{self, Anchor, Standing};
use vf_core::tx::{Actor, NoFileOps, Stmt, Transaction};

use crate::cabinet;

/// Days between approval and destruction.
pub const WAIT_DAYS: i64 = 30;
/// How long a deferred document stays off the report.
pub const DEFER_MONTHS: u8 = 12;
/// `_Vibe/disposition/`: the certificates.
pub const DIR: &str = "disposition";
/// Transaction labels (off the undo stack: decisions are changed by deciding again, and a
/// destruction cannot be undone).
pub const DECIDE_LABEL: &str = "disposition_decide";
pub const RUN_LABEL: &str = "disposition_run";

/// Folders of the cabinet that can hold old copies of a document's bytes.
const COPY_DIRS: [&str; 4] = ["trash", "overwritten", "split_originals", "tmp"];

fn ymd(d: Date) -> String {
    format!("{:04}-{:02}-{:02}", d.year(), d.month() as u8, d.day())
}

fn plus_days(d: Date, days: i64) -> Date {
    d + time::Duration::days(days)
}

fn plus_months(d: Date, months: u8) -> Date {
    let total = d.year() * 12 + (d.month() as i32 - 1) + months as i32;
    let (y, m) = (total.div_euclid(12), (total.rem_euclid(12) + 1) as u8);
    let month = time::Month::try_from(m).expect("1..=12");
    let last = month.length(y);
    Date::from_calendar_date(y, month, d.day().min(last)).expect("valid")
}

/// One document on the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub doc_id: String,
    pub client: Option<String>,
    pub client_id: Option<String>,
    pub rel_path: String,
    pub hash: String,
    pub doc_type: String,
    pub class_id: String,
    pub class: String,
    pub tax_year: Option<i32>,
    pub eligible_on: Date,
    pub size: i64,
}

/// What [`eligible`] found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// By client name, then class, then path.
    pub lines: Vec<Line>,
    /// Live documents with no doc type, or a doc type with no class: never disposed.
    pub unclassified: usize,
    /// Eligible but under a legal hold.
    pub held: usize,
    /// Eligible but approved (waiting) or deferred.
    pub decided: usize,
}

struct Doc {
    id: String,
    client_id: Option<String>,
    client: Option<String>,
    rel_path: String,
    hash: String,
    doc_type: Option<String>,
    class: Option<(String, String, Option<i32>, String)>, // id, name, years, anchor
    tax_year: Option<i32>,
    filed_at: String,
    size: i64,
}

fn live_docs(conn: &Connection) -> Result<Vec<Doc>, String> {
    let mut st = conn
        .prepare(
            "SELECT d.id, d.client_id, c.name, d.rel_path, d.hash, t.name, r.id, r.name, r.years,
                    r.anchor, d.tax_year, d.filed_at, d.size
             FROM document d
             LEFT JOIN client c ON c.id = d.client_id
             LEFT JOIN doc_type t ON t.id = d.doc_type_id
             LEFT JOIN retention_class r ON r.id = t.retention_class_id
             WHERE d.deleted_at IS NULL",
        )
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map([], |r| {
            let class = match r.get::<_, Option<String>>(6)? {
                Some(id) => Some((id, r.get(7)?, r.get(8)?, r.get(9)?)),
                None => None,
            };
            Ok(Doc {
                id: r.get(0)?,
                client_id: r.get(1)?,
                client: r.get(2)?,
                rel_path: r.get(3)?,
                hash: r.get(4)?,
                doc_type: r.get(5)?,
                class,
                tax_year: r.get(10)?,
                filed_at: r.get(11)?,
                size: r.get(12)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
}

/// Whether a hold covers `d` now: this catalog's rows or the cabinet's file.
fn held(
    conn: &Connection,
    root: &Path,
    id: &str,
    client: Option<&str>,
    rel: &str,
    hash: &str,
) -> bool {
    crate::trash::held(conn, id) || crate::holds::covers(root, client, rel, hash)
}

/// Documents whose retention ended on or before `today`.
pub fn eligible(conn: &Connection, root: &Path, today: Date) -> Result<Report, String> {
    let decided: BTreeMap<String, (String, Option<String>)> = {
        let mut st = conn
            .prepare(
                "SELECT doc_id, decision, deferred_until FROM disposition WHERE destroyed_at IS NULL",
            )
            .map_err(|e| e.to_string())?;
        let rows = st
            .query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())?
    };
    let today_s = ymd(today);
    let mut rep = Report::default();
    for d in live_docs(conn)? {
        let Some((class_id, class, years, anchor)) = d.class.clone() else {
            rep.unclassified += 1;
            continue;
        };
        let anchor = Anchor::parse(&anchor).unwrap_or(Anchor::Never);
        let Standing::Eligible(on) =
            retention::standing(anchor, years, d.tax_year, &d.filed_at, today)
        else {
            continue;
        };
        if held(
            conn,
            root,
            &d.id,
            d.client_id.as_deref(),
            &d.rel_path,
            &d.hash,
        ) {
            rep.held += 1;
            continue;
        }
        match decided.get(&d.id) {
            Some((dec, _)) if dec == "approved" => {
                rep.decided += 1;
                continue;
            }
            Some((_, Some(until))) if *until > today_s => {
                rep.decided += 1;
                continue;
            }
            _ => {}
        }
        rep.lines.push(Line {
            doc_id: d.id,
            client: d.client,
            client_id: d.client_id,
            rel_path: d.rel_path,
            hash: d.hash,
            doc_type: d.doc_type.unwrap_or_default(),
            class_id,
            class,
            tax_year: d.tax_year,
            eligible_on: on,
            size: d.size,
        });
    }
    rep.lines.sort_by(|a, b| {
        (a.client.as_deref().unwrap_or("~"), &a.class, &a.rel_path).cmp(&(
            b.client.as_deref().unwrap_or("~"),
            &b.class,
            &b.rel_path,
        ))
    });
    Ok(rep)
}

fn decide(
    conn: &mut Connection,
    device_id: &str,
    lines: &[Line],
    approve: bool,
    today: Date,
) -> Result<usize, String> {
    if lines.is_empty() {
        return Ok(0);
    }
    let now = vf_core::now_rfc3339();
    let by = vf_core::windows_login();
    let (until, after) = if approve {
        (None, Some(ymd(plus_days(today, WAIT_DAYS))))
    } else {
        (Some(ymd(plus_months(today, DEFER_MONTHS))), None)
    };
    let decision = if approve { "approved" } else { "deferred" };
    let mut tx = Transaction::new(DECIDE_LABEL);
    for l in lines {
        tx = tx
            .catalog(Stmt::new(
                "INSERT INTO disposition (doc_id, class_id, eligible_on, decision, decided_by,
                   decided_at, destroy_after, deferred_until, device_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(doc_id) DO UPDATE SET decision = excluded.decision,
                   decided_by = excluded.decided_by, decided_at = excluded.decided_at,
                   destroy_after = excluded.destroy_after, deferred_until = excluded.deferred_until,
                   device_id = excluded.device_id",
                vec![
                    l.doc_id.clone().into(),
                    l.class_id.clone().into(),
                    ymd(l.eligible_on).into(),
                    decision.into(),
                    by.clone().into(),
                    now.clone().into(),
                    after.clone().into(),
                    until.clone().into(),
                    device_id.into(),
                ],
            ))
            .audit(AuditEntry::new(
                if approve {
                    "disposition_approve"
                } else {
                    "disposition_defer"
                },
                Some(&l.doc_id),
                json!({ "class": l.class, "eligible_on": ymd(l.eligible_on),
                        "destroy_after": after, "deferred_until": until }),
            ));
    }
    tx.commit(conn, &mut NoFileOps, &Actor::device(device_id))
        .map_err(|e| e.to_string())?;
    Ok(lines.len())
}

/// Approves the lines: destroyed once [`WAIT_DAYS`] have passed.
pub fn approve(
    conn: &mut Connection,
    device_id: &str,
    lines: &[Line],
    today: Date,
) -> Result<usize, String> {
    decide(conn, device_id, lines, true, today)
}

/// Defers the lines for [`DEFER_MONTHS`] months.
pub fn defer(
    conn: &mut Connection,
    device_id: &str,
    lines: &[Line],
    today: Date,
) -> Result<usize, String> {
    decide(conn, device_id, lines, false, today)
}

/// An approved document waiting for destruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub doc_id: String,
    pub rel_path: String,
    pub client: Option<String>,
    pub class: Option<String>,
    pub destroy_after: String,
    pub decided_by: Option<String>,
}

/// Approved and not destroyed yet, soonest first.
pub fn pending(conn: &Connection) -> Result<Vec<Pending>, String> {
    let mut st = conn
        .prepare(
            "SELECT p.doc_id, d.rel_path, c.name, r.name, p.destroy_after, p.decided_by
             FROM disposition p
             JOIN document d ON d.id = p.doc_id
             LEFT JOIN client c ON c.id = d.client_id
             LEFT JOIN retention_class r ON r.id = p.class_id
             WHERE p.decision = 'approved' AND p.destroyed_at IS NULL AND d.deleted_at IS NULL
             ORDER BY p.destroy_after, d.rel_path",
        )
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map([], |r| {
            Ok(Pending {
                doc_id: r.get(0)?,
                rel_path: r.get(1)?,
                client: r.get(2)?,
                class: r.get(3)?,
                destroy_after: r.get(4)?,
                decided_by: r.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
}

/// Overwrites `p` with zeros, flushes, and deletes it. Best effort on SSDs and NTFS (the
/// certificate says "overwritten and deleted", not "unrecoverable").
fn wipe(p: &Path) -> std::io::Result<()> {
    let len = std::fs::metadata(p)?.len();
    {
        let mut f = std::fs::OpenOptions::new().write(true).open(p)?;
        let zeros = vec![0u8; 64 * 1024];
        let mut left = len;
        while left > 0 {
            let n = left.min(zeros.len() as u64) as usize;
            f.write_all(&zeros[..n])?;
            left -= n as u64;
        }
        f.sync_all()?;
    }
    std::fs::remove_file(p)
}

fn files_in(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            files_in(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// What a run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunReport {
    pub run_id: Option<String>,
    pub destroyed: usize,
    /// Copies besides the file itself (versions, leftovers) wiped.
    pub copies: usize,
    /// Due but now under a hold: kept, still approved.
    pub held: usize,
    /// Could not be wiped (open in another program …): kept, still approved.
    pub failed: Vec<String>,
    /// The certificate, cabinet-relative.
    pub certificate: Option<String>,
}

/// One document being destroyed.
struct Victim {
    doc_id: String,
    rel_path: String,
    hashes: Vec<String>,
    client: Option<String>,
    doc_type: Option<String>,
    class: Option<String>,
    tax_year: Option<i32>,
    eligible_on: String,
    decided_by: Option<String>,
    decided_at: String,
    hash: String,
}

/// Destroys every approved document whose wait ended on or before `today`. `cabinet_name`
/// heads the certificate.
/// Applies a disposal made on another workstation (sync): the document's file, its versions
/// no other document uses and its OCR text are destroyed here too, and it is tombstoned as
/// disposed. `false` (nothing done) when a legal hold here covers it.
pub fn destroy_pulled(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    doc_id: &str,
) -> Result<bool, String> {
    let (rel, hash, client): (String, String, Option<String>) = conn
        .query_row(
            "SELECT rel_path, hash, client_id FROM document WHERE id = ?1 AND deleted_at IS NULL",
            [doc_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|_| "the document is not in the catalog".to_string())?;
    if held(conn, root, doc_id, client.as_deref(), &rel, &hash) {
        return Ok(false);
    }
    let mut hashes = vec![hash.clone()];
    {
        let mut st = conn
            .prepare("SELECT DISTINCT hash FROM version WHERE doc_id = ?1")
            .map_err(|e| e.to_string())?;
        let hs = st
            .query_map([doc_id], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        for h in hs.flatten() {
            if !hashes.contains(&h) {
                hashes.push(h);
            }
        }
    }
    let file = root.join(&rel);
    if file.exists() {
        wipe(&file).map_err(|e| format!("cannot destroy {rel}: {e}"))?;
    }
    for h in &hashes {
        let used: bool = conn
            .query_row(
                "SELECT 1 FROM document WHERE hash = ?1 AND id != ?2
                   AND coalesce(tombstone_reason, '') NOT IN ('disposed', 'purged')
                 UNION SELECT 1 FROM version WHERE hash = ?1 AND doc_id != ?2",
                [h, doc_id],
                |_| Ok(()),
            )
            .is_ok();
        if used {
            continue;
        }
        let stored = cabinet::version_path(root, h);
        if stored.exists() {
            let _ = wipe(&stored);
        }
        let _ = std::fs::remove_file(stored.with_extension("json"));
    }
    let now = vf_core::now_rfc3339();
    let mut tx = Transaction::new(crate::disposition::RUN_LABEL)
        .catalog(Stmt::new(
            "UPDATE document SET deleted_at = ?2, tombstone_reason = 'disposed', rev = rev + 1,
               updated_at = ?2, device_id = ?3 WHERE id = ?1",
            vec![doc_id.into(), now.clone().into(), device_id.into()],
        ))
        .catalog(Stmt::new(
            "DELETE FROM version WHERE doc_id = ?1",
            vec![doc_id.into()],
        ))
        .catalog(Stmt::new(
            "DELETE FROM doc_fts WHERE doc_id = ?1",
            vec![doc_id.into()],
        ))
        .catalog(Stmt::new(
            "DELETE FROM ocr_job WHERE doc_id = ?1",
            vec![doc_id.into()],
        ))
        .audit(AuditEntry::new(
            "disposed_elsewhere",
            Some(doc_id),
            json!({ "versions": hashes.len() - 1 }),
        ));
    for h in &hashes {
        tx = tx.catalog(Stmt::new(
            "DELETE FROM page_text WHERE hash = ?1
               AND NOT EXISTS (SELECT 1 FROM document WHERE hash = ?1 AND deleted_at IS NULL)",
            vec![h.clone().into()],
        ));
    }
    crate::fileops::commit(conn, root, tx, device_id)?;
    Ok(true)
}

pub fn destroy_due(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    today: Date,
    cabinet_name: &str,
) -> Result<RunReport, String> {
    let today_s = ymd(today);
    let (due, held_now): (Vec<Victim>, usize) = {
        let mut st = conn
            .prepare(
                "SELECT p.doc_id, d.rel_path, d.hash, d.client_id, c.name, t.name, r.name,
                        d.tax_year, p.eligible_on, p.decided_by, p.decided_at
                 FROM disposition p
                 JOIN document d ON d.id = p.doc_id
                 LEFT JOIN client c ON c.id = d.client_id
                 LEFT JOIN doc_type t ON t.id = d.doc_type_id
                 LEFT JOIN retention_class r ON r.id = p.class_id
                 WHERE p.decision = 'approved' AND p.destroyed_at IS NULL
                   AND d.deleted_at IS NULL AND p.destroy_after <= ?1",
            )
            .map_err(|e| e.to_string())?;
        let rows = st
            .query_map([&today_s], |r| {
                Ok((
                    Victim {
                        doc_id: r.get(0)?,
                        rel_path: r.get(1)?,
                        hash: r.get(2)?,
                        hashes: vec![],
                        client: r.get(4)?,
                        doc_type: r.get(5)?,
                        class: r.get(6)?,
                        tax_year: r.get(7)?,
                        eligible_on: r.get(8)?,
                        decided_by: r.get(9)?,
                        decided_at: r.get(10)?,
                    },
                    r.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        let mut rep_held = 0;
        for row in rows {
            let (mut v, client_id) = row.map_err(|e| e.to_string())?;
            // Holds again, from the cabinet file, just before anything is destroyed.
            if held(
                conn,
                root,
                &v.doc_id,
                client_id.as_deref(),
                &v.rel_path,
                &v.hash,
            ) {
                rep_held += 1;
                continue;
            }
            v.hashes.push(v.hash.clone());
            out.push(v);
        }
        if out.is_empty() {
            return Ok(RunReport {
                held: rep_held,
                ..Default::default()
            });
        }
        for v in &mut out {
            let mut st = conn
                .prepare("SELECT DISTINCT hash FROM version WHERE doc_id = ?1")
                .map_err(|e| e.to_string())?;
            let hs = st
                .query_map([&v.doc_id], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            for h in hs.flatten() {
                if !v.hashes.contains(&h) {
                    v.hashes.push(h);
                }
            }
        }
        (out, rep_held)
    };
    let mut rep = RunReport {
        held: held_now,
        ..Default::default()
    };
    let ids: HashSet<&str> = due.iter().map(|v| v.doc_id.as_str()).collect();
    // A content stays if any other document (not being destroyed, not already disposed) uses
    // it as its current content or as a version.
    let shared = |conn: &Connection, hash: &str| -> bool {
        let mut st = match conn.prepare(
            "SELECT id FROM document WHERE hash = ?1
               AND coalesce(tombstone_reason, '') NOT IN ('disposed', 'purged')
             UNION SELECT doc_id FROM version WHERE hash = ?1",
        ) {
            Ok(st) => st,
            Err(_) => return true,
        };
        let users: Vec<String> = st
            .query_map([hash], |r| r.get(0))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default();
        users.iter().any(|u| !ids.contains(u.as_str()))
    };
    // Leftover copies in the cabinet's working folders, by content.
    let mut leftovers: Vec<(PathBuf, String)> = Vec::new();
    {
        let mut files = Vec::new();
        for d in COPY_DIRS {
            files_in(&root.join(cabinet::VIBE_DIR).join(d), &mut files);
        }
        let wanted: HashSet<&str> = due
            .iter()
            .flat_map(|v| v.hashes.iter().map(String::as_str))
            .collect();
        for f in files {
            if let Ok(h) = filer_intake::commit::sha256_file(&f)
                && wanted.contains(h.as_str())
            {
                leftovers.push((f, h));
            }
        }
    }
    let mut done: Vec<&Victim> = Vec::new();
    for v in &due {
        // The file itself first: if it cannot be wiped (open in Acrobat), nothing of this
        // document is touched and its approval stays.
        let file = root.join(&v.rel_path);
        if file.exists()
            && let Err(e) = wipe(&file)
        {
            rep.failed.push(format!("{}: {e}", v.rel_path));
            continue;
        }
        for h in &v.hashes {
            if shared(conn, h) {
                continue;
            }
            let stored = cabinet::version_path(root, h);
            if stored.exists() && wipe(&stored).is_ok() {
                rep.copies += 1;
            }
            let sidecar = stored.with_extension("json");
            if sidecar.exists() {
                let _ = std::fs::remove_file(&sidecar);
            }
            for (p, lh) in &leftovers {
                if lh == h && p.exists() && wipe(p).is_ok() {
                    rep.copies += 1;
                }
            }
        }
        done.push(v);
    }
    if done.is_empty() {
        return Ok(rep);
    }
    let run_id = vf_core::new_id();
    let now = vf_core::now_rfc3339();
    let cert_rel = format!(
        "{}/{}/Certificate of destruction {} {}.pdf",
        cabinet::VIBE_DIR,
        DIR,
        &now[..10],
        &run_id[run_id.len() - 8..]
    );
    let by = vf_core::windows_login();
    // The certificate: what was destroyed, when, by whom, approved by whom.
    {
        use vf_pdf::report::Column;
        let cols = [
            Column {
                heading: "client",
                width: 24,
            },
            Column {
                heading: "document",
                width: 44,
            },
            Column {
                heading: "doc type",
                width: 16,
            },
            Column {
                heading: "class",
                width: 18,
            },
            Column {
                heading: "year",
                width: 5,
            },
            Column {
                heading: "eligible",
                width: 10,
            },
            Column {
                heading: "approved by",
                width: 18,
            },
            Column {
                heading: "sha-256",
                width: 16,
            },
        ];
        let rows: Vec<Vec<String>> = done
            .iter()
            .map(|v| {
                vec![
                    v.client.clone().unwrap_or_default(),
                    v.rel_path.clone(),
                    v.doc_type.clone().unwrap_or_default(),
                    v.class.clone().unwrap_or_default(),
                    v.tax_year.map(|y| y.to_string()).unwrap_or_default(),
                    v.eligible_on.clone(),
                    format!(
                        "{} {}",
                        v.decided_by.clone().unwrap_or_default(),
                        v.decided_at.get(..10).unwrap_or("")
                    ),
                    v.hash.clone(),
                ]
            })
            .collect();
        let subtitle = format!(
            "{cabinet_name} · run {run_id} · {now} UTC · by {} on device {device_id} · {} documents and {} other copies overwritten and deleted (file, versions not shared with other documents, leftover copies in the cabinet's working folders, OCR text and index entries). Rows remain in the catalog as tombstones.",
            by.clone().unwrap_or_else(|| "unknown".into()),
            done.len(),
            rep.copies
        );
        let pdf = vf_pdf::report::table("Certificate of destruction", &subtitle, &cols, &rows);
        let path = root.join(&cert_rel);
        std::fs::create_dir_all(path.parent().expect("has a parent"))
            .map_err(|e| format!("cannot create the disposition folder: {e}"))?;
        cabinet::write_new(&path, &pdf)?;
    }
    let mut tx = Transaction::new(RUN_LABEL).catalog(Stmt::new(
        "INSERT INTO disposition_run (id, ts, by_user, device_id, documents, certificate)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        vec![
            run_id.clone().into(),
            now.clone().into(),
            by.clone().into(),
            device_id.into(),
            (done.len() as i64).into(),
            cert_rel.clone().into(),
        ],
    ));
    for v in &done {
        tx = tx
            .catalog(Stmt::new(
                "UPDATE document SET deleted_at = ?2, tombstone_reason = 'disposed', rev = rev + 1,
                 updated_at = ?2, device_id = ?3 WHERE id = ?1",
                vec![
                    v.doc_id.clone().into(),
                    now.clone().into(),
                    device_id.into(),
                ],
            ))
            .catalog(Stmt::new(
                "UPDATE disposition SET destroyed_at = ?2, run_id = ?3 WHERE doc_id = ?1",
                vec![
                    v.doc_id.clone().into(),
                    now.clone().into(),
                    run_id.clone().into(),
                ],
            ))
            .catalog(Stmt::new(
                "DELETE FROM doc_fts WHERE doc_id = ?1",
                vec![v.doc_id.clone().into()],
            ))
            .catalog(Stmt::new(
                "DELETE FROM ocr_job WHERE doc_id = ?1",
                vec![v.doc_id.clone().into()],
            ))
            .catalog(Stmt::new(
                "DELETE FROM version WHERE doc_id = ?1",
                vec![v.doc_id.clone().into()],
            ))
            .catalog(Stmt::new(
                "INSERT INTO sync_pending (doc_id, op, since) VALUES (?1, 'delete', ?2)
                 ON CONFLICT(doc_id, op) DO NOTHING",
                vec![v.doc_id.clone().into(), now.clone().into()],
            ))
            .audit(AuditEntry::new(
                "dispose",
                Some(&v.doc_id),
                json!({ "rel_path": v.rel_path, "class": v.class, "run": run_id,
                        "certificate": cert_rel }),
            ));
        for h in &v.hashes {
            // Page text is kept by content: only when nothing else has that content.
            tx = tx.catalog(Stmt::new(
                "DELETE FROM page_text WHERE hash = ?1
                   AND NOT EXISTS (SELECT 1 FROM document WHERE hash = ?1 AND deleted_at IS NULL)",
                vec![h.clone().into()],
            ));
        }
    }
    tx = tx.audit(AuditEntry::new(
        "disposition_run",
        None,
        json!({ "run": run_id, "documents": done.len(), "copies": rep.copies,
                "certificate": cert_rel }),
    ));
    tx.commit(conn, &mut NoFileOps, &Actor::device(device_id))
        .map_err(|e| e.to_string())?;
    rep.run_id = Some(run_id);
    rep.destroyed = done.len();
    rep.certificate = Some(cert_rel);
    Ok(rep)
}

/// A finished run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub id: String,
    pub ts: String,
    pub by_user: Option<String>,
    pub documents: i64,
    pub certificate: String,
}

/// Runs, newest first; `since`: RFC 3339 lower bound.
pub fn runs(conn: &Connection, since: Option<&str>) -> Result<Vec<Run>, String> {
    let mut st = conn
        .prepare(
            "SELECT id, ts, by_user, documents, certificate FROM disposition_run
             WHERE ?1 IS NULL OR ts >= ?1 ORDER BY ts DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map([since], |r| {
            Ok(Run {
                id: r.get(0)?,
                ts: r.get(1)?,
                by_user: r.get(2)?,
                documents: r.get(3)?,
                certificate: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use vf_core::db::Storage;

    use super::*;

    fn d(y: i32, m: u8, day: u8) -> Date {
        Date::from_calendar_date(y, time::Month::try_from(m).unwrap(), day).unwrap()
    }

    struct Fx {
        _t: tempfile::TempDir,
        root: PathBuf,
        conn: Connection,
    }

    /// Two clients (c1 under a legal hold), the seeded doc types and schedule, and documents
    /// with synthetic years and filing dates. Each file's content is its own path.
    fn fx() -> Fx {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("cab");
        std::fs::create_dir_all(root.join("_Vibe")).unwrap();
        crate::doctypes::seed_if_missing(&root).unwrap();
        crate::retention::seed_if_missing(&root).unwrap();
        let mut conn = vf_core::db::open(&t.path().join("c.db"), Storage::Local).unwrap();
        crate::retention::sync(&mut conn, &root, "dev", "seed").unwrap();
        conn.execute_batch(
            "INSERT INTO client (id, ext_id, name, folder_rel, device_id, updated_at)
               VALUES ('c1', 'ANDE1042', 'Anderson Farms LLC', 'A', 'd', 't'),
                      ('c2', 'BELL2002', 'Bell Ranch', 'B', 'd', 't');",
        )
        .unwrap();
        #[allow(clippy::type_complexity)]
        let docs: [(&str, &str, &str, Option<&str>, Option<i32>, &str); 8] = [
            (
                "held-w2",
                "c1",
                "A/2018/w2.pdf",
                Some("W-2"),
                Some(2018),
                "2019-02-01T00:00:00Z",
            ),
            (
                "w2-2019",
                "c2",
                "B/2019/w2.pdf",
                Some("W-2"),
                Some(2019),
                "2020-02-01T00:00:00Z",
            ),
            (
                "w2-2023",
                "c2",
                "B/2023/w2.pdf",
                Some("W-2"),
                Some(2023),
                "2024-02-01T00:00:00Z",
            ),
            (
                "ret-2018",
                "c2",
                "B/2018/return.pdf",
                Some("Return copy — federal"),
                Some(2018),
                "2019-04-01T00:00:00Z",
            ),
            (
                "ret-2020",
                "c2",
                "B/2020/return.pdf",
                Some("Return copy — federal"),
                Some(2020),
                "2021-04-01T00:00:00Z",
            ),
            (
                "ein",
                "c2",
                "B/Permanent/ein.pdf",
                Some("EIN letter"),
                None,
                "2010-01-01T00:00:00Z",
            ),
            (
                "loose",
                "c2",
                "B/2015/scan.pdf",
                None,
                Some(2015),
                "2016-01-01T00:00:00Z",
            ),
            (
                "engage",
                "c2",
                "B/2017/engagement.pdf",
                Some("Engagement letter"),
                Some(2017),
                "2017-01-15T00:00:00Z",
            ),
        ];
        for (id, client, rel, ty, year, filed) in docs {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, rel.as_bytes()).unwrap();
            let hash = filer_intake::commit::sha256_file(&p).unwrap();
            conn.execute(
                "INSERT INTO document (id, client_id, doc_type_id, tax_year, rel_path, hash, size,
                   filed_at, device_id, updated_at)
                 VALUES (?1, ?2, (SELECT id FROM doc_type WHERE name = ?3), ?4, ?5, ?6, 10, ?7, 'd', 't')",
                vf_core::db::params![id, client, ty, year, rel, hash, filed],
            )
            .unwrap();
        }
        crate::holds::place(
            &mut conn,
            &root,
            "dev",
            crate::holds::Target::Client("c1".into()),
            "IRS exam",
        )
        .unwrap();
        Fx { _t: t, root, conn }
    }

    fn ids(r: &Report) -> Vec<&str> {
        let mut v: Vec<&str> = r.lines.iter().map(|l| l.doc_id.as_str()).collect();
        v.sort();
        v
    }

    /// PHASES P07 done-whens 1 and 2: synthetic dates land in the right buckets; a held
    /// client's documents never appear.
    #[test]
    fn buckets_by_class_and_date_and_holds_stay_out() {
        let f = fx();
        let today = d(2026, 11, 3);
        let r = eligible(&f.conn, &f.root, today).unwrap();
        // W-2 2019 (4y, from 2024), return 2018 (7y, from 2026-01-01), engagement 2017 (7y).
        assert_eq!(ids(&r), ["engage", "ret-2018", "w2-2019"]);
        assert_eq!(r.unclassified, 1, "the document with no doc type");
        assert_eq!(r.held, 1, "the held client's 2018 W-2");
        let w2 = r.lines.iter().find(|l| l.doc_id == "w2-2019").unwrap();
        assert_eq!(
            (w2.class.as_str(), w2.eligible_on),
            ("Client source documents", d(2024, 1, 1))
        );
        // A day before the 2018 return's retention ends, it is not on the report.
        let r = eligible(&f.conn, &f.root, d(2025, 12, 31)).unwrap();
        assert_eq!(ids(&r), ["engage", "w2-2019"]);
        // Years later, everything dated is, except what the hold keeps and the permanent file.
        let r = eligible(&f.conn, &f.root, d(2040, 1, 1)).unwrap();
        assert_eq!(
            ids(&r),
            ["engage", "ret-2018", "ret-2020", "w2-2019", "w2-2023"]
        );
        assert!(r.lines.iter().all(|l| l.client_id.as_deref() != Some("c1")));
    }

    #[test]
    fn approve_waits_30_days_and_defer_hides_for_a_year() {
        let mut f = fx();
        let today = d(2026, 11, 3);
        let r = eligible(&f.conn, &f.root, today).unwrap();
        let (ret, rest): (Vec<Line>, Vec<Line>) =
            r.lines.into_iter().partition(|l| l.doc_id == "ret-2018");
        approve(&mut f.conn, "dev", &ret, today).unwrap();
        defer(&mut f.conn, "dev", &rest, today).unwrap();
        assert!(eligible(&f.conn, &f.root, today).unwrap().lines.is_empty());
        let p = pending(&f.conn).unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].destroy_after, "2026-12-03");
        // Nothing is due yet.
        let run = destroy_due(&mut f.conn, &f.root, "dev", today, "Firm").unwrap();
        assert_eq!(run.destroyed, 0);
        assert!(f.root.join("B/2018/return.pdf").is_file());
        // Deferred ones come back after 12 months.
        assert_eq!(
            ids(&eligible(&f.conn, &f.root, d(2027, 11, 3)).unwrap()),
            ["engage", "w2-2019"]
        );
    }

    /// PHASES P07 done-when 3: secure delete leaves no copy in the versions store, the
    /// cabinet's leftover copies, or the OCR text and index; the tombstone and the certificate
    /// remain. (Thumbnails are only ever in memory: nothing on disk to remove.)
    #[test]
    fn destruction_leaves_no_copy_and_keeps_the_record() {
        let mut f = fx();
        let root = f.root.clone();
        // The 2018 return gets an earlier version (kept in the store) and a leftover copy.
        let tmp = crate::content::tmp_file(&root, "pdf").unwrap();
        std::fs::write(&tmp, b"return with annotations").unwrap();
        let rep = crate::content::replace(
            &mut f.conn,
            &root,
            "dev",
            "ret-2018",
            &tmp,
            crate::content::Change {
                label: "annotate",
                created_by: "annotate",
                reason: "before annotations",
                detail: json!({}),
                extra: vec![],
            },
        )
        .unwrap()
        .unwrap();
        let old = cabinet::version_path(&root, &rep.old_hash);
        assert!(old.is_file());
        let leftover = root.join("_Vibe/trash/annotate-x-return.pdf");
        std::fs::create_dir_all(leftover.parent().unwrap()).unwrap();
        std::fs::copy(&old, &leftover).unwrap();
        f.conn
            .execute_batch(&format!(
                "INSERT INTO page_text (hash, page, source, text) VALUES ('{h}', 0, 'pdf', 'Form 1040');
                 INSERT INTO doc_fts (text, doc_id, page) VALUES ('Form 1040', 'ret-2018', 0);",
                h = rep.new_hash
            ))
            .unwrap();

        let today = d(2026, 11, 3);
        let lines: Vec<Line> = eligible(&f.conn, &root, today)
            .unwrap()
            .lines
            .into_iter()
            .filter(|l| l.doc_id == "ret-2018" || l.doc_id == "w2-2019")
            .collect();
        approve(&mut f.conn, "dev", &lines, today).unwrap();
        // A hold placed on the W-2 after approval keeps it.
        crate::holds::place(
            &mut f.conn,
            &root,
            "dev",
            crate::holds::Target::Docs(vec!["w2-2019".into()]),
            "subpoena",
        )
        .unwrap();
        let run = destroy_due(&mut f.conn, &root, "dev", d(2026, 12, 3), "Firm").unwrap();
        assert_eq!((run.destroyed, run.held), (1, 1), "{run:?}");
        assert!(run.copies >= 2, "{run:?}");
        assert!(!root.join("B/2018/return.pdf").exists());
        assert!(!old.exists(), "no copy in the versions store");
        assert!(!old.with_extension("json").exists());
        assert!(!leftover.exists(), "no leftover copy");
        assert!(root.join("B/2019/w2.pdf").is_file(), "held: kept");
        let n = |sql: &str| -> i64 { f.conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(n("SELECT count(*) FROM page_text"), 0);
        assert_eq!(
            n("SELECT count(*) FROM doc_fts WHERE doc_id = 'ret-2018'"),
            0
        );
        assert_eq!(
            n("SELECT count(*) FROM version WHERE doc_id = 'ret-2018'"),
            0
        );
        assert_eq!(
            n(
                "SELECT count(*) FROM document WHERE id = 'ret-2018' AND tombstone_reason = 'disposed'"
            ),
            1,
            "the tombstone stays"
        );
        let cert = root.join(run.certificate.as_ref().unwrap());
        assert!(cert.is_file());
        let text = lopdf::Document::load(&cert)
            .unwrap()
            .extract_text(&[1])
            .unwrap();
        assert!(
            text.contains("Certificate of destruction") && text.contains("B/2018/return.pdf"),
            "{text}"
        );
        assert_eq!(
            n("SELECT count(*) FROM audit WHERE action = 'dispose'
               AND json_extract(detail, '$.certificate') IS NOT NULL"),
            1
        );
        assert_eq!(runs(&f.conn, None).unwrap().len(), 1);
        // The other documents are untouched.
        assert!(root.join("B/2020/return.pdf").is_file());
    }
}
