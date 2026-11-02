//! What a previous session left behind: temp PDFs in `data/tmp` (combinations and splits that
//! were never filed) and a commit that did not finish. Shown at launch; nothing is cleaned up
//! automatically (X103).

use std::path::{Path, PathBuf};

use crate::commit::LastBatch;
use crate::pdf::TMP_DIR;

#[derive(Debug, Clone, Default)]
pub struct Recovery {
    /// Files under data/tmp.
    pub leftovers: Vec<PathBuf>,
    /// The last commit was still running when the app stopped.
    pub unfinished: Option<LastBatch>,
}

impl Recovery {
    pub fn scan(data_dir: &Path, last_batch: Option<&LastBatch>) -> Self {
        let mut leftovers = Vec::new();
        collect_files(&data_dir.join(TMP_DIR), 0, &mut leftovers);
        leftovers.sort();
        Self {
            leftovers,
            unfinished: last_batch.filter(|b| b.in_progress).cloned(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.leftovers.is_empty() && self.unfinished.is_none()
    }
}

fn collect_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 8 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        // `DirEntry::file_type` does not follow links: a junction or symlink inside data/tmp is
        // neither listed nor entered, so nothing outside data/tmp is ever offered for deletion.
        let Ok(t) = e.file_type() else { continue };
        // data/tmp/undo holds the last batch's zip for undo (P07b), not a leftover.
        if depth == 0 && t.is_dir() && e.file_name() == "undo" {
            continue;
        }
        if t.is_dir() {
            collect_files(&e.path(), depth + 1, out);
        } else if t.is_file() {
            out.push(e.path());
        }
    }
}

/// Deletes `files` (leftovers listed by [`Recovery::scan`], the app's own temp PDFs), each only
/// if it is a plain file under data/tmp, then each one's folders up to data/tmp when left empty.
/// Files a running combine or split is writing are not in the list, so they are not touched.
/// Never a recursive delete. Returns how many files were deleted and the problems.
pub fn delete_leftovers(data_dir: &Path, files: &[PathBuf]) -> (usize, Vec<String>) {
    let tmp = data_dir.join(TMP_DIR);
    let mut deleted = 0;
    let mut problems = Vec::new();
    for f in files {
        let plain = std::fs::symlink_metadata(f).is_ok_and(|m| m.file_type().is_file());
        if !f.starts_with(&tmp) || !plain {
            continue;
        }
        match std::fs::remove_file(f) {
            Ok(()) => deleted += 1,
            Err(e) => {
                problems.push(format!("{}: {e}", f.display()));
                continue;
            }
        }
        let mut dir = f.parent();
        while let Some(d) = dir
            && d != tmp
            && d.starts_with(&tmp)
            && std::fs::remove_dir(d).is_ok()
        {
            dir = d.parent();
        }
    }
    tracing::info!(
        deleted,
        problems = problems.len(),
        "leftover temp files deleted"
    );
    (deleted, problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(in_progress: bool) -> LastBatch {
        LastBatch {
            batch: uuid::Uuid::new_v4(),
            committed_at: chrono::Utc::now(),
            in_progress,
            undone: false,
            items: vec![],
            created_dirs: vec![],
            created_files: vec![],
            backups: vec![],
            split_originals: vec![],
            zip_entries: vec![],
            zip: None,
            routed_zips: Vec::new(),
            renamed_dirs: Vec::new(),
        }
    }

    #[test]
    fn scan_and_delete_only_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        assert!(Recovery::scan(data, None).is_empty());
        let a = data.join(TMP_DIR).join("g1").join("a.pdf");
        let b = data.join(TMP_DIR).join("g2").join("1").join("b.pdf");
        for p in [&a, &b] {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        std::fs::write(data.join("ops.jsonl"), b"keep").unwrap();
        let r = Recovery::scan(data, Some(&batch(true)));
        assert_eq!(r.leftovers, vec![a.clone(), b.clone()]);
        assert!(r.unfinished.is_some());
        assert!(
            Recovery::scan(data, Some(&batch(false)))
                .unfinished
                .is_none()
        );

        // Only the listed files go; a folder a running job just made (empty) stays.
        let fresh = data.join(TMP_DIR).join("g3");
        std::fs::create_dir_all(&fresh).unwrap();
        let (deleted, problems) = delete_leftovers(data, std::slice::from_ref(&a));
        assert_eq!((deleted, problems.len()), (1, 0));
        assert!(
            !a.exists() && !a.parent().unwrap().exists(),
            "file and empty folder gone"
        );
        assert!(b.exists());
        assert!(fresh.exists(), "unlisted empty folder kept");
        // Nothing outside data/tmp, even if listed.
        let outside = data.join("ops.jsonl");
        assert_eq!(delete_leftovers(data, &[outside]).0, 0);
        assert!(data.join("ops.jsonl").exists());
        assert!(data.join(TMP_DIR).exists(), "tmp itself stays");
    }
}
