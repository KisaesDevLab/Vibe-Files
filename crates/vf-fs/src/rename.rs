//! Bulk rename (P01 step 6) with Filer Desktop's filename tokens: `{Client}`, `{ID}`, `{Year}`,
//! `{DocType}`, `{Rest}`, `{Original}`, `{Ext}` and the client's CSV columns. Tokens are filled the
//! way Filer's rule engine fills them (`filer_core::template::client_tokens`,
//! `filer_core::parse::parse_filename`); rendering and the Windows name rules are Filer's
//! (`template::render_segment`, `MAX_PATH_LEN`). Files stay in their folder.
//!
//! [`plan`] is the preview; [`commit`] renames in one Transaction (`FileOp::Move`, Filer
//! `transfer`, never replacing a file), updates the catalog, and moves `docmeta.jsonl` keys.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use filer_core::clients::Client;
use filer_core::template::{self, Tokens};
use serde_json::json;
use vf_core::audit::AuditEntry;
use vf_core::db::Connection;
use vf_core::tx::{FileOp, Stmt, Transaction};

use crate::docmeta;

pub const LABEL: &str = "doc_rename";
pub const DEFAULT_TEMPLATE: &str = "{Rest}.{Ext}";

/// One document in the preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub doc_id: String,
    /// Cabinet-relative, forward slashes.
    pub old_rel: String,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Renamed to this cabinet-relative path.
    Rename(String),
    /// The name was taken; Filer's Keep both gives it this ` (2)` path.
    KeepBoth(String),
    /// Already has this name.
    Same,
    /// Not renamed, with why.
    Skip(String),
}

impl Row {
    /// The new cabinet-relative path, when this row renames.
    pub fn new_rel(&self) -> Option<&str> {
        match &self.outcome {
            Outcome::Rename(p) | Outcome::KeepBoth(p) => Some(p),
            Outcome::Same | Outcome::Skip(_) => None,
        }
    }

    pub fn renames(&self) -> bool {
        self.new_rel().is_some()
    }
}

/// Checks a filename template the way Filer checks a rule's: one segment, known tokens, `{Ext}`.
pub fn check_template(t: &str, extra_tokens: &[String]) -> Result<(), String> {
    if t.trim().is_empty() {
        return Err("Enter a file name template, e.g. {Rest}.{Ext}.".into());
    }
    if t.contains('/') || t.contains('\\') {
        return Err("The file name cannot contain / or \\.".into());
    }
    let mut allowed: Vec<&str> = template::BUILTIN_TOKENS.to_vec();
    allowed.extend(extra_tokens.iter().map(String::as_str));
    if let Some(e) = template::validate_segment(t, &allowed).into_iter().next() {
        return Err(e);
    }
    let tokens = template::tokens_in(t)?;
    if !tokens.iter().any(|x| x == "Ext") {
        return Err(
            "The file name must keep the extension: include {Ext}, e.g. {Rest}.{Ext}.".into(),
        );
    }
    Ok(())
}

struct DocInfo {
    id: String,
    rel: String,
    locked_by: Option<String>,
    /// Every token except the template-independent ones are filled here, once.
    tokens: Tokens,
}

/// Everything a rename preview needs, read once when the dialog opens; [`Planner::plan`] then
/// only renders names (it is re-run on every keystroke in the template box).
pub struct Planner {
    docs: Vec<DocInfo>,
    /// CSV columns of the documents' clients (allowed tokens).
    extras: Vec<String>,
    /// Lower-case file names per cabinet-relative folder, as on disk now.
    on_disk: HashMap<String, HashSet<String>>,
}

