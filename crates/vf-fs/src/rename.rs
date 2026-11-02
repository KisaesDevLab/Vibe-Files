//! Bulk rename with Filer Desktop's filename tokens (PHASES P01 step 6).
//!
//! Tokens are filled exactly as Filer's rule engine fills them: the client's `{Client}`, `{ID}`
//! and CSV extras, `{Year}`, `{DocType}` (the document's doc type name), and `{Rest}`,
//! `{Original}`, `{Ext}` from Filer's filename parser (`ClientName_ID_rest.ext`). Like a Filer
//! rule, the template must keep the extension (`{Ext}`). Every rename is a Filer move
//! (`transfer`, never replacing a file); a name already taken gets Filer's "keep both" suffix
//! (` (2)`). The batch is one Transaction.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use filer_core::parse;
use filer_core::template::{self, Tokens};
use serde_json::json;
use vf_core::audit::AuditEntry;
use vf_core::db::Connection;
use vf_core::tx::{Actor, Committed, FileOp, Stmt, Transaction};

use crate::fileops::FilerFileOps;

/// Default suggestion: Filer's `ClientName_ID_rest.ext` convention.
pub const DEFAULT_TEMPLATE: &str = "{Client}_{ID}_{Year} {DocType}.{Ext}";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameRow {
    pub doc_id: String,
    pub from_rel: String,
    /// New cabinet-relative path, or why this document can't be renamed.
    pub to_rel: Result<String, String>,
    /// The rendered name was taken, so a ` (n)` suffix was added (Filer "Keep both").
    pub kept_both: bool,
}

impl RenameRow {
    pub fn will_rename(&self) -> bool {
        matches!(&self.to_rel, Ok(t) if *t != self.from_rel)
    }
}

/// Template problems (empty list = usable).
pub fn check_template(tpl: &str, extra_tokens: &[String]) -> Vec<String> {
    let tpl = tpl.trim();
    if tpl.is_empty() {
        return vec!["Enter a file name template.".into()];
    }
    if tpl.contains('/') || tpl.contains('\\') {
        return vec!["The file name cannot contain / or \\.".into()];
    }
    let mut allowed: Vec<&str> = template::BUILTIN_TOKENS.to_vec();
    allowed.extend(extra_tokens.iter().map(String::as_str));
    let mut errors = template::validate_segment(tpl, &allowed);
    if !template::tokens_in(tpl)
        .unwrap_or_default()
        .iter()
        .any(|t| t == "Ext")
    {
        errors.push(
            "The file name must keep the extension: include {Ext}, e.g. {Rest}.{Ext}.".into(),
        );
    }
    errors
}

/// Every CSV-extra token any live client has (allowed in templates).
pub fn extra_tokens(conn: &Connection) -> vf_core::Result<Vec<String>> {
    let mut st = conn.prepare("SELECT extra FROM client WHERE deleted_at IS NULL")?;
    let mut keys: std::collections::BTreeSet<String> = Default::default();
    for e in st.query_map([], |r| r.get::<_, String>(0))? {
        let m: BTreeMap<String, String> = serde_json::from_str(&e?).unwrap_or_default();
        keys.extend(m.into_keys());
    }
    Ok(keys.into_iter().collect())
}

struct DocInfo {
    rel: String,
    year: Option<i64>,
    doc_type: Option<String>,
    client: Option<(String, String, String)>, // name, ext_id, extra json
}

fn info(conn: &Connection, id: &str) -> vf_core::Result<DocInfo> {
    Ok(conn.query_row(
        "SELECT d.rel_path, d.tax_year, t.name, c.name, c.ext_id, c.extra
         FROM document d LEFT JOIN doc_type t ON t.id = d.doc_type_id
         LEFT JOIN client c ON c.id = d.client_id
         WHERE d.id = ?1 AND d.deleted_at IS NULL",
        [id],
        |r| {
            let name: Option<String> = r.get(3)?;
            Ok(DocInfo {
                rel: r.get(0)?,
                year: r.get(1)?,
                doc_type: r.get(2)?,
                client: match name {
                    Some(n) => Some((n, r.get(4)?, r.get(5)?)),
                    None => None,
                },
            })
        },
    )?)
}

fn split_rel(rel: &str) -> (&str, &str) {
    rel.rfind('/')
        .map_or(("", rel), |i| (&rel[..i], &rel[i + 1..]))
}

fn join_rel(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

fn abs(root: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(root.to_path_buf(), |p, s| p.join(s))
}

/// `name (2).ext`, `name (3).ext`, ... like Filer's `free_path`, also avoiding names other rows
/// of this batch will take.
fn free_name(root: &Path, dir: &str, name: &str, taken: &HashSet<String>) -> (String, bool) {
    let free = |n: &str| {
        let rel = join_rel(dir, n);
        !taken.contains(&rel.to_lowercase()) && !abs(root, &rel).exists()
    };
    if free(name) {
        return (name.to_owned(), false);
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, Some(e)),
        _ => (name, None),
    };
    let n = (2..)
        .map(|n| match ext {
            Some(e) => format!("{stem} ({n}).{e}"),
            None => format!("{stem} ({n})"),
        })
        .find(|n| free(n))
        .expect("unbounded range");
    (n, true)
}

