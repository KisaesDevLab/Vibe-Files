//! Move documents to another folder of their client folder (PA-23: drag and drop in Browse).
//!
//! [`plan`] is the preview; [`commit`] moves in one Transaction the way a rename does
//! ([`crate::rename::commit_as`]: `FileOp::Move`, Filer `transfer`, never replacing a file;
//! catalog paths and `docmeta.jsonl` keys follow), so Undo puts every file back.
//! A name already taken in the destination gets Filer's ` (2)` form (Keep both).

use std::collections::HashSet;
use std::path::Path;

use filer_core::template;
use vf_core::db::Connection;

use crate::rename::{Outcome, Row};

pub const LABEL: &str = "doc_move";

/// What moving `ids` into `dest` (cabinet-relative folder, `/`-separated) would do.
/// Only folders inside the documents' own client folder are offered; a document of another
/// client, or with none, is skipped with the reason.
pub fn plan(
    conn: &Connection,
    root: &Path,
    ids: &[String],
    dest: &str,
) -> Result<Vec<Row>, String> {
    let dest = dest.trim_matches('/');
    if dest.is_empty() {
        return Err("Pick a folder inside a client folder.".into());
    }
    let dest_path = template::checked_join(root, Path::new(dest)).map_err(|e| e.to_string())?;
    if !dest_path.is_dir() {
        return Err(format!("The folder {dest} is not on disk any more."));
    }
    let taken_on_disk: HashSet<String> = std::fs::read_dir(&dest_path)
        .map_err(|e| format!("{dest}: {e}"))?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_lowercase())
        .collect();
    let mut st = conn
        .prepare_cached(
            "SELECT d.rel_path, c.folder_rel, l.user
             FROM document d
             LEFT JOIN client c ON c.id = d.client_id AND c.deleted_at IS NULL
             LEFT JOIN lock l ON l.doc_id = d.id
             WHERE d.id = ?1 AND d.deleted_at IS NULL",
        )
        .map_err(|e| e.to_string())?;
    let mut claimed: HashSet<String> = HashSet::new();
    let mut rows = Vec::with_capacity(ids.len());
    for id in ids {
        let (rel, client_folder, locked_by): (String, Option<String>, Option<String>) = st
            .query_row([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(|e| format!("document {id}: {e}"))?;
        let (dir, name) = rel.rsplit_once('/').unwrap_or(("", &rel));
        let inside = |cf: &str| dest == cf || dest.starts_with(&format!("{cf}/"));
        let outcome = if let Some(u) = locked_by {
            Outcome::Skip(format!("checked out by {u}"))
        } else if !client_folder.as_deref().is_some_and(inside) {
            Outcome::Skip("that folder belongs to another client".into())
        } else if dir == dest {
            Outcome::Same
        } else {
            let taken = |n: &str| {
                let n = n.to_lowercase();
                taken_on_disk.contains(&n) || claimed.contains(&n)
            };
            let free = crate::rename::free_name(name, taken);
            let new_rel = format!("{dest}/{free}");
            match template::checked_join(root, Path::new(&new_rel)) {
                Err(e) => Outcome::Skip(e.to_string()),
                Ok(_) => {
                    claimed.insert(free.to_lowercase());
                    if free == name {
                        Outcome::Rename(new_rel)
                    } else {
                        Outcome::KeepBoth(new_rel)
                    }
                }
            }
        };
        rows.push(Row {
            doc_id: id.clone(),
            old_rel: rel,
            outcome,
        });
    }
    Ok(rows)
}

/// Moves the rows that move, in one Transaction. Returns (transaction id, files moved).
pub fn commit(
    conn: &mut Connection,
    root: &Path,
    rows: &[Row],
    device_id: &str,
) -> Result<(String, usize), String> {
    if !rows.iter().any(Row::renames) {
        return Err("Nothing to move: the documents are in that folder already.".into());
    }
    crate::rename::commit_as(conn, root, rows, device_id, LABEL)
}

#[cfg(test)]
mod tests {
    use vf_core::db::Storage;

    use super::*;

    const ACME: &str = "Acme LLC (1001)";
    const BAKER: &str = "Baker Co (1002)";

    struct Fx {
        _t: tempfile::TempDir,
        root: std::path::PathBuf,
        conn: Connection,
    }

    fn fx() -> Fx {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("cab");
        for d in [
            "Income Tax",
            "Income Tax/2025",
            "Correspondence/2025",
            "Permanent",
        ] {
            std::fs::create_dir_all(root.join(ACME).join(d)).unwrap();
        }
        std::fs::create_dir_all(root.join(BAKER).join("Income Tax")).unwrap();
        let conn = vf_core::db::open(&t.path().join("c.db"), Storage::Local).unwrap();
        for (id, ext, name, folder) in [
            (
                "00000000-0000-0000-0000-000000000001",
                "1001",
                "Acme LLC",
                ACME,
            ),
            (
                "00000000-0000-0000-0000-000000000002",
                "1002",
                "Baker Co",
                BAKER,
            ),
        ] {
            conn.execute(
                "INSERT INTO client (id, ext_id, name, folder_rel, device_id, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 'dev', 't')",
                (id, ext, name, folder),
            )
            .unwrap();
        }
        let docs = [
            ("d0", 1, format!("{ACME}/Income Tax/letter.pdf")),
            ("d1", 1, format!("{ACME}/Income Tax/w2.pdf")),
            ("d2", 1, format!("{ACME}/Income Tax/2025/w2.pdf")),
            ("d3", 2, format!("{BAKER}/Income Tax/b.pdf")),
        ];
        for (i, (id, c, rel)) in docs.iter().enumerate() {
            std::fs::write(root.join(rel), format!("content {i}")).unwrap();
            conn.execute(
                "INSERT INTO document (id, client_id, rel_path, hash, size, filed_at, device_id, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 9, 't', 'dev', 't')",
                (
                    *id,
                    format!("00000000-0000-0000-0000-00000000000{c}"),
                    rel,
                    format!("{i:064x}"),
                ),
            )
            .unwrap();
        }
        Fx { _t: t, root, conn }
    }

    fn rel(f: &Fx, id: &str) -> String {
        f.conn
            .query_row("SELECT rel_path FROM document WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap()
    }

    #[test]
    fn moves_into_a_folder_keeping_both_on_a_clash_and_undo_puts_back() {
        let mut f = fx();
        let ids: Vec<String> = ["d0", "d1", "d2", "d3"].map(String::from).to_vec();
        let dest = format!("{ACME}/Income Tax/2025");
        let rows = plan(&f.conn, &f.root, &ids, &dest).unwrap();
        assert_eq!(
            rows[0].outcome,
            Outcome::Rename(format!("{dest}/letter.pdf"))
        );
        // w2.pdf is in 2025 already: the moved one keeps both.
        assert_eq!(
            rows[1].outcome,
            Outcome::KeepBoth(format!("{dest}/w2 (2).pdf"))
        );
        assert_eq!(rows[2].outcome, Outcome::Same);
        assert!(matches!(&rows[3].outcome, Outcome::Skip(w) if w.contains("another client")));

        let (tx, n) = commit(&mut f.conn, &f.root, &rows, "dev").unwrap();
        assert_eq!(n, 2);
        assert_eq!(rel(&f, "d1"), format!("{dest}/w2 (2).pdf"));
        assert!(f.root.join(&dest).join("w2 (2).pdf").is_file());
        assert!(!f.root.join(ACME).join("Income Tax/w2.pdf").exists());
        assert_eq!(
            vf_core::tx::last_undoable(&f.conn, "dev", &crate::docmeta::UNDOABLE).unwrap(),
            tx
        );

        crate::fileops::undo(&mut f.conn, &f.root, &tx, "dev").unwrap();
        assert_eq!(rel(&f, "d1"), format!("{ACME}/Income Tax/w2.pdf"));
        assert!(f.root.join(ACME).join("Income Tax/w2.pdf").is_file());
        assert!(f.root.join(ACME).join("Income Tax/letter.pdf").is_file());
    }

    #[test]
    fn refuses_a_missing_folder_or_the_cabinet_top() {
        let f = fx();
        let ids = vec!["d0".to_string()];
        assert!(plan(&f.conn, &f.root, &ids, "").is_err());
        assert!(plan(&f.conn, &f.root, &ids, &format!("{ACME}/Nope")).is_err());
        assert!(plan(&f.conn, &f.root, &ids, "../outside").is_err());
    }

    #[test]
    fn nothing_to_move_is_an_error() {
        let mut f = fx();
        let rows = plan(
            &f.conn,
            &f.root,
            &["d2".to_string()],
            &format!("{ACME}/Income Tax/2025"),
        )
        .unwrap();
        assert!(commit(&mut f.conn, &f.root, &rows, "dev").is_err());
    }
}
