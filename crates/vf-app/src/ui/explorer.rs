//! Browse's explorer view (owner 2026-11-15, Q-114): the list shows one folder as it is on
//! disk, like Windows Explorer's Details view. Its sub-folders come first, then every file in
//! it, whether the catalog knows the file or not. Browse draws it and keeps it in step with the
//! tree; here are the folder's listing (read on a thread: it asks the share), its order, and
//! the words of its columns.

use std::path::Path;
use std::time::SystemTime;

use vf_core::db::Connection;

/// One row of a folder as the disk has it. Size and date are what the listing itself gave:
/// no file is asked about again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskEntry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

/// A folder read from the disk, and the catalog's documents directly in it.
#[derive(Debug, Clone)]
pub struct Listed {
    /// Cabinet-relative, `/`-separated; `""` is the cabinet folder itself.
    pub dir: String,
    pub entries: Result<Vec<DiskEntry>, String>,
    /// Ids of the catalog's documents directly in `dir`: a large cabinet's window holds them
    /// while the folder is shown (it does not hold every document).
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Col {
    #[default]
    Name,
    Modified,
    Type,
    Size,
}

impl Col {
    pub const ALL: [(Col, &'static str); 4] = [
        (Col::Name, "Name"),
        (Col::Modified, "Date modified"),
        (Col::Type, "Type"),
        (Col::Size, "Size"),
    ];
}

/// Whether `dir` is a folder inside the cabinet as this view names one: relative, no drive, no
/// `..` (Up stops at the cabinet folder).
pub fn inside(dir: &str) -> bool {
    !dir.starts_with('/')
        && !dir.contains('\\')
        && !dir.contains(':')
        && dir
            .split('/')
            .all(|seg| dir.is_empty() || (!seg.is_empty() && seg != "." && seg != ".."))
}

/// The folder `dir` is in (`None` for the cabinet folder).
pub fn parent(dir: &str) -> Option<String> {
    if dir.is_empty() {
        return None;
    }
    Some(
        dir.rsplit_once('/')
            .map_or(String::new(), |(p, _)| p.to_string()),
    )
}

/// `name` inside `dir`.
pub fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// The folder a document's cabinet-relative path is in.
pub fn dir_of(rel_path: &str) -> &str {
    rel_path.rsplit_once('/').map_or("", |(d, _)| d)
}

/// Reads `dir` of the cabinet at `root`: one listing of the folder, and with `conn` the ids
/// of the catalog's documents directly in it. Asks the share: never on the window's thread.
pub fn list(root: &Path, dir: &str, conn: Option<&Connection>) -> Listed {
    let entries = if inside(dir) {
        read(&root.join(dir))
    } else {
        Err(format!("{dir} is not a folder of the cabinet."))
    };
    let ids = conn.and_then(|c| ids_in(c, dir)).unwrap_or_default();
    Listed {
        dir: dir.to_string(),
        entries,
        ids,
    }
}

/// The folder of a client the catalog has no folder for yet (PA-81, PA-83): the top folder
/// whose own client file (`_Vibe/client.json`) names this client, found through Intake's
/// folder index. Nothing is assumed from a folder's name (owner 2026-11-16: a client's
/// folder is what was imported or what is read on the disk). Reads the share: never on the
/// window's thread. No client is bound and no catalog row written.
pub fn client_folder(root: &Path, ext_id: &str) -> Result<String, String> {
    vf_fs::rebuild::bound_folder_of(root, ext_id)
        .filter(|f| inside(f) && !f.contains('/'))
        .ok_or_else(|| {
            "No folder in the cabinet is bound to this client. Its folder, if it has one, is reached from the cabinet's own folder (All documents on the left); Intake's Setup binds a folder to a client, and + New client makes one."
                .to_string()
        })
}
fn read(path: &Path) -> Result<Vec<DiskEntry>, String> {
    let rd =
        std::fs::read_dir(path).map_err(|e| format!("{} cannot be read: {e}", path.display()))?;
    let mut out = Vec::new();
    for e in rd.flatten() {
        // On Windows this is what the listing already returned (no second question).
        let Ok(m) = e.metadata() else { continue };
        out.push(DiskEntry {
            name: e.file_name().to_string_lossy().into_owned(),
            dir: m.is_dir(),
            size: if m.is_dir() { 0 } else { m.len() },
            modified: m.modified().ok(),
        });
    }
    Ok(out)
}

/// Ids of the documents whose file is directly in `dir`.
fn ids_in(conn: &Connection, dir: &str) -> Option<Vec<String>> {
    if dir.is_empty() {
        let mut st = conn
            .prepare(
                "SELECT id FROM document WHERE deleted_at IS NULL AND instr(rel_path, '/') = 0",
            )
            .ok()?;
        return st
            .query_map([], |r| r.get(0))
            .ok()?
            .collect::<Result<_, _>>()
            .ok();
    }
    // `dir/` up to, not including, `dir0` ('0' follows '/'): the paths below the folder, by
    // the path's index; then only those with no further folder.
    let (from, to) = (format!("{dir}/"), format!("{dir}0"));
    let mut st = conn
        .prepare(
            "SELECT id FROM document
             WHERE deleted_at IS NULL AND rel_path >= ?1 AND rel_path < ?2
               AND instr(substr(rel_path, ?3), '/') = 0",
        )
        .ok()?;
    st.query_map(
        (from.as_str(), to.as_str(), from.chars().count() as i64 + 1),
        |r| r.get(0),
    )
    .ok()?
    .collect::<Result<_, _>>()
    .ok()
}

/// What Explorer's Type column says.
pub fn type_of(e: &DiskEntry) -> String {
    if e.dir {
        return "File folder".into();
    }
    match e.name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() && ext.len() <= 8 => {
            format!("{} file", ext.to_uppercase())
        }
        _ => "File".into(),
    }
}

/// A file's date in local time, `2026-11-02 14:05`.
pub fn modified_text(t: Option<SystemTime>) -> String {
    t.map_or(String::new(), |t| {
        chrono::DateTime::<chrono::Local>::from(t)
            .format("%Y-%m-%d %H:%M")
            .to_string()
    })
}

/// Folders first (as Explorer does), then by `col`; names without regard to case.
pub fn sort(entries: &mut [DiskEntry], col: Col, asc: bool) {
    entries.sort_by(|a, b| {
        let by_name = || a.name.to_lowercase().cmp(&b.name.to_lowercase());
        let o = match col {
            Col::Name => by_name(),
            Col::Modified => a.modified.cmp(&b.modified).then_with(by_name),
            Col::Type => type_of(a).cmp(&type_of(b)).then_with(by_name),
            Col::Size => a.size.cmp(&b.size).then_with(by_name),
        };
        b.dir.cmp(&a.dir).then(if asc { o } else { o.reverse() })
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, dir: bool, size: u64) -> DiskEntry {
        DiskEntry {
            name: name.into(),
            dir,
            size,
            modified: None,
        }
    }

    #[test]
    fn folders_come_first_and_names_ignore_case() {
        let mut v = vec![
            e("b.pdf", false, 5),
            e("Zeta", true, 0),
            e("A.docx", false, 9),
            e("alpha", true, 0),
        ];
        sort(&mut v, Col::Name, true);
        let names = |v: &[DiskEntry]| v.iter().map(|x| x.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&v), ["alpha", "Zeta", "A.docx", "b.pdf"]);
        sort(&mut v, Col::Size, false);
        assert_eq!(names(&v), ["Zeta", "alpha", "A.docx", "b.pdf"]);
        assert_eq!(type_of(&v[0]), "File folder");
        assert_eq!(type_of(&e("w2.PDF", false, 1)), "PDF file");
        assert_eq!(type_of(&e("README", false, 1)), "File");
        assert_eq!(type_of(&e(".gitignore", false, 1)), "File");
    }

    #[test]
    fn paths_stay_inside_the_cabinet() {
        assert!(inside(""));
        assert!(inside("Smith, John (1001)/Income Tax/2025"));
        for bad in ["..", "a/../b", "/a", "a//b", "C:/x", r"a\b", "a/."] {
            assert!(!inside(bad), "{bad}");
        }
        assert_eq!(parent(""), None);
        assert_eq!(parent("a"), Some(String::new()));
        assert_eq!(parent("a/b/c"), Some("a/b".into()));
        assert_eq!(join("", "a"), "a");
        assert_eq!(join("a/b", "c"), "a/b/c");
        assert_eq!(dir_of("a/b/c.pdf"), "a/b");
        assert_eq!(dir_of("c.pdf"), "");
    }

    #[test]
    fn a_folder_is_listed_with_its_own_files_only() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        std::fs::create_dir_all(root.join("Client/Income Tax/2025")).unwrap();
        std::fs::write(root.join("Client/letter.pdf"), "x").unwrap();
        std::fs::write(root.join("Client/Income Tax/2025/w2.pdf"), "xy").unwrap();
        let got = list(root, "Client", None);
        let mut names: Vec<(String, bool)> = got
            .entries
            .unwrap()
            .into_iter()
            .map(|e| (e.name, e.dir))
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                ("Income Tax".to_string(), true),
                ("letter.pdf".to_string(), false)
            ]
        );
        assert!(list(root, "../elsewhere", None).entries.is_err());
        assert!(list(root, "Missing", None).entries.is_err());
    }
}
