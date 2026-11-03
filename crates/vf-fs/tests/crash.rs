//! PHASES P13 step 2: crash safety.
//!
//! - A child process changes documents as fast as it can (new content kept as versions, the way
//!   redaction, stamps and OCR write-back do; metadata edits; renames) and is killed at random
//!   moments, many times. After each kill, recovery runs and the cabinet must agree with the
//!   catalog: every live document's file is there with its catalog hash, every version's
//!   content is kept, no `~vfd-partial-` file is left, no journal row remains.
//! - Transaction rollback is fuzzed in-process: a file op fails at a random step and nothing
//!   may change, on disk or in the catalog.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use vf_core::audit::AuditEntry;
use vf_core::db::{Connection, Storage};
use vf_core::tx::{AppliedOp, FileOp, FileOps, Transaction};
use vf_fs::cabinet::CabinetFile;

const DOCS: usize = 12;

fn cabinet(dir: &Path) -> (PathBuf, Connection) {
    let root = dir.join("cab");
    vf_fs::cabinet::create(
        &root,
        &CabinetFile {
            cabinet_id: vf_core::new_id(),
            firm_id: uuid::Uuid::new_v4().to_string(),
            schema_version: vf_core::db::SCHEMA_VERSION,
            layout_version: 1,
            created: vf_core::now_rfc3339(),
            name: None,
        },
    )
    .unwrap();
    let mut conn = vf_core::db::open(&dir.join("catalog.db"), Storage::Local).unwrap();
    vf_fs::doctypes::seed_if_missing(&root).unwrap();
    vf_fs::doctypes::sync(&mut conn, &root, "dev", "seed").unwrap();
    for i in 0..DOCS {
        let p = root.join(format!("Loose/{:02}/doc {i}.pdf", i % 3));
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, format!("invented content {i} v0")).unwrap();
    }
    vf_fs::rebuild::rebuild(&mut conn, &root, "dev").unwrap();
    (root, conn)
}

fn ids(conn: &Connection) -> Vec<String> {
    let mut st = conn
        .prepare("SELECT id FROM document WHERE deleted_at IS NULL ORDER BY id")
        .unwrap();
    st.query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

/// The child: changes documents until it is killed.
#[test]
#[ignore = "run by crash_kill_and_recover as a child process"]
fn crash_child() {
    let Ok(dir) = std::env::var("VF_CRASH_DIR") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let root = dir.join("cab");
    let mut conn = vf_core::db::open(&dir.join("catalog.db"), Storage::Local).unwrap();
    let types: Vec<String> = {
        let mut st = conn.prepare("SELECT id FROM doc_type").unwrap();
        st.query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    let mut n: u64 = std::process::id() as u64;
    loop {
        n = n
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let docs = ids(&conn);
        let id = &docs[(n >> 33) as usize % docs.len()];
        match (n >> 20) % 3 {
            0 => {
                let tmp = vf_fs::content::tmp_file(&root, "pdf").unwrap();
                std::fs::write(&tmp, format!("content {n}")).unwrap();
                let _ = vf_fs::content::replace(
                    &mut conn,
                    &root,
                    "dev",
                    id,
                    &tmp,
                    vf_fs::content::Change {
                        label: "redact",
                        created_by: "crash-test",
                        reason: "before redaction",
                        detail: serde_json::json!({}),
                        extra: vec![],
                    },
                );
            }
            1 => {
                let t = types[(n >> 40) as usize % types.len()].clone();
                let _ = vf_fs::docmeta::edit(
                    &mut conn,
                    &root,
                    std::slice::from_ref(id),
                    &vf_fs::docmeta::MetaEdit {
                        doc_type: Some(Some(t)),
                        tax_year: Some(Some(2000 + (n % 25) as i32)),
                        ..Default::default()
                    },
                    "dev",
                );
            }
            _ => {
                let rows = vf_fs::rename::plan(
                    &conn,
                    &root,
                    std::slice::from_ref(id),
                    &format!("doc {n}{{Ext}}"),
                    2026,
                )
                .unwrap_or_default();
                let _ = vf_fs::rename::commit(&mut conn, &root, &rows, "dev");
            }
        }
    }
}

fn partials(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            partials(&p, out);
        } else if e.file_name().to_string_lossy().starts_with("~vfd-partial-") {
            out.push(p);
        }
    }
}

/// Every live document's file matches the catalog; versions are kept; nothing half-written.
fn consistent(conn: &Connection, root: &Path) -> Result<(), String> {
    let mut st = conn
        .prepare("SELECT rel_path, hash FROM document WHERE deleted_at IS NULL")
        .unwrap();
    let rows: Vec<(String, String)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    for (rel, hash) in rows {
        let have = filer_intake::commit::sha256_file(&root.join(&rel))
            .map_err(|e| format!("{rel}: {e}"))?;
        if have != hash {
            return Err(format!("{rel}: on disk {have}, catalog {hash}"));
        }
    }
    let mut st = conn.prepare("SELECT hash FROM version").unwrap();
    for h in st.query_map([], |r| r.get::<_, String>(0)).unwrap() {
        let h = h.unwrap();
        if !vf_fs::cabinet::version_path(root, &h).is_file() {
            return Err(format!("version {h} is not kept"));
        }
    }
    let mut left = vec![];
    partials(root, &mut left);
    if !left.is_empty() {
        return Err(format!("partial files left: {left:?}"));
    }
    let j: i64 = conn
        .query_row("SELECT count(*) FROM tx_journal", [], |r| r.get(0))
        .unwrap();
    if j != 0 {
        return Err(format!("{j} journal rows left"));
    }
    Ok(())
}

