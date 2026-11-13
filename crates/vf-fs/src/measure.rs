//! `vibe-files measure <cabinet>` (PA-60): how long this PC takes to list the cabinet's files
//! and how long it would take to read them all, measured on a sample. Reads only: nothing in
//! the cabinet or the catalog changes, and online-only files are never downloaded.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use filer_intake::commit::sha256_file;

use crate::cabinet;
use crate::rebuild::{READERS, par_map};

/// What [`measure`] found.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Measure {
    /// Folders directly in the cabinet (client folders and others), `_Vibe` aside.
    pub top_folders: usize,
    /// Every folder under the cabinet, the top ones included.
    pub dirs: usize,
    pub files: usize,
    pub bytes: u64,
    /// Folders that could not be listed.
    pub unlisted: usize,
    /// Files whose contents are not on this PC (never read here).
    pub online_only: usize,
    /// Listing every folder with each file's size and date.
    pub list_secs: f64,
    /// The sample: files asked for their details one by one, then read whole.
    pub sample_files: usize,
    pub sample_bytes: u64,
    pub sample_unreadable: usize,
    pub stat_secs: f64,
    pub read_secs: f64,
}

impl Measure {
    fn scaled(&self, secs: f64) -> f64 {
        if self.sample_files == 0 {
            return 0.0;
        }
        secs * self.files as f64 / self.sample_files as f64
    }

    /// Asking every file for its details one by one, at the sample's pace.
    pub fn full_stat_secs(&self) -> f64 {
        self.scaled(self.stat_secs)
    }

    /// Reading every file whole, at the sample's pace.
    pub fn full_read_secs(&self) -> f64 {
        self.scaled(self.read_secs)
    }
}

/// `90.0 s`, `12.5 min`, `3.2 h`.
fn span(secs: f64) -> String {
    if secs < 120.0 {
        format!("{secs:.1} s")
    } else if secs < 7200.0 {
        format!("{:.1} min", secs / 60.0)
    } else {
        format!("{:.1} h", secs / 3600.0)
    }
}

impl fmt::Display for Measure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "Listed {} files ({:.1} GB) in {} folders under {} top folders: {}",
            self.files,
            self.bytes as f64 / 1_073_741_824.0,
            self.dirs,
            self.top_folders,
            span(self.list_secs)
        )?;
        if self.unlisted > 0 {
            writeln!(f, "Folders that could not be listed: {}", self.unlisted)?;
        }
        if self.online_only > 0 {
            writeln!(f, "Online-only files (not read): {}", self.online_only)?;
        }
        if self.sample_files == 0 {
            return writeln!(f, "No file to sample.");
        }
        let rate = if self.read_secs > 0.0 {
            self.sample_bytes as f64 / 1_048_576.0 / self.read_secs
        } else {
            0.0
        };
        writeln!(
            f,
            "Sample of {} files ({:.1} MB) on {READERS} threads: details one by one {}, read whole {} ({rate:.1} MB/s)",
            self.sample_files,
            self.sample_bytes as f64 / 1_048_576.0,
            span(self.stat_secs),
            span(self.read_secs),
        )?;
        if self.sample_unreadable > 0 {
            writeln!(
                f,
                "Sample files that could not be read: {}",
                self.sample_unreadable
            )?;
        }
        writeln!(
            f,
            "At that pace, every file: details one by one about {}, read whole about {}",
            span(self.full_stat_secs()),
            span(self.full_read_secs())
        )
    }
}

/// One walk: files with their size and whether they are online only, folders seen, folders
/// that could not be listed.
#[derive(Default)]
struct Walk {
    files: Vec<(PathBuf, u64, bool)>,
    dirs: usize,
    unlisted: usize,
}

/// Every file under `dir` with the size and attributes its folder's listing gave (no call per
/// file), skipping `_Vibe` folders and links.
fn walk(dir: &Path, out: &mut Walk) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        out.unlisted += 1;
        return;
    };
    for e in rd.filter_map(|e| e.ok()) {
        let Ok(m) = e.metadata() else { continue };
        if m.file_type().is_symlink() {
            continue;
        }
        if m.is_dir() {
            if e.file_name() != cabinet::VIBE_DIR {
                out.dirs += 1;
                walk(&e.path(), out);
            }
        } else {
            out.files
                .push((e.path(), m.len(), crate::cloud::online_only_meta(&m)));
        }
    }
}