impl Planner {
    pub fn load(
        conn: &Connection,
        root: &Path,
        ids: &[String],
        current_year: i32,
    ) -> Result<Self, String> {
        let re = filer_core::parse::filename_regex(filer_core::parse::DEFAULT_ID_PATTERN)
            .expect("Filer's default ID pattern compiles");
        let mut st = conn
            .prepare_cached(
                "SELECT d.id, d.rel_path, d.tax_year, t.name, l.user,
                        c.ext_id, c.id, c.name, c.extra
                 FROM document d
                 LEFT JOIN doc_type t ON t.id = d.doc_type_id
                 LEFT JOIN lock l ON l.doc_id = d.id
                 LEFT JOIN client c ON c.id = d.client_id
                 WHERE d.id = ?1 AND d.deleted_at IS NULL",
            )
            .map_err(|e| e.to_string())?;
        // Client tokens are the same for every document of a client: parse `extra` once.
        let mut clients: HashMap<String, Tokens> = HashMap::new();
        let mut extras = std::collections::BTreeSet::new();
        let mut docs = Vec::with_capacity(ids.len());
        for id in ids {
            type Raw = (
                String,
                String,
                Option<i32>,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<String>,
            );
            let r: Raw = st
                .query_row([id], |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                    ))
                })
                .map_err(|e| format!("document {id}: {e}"))?;
            let (id, rel, year, doc_type, locked_by, ext, client_id, name, extra) = r;
            let mut tokens = match (ext, client_id, name) {
                (Some(ext), Some(cid), Some(name)) => clients
                    .entry(cid.clone())
                    .or_insert_with(|| {
                        let c = Client {
                            id: ext,
                            uuid: uuid::Uuid::parse_str(&cid).unwrap_or_default(),
                            name,
                            extra: extra
                                .and_then(|e| serde_json::from_str(&e).ok())
                                .unwrap_or_default(),
                        };
                        extras.extend(c.extra.keys().cloned());
                        template::client_tokens(&c)
                    })
                    .clone(),
                _ => Tokens::new(),
            };
            if let Some(y) = year {
                tokens.insert("Year".into(), y.to_string());
            }
            if let Some(dt) = doc_type {
                tokens.insert("DocType".into(), dt);
            }
            // The tokens Filer's rule engine gives a file name.
            let file = rel.rsplit('/').next().unwrap_or(&rel);
            let parsed = filer_core::parse::parse_filename(file, &re, current_year);
            tokens.insert("Rest".into(), parsed.rest);
            tokens.insert("Original".into(), parsed.stem);
            tokens.insert("Ext".into(), parsed.ext);
            docs.push(DocInfo {
                id,
                rel,
                locked_by,
                tokens,
            });
        }
        let mut on_disk: HashMap<String, HashSet<String>> = HashMap::new();
        for d in &docs {
            let dir = d.rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
            on_disk.entry(dir.to_owned()).or_insert_with(|| {
                std::fs::read_dir(root.join(dir))
                    .map(|rd| {
                        rd.filter_map(|e| e.ok())
                            .map(|e| e.file_name().to_string_lossy().to_lowercase())
                            .collect()
                    })
                    .unwrap_or_default()
            });
        }
        Ok(Planner {
            docs,
            extras: extras.into_iter().collect(),
            on_disk,
        })
    }

    /// The preview: what each document would be called with `tpl`.
    pub fn plan(&self, root: &Path, tpl: &str) -> Result<Vec<Row>, String> {
        check_template(tpl, &self.extras)?;
        // Names taken in this batch (lower-case cabinet paths): renamed-to paths, and the paths
        // of documents that keep their name.
        let mut claimed: HashSet<String> = HashSet::new();
        let mut rows = Vec::with_capacity(self.docs.len());
        for d in &self.docs {
            let outcome = self.plan_one(root, d, tpl, &claimed);
            let keeps = match &outcome {
                Outcome::Rename(p) | Outcome::KeepBoth(p) => p,
                Outcome::Same | Outcome::Skip(_) => &d.rel,
            };
            claimed.insert(keeps.to_lowercase());
            rows.push(Row {
                doc_id: d.id.clone(),
                old_rel: d.rel.clone(),
                outcome,
            });
        }
        Ok(rows)
    }

    fn plan_one(&self, root: &Path, d: &DocInfo, tpl: &str, claimed: &HashSet<String>) -> Outcome {
        let (dir, old_name) = d.rel.rsplit_once('/').unwrap_or(("", &d.rel));
        if let Some(u) = &d.locked_by {
            return Outcome::Skip(format!("checked out by {u}"));
        }
        let name = match template::render_segment(tpl, &d.tokens) {
            Ok(n) => n,
            Err(e) => return Outcome::Skip(e.to_string()),
        };
        if name == old_name {
            return Outcome::Same;
        }
        if name.eq_ignore_ascii_case(old_name) {
            return Outcome::Skip("only the letter case changes; rename it in Explorer".into());
        }
        let on_disk = self.on_disk.get(dir);
        let taken = |n: &str| {
            claimed.contains(&join(dir, n).to_lowercase())
                || on_disk.is_some_and(|s| s.contains(&n.to_lowercase()))
        };
        let free = free_name(&name, taken);
        let new_rel = join(dir, &free);
        if let Err(e) = template::checked_join(root, Path::new(&new_rel)) {
            return Outcome::Skip(e.to_string());
        }
        if free == name {
            Outcome::Rename(new_rel)
        } else {
            Outcome::KeepBoth(new_rel)
        }
    }
}