#[test]
fn crash_kill_and_recover() {
    let t = tempfile::tempdir().unwrap();
    let (root, conn) = cabinet(t.path());
    drop(conn);
    let exe = std::env::current_exe().unwrap();
    let mut recovered = 0;
    for round in 0..15u64 {
        let mut child = std::process::Command::new(&exe)
            .args(["--exact", "crash_child", "--ignored", "--nocapture"])
            .env("VF_CRASH_DIR", t.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(150 + (round * 37) % 300));
        child.kill().unwrap();
        child.wait().unwrap();
        let conn = vf_core::db::open(&t.path().join("catalog.db"), Storage::Local).unwrap();
        let r = vf_fs::fileops::recover(&conn).unwrap();
        recovered += r.transactions;
        assert!(r.problems.is_empty(), "round {round}: {:?}", r.problems);
        if let Err(e) = consistent(&conn, &root) {
            panic!("round {round} (recovered {r:?}): {e}");
        }
    }
    eprintln!("15 kills, {recovered} interrupted transactions put back");
}

/// Fails at the `fail_at`-th file op.
struct Flaky {
    inner: vf_fs::fileops::FilerFileOps,
    n: usize,
    fail_at: usize,
}

impl FileOps for Flaky {
    fn apply(&mut self, op: &FileOp) -> Result<AppliedOp, String> {
        self.n += 1;
        if self.n == self.fail_at {
            return Err("injected failure".into());
        }
        self.inner.apply(op)
    }
    fn reverse(&mut self, a: &AppliedOp) -> Result<(), String> {
        self.inner.reverse(a)
    }
}

fn snapshot(root: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    fn walk(d: &Path, root: &Path, out: &mut BTreeMap<String, String>) {
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            if p.is_dir() {
                out.insert(rel, "dir".into());
                walk(&p, root, out);
            } else {
                out.insert(rel, filer_intake::commit::sha256_file(&p).unwrap());
            }
        }
    }
    walk(root, root, &mut out);
    out
}

fn catalog_state(conn: &Connection) -> String {
    let mut st = conn
        .prepare(
            "SELECT id, rel_path, hash, rev, coalesce(doc_type_id, '') FROM document ORDER BY id",
        )
        .unwrap();
    let rows: Vec<String> = st
        .query_map([], |r| {
            Ok(format!(
                "{}|{}|{}|{}|{}",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?
            ))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    let a: i64 = conn
        .query_row("SELECT count(*) FROM audit", [], |r| r.get(0))
        .unwrap();
    format!("{rows:?} audit={a}")
}

#[test]
fn transaction_rollback_fuzz() {
    let t = tempfile::tempdir().unwrap();
    let (root, mut conn) = cabinet(t.path());
    let docs = ids(&conn);
    let mut n: u64 = 0x5DEECE66D;
    for round in 0..200 {
        n = n
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let before = (snapshot(&root), catalog_state(&conn));
        // A transaction of 2–6 ops over random documents: moves, copies, folders, appends.
        let k = 2 + (n >> 60) as usize % 5;
        let mut tx =
            Transaction::new("fuzz").audit(AuditEntry::new("fuzz", None, serde_json::json!({})));
        let mut moved: Vec<String> = vec![];
        for j in 0..k {
            let m = n.rotate_left(j as u32 * 7);
            let id = &docs[(m >> 33) as usize % docs.len()];
            let rel: String = conn
                .query_row("SELECT rel_path FROM document WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .unwrap();
            let src = root.join(&rel);
            match m % 4 {
                0 if !moved.contains(&rel) => {
                    moved.push(rel.clone());
                    tx = tx
                        .file(FileOp::Move {
                            src: src.clone(),
                            dst: root.join(format!("Moved/{round}/{j}.pdf")),
                        })
                        .catalog(vf_core::tx::Stmt::new(
                            "UPDATE document SET rev = rev + 1 WHERE id = ?1",
                            vec![id.clone().into()],
                        ));
                }
                1 if !moved.contains(&rel) => {
                    tx = tx.file(FileOp::Copy {
                        src,
                        dst: root.join(format!("Copies/{round}/{j}.pdf")),
                    });
                }
                2 => {
                    tx = tx.file(FileOp::Mkdir {
                        path: root.join(format!("New/{round}/{j}")),
                    });
                }
                _ => {
                    tx = tx.file(FileOp::AppendLines {
                        path: root.join("_Vibe/fuzz.jsonl"),
                        lines: vec![format!("{{\"round\":{round},\"op\":{j}}}")],
                        undo_lines: vec![],
                    });
                }
            }
        }
        let ops = k;
        let fail_at = 1 + (n >> 50) as usize % ops;
        let mut fs = Flaky {
            inner: vf_fs::fileops::FilerFileOps::new(&[&root]).unwrap(),
            n: 0,
            fail_at,
        };
        let r = tx.commit(&mut conn, &mut fs, &vf_core::tx::Actor::device("dev"));
        if fs.n >= fail_at {
            assert!(r.is_err(), "round {round}");
            let after = (snapshot(&root), catalog_state(&conn));
            // Appended lines stay (the file only grows) but nothing else may differ.
            let strip = |m: &BTreeMap<String, String>| {
                m.iter()
                    .filter(|(k, _)| !k.ends_with("fuzz.jsonl") && !k.ends_with("_Vibe"))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<BTreeMap<_, _>>()
            };
            assert_eq!(
                strip(&after.0),
                strip(&before.0),
                "round {round}: files changed"
            );
            assert_eq!(after.1, before.1, "round {round}: catalog changed");
            let j: i64 = conn
                .query_row("SELECT count(*) FROM tx_journal", [], |r| r.get(0))
                .unwrap();
            assert_eq!(j, 0, "round {round}: journal left");
        }
    }
}