/// Renders the new name of every document; nothing changes on disk.
pub fn plan(
    conn: &Connection,
    root: &Path,
    tpl: &str,
    doc_ids: &[String],
) -> vf_core::Result<Vec<RenameRow>> {
    let re = parse::filename_regex(parse::DEFAULT_ID_PATTERN).expect("default regex compiles");
    let year_now = chrono::Datelike::year(&chrono::Local::now());
    let tpl = tpl.trim();
    let mut taken: HashSet<String> = HashSet::new();
    let mut rows = Vec::with_capacity(doc_ids.len());
    // Documents in the batch free their old names only once they move; treat the old names as
    // taken unless the row keeps its name.
    for id in doc_ids {
        let d = info(conn, id)?;
        let (dir, file) = split_rel(&d.rel);
        let parsed = parse::parse_filename(file, &re, year_now);
        let mut tokens: Tokens = BTreeMap::new();
        if let Some((name, ext_id, extra)) = &d.client {
            tokens = serde_json::from_str(extra).unwrap_or_default();
            tokens.insert("Client".into(), name.clone());
            tokens.insert("ID".into(), ext_id.clone());
        }
        if let Some(y) = d.year {
            tokens.insert("Year".into(), y.to_string());
        }
        if let Some(t) = &d.doc_type {
            tokens.insert("DocType".into(), t.clone());
        }
        tokens.insert("Rest".into(), parsed.rest.clone());
        tokens.insert("Original".into(), parsed.stem.clone());
        tokens.insert("Ext".into(), parsed.ext.clone());
        let rendered = template::render_segment(tpl, &tokens).map_err(|e| match e {
            template::RenderError::MissingToken(t) if t == "Client" || t == "ID" => {
                "not in a client folder, so {Client} / {ID} are empty".to_owned()
            }
            template::RenderError::MissingToken(t) if t == "Year" => {
                "no tax year (permanent file)".into()
            }
            template::RenderError::MissingToken(t) if t == "DocType" => "no doc type yet".into(),
            e => e.to_string(),
        });
        let to_rel = rendered.and_then(|name| {
            // An unchanged name is not a collision with itself.
            if join_rel(dir, &name) == d.rel {
                return Ok((d.rel.clone(), false));
            }
            if join_rel(dir, &name).to_lowercase() == d.rel.to_lowercase() {
                return Err("only the letter case changes; rename it in Explorer".into());
            }
            let (free, kept) = free_name(root, dir, &name, &taken);
            let rel = join_rel(dir, &free);
            template::checked_join(root, Path::new(&rel)).map_err(|e| e.to_string())?;
            Ok((rel, kept))
        });
        let row = match to_rel {
            Ok((rel, kept_both)) => {
                taken.insert(rel.to_lowercase());
                RenameRow {
                    doc_id: id.clone(),
                    from_rel: d.rel.clone(),
                    to_rel: Ok(rel),
                    kept_both,
                }
            }
            Err(e) => RenameRow {
                doc_id: id.clone(),
                from_rel: d.rel.clone(),
                to_rel: Err(e),
                kept_both: false,
            },
        };
        rows.push(row);
    }
    Ok(rows)
}

