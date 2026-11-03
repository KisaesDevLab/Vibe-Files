//! Bulk rename (P01 step 6) with Filer Desktop's filename tokens: `{Client}`, `{ID}`, `{Year}`,
//! `{DocType}`, `{Rest}`, `{Original}`, `{Ext}` and the client's CSV columns. Tokens are filled the
//! way Filer's rule engine fills them (`filer_core::template::client_tokens`,
//! `filer_core::parse::parse_filename`); rendering and the Windows name rules are Filer's
//! (`template::render_segment`, `MAX_PATH_LEN`). Files stay in their folder.
//!
//! [`plan`] is the preview; [`commit`] renames in one Transaction (`FileOp::Move`, Filer
//! `transfer`, never replacing a file), updates the catalog, and moves `docmeta.jsonl` keys.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use filer_core::clients::Client;
use filer_core::template::{self, Tokens};
use serde_json::json;
use vf_core::audit::AuditEntry;
use vf_core::db::Connection;
use vf_core::tx::{Actor, FileOp, Stmt, Transaction};

use crate::docmeta;
use crate::fileops::FilerFileOps;

pub const LABEL: &str = "doc_rename";
pub const DEFAULT_TEMPLATE: &str = "{Rest}.{Ext}";

/// One document in the preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub doc_id: String,
    /// Cabinet-relative, forward slashes.
    pub old_rel: String,
    pub new_rel: Option<String>,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Rename,
    /// The name was taken; Filer's Keep both gives it a ` (2)` suffix.
    KeepBoth,
    /// Already has this name.
    Same,
    /// Not renamed, with why.
    Skip(String),
}