/// `name`, or Filer's ` (2)`, ` (3)`… form (as `commit::free_path`) when `taken`.
fn free_name(name: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(name) {
        return name.to_owned();
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, Some(e)),
        _ => (name, None),
    };
    (2..)
        .map(|n| match ext {
            Some(e) => format!("{stem} ({n}).{e}"),
            None => format!("{stem} ({n})"),
        })
        .find(|n| !taken(n))
        .expect("unbounded range")
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// Preview in one call (tests, small batches): [`Planner::load`] then [`Planner::plan`].
pub fn plan(
    conn: &Connection,
    root: &Path,
    ids: &[String],
    tpl: &str,
    current_year: i32,
) -> Result<Vec<Row>, String> {
    Planner::load(conn, root, ids, current_year)?.plan(root, tpl)
}

/// Renames the rows that rename, in one Transaction. Returns (transaction id, files renamed).
pub fn commit(
    conn: &mut Connection,
    root: &Path,
    rows: &[Row],
    device_id: &str,
) -> Result<(String, usize), String> {
    let go: Vec<&Row> = rows.iter().filter(|r| r.renames()).collect();
    if go.is_empty() {
        return Err("No file gets a new name.".into());
    }
    let now = vf_core::now_rfc3339();
    let mut tx = Transaction::new(LABEL);
    // docmeta keys: append a record at the new path for documents that have one.
    let mut meta_lines: BTreeMap<PathBuf, (Vec<String>, Vec<String>)> = BTreeMap::new();
    let metas: Vec<docmeta::Meta> = go
        .iter()
        .map(|r| docmeta::load(conn, &r.doc_id))
        .collect::<Result<_, _>>()?;
    let folders: Vec<String> = metas
        .iter()
        .filter_map(|m| m.folder_rel.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let ix = docmeta::Index::load(root, &folders);
    for (r, m) in go.iter().zip(&metas) {
        let new_rel = r
            .new_rel()
            .expect("filtered to rows that rename")
            .to_owned();
        tx = tx
            .file(FileOp::Move {
                src: root.join(&r.old_rel),
                dst: root.join(&new_rel),
            })
            .catalog(Stmt::new(
                "UPDATE document SET rel_path = ?2, device_id = ?3, updated_at = ?4, rev = rev + 1
                 WHERE id = ?1 AND deleted_at IS NULL",
                vec![
                    r.doc_id.clone().into(),
                    new_rel.clone().into(),
                    device_id.into(),
                    now.clone().into(),
                ],
            ))
            .catalog_undo(Stmt::new(
                "UPDATE document SET rel_path = ?2, device_id = ?3, updated_at = ?4, rev = rev + 1
                 WHERE id = ?1",
                vec![
                    r.doc_id.clone().into(),
                    r.old_rel.clone().into(),
                    device_id.into(),
                    now.clone().into(),
                ],
            ));
        if ix.get(&r.old_rel, &m.hash).is_some() {
            let moved = docmeta::Meta {
                rel_path: new_rel.clone(),
                ..m.clone()
            };
            let (file, line) = docmeta::record_line(root, &moved, &now, device_id);
            let (_, back) = docmeta::record_line(root, m, &now, device_id);
            let e = meta_lines.entry(file).or_default();
            e.0.push(line);
            e.1.push(back);
        }
    }
    for (path, (lines, undo_lines)) in meta_lines {
        tx = tx.file(FileOp::AppendLines {
            path,
            lines,
            undo_lines,
        });
    }
    tx = tx.audit(AuditEntry::new(
        LABEL,
        if go.len() == 1 {
            Some(go[0].doc_id.as_str())
        } else {
            None
        },
        json!({ "documents": go.len() }),
    ));
    let c = crate::fileops::commit(conn, root, tx, device_id)?;
    Ok((c.tx_id, go.len()))
}

#[cfg(test)]
mod tests {
    use vf_core::db::{CatalogKey, Storage};

    use super::*;

    struct Fx {
        _t: tempfile::TempDir,
        root: PathBuf,
        conn: Connection,
    }

    const FOLDER: &str = "Acme LLC (1001)";

    fn fx(files: &[&str]) -> Fx {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("cab");
        std::fs::create_dir_all(root.join(FOLDER).join("Income Tax/2025")).unwrap();
        let mut conn =
            vf_core::db::open(&t.path().join("c.db"), &CatalogKey([1; 32]), Storage::Local)
                .unwrap();
        crate::doctypes::seed_if_missing(&root).unwrap();
        crate::doctypes::sync(&mut conn, &root, "dev", "seed").unwrap();
        conn.execute(
            "INSERT INTO client (id, ext_id, name, folder_rel, extra, device_id, updated_at)
             VALUES ('00000000-0000-0000-0000-000000000001', '1001', 'Acme LLC', ?1,
                     '{\"Partner\":\"KW\"}', 'dev', 't')",
            [FOLDER],
        )
        .unwrap();
        for (i, f) in files.iter().enumerate() {
            let rel = format!("{FOLDER}/Income Tax/2025/{f}");
            std::fs::write(root.join(&rel), format!("content {i}")).unwrap();
            conn.execute(
                "INSERT INTO document (id, client_id, tax_year, rel_path, hash, size, filed_at, device_id, updated_at)
                 VALUES (?1, '00000000-0000-0000-0000-000000000001', 2025, ?2, ?3, 9, 't', 'dev', 't')",
                (format!("d{i}"), &rel, format!("{i:064x}")),
            )
            .unwrap();
        }
        Fx { _t: t, root, conn }
    }

    fn ids(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("d{i}")).collect()
    }

    fn names(f: &Fx) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(f.root.join(FOLDER).join("Income Tax/2025"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn filer_tokens_fill_the_name() {
        let f = fx(&["Acme LLC_1001_W2 from employer.pdf", "scan0001.pdf"]);
        let w2: String = f
            .conn
            .query_row("SELECT id FROM doc_type WHERE name = 'W-2'", [], |r| {
                r.get(0)
            })
            .unwrap();
        f.conn
            .execute("UPDATE document SET doc_type_id = ?1", [&w2])
            .unwrap();
        let rows = plan(
            &f.conn,
            &f.root,
            &ids(2),
            "{Year} {DocType} {ID} - {Rest}.{Ext}",
            2026,
        )
        .unwrap();
        let new: Vec<_> = rows
            .iter()
            .map(|r| r.new_rel().unwrap().to_owned())
            .collect();
        assert_eq!(
            new,
            [
                format!("{FOLDER}/Income Tax/2025/2025 W-2 1001 - W2 from employer.pdf"),
                format!("{FOLDER}/Income Tax/2025/2025 W-2 1001 - scan0001.pdf"),
            ]
        );
        // CSV columns are tokens too.
        let rows = plan(
            &f.conn,
            &f.root,
            &ids(1),
            "{Partner} {Original}.{Ext}",
            2026,
        )
        .unwrap();
        assert!(
            rows[0]
                .new_rel()
                .unwrap()
                .ends_with("/KW Acme LLC_1001_W2 from employer.pdf")
        );
    }

    #[test]
    fn template_rules_match_filer() {
        assert!(check_template("{Rest}", &[]).unwrap_err().contains("{Ext}"));
        assert!(check_template("a/{Rest}.{Ext}", &[]).is_err());
        assert!(check_template("{Nope}.{Ext}", &[]).is_err());
        assert!(check_template("{Partner}.{Ext}", &["Partner".into()]).is_ok());
    }

    #[test]
    fn collisions_keep_both_and_missing_tokens_skip() {
        let f = fx(&["a.pdf", "b.pdf", "Taken.pdf"]);
        f.conn
            .execute("UPDATE document SET tax_year = NULL WHERE id = 'd2'", [])
            .unwrap();
        let rows = plan(&f.conn, &f.root, &ids(3), "Taken {Year}.{Ext}", 2026).unwrap();
        // d0 and d1 both render "Taken 2025.pdf": the second keeps both.
        assert!(matches!(rows[0].outcome, Outcome::Rename(_)));
        assert!(matches!(rows[1].outcome, Outcome::KeepBoth(_)));
        assert!(rows[1].new_rel().unwrap().ends_with("Taken 2025 (2).pdf"));
        assert!(matches!(&rows[2].outcome, Outcome::Skip(why) if why.contains("{Year}")));
    }

    #[test]
    fn commit_renames_in_one_transaction_and_undo_puts_back() {
        let mut f = fx(&["a.pdf", "b.pdf", "c.pdf"]);
        // d1 has a metadata record: its key must follow the rename.
        crate::docmeta::edit(
            &mut f.conn,
            &f.root,
            &["d1".to_string()],
            &crate::docmeta::MetaEdit {
                status: Some("final".into()),
                ..Default::default()
            },
            "dev",
        )
        .unwrap();
        let before = names(&f);
        let rows = plan(&f.conn, &f.root, &ids(3), "2025 {Rest}.{Ext}", 2026).unwrap();
        let (tx, n) = commit(&mut f.conn, &f.root, &rows, "dev").unwrap();
        assert_eq!(n, 3);
        assert_eq!(names(&f), ["2025 a.pdf", "2025 b.pdf", "2025 c.pdf"]);
        let rel: String = f
            .conn
            .query_row("SELECT rel_path FROM document WHERE id = 'd1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(rel.ends_with("/2025 b.pdf"));
        let ix = crate::docmeta::Index::load(&f.root, &[FOLDER.into()]);
        let r = ix.get(&rel, &format!("{:064x}", 1)).unwrap();
        assert_eq!(
            (r.path.as_str(), r.status.as_str()),
            ("Income Tax/2025/2025 b.pdf", "final")
        );
        let audits: i64 = f
            .conn
            .query_row(
                "SELECT count(*) FROM audit WHERE action = 'doc_rename'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(audits, 1);

        crate::fileops::undo(&mut f.conn, &f.root, &tx, "dev").unwrap();
        assert_eq!(names(&f), before);
        let rel: String = f
            .conn
            .query_row("SELECT rel_path FROM document WHERE id = 'd1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(rel.ends_with("/b.pdf"));
    }

    #[test]
    fn locked_and_unchanged_files_are_left() {
        let mut f = fx(&["a.pdf", "b.pdf"]);
        f.conn
            .execute(
                "INSERT INTO lock (doc_id, user, device_id, since) VALUES ('d1', 'tross', 'dev', 't')",
                [],
            )
            .unwrap();
        let rows = plan(&f.conn, &f.root, &ids(2), "{Rest}.{Ext}", 2026).unwrap();
        assert_eq!(rows[0].outcome, Outcome::Same);
        assert!(matches!(&rows[1].outcome, Outcome::Skip(w) if w.contains("tross")));
        assert!(
            commit(&mut f.conn, &f.root, &rows, "dev").is_err(),
            "nothing to do"
        );
    }
}
