//! Document metadata edits (PHASES P01 step 5): doc type, tax year, status, source, tags. A bulk
//! edit over any number of documents is one Transaction: one audit row per changed document (one
//! batch, sharing the tx id) and one undo record.

use std::collections::BTreeSet;

use rusqlite::Connection;
use serde_json::{Value, json};

use crate::audit::AuditEntry;
use crate::tx::{Actor, Committed, NoFileOps, Stmt, Transaction};
use crate::{Error, Result};

pub const STATUSES: [&str; 4] = ["received", "in_work", "final", "delivered"];
pub const SOURCES: [&str; 4] = ["client", "firm", "government", "third_party"];

/// What to set. `None` leaves the field as it is on each document (the inspector's "Mixed").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MetaChange {
    pub doc_type_id: Option<Option<String>>,
    /// `Some(None)` = permanent file (no tax year).
    pub tax_year: Option<Option<i64>>,
    pub status: Option<String>,
    pub source: Option<Option<String>>,
    pub tags: Option<BTreeSet<String>>,
}

impl MetaChange {
    pub fn is_empty(&self) -> bool {
        *self == MetaChange::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Meta {
    doc_type_id: Option<String>,
    tax_year: Option<i64>,
    status: String,
    source: Option<String>,
    tags: BTreeSet<String>,
}

fn load(conn: &Connection, id: &str) -> Result<Meta> {
    let (doc_type_id, tax_year, status, source) = conn.query_row(
        "SELECT doc_type_id, tax_year, status, source FROM document WHERE id = ?1 AND deleted_at IS NULL",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    let mut st = conn.prepare_cached("SELECT tag FROM document_tag WHERE doc_id = ?1")?;
    let tags = st
        .query_map([id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Meta {
        doc_type_id,
        tax_year,
        status,
        source,
        tags,
    })
}

/// Splits a comma-separated tag field: trimmed, empty ones dropped, duplicates merged.
pub fn parse_tags(s: &str) -> BTreeSet<String> {
    s.split(',')
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
        .collect()
}

fn validate(c: &MetaChange) -> Result<()> {
    if let Some(s) = &c.status
        && !STATUSES.contains(&s.as_str())
    {
        return Err(Error::Tx(format!("unknown status {s}")));
    }
    if let Some(Some(s)) = &c.source
        && !SOURCES.contains(&s.as_str())
    {
        return Err(Error::Tx(format!("unknown source {s}")));
    }
    if let Some(Some(y)) = c.tax_year
        && !(1900..=2100).contains(&y)
    {
        return Err(Error::Tx(format!("{y} is not a plausible tax year")));
    }
    Ok(())
}

fn set_stmts(id: &str, m: &Meta, device: &str, now: &str) -> Vec<Stmt> {
    let mut v = vec![
        Stmt::new(
            "UPDATE document SET doc_type_id = ?2, tax_year = ?3, status = ?4, source = ?5,
               device_id = ?6, updated_at = ?7, rev = rev + 1 WHERE id = ?1",
            vec![
                id.into(),
                m.doc_type_id.clone().into(),
                m.tax_year.into(),
                m.status.clone().into(),
                m.source.clone().into(),
                device.into(),
                now.into(),
            ],
        ),
        Stmt::new(
            "DELETE FROM document_tag WHERE doc_id = ?1",
            vec![id.into()],
        ),
    ];
    for t in &m.tags {
        v.push(Stmt::new(
            "INSERT INTO document_tag (doc_id, tag) VALUES (?1, ?2)",
            vec![id.into(), t.clone().into()],
        ));
    }
    v
}

fn diff(old: &Meta, new: &Meta) -> Value {
    let mut d = serde_json::Map::new();
    let mut put = |k: &str, a: Value, b: Value| {
        if a != b {
            d.insert(k.into(), json!({"old": a, "new": b}));
        }
    };
    put("doc_type", json!(old.doc_type_id), json!(new.doc_type_id));
    put("tax_year", json!(old.tax_year), json!(new.tax_year));
    put("status", json!(old.status), json!(new.status));
    put("source", json!(old.source), json!(new.source));
    put("tags", json!(old.tags), json!(new.tags));
    Value::Object(d)
}

/// Applies `change` to every document in `ids` as one Transaction. Documents the change would
/// not alter are left out. `Ok(None)` when nothing would change.
pub fn bulk_edit(
    conn: &mut Connection,
    actor: &Actor,
    ids: &[String],
    change: &MetaChange,
) -> Result<Option<Committed>> {
    validate(change)?;
    let now = crate::now_rfc3339();
    let mut tx = Transaction::new(&format!("edit {} documents", ids.len()));
    let mut changed = 0usize;
    for id in ids {
        let old = load(conn, id)?;
        let new = Meta {
            doc_type_id: change
                .doc_type_id
                .clone()
                .unwrap_or(old.doc_type_id.clone()),
            tax_year: change.tax_year.unwrap_or(old.tax_year),
            status: change.status.clone().unwrap_or(old.status.clone()),
            source: change.source.clone().unwrap_or(old.source.clone()),
            tags: change.tags.clone().unwrap_or(old.tags.clone()),
        };
        if new == old {
            continue;
        }
        changed += 1;
        for s in set_stmts(id, &new, &actor.device_id, &now) {
            tx = tx.catalog(s);
        }
        tx = tx.catalog(Stmt::new(
            "INSERT INTO sync_pending (doc_id, op, since) VALUES (?1, 'meta', ?2) ON CONFLICT DO NOTHING",
            vec![id.clone().into(), now.clone().into()],
        ));
        // Undo runs newest first: restore tags and fields as they were.
        for s in set_stmts(id, &old, &actor.device_id, &now)
            .into_iter()
            .rev()
        {
            tx = tx.catalog_undo(s);
        }
        tx = tx.audit(AuditEntry::new("meta_edit", Some(id), diff(&old, &new)));
    }
    if changed == 0 {
        return Ok(None);
    }
    Ok(Some(tx.commit(conn, &mut NoFileOps, actor)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browse::tests::{add_client, add_doc, catalog};

    fn actor() -> Actor {
        Actor {
            user: Some("kwk".into()),
            device_id: "dev".into(),
        }
    }

    fn count(c: &Connection, sql: &str) -> i64 {
        c.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    /// P01 done-when: bulk edit of 200 docs is one Transaction, one audit batch, one undo.
    #[test]
    fn bulk_edit_200_docs_is_one_transaction() {
        let (_t, mut c) = catalog();
        crate::doctype::seed(&mut c, &actor()).unwrap();
        add_client(&c, "c1", "1042", "Anderson Farms LLC");
        let ids: Vec<String> = (0..200).map(|i| format!("d{i:03}")).collect();
        for (i, id) in ids.iter().enumerate() {
            add_doc(
                &c,
                id,
                Some("c1"),
                Some(2024),
                &format!("A/Income Tax/2024/{i}.pdf"),
            );
        }
        c.execute(
            "INSERT INTO document_tag (doc_id, tag) VALUES ('d000', 'keep')",
            [],
        )
        .unwrap();
        let w2: String = c
            .query_row("SELECT id FROM doc_type WHERE name = 'W-2'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let undo_before = count(&c, "SELECT count(*) FROM undo_record");
        let audit_before = count(&c, "SELECT count(*) FROM audit");

        let change = MetaChange {
            doc_type_id: Some(Some(w2.clone())),
            tax_year: Some(Some(2025)),
            status: Some("final".into()),
            source: Some(Some("client".into())),
            tags: Some(parse_tags("reviewed, w2 ,reviewed")),
        };
        let done = bulk_edit(&mut c, &actor(), &ids, &change).unwrap().unwrap();
        assert_eq!(
            count(&c, "SELECT count(*) FROM undo_record") - undo_before,
            1
        );
        assert_eq!(done.audit.len(), 200);
        let txs: BTreeSet<String> = done
            .audit
            .iter()
            .map(|r| {
                serde_json::from_str::<Value>(&r.detail).unwrap()["tx"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(txs, BTreeSet::from([done.tx_id.clone()]), "one audit batch");
        assert_eq!(count(&c, "SELECT count(*) FROM audit") - audit_before, 200);
        assert_eq!(
            count(
                &c,
                "SELECT count(*) FROM document WHERE tax_year = 2025 AND status = 'final'"
            ),
            200
        );
        assert_eq!(count(&c, "SELECT count(*) FROM document_tag"), 400);
        assert_eq!(
            count(&c, "SELECT count(*) FROM sync_pending WHERE op = 'meta'"),
            200
        );

        // Same change again: nothing to do.
        assert!(
            bulk_edit(&mut c, &actor(), &ids, &change)
                .unwrap()
                .is_none()
        );

        crate::tx::undo(&mut c, &mut NoFileOps, &actor(), &done.tx_id).unwrap();
        assert_eq!(
            count(
                &c,
                "SELECT count(*) FROM document WHERE tax_year = 2024 AND status = 'received' AND doc_type_id IS NULL"
            ),
            200
        );
        assert_eq!(count(&c, "SELECT count(*) FROM document_tag"), 1);
        assert_eq!(
            count(
                &c,
                "SELECT count(*) FROM document_tag WHERE doc_id = 'd000' AND tag = 'keep'"
            ),
            1
        );
        assert!(crate::audit::verify(&c).unwrap().ok());
    }

    #[test]
    fn mixed_fields_are_left_alone_and_bad_values_refused() {
        let (_t, mut c) = catalog();
        add_doc(&c, "a", None, Some(2023), "x/a.pdf");
        add_doc(&c, "b", None, None, "x/b.pdf");
        let ids = vec!["a".to_owned(), "b".to_owned()];
        let only_status = MetaChange {
            status: Some("in_work".into()),
            ..MetaChange::default()
        };
        bulk_edit(&mut c, &actor(), &ids, &only_status)
            .unwrap()
            .unwrap();
        assert_eq!(
            count(
                &c,
                "SELECT count(*) FROM document WHERE id = 'a' AND tax_year = 2023"
            ),
            1
        );
        assert_eq!(
            count(
                &c,
                "SELECT count(*) FROM document WHERE id = 'b' AND tax_year IS NULL"
            ),
            1
        );
        for bad in [
            MetaChange {
                status: Some("done".into()),
                ..MetaChange::default()
            },
            MetaChange {
                source: Some(Some("bank".into())),
                ..MetaChange::default()
            },
            MetaChange {
                tax_year: Some(Some(25)),
                ..MetaChange::default()
            },
        ] {
            assert!(bulk_edit(&mut c, &actor(), &ids, &bad).is_err());
        }
    }
}