impl Row {
    pub fn renames(&self) -> bool {
        matches!(self.outcome, Outcome::Rename | Outcome::KeepBoth)
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
    tax_year: Option<i32>,
    doc_type: Option<String>,
    locked_by: Option<String>,
    client: Option<Client>,
}

fn doc_info(conn: &Connection, id: &str) -> Result<DocInfo, String> {
    conn.query_row(
        "SELECT d.id, d.rel_path, d.tax_year, t.name, l.user,
                c.ext_id, c.id, c.name, c.extra
         FROM document d
         LEFT JOIN doc_type t ON t.id = d.doc_type_id
         LEFT JOIN lock l ON l.doc_id = d.id
         LEFT JOIN client c ON c.id = d.client_id
         WHERE d.id = ?1 AND d.deleted_at IS NULL",
        [id],
        |r| {
            let ext: Option<String> = r.get(5)?;
            let client = match ext {
                Some(ext) => {
                    let extra: String = r.get(8)?;
                    Some(Client {
                        id: ext,
                        uuid: uuid::Uuid::parse_str(&r.get::<_, String>(6)?).unwrap_or_default(),
                        name: r.get(7)?,
                        extra: serde_json::from_str(&extra).unwrap_or_default(),
                    })
                }
                None => None,
            };
            Ok(DocInfo {
                id: r.get(0)?,
                rel: r.get(1)?,
                tax_year: r.get(2)?,
                doc_type: r.get(3)?,
                locked_by: r.get(4)?,
                client,
            })
        },
    )
    .map_err(|e| format!("document {id}: {e}"))
}

/// The tokens Filer's rule engine would give this document.
fn tokens_for(d: &DocInfo, current_year: i32) -> Tokens {
    let name = d.rel.rsplit('/').next().unwrap_or(&d.rel);
    let re = filer_core::parse::filename_regex(filer_core::parse::DEFAULT_ID_PATTERN)
        .expect("Filer's default ID pattern compiles");
    let parsed = filer_core::parse::parse_filename(name, &re, current_year);
    let mut t = match &d.client {
        Some(c) => template::client_tokens(c),
        None => Tokens::new(),
    };
    if let Some(y) = d.tax_year {
        t.insert("Year".into(), y.to_string());
    }
    if let Some(dt) = &d.doc_type {
        t.insert("DocType".into(), dt.clone());
    }
    t.insert("Rest".into(), parsed.rest);
    t.insert("Original".into(), parsed.stem);
    t.insert("Ext".into(), parsed.ext);
    t
}

/// `name` in `dir`, or Filer's ` (2)`, ` (3)`… form (as `commit::free_path`), avoiding files on
/// disk and names this batch already claimed (lower-case: Windows names ignore case).
fn free_name(root: &Path, dir: &str, name: &str, claimed: &HashSet<String>) -> String {
    let taken = |n: &str| {
        let rel = join(dir, n);
        claimed.contains(&rel.to_lowercase()) || root.join(&rel).exists()
    };
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

/// The preview: what each document would be called.
pub fn plan(
    conn: &Connection,
    root: &Path,
    ids: &[String],
    tpl: &str,
    current_year: i32,
) -> Result<Vec<Row>, String> {
    let infos: Vec<DocInfo> = ids
        .iter()
        .map(|id| doc_info(conn, id))
        .collect::<Result<_, _>>()?;
    let extras: Vec<String> = infos
        .iter()
        .filter_map(|d| d.client.as_ref())
        .flat_map(|c| c.extra.keys().cloned())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    check_template(tpl, &extras)?;
    // Names leaving the folder are free for others in the same batch only after the move; keep
    // it simple and treat every current name of the batch as taken except a row's own.
    let mut claimed: HashSet<String> = HashSet::new();
    let mut rows = Vec::with_capacity(infos.len());
    for d in &infos {
        let (dir, old_name) = d.rel.rsplit_once('/').unwrap_or(("", &d.rel));
        let skip = |why: String| Row {
            doc_id: d.id.clone(),
            old_rel: d.rel.clone(),
            new_rel: None,
            outcome: Outcome::Skip(why),
        };
        if let Some(u) = &d.locked_by {
            rows.push(skip(format!("checked out by {u}")));
            claimed.insert(d.rel.to_lowercase());
            continue;
        }
        let name = match template::render_segment(tpl, &tokens_for(d, current_year)) {
            Ok(n) => n,
            Err(e) => {
                rows.push(skip(e.to_string()));
                claimed.insert(d.rel.to_lowercase());
                continue;
            }
        };
        if name == old_name {
            rows.push(Row {
                doc_id: d.id.clone(),
                old_rel: d.rel.clone(),
                new_rel: Some(d.rel.clone()),
                outcome: Outcome::Same,
            });
            claimed.insert(d.rel.to_lowercase());
            continue;
        }
        if name.eq_ignore_ascii_case(old_name) {
            rows.push(skip(
                "only the letter case changes; rename it in Explorer".into(),
            ));
            claimed.insert(d.rel.to_lowercase());
            continue;
        }
        let free = free_name(root, dir, &name, &claimed);
        let new_rel = join(dir, &free);
        if let Err(e) = template::checked_join(root, Path::new(&new_rel)) {
            rows.push(skip(e.to_string()));
            claimed.insert(d.rel.to_lowercase());
            continue;
        }
        claimed.insert(new_rel.to_lowercase());
        rows.push(Row {
            doc_id: d.id.clone(),
            old_rel: d.rel.clone(),
            new_rel: Some(new_rel),
            outcome: if free == name {
                Outcome::Rename
            } else {
                Outcome::KeepBoth
            },
        });
    }
    Ok(rows)
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
    let mut indexes: BTreeMap<String, docmeta::Index> = BTreeMap::new();
    for r in &go {
        let new_rel = r.new_rel.as_ref().expect("renames have a new name");
        tx = tx
            .file(FileOp::Move {
                src: root.join(&r.old_rel),
                dst: root.join(new_rel),
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
        let m = docmeta::load(conn, &r.doc_id)?;
        let folder = m.folder_rel.clone().unwrap_or_default();
        let ix = indexes.entry(folder.clone()).or_insert_with(|| {
            docmeta::Index::load(
                root,
                &if folder.is_empty() {
                    vec![]
                } else {
                    vec![folder.clone()]
                },
            )
        });
        if ix.get(&r.old_rel, &m.hash).is_some() {
            let moved = docmeta::Meta {
                rel_path: new_rel.clone(),
                ..m.clone()
            };
            let (file, line) = docmeta::record_line(root, &moved, &now, device_id);
            let (_, back) = docmeta::record_line(root, &m, &now, device_id);
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
    let mut fs = FilerFileOps::new(&[root]).map_err(|e| format!("cannot open the cabinet: {e}"))?;
    let actor = Actor {
        user: None,
        device_id: device_id.into(),
    };
    let c = tx
        .commit(conn, &mut fs, &actor)
        .map_err(|e| e.to_string())?;
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
        let new: Vec<_> = rows.iter().map(|r| r.new_rel.clone().unwrap()).collect();
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
                .new_rel
                .as_ref()
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
        assert_eq!(rows[0].outcome, Outcome::Rename);
        assert_eq!(rows[1].outcome, Outcome::KeepBoth);
        assert!(
            rows[1]
                .new_rel
                .as_ref()
                .unwrap()
                .ends_with("Taken 2025 (2).pdf")
        );
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

        let mut fs = FilerFileOps::new(&[&f.root]).unwrap();
        let actor = Actor {
            user: None,
            device_id: "dev".into(),
        };
        vf_core::tx::undo(&mut f.conn, &mut fs, &actor, &tx).unwrap();
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