/// Lists `root` (several top folders at a time, as the index does), then asks `sample` files
/// spread evenly over the list for their details and reads them whole.
pub fn measure(root: &Path, sample: usize) -> Result<Measure, String> {
    let started = Instant::now();
    let mut tops: Vec<PathBuf> = Vec::new();
    let mut here = Walk::default();
    let rd = std::fs::read_dir(root).map_err(|e| format!("cannot read {}: {e}", root.display()))?;
    for e in rd.filter_map(|e| e.ok()) {
        let Ok(m) = e.metadata() else { continue };
        if m.file_type().is_symlink() {
            continue;
        }
        if m.is_dir() {
            if e.file_name() != cabinet::VIBE_DIR {
                tops.push(e.path());
            }
        } else {
            here.files
                .push((e.path(), m.len(), crate::cloud::online_only_meta(&m)));
        }
    }
    tops.sort();
    let walks = par_map(&tops, |d| {
        let mut w = Walk::default();
        walk(d, &mut w);
        w
    });
    let mut out = Measure {
        top_folders: tops.len(),
        dirs: tops.len(),
        ..Measure::default()
    };
    let mut local: Vec<PathBuf> = Vec::new();
    for w in std::iter::once(here).chain(walks) {
        out.dirs += w.dirs;
        out.unlisted += w.unlisted;
        for (p, size, online) in w.files {
            out.files += 1;
            out.bytes += size;
            if online {
                out.online_only += 1;
            } else {
                local.push(p);
            }
        }
    }
    out.list_secs = started.elapsed().as_secs_f64();

    let n = sample.min(local.len());
    let picked: Vec<&PathBuf> = (0..n).map(|i| &local[i * local.len() / n]).collect();
    out.sample_files = n;
    let t = Instant::now();
    let _ = par_map(&picked, |p| crate::cloud::stat(p));
    out.stat_secs = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let read = par_map(&picked, |p| {
        sha256_file(p)
            .ok()
            .and_then(|_| std::fs::metadata(p).ok())
            .map(|m| m.len())
    });
    out.read_secs = t.elapsed().as_secs_f64();
    for r in read {
        match r {
            Some(b) => out.sample_bytes += b,
            None => out.sample_unreadable += 1,
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Counts files and bytes from the listing, leaves `_Vibe` out, and reads only the sample.
    #[test]
    fn measure_lists_and_samples() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("_Vibe")).unwrap();
        std::fs::write(root.join("_Vibe/cabinet.json"), b"{}").unwrap();
        for c in ["A (1)", "B (2)"] {
            std::fs::create_dir_all(root.join(c).join("Income Tax/2025")).unwrap();
            std::fs::create_dir_all(root.join(c).join("_Vibe")).unwrap();
            std::fs::write(root.join(c).join("_Vibe/client.json"), b"{}").unwrap();
            for i in 0..5 {
                std::fs::write(
                    root.join(c).join(format!("Income Tax/2025/doc{i}.pdf")),
                    vec![b'x'; 100],
                )
                .unwrap();
            }
        }
        std::fs::write(root.join("loose.txt"), b"12345").unwrap();
        let m = measure(root, 4).unwrap();
        assert_eq!(m.top_folders, 2);
        assert_eq!(m.files, 11);
        assert_eq!(m.bytes, 1005);
        assert_eq!(m.dirs, 6, "two clients, each with Income Tax and 2025");
        assert_eq!((m.sample_files, m.sample_unreadable), (4, 0));
        assert!((305..=400).contains(&m.sample_bytes), "{}", m.sample_bytes);
        let text = m.to_string();
        assert!(text.contains("Listed 11 files"), "{text}");
        assert!(text.contains("At that pace"), "{text}");

        let none = measure(&root.join("A (1)/_Vibe"), 4).unwrap();
        assert_eq!(none.files, 1);
        assert!(measure(&root.join("missing"), 4).is_err());
    }
}
