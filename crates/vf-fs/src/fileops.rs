//! [`vf_core::tx::FileOps`] backed by the vendored Filer Desktop primitives
//! (`filer_intake::commit::{transfer, copy_verified, sha256_file}`), so every Vibe Files move has
//! Filer's guarantees: never replace an existing file, rename on one volume, copy + SHA-256 verify
//! + delete across volumes. See CLAUDE.md §Document moves and `crates/filer-intake/VENDORED.md`.

use std::path::{Path, PathBuf};

use filer_intake::commit::{self, Method};
use vf_core::tx::{AppliedOp, FileOp, FileOps};

pub struct FilerFileOps {
    /// Folders writes may land in (cabinet root, inboxes). Canonical.
    allowed: Vec<PathBuf>,
    /// Take Filer's copy-verify-delete path even on one volume (tests).
    pub force_copy: bool,
}

impl FilerFileOps {
    /// `allowed`: the cabinet root and the inboxes. Each must exist.
    pub fn new(allowed: &[&Path]) -> std::io::Result<Self> {
        Ok(Self {
            allowed: allowed
                .iter()
                .map(std::fs::canonicalize)
                .collect::<std::io::Result<_>>()?,
            force_copy: false,
        })
    }

    /// Creates the missing folders above `file` (outermost first), refusing anything outside the
    /// allowed roots. Returns the folders it created.
    fn make_parents(&self, file: &Path) -> Result<Vec<PathBuf>, String> {
        let parent = file
            .parent()
            .ok_or_else(|| format!("{} has no folder", file.display()))?;
        self.make_dirs(parent)
    }

    fn make_dirs(&self, dir: &Path) -> Result<Vec<PathBuf>, String> {
        let mut missing = Vec::new();
        let mut cur = dir;
        while !cur.exists() {
            missing.push(cur.to_path_buf());
            cur = cur
                .parent()
                .ok_or_else(|| format!("{} has no existing folder above it", dir.display()))?;
        }
        self.check_inside(cur, dir)?;
        let mut created = Vec::new();
        for d in missing.iter().rev() {
            if let Err(e) = std::fs::create_dir(d) {
                remove_empty(&created);
                return Err(format!("cannot create {}: {e}", d.display()));
            }
            created.push(d.clone());
        }
        Ok(created)
    }

    /// `existing` is the deepest existing folder of `target`; it must be inside an allowed root.
    fn check_inside(&self, existing: &Path, target: &Path) -> Result<(), String> {
        let c = std::fs::canonicalize(existing)
            .map_err(|e| format!("cannot open {}: {e}", existing.display()))?;
        if self.allowed.iter().any(|a| c.starts_with(a)) {
            Ok(())
        } else {
            Err(format!(
                "{} is outside the cabinet and inboxes",
                target.display()
            ))
        }
    }
}

/// Removes folders this op created, innermost first, only while they are empty.
fn remove_empty(created: &[PathBuf]) {
    for d in created.iter().rev() {
        if std::fs::remove_dir(d).is_err() {
            break;
        }
    }
}

fn method_name(m: Method) -> &'static str {
    match m {
        Method::Move => "move",
        Method::CopyVerifyDelete => "copy_verify_delete",
    }
}

fn size(p: &Path) -> Option<u64> {
    std::fs::metadata(p).ok().map(|m| m.len())
}

impl FileOps for FilerFileOps {
    fn apply(&mut self, op: &FileOp) -> Result<AppliedOp, String> {
        match op {
            FileOp::Move { src, dst } | FileOp::Copy { src, dst } => {
                if dst.exists() {
                    return Err(format!("{} already exists", dst.display()));
                }
                let created = self.make_parents(dst)?;
                let res = match op {
                    FileOp::Move { .. } => commit::transfer(src, dst, self.force_copy)
                        .map(|(m, bytes, sha)| (method_name(m), bytes, sha)),
                    _ => commit::copy_verified(src, dst).map(|(b, sha)| ("copy", b, Some(sha))),
                };
                match res {
                    Ok((method, bytes, sha256)) => Ok(AppliedOp {
                        op: op.clone(),
                        bytes,
                        sha256,
                        method: method.into(),
                        created_dirs: created,
                    }),
                    Err(e) => {
                        remove_empty(&created);
                        Err(e)
                    }
                }
            }
            FileOp::Mkdir { path } => Ok(AppliedOp {
                op: op.clone(),
                bytes: 0,
                sha256: None,
                method: "mkdir".into(),
                created_dirs: self.make_dirs(path)?,
            }),
        }
    }