/// Renames every row that changes name as one Transaction of Filer moves. `Ok(None)` when no
/// row changes.
pub fn commit(
    conn: &mut Connection,
    root: &Path,
    actor: &Actor,
    rows: &[RenameRow],
) -> vf_core::Result<Option<Committed>> {
    let now = vf_core::now_rfc3339();
    let todo: Vec<&RenameRow> = rows.iter().filter(|r| r.will_rename()).collect();
    if todo.is_empty() {
        return Ok(None);
    }
    let mut tx = Transaction::new(&format!("rename {} documents", todo.len()));
    for r in &todo {
        let to = r.to_rel.as_ref().expect("will_rename");
        tx = tx
            .file(FileOp::Move {
                src: abs(root, &r.from_rel),
                dst: abs(root, to),
            })
            .catalog(Stmt::new(
                "UPDATE document SET rel_path = ?2, device_id = ?3, updated_at = ?4, rev = rev + 1 WHERE id = ?1",
                vec![r.doc_id.clone().into(), to.clone().into(), actor.device_id.clone().into(), now.clone().into()],
            ))
            .catalog(Stmt::new(
                "INSERT INTO sync_pending (doc_id, op, since) VALUES (?1, 'meta', ?2) ON CONFLICT DO NOTHING",
                vec![r.doc_id.clone().into(), now.clone().into()],
            ))
            .catalog_undo(Stmt::new(
                "UPDATE document SET rel_path = ?2, rev = rev + 1 WHERE id = ?1",
                vec![r.doc_id.clone().into(), r.from_rel.clone().into()],
            ))
            .audit(AuditEntry::new(
                "rename",
                Some(&r.doc_id),
                json!({"from": r.from_rel, "to": to, "kept_both": r.kept_both}),
            ));
    }
    let mut fs = FilerFileOps::new(&[root]).map_err(|e| vf_core::Error::Tx(e.to_string()))?;
    Ok(Some(tx.commit(conn, &mut fs, actor)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::{NewClient, create_client, template as cab_template};

    #[test]
    fn rename_with_filer_tokens_keep_both_and_undo() {
        let mut w = crate::clients::tests::world();
        let tpl = cab_template(&w.root);
        let (cid, _) = create_client(
            &mut w.conn,
            &w.root,
            &w.cab,
            &tpl,
            &NewClient {
                ext_id: "1042".into(),
                name: "Anderson Farms LLC".into(),
                ..NewClient::default()
            },
            &w.actor,
        )
        .unwrap();
        let dir = w.root.join("Anderson Farms LLC (1042)/Income Tax/2025");
        std::fs::create_dir_all(&dir).unwrap();
        for (f, body) in [
            ("Anderson Farms LLC_1042_w2 employer.pdf", "a"),
            ("scan 0001.pdf", "b"),
            ("taken.pdf", "c"),
        ] {
            std::fs::write(dir.join(f), body).unwrap();
        }
        crate::rebuild::rebuild(&mut w.conn, &w.root, "dev").unwrap();
        vf_core::doctype::seed(&mut w.conn, &w.actor).unwrap();
        let w2: String = w
            .conn
            .query_row("SELECT id FROM doc_type WHERE name = 'W-2'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let ids: Vec<String> = {
            let mut st = w
                .conn
                .prepare("SELECT id FROM document WHERE client_id = ?1 ORDER BY rel_path")
                .unwrap();
            st.query_map([&cid], |r| r.get(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        assert_eq!(ids.len(), 3);
        vf_core::meta::bulk_edit(
            &mut w.conn,
            &w.actor,
            &ids,
            &vf_core::meta::MetaChange {
                doc_type_id: Some(Some(w2)),
                ..Default::default()
            },
        )
        .unwrap();

        assert!(
            !check_template("{Rest}", &[]).is_empty(),
            "must keep {{Ext}}"
        );
        assert!(!check_template("a/{Ext}", &[]).is_empty());
        assert!(check_template(DEFAULT_TEMPLATE, &[]).is_empty());

        // Two documents render the same name: the second keeps both.
        let rows = plan(
            &w.conn,
            &w.root,
            "{Client}_{ID}_{Year} {DocType}.{Ext}",
            &ids,
        )
        .unwrap();
        let names: Vec<String> = rows
            .iter()
            .map(|r| {
                r.to_rel
                    .clone()
                    .unwrap()
                    .rsplit('/')
                    .next()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(
            names,
            [
                "Anderson Farms LLC_1042_2025 W-2.pdf",
                "Anderson Farms LLC_1042_2025 W-2 (2).pdf",
                "Anderson Farms LLC_1042_2025 W-2 (3).pdf",
            ]
        );
        assert!(rows[1].kept_both && !rows[0].kept_both);
        // {Rest} comes from Filer's parser: the part after ClientName_ID_.
        let rest = plan(&w.conn, &w.root, "{Rest}.{Ext}", &ids[..1]).unwrap();
        assert!(
            rest[0]
                .to_rel
                .as_ref()
                .unwrap()
                .ends_with("/w2 employer.pdf")
        );

        let done = commit(&mut w.conn, &w.root, &w.actor, &rows)
            .unwrap()
            .unwrap();
        assert!(
            dir.join("Anderson Farms LLC_1042_2025 W-2 (3).pdf")
                .is_file()
        );
        assert!(!dir.join("scan 0001.pdf").exists());
        let rep = crate::rebuild::rebuild(&mut w.conn, &w.root, "dev").unwrap();
        assert_eq!(rep.docs_unchanged, 3, "catalog paths match disk");

        let mut fs = FilerFileOps::new(&[&w.root]).unwrap();
        vf_core::tx::undo(&mut w.conn, &mut fs, &w.actor, &done.tx_id).unwrap();
        for f in [
            "Anderson Farms LLC_1042_w2 employer.pdf",
            "scan 0001.pdf",
            "taken.pdf",
        ] {
            assert!(dir.join(f).is_file(), "{f} back");
        }
        let rep = crate::rebuild::rebuild(&mut w.conn, &w.root, "dev").unwrap();
        assert_eq!(rep.docs_unchanged, 3);
    }

    #[test]
    fn rows_without_a_value_are_refused_not_guessed() {
        let mut w = crate::clients::tests::world();
        std::fs::create_dir_all(w.root.join("Loose")).unwrap();
        std::fs::write(w.root.join("Loose/a.pdf"), "x").unwrap();
        crate::rebuild::rebuild(&mut w.conn, &w.root, "dev").unwrap();
        let id: String = w
            .conn
            .query_row("SELECT id FROM document", [], |r| r.get(0))
            .unwrap();
        let rows = plan(&w.conn, &w.root, DEFAULT_TEMPLATE, &[id]).unwrap();
        assert!(rows[0].to_rel.is_err());
        assert!(
            commit(&mut w.conn, &w.root, &w.actor, &rows)
                .unwrap()
                .is_none()
        );
    }
}
