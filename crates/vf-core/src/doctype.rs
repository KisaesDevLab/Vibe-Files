//! Firm-editable doc type catalog (PHASES P01 step 4). Seeded once per catalog; edits and
//! archiving are Transactions. Archived types are tombstoned (`deleted_at`), never removed, so
//! documents keep their type name.

use rusqlite::Connection;
use serde_json::json;

use crate::Result;
use crate::audit::AuditEntry;
use crate::tx::{Actor, Committed, NoFileOps, Stmt, Transaction};

/// `(name, default folder template, client visible)`. Folders follow Filer Desktop's default
/// cabinet template.
pub const SEED: &[(&str, &str, bool)] = &[
    ("W-2", "Income Tax/{Year}", false),
    ("1099-INT", "Income Tax/{Year}", false),
    ("1099-DIV", "Income Tax/{Year}", false),
    ("1099-B", "Income Tax/{Year}", false),
    ("1099-R", "Income Tax/{Year}", false),
    ("1099-NEC", "Income Tax/{Year}", false),
    ("1099-MISC", "Income Tax/{Year}", false),
    ("1099-K", "Income Tax/{Year}", false),
    ("1098", "Income Tax/{Year}", false),
    ("1095", "Income Tax/{Year}", false),
    ("K-1 (1065)", "Income Tax/{Year}", false),
    ("K-1 (1120-S)", "Income Tax/{Year}", false),
    ("K-1 (1041)", "Income Tax/{Year}", false),
    ("SSA-1099", "Income Tax/{Year}", false),
    ("Organizer", "Income Tax/{Year}", true),
    ("Engagement letter", "Correspondence/{Year}", true),
    ("Form 8879", "Income Tax/{Year}", true),
    ("Return copy — federal", "Income Tax/{Year}", true),
    ("Return copy — state", "Income Tax/{Year}", true),
    ("Extension", "Income Tax/{Year}", false),
    ("Estimate vouchers", "Income Tax/{Year}", true),
    ("Notice — IRS", "Correspondence/{Year}", false),
    ("Notice — state", "Correspondence/{Year}", false),
    ("Correspondence", "Correspondence/{Year}", false),
    ("Workpaper", "Workpapers & Support/{Year}", false),
    ("Trial balance", "Financials/{Year}", false),
    ("Financial statements", "Financials/{Year}", true),
    ("Bank statement", "Bookkeeping/{Year}", false),
    ("Credit card statement", "Bookkeeping/{Year}", false),
    (
        "Depreciation schedule",
        "Workpapers & Support/{Year}",
        false,
    ),
    ("Payroll report", "Payroll/{Year}", false),
    ("Sales tax return", "Sales Tax/{Year}", false),
    ("Formation documents", "Permanent/Setup", false),
    ("EIN letter", "Permanent/Setup", false),
    ("S election", "Permanent/Setup", false),
    ("Operating agreement", "Permanent/Setup", false),
    ("Stock basis", "Permanent/Stock Basis", false),
    ("POA 2848", "Permanent/Setup", false),
    ("Form 8821", "Permanent/Setup", false),
    ("Consent 7216", "Permanent/Setup", false),
    ("Invoice", "Other/Practice/{Year}", false),
    ("Miscellaneous", "Other/{Year}", false),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocType {
    pub id: String,
    pub name: String,
    pub default_folder_tpl: Option<String>,
    pub client_visible: bool,
    pub sort: i64,
    pub archived: bool,
}

/// Seeds [`SEED`] into an empty doc type table, as one Transaction. `None` when types exist.
pub fn seed(conn: &mut Connection, actor: &Actor) -> Result<Option<Committed>> {
    let n: i64 = conn.query_row("SELECT count(*) FROM doc_type", [], |r| r.get(0))?;
    if n > 0 {
        return Ok(None);
    }
    let now = crate::now_rfc3339();
    let mut tx = Transaction::new("seed doc types").audit(AuditEntry::new(
        "doctype_seed",
        None,
        json!({"count": SEED.len()}),
    ));
    for (i, (name, folder, visible)) in SEED.iter().enumerate() {
        tx = tx.catalog(Stmt::new(
            "INSERT INTO doc_type (id, name, default_folder_tpl, client_visible, sort, device_id, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            vec![
                crate::new_id().into(),
                (*name).into(),
                (*folder).into(),
                (*visible).into(),
                ((i as i64 + 1) * 10).into(),
                actor.device_id.clone().into(),
                now.clone().into(),
            ],
        ));
    }
    tx = tx.catalog_undo(Stmt::new("DELETE FROM doc_type", vec![]));
    Ok(Some(tx.commit(conn, &mut NoFileOps, actor)?))
}