    fn check_reverse(&self, a: &AppliedOp) -> Result<(), String> {
        match &a.op {
            FileOp::Move { src, dst } => {
                if size(dst) != Some(a.bytes) {
                    return Err(format!(
                        "{} has changed or is gone since it was filed",
                        dst.display()
                    ));
                }
                if src.exists() {
                    return Err(format!("{} exists again", src.display()));
                }
                Ok(())
            }
            FileOp::Copy { dst, .. } => match (&a.sha256, commit::sha256_file(dst)) {
                (Some(want), Ok(have)) if *want == have => Ok(()),
                _ => Err(format!(
                    "{} was changed after it was copied; it is left as it is",
                    dst.display()
                )),
            },
            FileOp::Mkdir { .. } => Ok(()),
        }
    }

    fn reverse(&mut self, a: &AppliedOp) -> Result<(), String> {
        self.check_reverse(a)?;
        match &a.op {
            FileOp::Move { src, dst } => {
                commit::transfer(dst, src, self.force_copy)?;
            }
            // Only a copy this op wrote whose hash still matches (checked above).
            FileOp::Copy { dst, .. } => std::fs::remove_file(dst)
                .map_err(|e| format!("cannot remove {}: {e}", dst.display()))?,
            FileOp::Mkdir { .. } => {}
        }
        remove_empty(&a.created_dirs);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use vf_core::audit::AuditEntry;
    use vf_core::db::{CatalogKey, Storage};
    use vf_core::tx::{self, Actor, Stmt, Transaction};

    use super::*;

    /// Every folder and file under `dir`: relative path → SHA-256 (folders: "dir").
    fn snapshot(dir: &Path) -> BTreeMap<String, String> {
        fn walk(base: &Path, d: &Path, out: &mut BTreeMap<String, String>) {
            for e in std::fs::read_dir(d).unwrap() {
                let p = e.unwrap().path();
                let rel = p
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if p.is_dir() {
                    out.insert(rel, "dir".into());
                    walk(base, &p, out);
                } else {
                    out.insert(rel, commit::sha256_file(&p).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(dir, dir, &mut out);
        out
    }

    struct World {
        _tmp: tempfile::TempDir,
        cab: PathBuf,
        inbox: PathBuf,
        root: PathBuf,
        conn: vf_core::db::Connection,
        actor: Actor,
    }

    fn world() -> World {
        let tmp = tempfile::tempdir().unwrap();
        let cab = tmp.path().join("cab");
        let (inbox, root) = (cab.join("inbox"), cab.join("root"));
        std::fs::create_dir_all(&inbox).unwrap();
        std::fs::create_dir_all(root.join("Smith, John (1001)")).unwrap();
        std::fs::create_dir_all(root.join("Smith Holdings LLC (1002)")).unwrap();
        for (i, name) in ["w2.pdf", "1099-int.pdf", "k1.pdf"].iter().enumerate() {
            std::fs::write(
                inbox.join(name),
                format!("%PDF-1.4 synthetic {i}").repeat(100 + i),
            )
            .unwrap();
        }
        let conn = vf_core::db::open(
            &tmp.path().join("catalog.db"),
            &CatalogKey([5; 32]),
            Storage::Local,
        )
        .unwrap();
        World {
            _tmp: tmp,
            cab,
            inbox,
            root,
            conn,
            actor: Actor {
                user: Some("kwk".into()),
                device_id: "dev-1".into(),
            },
        }
    }

    fn batch(w: &World) -> Transaction {
        let client = w.root.join("Smith, John (1001)");
        let year = client.join("Income Tax").join("2025");
        let mut tx = Transaction::new("intake batch");
        for name in ["w2.pdf", "1099-int.pdf", "k1.pdf"] {
            tx = tx.file(FileOp::Move {
                src: w.inbox.join(name),
                dst: year.join(name),
            });
        }
        // K-1 copy to the related entity (Filer's own commit writes copies before the primary
        // move; this test checks reversibility only).
        tx.file(FileOp::Copy {
            src: year.join("k1.pdf"),
            dst: w.root.join("Smith Holdings LLC (1002)").join("K-1").join("k1.pdf"),
        })
        .catalog(Stmt::new(
            "INSERT INTO device (id, name, first_seen, last_seen, mode) VALUES ('dev-1', 'PC', 't', 't', 'local')",
            vec![],
        ))
        .catalog_undo(Stmt::new("DELETE FROM device WHERE id = 'dev-1'", vec![]))
        .audit(AuditEntry::new("file", None, serde_json::json!({"files": 3, "copies": 1})))
    }

    fn round_trip(force_copy: bool) {
        let mut w = world();
        let mut fs = FilerFileOps::new(&[&w.root, &w.inbox]).unwrap();
        fs.force_copy = force_copy;
        let before = snapshot(&w.cab);

        let done = batch(&w).commit(&mut w.conn, &mut fs, &w.actor).unwrap();
        let filed = w.root.join("Smith, John (1001)/Income Tax/2025/w2.pdf");
        assert!(filed.is_file());
        assert!(!w.inbox.join("w2.pdf").exists());
        let want = if force_copy {
            "copy_verify_delete"
        } else {
            "move"
        };
        assert_eq!(done.applied[0].method, want);

        tx::undo(&mut w.conn, &mut fs, &w.actor, &done.tx_id).unwrap();
        assert_eq!(
            snapshot(&w.cab),
            before,
            "undo leaves cabinet + inbox byte-identical"
        );

        batch(&w).commit(&mut w.conn, &mut fs, &w.actor).unwrap();
        assert!(filed.is_file());
        assert!(vf_core::audit::verify(&w.conn).unwrap().ok());
    }

    /// P00 done-when: commit → undo → commit through Transaction, same-volume rename.
    #[test]
    fn commit_undo_commit_rename() {
        round_trip(false);
    }

    /// P00 done-when: the same through Filer's forced copy + SHA-256 verify + delete path.
    #[test]
    fn commit_undo_commit_copy_verify() {
        round_trip(true);
    }

    #[test]
    fn collision_mid_batch_puts_everything_back() {
        let mut w = world();
        let mut fs = FilerFileOps::new(&[&w.root, &w.inbox]).unwrap();
        let taken = w.root.join("Smith, John (1001)/Income Tax/2025");
        std::fs::create_dir_all(&taken).unwrap();
        std::fs::write(taken.join("k1.pdf"), b"already filed").unwrap();
        let before = snapshot(&w.cab);
        assert!(batch(&w).commit(&mut w.conn, &mut fs, &w.actor).is_err());
        assert_eq!(snapshot(&w.cab), before);
    }

    #[test]
    fn modified_copy_blocks_undo_and_nothing_moves() {
        let mut w = world();
        let mut fs = FilerFileOps::new(&[&w.root, &w.inbox]).unwrap();
        let done = batch(&w).commit(&mut w.conn, &mut fs, &w.actor).unwrap();
        let copy = w.root.join("Smith Holdings LLC (1002)/K-1/k1.pdf");
        std::fs::write(&copy, b"edited by someone").unwrap();
        let after = snapshot(&w.cab);
        let err = tx::undo(&mut w.conn, &mut fs, &w.actor, &done.tx_id).unwrap_err();
        assert!(matches!(err, vf_core::Error::UndoBlocked(_)), "{err}");
        assert_eq!(snapshot(&w.cab), after);
    }

    #[test]
    fn writes_outside_the_cabinet_are_refused() {
        let mut w = world();
        let mut fs = FilerFileOps::new(&[&w.root, &w.inbox]).unwrap();
        let outside = w.cab.parent().unwrap().join("elsewhere").join("w2.pdf");
        let tx = Transaction::new("bad")
            .file(FileOp::Move {
                src: w.inbox.join("w2.pdf"),
                dst: outside.clone(),
            })
            .audit(AuditEntry::new("file", None, serde_json::json!({})));
        assert!(tx.commit(&mut w.conn, &mut fs, &w.actor).is_err());
        assert!(w.inbox.join("w2.pdf").is_file());
        assert!(!outside.parent().unwrap().exists());
    }
}
