//! A document's earlier contents (PHASES P05 step 1). Every content change through
//! [`crate::content::replace`] keeps the old bytes in `_Vibe/versions/<hash>`, once per
//! distinct content, with a `version` row and a sidecar entry. This module lists them, opens
//! or saves a copy, and restores one as a new version (the current content is kept too).

use std::path::{Path, PathBuf};

use vf_core::db::Connection;

use crate::cabinet;
use crate::content::{self, Change, Refused, Replaced};

/// One earlier content of a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub hash: String,
    pub size: i64,
    /// When it stopped being the current content (RFC 3339).
    pub created: String,
    /// What replaced it: `annotate`, `ocr`, `pages`, `restore`, `external_modify`, …
    pub created_by: Option<String>,
    pub reason: Option<String>,
    /// Its bytes are in the versions store (not for an edit made outside Vibe Files, whose
    /// earlier bytes were never seen).
    pub kept: bool,
}

/// `doc_id`'s versions, newest first.
pub fn list(conn: &Connection, root: &Path, doc_id: &str) -> Result<Vec<Version>, String> {
    let mut st = conn
        .prepare(
            "SELECT hash, size, created, created_by, reason FROM version WHERE doc_id = ?1
             ORDER BY created DESC, rowid DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map([doc_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for r in rows {
        let (hash, size, created, created_by, reason) = r.map_err(|e| e.to_string())?;
        let kept = cabinet::version_path(root, &hash).is_file();
        out.push(Version {
            hash,
            size,
            created,
            created_by,
            reason,
            kept,
        });
    }
    Ok(out)
}

/// The stored bytes of version `hash`, checked against their hash.
fn stored(root: &Path, hash: &str) -> Result<PathBuf, String> {
    if hash.len() < 2 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("not a version hash".into());
    }
    let p = cabinet::version_path(root, hash);
    if !p.is_file() {
        return Err("this version's content was not kept".into());
    }
    Ok(p)
}

/// Copies version `hash` to `dest` (which must not exist), verified by hash.
pub fn save_copy(root: &Path, hash: &str, dest: &Path) -> Result<(), String> {
    let src = stored(root, hash)?;
    if dest.exists() {
        return Err(format!("{} already exists", dest.display()));
    }
    std::fs::copy(&src, dest).map_err(|e| format!("cannot write {}: {e}", dest.display()))?;
    let got = filer_intake::commit::sha256_file(dest).map_err(|e| e.to_string())?;
    if got != hash {
        let _ = std::fs::remove_file(dest);
        return Err("the copy does not match the stored version".into());
    }
    Ok(())
}

/// A read-only copy of version `hash` in `dir` (the workstation's temp folder) to open in
/// another program; `name` is the document's file name.
pub fn open_copy(root: &Path, hash: &str, name: &str, dir: &Path) -> Result<PathBuf, String> {
    let p = Path::new(name);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document".into());
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let dest = dir.join(format!(
        "{stem} (version {}){ext}",
        &hash[..8.min(hash.len())]
    ));
    if !dest.exists() {
        save_copy(root, hash, &dest)?;
        if let Ok(m) = std::fs::metadata(&dest) {
            let mut perm = m.permissions();
            perm.set_readonly(true);
            let _ = std::fs::set_permissions(&dest, perm);
        }
    }
    Ok(dest)
}

/// Makes version `hash` the current content again, as a new version: the current content is
/// kept like any other change, and the step can be undone.
pub fn restore(
    conn: &mut Connection,
    root: &Path,
    device_id: &str,
    doc_id: &str,
    hash: &str,
) -> Result<Result<Replaced, Refused>, String> {
    let src = stored(root, hash)?;
    let ext = conn
        .query_row(
            "SELECT rel_path FROM document WHERE id = ?1",
            [doc_id],
            |r| r.get::<_, String>(0),
        )
        .ok()
        .and_then(|rel| {
            Path::new(&rel)
                .extension()
                .map(|e| e.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "bin".into());
    let tmp = content::tmp_file(root, &ext)?;
    std::fs::copy(&src, &tmp).map_err(|e| format!("cannot copy the version: {e}"))?;
    content::replace(
        conn,
        root,
        device_id,
        doc_id,
        &tmp,
        Change {
            label: "restore_version",
            created_by: "restore",
            reason: "before restoring an earlier version",
            detail: serde_json::json!({ "restored": hash }),
            extra: vec![],
            based_on: None,
        },
    )
}

#[cfg(test)]
mod tests {
    use vf_core::db::Storage;

    use super::*;

    fn fx() -> (tempfile::TempDir, PathBuf, Connection) {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("cab");
        std::fs::create_dir_all(root.join("A (1)/2025")).unwrap();
        let conn = vf_core::db::open(&t.path().join("c.db"), Storage::Local).unwrap();
        std::fs::write(root.join("A (1)/2025/doc.pdf"), b"content 0").unwrap();
        let hash = filer_intake::commit::sha256_file(&root.join("A (1)/2025/doc.pdf")).unwrap();
        conn.execute(
            "INSERT INTO document (id, tax_year, rel_path, hash, size, filed_at, device_id, updated_at)
             VALUES ('d1', 2025, 'A (1)/2025/doc.pdf', ?1, 9, 't', 'dev', 't')",
            [&hash],
        )
        .unwrap();
        (t, root, conn)
    }

    fn overwrite(conn: &mut Connection, root: &Path, bytes: &[u8]) -> Replaced {
        let tmp = content::tmp_file(root, "pdf").unwrap();
        std::fs::write(&tmp, bytes).unwrap();
        content::replace(
            conn,
            root,
            "dev",
            "d1",
            &tmp,
            Change {
                label: "annotate",
                created_by: "annotate",
                reason: "before annotations",
                detail: serde_json::json!({}),
                extra: vec![],
                based_on: None,
            },
        )
        .unwrap()
        .unwrap()
    }

    fn files_in(dir: &Path) -> usize {
        walkdir(dir)
            .into_iter()
            .filter(|p| p.extension().is_none())
            .count()
    }

    fn walkdir(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walkdir(&p));
            } else {
                out.push(p);
            }
        }
        out
    }

    /// PHASES P05 done-when: overwriting a file 5 times yields 5 versions and 1 physical copy
    /// per distinct content.
    #[test]
    fn five_overwrites_five_versions_one_copy_per_content() {
        let (_t, root, mut conn) = fx();
        // Contents 0 → 1 → 2 → 1 → 2 → 1: the five replaced contents are 0, 1, 2, 1, 2.
        for b in [
            b"content 1",
            b"content 2",
            b"content 1",
            b"content 2",
            b"content 1",
        ] {
            overwrite(&mut conn, &root, b);
        }
        let v = list(&conn, &root, "d1").unwrap();
        assert_eq!(v.len(), 5);
        assert!(v.iter().all(|v| v.kept));
        let distinct: std::collections::BTreeSet<&str> =
            v.iter().map(|v| v.hash.as_str()).collect();
        assert_eq!(distinct.len(), 3);
        let store = root.join("_Vibe/versions");
        assert_eq!(
            files_in(&store),
            3,
            "one physical copy per distinct content"
        );
        // Every version row has its sidecar entry (rebuild restores all five).
        let sidecar_entries: usize = distinct
            .iter()
            .map(|h| {
                let t =
                    std::fs::read_to_string(cabinet::version_path(&root, h).with_extension("json"))
                        .unwrap();
                serde_json::from_str::<Vec<serde_json::Value>>(&t)
                    .unwrap()
                    .len()
            })
            .sum();
        assert_eq!(sidecar_entries, 5);
    }

    #[test]
    fn restore_brings_back_an_earlier_content_as_a_new_version() {
        let (t, root, mut conn) = fx();
        let first = overwrite(&mut conn, &root, b"content 1").old_hash;
        overwrite(&mut conn, &root, b"content 2");
        let r = restore(&mut conn, &root, "dev", "d1", &first)
            .unwrap()
            .unwrap();
        let path = root.join("A (1)/2025/doc.pdf");
        assert_eq!(std::fs::read(&path).unwrap(), b"content 0");
        assert_eq!(r.new_hash, first);
        let v = list(&conn, &root, "d1").unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].created_by.as_deref(), Some("restore"));
        // Undo puts content 2 back.
        crate::fileops::undo(&mut conn, &root, &r.tx_id, "dev").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"content 2");

        // Save a copy and open a read-only copy.
        let dest = t.path().join("copy.pdf");
        save_copy(&root, &first, &dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"content 0");
        assert!(save_copy(&root, &first, &dest).is_err(), "never overwrites");
        let open = open_copy(&root, &first, "doc.pdf", &t.path().join("tmp")).unwrap();
        assert!(
            open.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("doc (version ")
        );
        assert!(std::fs::metadata(&open).unwrap().permissions().readonly());
        assert!(save_copy(&root, "zz", &dest).is_err());
        assert!(save_copy(&root, "../x", &dest).is_err());
    }
}