/// Doc types by sort order; archived ones only with `include_archived`.
pub fn list(conn: &Connection, include_archived: bool) -> Result<Vec<DocType>> {
    let mut st = conn.prepare_cached(
        "SELECT id, name, default_folder_tpl, client_visible, sort, deleted_at IS NOT NULL
         FROM doc_type WHERE ?1 OR deleted_at IS NULL ORDER BY sort, name",
    )?;
    let rows = st.query_map([include_archived], |r| {
        Ok(DocType {
            id: r.get(0)?,
            name: r.get(1)?,
            default_folder_tpl: r.get(2)?,
            client_visible: r.get(3)?,
            sort: r.get(4)?,
            archived: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// What the editor saves. `id: None` adds a type.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocTypeEdit {
    pub id: Option<String>,
    pub name: String,
    pub default_folder_tpl: String,
    pub client_visible: bool,
}

fn row(conn: &Connection, id: &str) -> Result<(String, Option<String>, bool, Option<String>)> {
    Ok(conn.query_row(
        "SELECT name, default_folder_tpl, client_visible, deleted_at FROM doc_type WHERE id = ?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?)
}

/// Adds or updates a doc type. Names are unique (archived ones included).
pub fn save(conn: &mut Connection, actor: &Actor, e: &DocTypeEdit) -> Result<(String, Committed)> {
    let name = e.name.trim();
    if name.is_empty() {
        return Err(crate::Error::Tx("Enter a name for the doc type.".into()));
    }
    let clash: Option<String> = conn
        .query_row(
            "SELECT id FROM doc_type WHERE name = ?1 COLLATE NOCASE AND id IS NOT ?2",
            rusqlite::params![name, e.id],
            |r| r.get(0),
        )
        .ok();
    if clash.is_some() {
        return Err(crate::Error::Tx(format!(
            "A doc type named \"{name}\" already exists (it may be archived)."
        )));
    }
    let folder = e.default_folder_tpl.trim();
    let folder: Option<String> = (!folder.is_empty()).then(|| folder.to_owned());
    let now = crate::now_rfc3339();
    let dev = actor.device_id.clone();
    let tx = match &e.id {
        None => {
            let id = crate::new_id();
            let sort: i64 = conn.query_row(
                "SELECT coalesce(max(sort), 0) + 10 FROM doc_type",
                [],
                |r| r.get(0),
            )?;
            let tx = Transaction::new("add doc type")
                .catalog(Stmt::new(
                    "INSERT INTO doc_type (id, name, default_folder_tpl, client_visible, sort, device_id, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    vec![id.clone().into(), name.into(), folder.clone().into(), e.client_visible.into(), sort.into(), dev.into(), now.into()],
                ))
                .catalog_undo(Stmt::new("DELETE FROM doc_type WHERE id = ?1", vec![id.clone().into()]))
                .audit(AuditEntry::new("doctype_add", None, json!({"id": id, "name": name})));
            (id, tx)
        }
        Some(id) => {
            let old = row(conn, id)?;
            let set = "UPDATE doc_type SET name = ?2, default_folder_tpl = ?3, client_visible = ?4,
                       device_id = ?5, updated_at = ?6, rev = rev + 1 WHERE id = ?1";
            let tx = Transaction::new("edit doc type")
                .catalog(Stmt::new(
                    set,
                    vec![id.clone().into(), name.into(), folder.clone().into(), e.client_visible.into(), dev.clone().into(), now.clone().into()],
                ))
                .catalog_undo(Stmt::new(
                    set,
                    vec![id.clone().into(), old.0.clone().into(), old.1.clone().into(), old.2.into(), dev.into(), now.into()],
                ))
                .audit(AuditEntry::new(
                    "doctype_edit",
                    None,
                    json!({"id": id, "old": {"name": old.0, "folder": old.1, "client_visible": old.2},
                           "new": {"name": name, "folder": folder, "client_visible": e.client_visible}}),
                ));
            (id.clone(), tx)
        }
    };
    let (id, tx) = tx;
    Ok((id, tx.commit(conn, &mut NoFileOps, actor)?))
}

/// Archives (or restores) a doc type. Documents keep it; pickers stop offering it.
pub fn set_archived(
    conn: &mut Connection,
    actor: &Actor,
    id: &str,
    archived: bool,
) -> Result<Committed> {
    let old = row(conn, id)?;
    let now = crate::now_rfc3339();
    let set = "UPDATE doc_type SET deleted_at = ?2, device_id = ?3, updated_at = ?4, rev = rev + 1 WHERE id = ?1";
    let new_val: Option<String> = archived.then(|| now.clone());
    Transaction::new(if archived {
        "archive doc type"
    } else {
        "restore doc type"
    })
    .catalog(Stmt::new(
        set,
        vec![
            id.into(),
            new_val.into(),
            actor.device_id.clone().into(),
            now.clone().into(),
        ],
    ))
    .catalog_undo(Stmt::new(
        set,
        vec![
            id.into(),
            old.3.into(),
            actor.device_id.clone().into(),
            now.into(),
        ],
    ))
    .audit(AuditEntry::new(
        if archived {
            "doctype_archive"
        } else {
            "doctype_restore"
        },
        None,
        json!({"id": id, "name": old.0}),
    ))
    .commit(conn, &mut NoFileOps, actor)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor() -> Actor {
        Actor {
            user: Some("kwk".into()),
            device_id: "dev".into(),
        }
    }

    #[test]
    fn seed_once_with_forty_plus_types() {
        let (_t, mut c) = crate::browse::tests::catalog();
        assert!(seed(&mut c, &actor()).unwrap().is_some());
        assert!(seed(&mut c, &actor()).unwrap().is_none());
        let all = list(&c, false).unwrap();
        assert!(all.len() >= 40, "{}", all.len());
        assert_eq!(all[0].name, "W-2");
        let w2 = &all[0];
        assert_eq!(w2.default_folder_tpl.as_deref(), Some("Income Tax/{Year}"));
        assert!(all.iter().any(|t| t.name == "K-1 (1120-S)"));
    }

    #[test]
    fn add_edit_archive_with_unique_names_and_undo() {
        let (_t, mut c) = crate::browse::tests::catalog();
        seed(&mut c, &actor()).unwrap();
        let (id, _) = save(
            &mut c,
            &actor(),
            &DocTypeEdit {
                name: "Form 1099-SA".into(),
                default_folder_tpl: "Income Tax/{Year}".into(),
                ..DocTypeEdit::default()
            },
        )
        .unwrap();
        let dup = DocTypeEdit {
            name: "w-2".into(),
            ..DocTypeEdit::default()
        };
        assert!(save(&mut c, &actor(), &dup).is_err());
        let rename = DocTypeEdit {
            id: Some(id.clone()),
            name: "1099-SA".into(),
            default_folder_tpl: String::new(),
            client_visible: true,
        };
        let done = save(&mut c, &actor(), &rename).unwrap().1;
        let t = list(&c, false)
            .unwrap()
            .into_iter()
            .find(|t| t.id == id)
            .unwrap();
        assert_eq!(
            (t.name.as_str(), t.default_folder_tpl, t.client_visible),
            ("1099-SA", None, true)
        );
        crate::tx::undo(&mut c, &mut NoFileOps, &actor(), &done.tx_id).unwrap();
        let t = list(&c, false)
            .unwrap()
            .into_iter()
            .find(|t| t.id == id)
            .unwrap();
        assert_eq!(t.name, "Form 1099-SA");

        set_archived(&mut c, &actor(), &id, true).unwrap();
        assert!(!list(&c, false).unwrap().iter().any(|t| t.id == id));
        assert!(
            list(&c, true)
                .unwrap()
                .iter()
                .any(|t| t.id == id && t.archived)
        );
        set_archived(&mut c, &actor(), &id, false).unwrap();
        assert!(list(&c, false).unwrap().iter().any(|t| t.id == id));
        assert!(crate::audit::verify(&c).unwrap().ok());
    }
}
