//! Virtual folders and per-user lists behind the Browse tree's Views section (PHASES P01 step 7):
//! Recent, Pinned, Needs doc type, Needs tax year, Received this week, and favorites.

use rusqlite::Connection;
use serde_json::json;

use crate::Result;
use crate::audit::AuditEntry;
use crate::tx::{Actor, Committed, NoFileOps, Stmt, Transaction};

/// How many recently opened documents Recent keeps.
pub const RECENT_LIMIT: i64 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum View {
    Recent,
    Pinned,
    NeedsDocType,
    NeedsTaxYear,
    ReceivedThisWeek,
}

impl View {
    pub const ALL: [View; 5] = [
        View::Recent,
        View::Pinned,
        View::NeedsDocType,
        View::NeedsTaxYear,
        View::ReceivedThisWeek,
    ];

    pub fn label(self) -> &'static str {
        match self {
            View::Recent => "Recent",
            View::Pinned => "Pinned",
            View::NeedsDocType => "Needs doc type",
            View::NeedsTaxYear => "Needs tax year",
            View::ReceivedThisWeek => "Received this week",
        }
    }

    /// Views that list work to do (shown with a warning color).
    pub fn is_attention(self) -> bool {
        matches!(self, View::NeedsDocType | View::NeedsTaxYear)
    }
}

fn week_ago() -> String {
    let t = time::OffsetDateTime::now_utc() - time::Duration::days(7);
    t.format(&time::format_description::well_known::Rfc3339)
        .expect("RFC 3339 formats")
}

/// Documents in a client folder with no tax year, outside the template's `Permanent` folder.
const NEEDS_YEAR: &str = "d.deleted_at IS NULL AND d.tax_year IS NULL AND d.client_id IS NOT NULL
     AND NOT EXISTS (SELECT 1 FROM client c WHERE c.id = d.client_id
                     AND substr(d.rel_path, 1, length(c.folder_rel) + 11) = c.folder_rel || '/Permanent/')";

fn sql(view: View) -> (String, bool, bool) {
    // (where/order sql, needs user param, needs week param)
    match view {
        View::Recent => (
            "SELECT d.id FROM user_recent r JOIN document d ON d.id = r.doc_id
             WHERE r.user = ?1 AND d.deleted_at IS NULL ORDER BY r.opened_at DESC"
                .into(),
            true,
            false,
        ),
        View::Pinned => (
            "SELECT d.id FROM user_pin p JOIN document d ON d.id = p.doc_id
             WHERE p.user = ?1 AND p.deleted_at IS NULL AND d.deleted_at IS NULL ORDER BY d.rel_path"
                .into(),
            true,
            false,
        ),
        View::NeedsDocType => (
            "SELECT d.id FROM document d WHERE d.deleted_at IS NULL AND d.doc_type_id IS NULL ORDER BY d.rel_path"
                .into(),
            false,
            false,
        ),
        View::NeedsTaxYear => (
            format!("SELECT d.id FROM document d WHERE {NEEDS_YEAR} ORDER BY d.rel_path"),
            false,
            false,
        ),
        View::ReceivedThisWeek => (
            "SELECT d.id FROM document d WHERE d.deleted_at IS NULL AND d.status = 'received'
             AND d.filed_at >= ?1 ORDER BY d.filed_at DESC"
                .into(),
            false,
            true,
        ),
    }
}

/// Document ids in a view, in the view's order.
pub fn ids(conn: &Connection, user: &str, view: View) -> Result<Vec<String>> {
    let (q, by_user, by_week) = sql(view);
    let mut st = conn.prepare_cached(&q)?;
    let rows = if by_user {
        st.query_map([user], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?
    } else if by_week {
        st.query_map([week_ago()], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?
    } else {
        st.query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?
    };
    Ok(rows)
}

/// Count per view, in [`View::ALL`] order.
pub fn counts(conn: &Connection, user: &str) -> Result<Vec<(View, usize)>> {
    View::ALL
        .iter()
        .map(|v| ids(conn, user, *v).map(|i| (*v, i.len())))
        .collect()
}

/// Pins (or unpins) documents for `actor.user`, as one Transaction.
pub fn set_pinned(
    conn: &mut Connection,
    actor: &Actor,
    doc_ids: &[String],
    pinned: bool,
) -> Result<Option<Committed>> {
    let user = actor.user.clone().unwrap_or_default();
    let now = crate::now_rfc3339();
    let mut tx = Transaction::new(if pinned { "pin" } else { "unpin" });
    let mut n = 0;
    for id in doc_ids {
        let before: Option<Option<String>> = conn
            .query_row(
                "SELECT deleted_at FROM user_pin WHERE user = ?1 AND doc_id = ?2",
                [&user, id],
                |r| r.get(0),
            )
            .ok();
        let is_pinned = matches!(before, Some(None));
        if is_pinned == pinned {
            continue;
        }
        n += 1;
        let deleted: Option<String> = (!pinned).then(|| now.clone());
        tx = tx
            .catalog(Stmt::new(
                "INSERT INTO user_pin (user, doc_id, created, device_id, deleted_at) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(user, doc_id) DO UPDATE SET deleted_at = excluded.deleted_at,
                   device_id = excluded.device_id, rev = user_pin.rev + 1",
                vec![user.clone().into(), id.clone().into(), now.clone().into(), actor.device_id.clone().into(), deleted.into()],
            ))
            .catalog_undo(match &before {
                None => Stmt::new(
                    "DELETE FROM user_pin WHERE user = ?1 AND doc_id = ?2",
                    vec![user.clone().into(), id.clone().into()],
                ),
                Some(d) => Stmt::new(
                    "UPDATE user_pin SET deleted_at = ?3, rev = rev + 1 WHERE user = ?1 AND doc_id = ?2",
                    vec![user.clone().into(), id.clone().into(), d.clone().into()],
                ),
            })
            .audit(AuditEntry::new(if pinned { "pin" } else { "unpin" }, Some(id), json!({})));
    }
    if n == 0 {
        return Ok(None);
    }
    Ok(Some(tx.commit(conn, &mut NoFileOps, actor)?))
}

pub fn is_pinned(conn: &Connection, user: &str, doc_id: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT count(*) FROM user_pin WHERE user = ?1 AND doc_id = ?2 AND deleted_at IS NULL",
        [user, doc_id],
        |r| r.get::<_, i64>(0),
    )? > 0)
}

/// Records that `actor.user` opened a document (Recent, and an `open` audit row).
pub fn record_open(conn: &mut Connection, actor: &Actor, doc_id: &str) -> Result<Committed> {
    let user = actor.user.clone().unwrap_or_default();
    let now = crate::now_rfc3339();
    let prev: Option<String> = conn
        .query_row(
            "SELECT opened_at FROM user_recent WHERE user = ?1 AND doc_id = ?2",
            [&user, doc_id],
            |r| r.get(0),
        )
        .ok();
    Transaction::new("open")
        .catalog(Stmt::new(
            "INSERT INTO user_recent (user, doc_id, opened_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(user, doc_id) DO UPDATE SET opened_at = excluded.opened_at",
            vec![user.clone().into(), doc_id.into(), now.into()],
        ))
        .catalog(Stmt::new(
            "DELETE FROM user_recent WHERE user = ?1 AND doc_id NOT IN
               (SELECT doc_id FROM user_recent WHERE user = ?1 ORDER BY opened_at DESC LIMIT ?2)",
            vec![user.clone().into(), RECENT_LIMIT.into()],
        ))
        .catalog_undo(match prev {
            None => Stmt::new(
                "DELETE FROM user_recent WHERE user = ?1 AND doc_id = ?2",
                vec![user.into(), doc_id.into()],
            ),
            Some(t) => Stmt::new(
                "UPDATE user_recent SET opened_at = ?3 WHERE user = ?1 AND doc_id = ?2",
                vec![user.into(), doc_id.into(), t.into()],
            ),
        })
        .audit(AuditEntry::new("open", Some(doc_id), json!({})))
        .commit(conn, &mut NoFileOps, actor)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Favorite {
    pub id: String,
    pub client_id: String,
    /// `None` = the whole client; `Some("perm")` or `Some("2025")`.
    pub bucket: Option<String>,
    pub folder: Option<String>,
    pub label: String,
}

pub fn favorites(conn: &Connection, user: &str) -> Result<Vec<Favorite>> {
    let mut st = conn.prepare_cached(
        "SELECT f.id, f.client_id, f.bucket, f.folder, f.label FROM user_favorite f
         JOIN client c ON c.id = f.client_id
         WHERE f.user = ?1 AND f.deleted_at IS NULL AND c.deleted_at IS NULL ORDER BY f.label",
    )?;
    let rows = st.query_map([user], |r| {
        Ok(Favorite {
            id: r.get(0)?,
            client_id: r.get(1)?,
            bucket: r.get(2)?,
            folder: r.get(3)?,
            label: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn add_favorite(
    conn: &mut Connection,
    actor: &Actor,
    client_id: &str,
    bucket: Option<&str>,
    folder: Option<&str>,
    label: &str,
) -> Result<Committed> {
    let user = actor.user.clone().unwrap_or_default();
    let exists: i64 = conn.query_row(
        "SELECT count(*) FROM user_favorite WHERE user = ?1 AND client_id = ?2 AND bucket IS ?3
         AND folder IS ?4 AND deleted_at IS NULL",
        rusqlite::params![user, client_id, bucket, folder],
        |r| r.get(0),
    )?;
    if exists > 0 {
        return Err(crate::Error::Tx("That is already a favorite.".into()));
    }
    let id = crate::new_id();
    Transaction::new("add favorite")
        .catalog(Stmt::new(
            "INSERT INTO user_favorite (id, user, client_id, bucket, folder, label, created, device_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            vec![
                id.clone().into(), user.into(), client_id.into(), bucket.into(), folder.into(),
                label.into(), crate::now_rfc3339().into(), actor.device_id.clone().into(),
            ],
        ))
        .catalog_undo(Stmt::new("DELETE FROM user_favorite WHERE id = ?1", vec![id.clone().into()]))
        .audit(AuditEntry::new("favorite_add", None, json!({"favorite": id, "label": label})))
        .commit(conn, &mut NoFileOps, actor)
}

pub fn remove_favorite(conn: &mut Connection, actor: &Actor, id: &str) -> Result<Committed> {
    let now = crate::now_rfc3339();
    Transaction::new("remove favorite")
        .catalog(Stmt::new(
            "UPDATE user_favorite SET deleted_at = ?2, device_id = ?3, rev = rev + 1 WHERE id = ?1",
            vec![id.into(), now.into(), actor.device_id.clone().into()],
        ))
        .catalog_undo(Stmt::new(
            "UPDATE user_favorite SET deleted_at = NULL, rev = rev + 1 WHERE id = ?1",
            vec![id.into()],
        ))
        .audit(AuditEntry::new(
            "favorite_remove",
            None,
            json!({"favorite": id}),
        ))
        .commit(conn, &mut NoFileOps, actor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browse::tests::{add_client, add_doc, catalog};

    fn actor(u: &str) -> Actor {
        Actor {
            user: Some(u.into()),
            device_id: "dev".into(),
        }
    }

    #[test]
    fn attention_views_and_received_this_week() {
        let (_t, c) = catalog();
        add_client(&c, "c1", "1042", "A");
        add_doc(
            &c,
            "y",
            Some("c1"),
            Some(2025),
            "A (1042)/Income Tax/2025/a.pdf",
        );
        add_doc(
            &c,
            "p",
            Some("c1"),
            None,
            "A (1042)/Permanent/Setup/ein.pdf",
        );
        add_doc(&c, "n", Some("c1"), None, "A (1042)/Not Yet Filed/x.pdf");
        add_doc(&c, "u", None, None, "Loose/u.pdf");
        c.execute(
            "UPDATE document SET filed_at = '2020-01-01T00:00:00Z' WHERE id != 'y'",
            [],
        )
        .unwrap();
        c.execute(
            "UPDATE document SET filed_at = ?1 WHERE id = 'y'",
            [crate::now_rfc3339()],
        )
        .unwrap();
        let get = |v| ids(&c, "kwk", v).unwrap();
        assert_eq!(get(View::NeedsDocType).len(), 4);
        assert_eq!(get(View::NeedsTaxYear), ["n"]);
        assert_eq!(get(View::ReceivedThisWeek), ["y"]);
    }

    #[test]
    fn pins_recent_and_favorites_are_per_user() {
        let (_t, mut c) = catalog();
        add_client(&c, "c1", "1042", "A");
        for i in 0..3 {
            add_doc(
                &c,
                &format!("d{i}"),
                Some("c1"),
                Some(2025),
                &format!("A (1042)/x/{i}.pdf"),
            );
        }
        let ids2 = vec!["d0".to_owned(), "d2".to_owned()];
        let done = set_pinned(&mut c, &actor("kwk"), &ids2, true)
            .unwrap()
            .unwrap();
        assert!(
            set_pinned(&mut c, &actor("kwk"), &ids2, true)
                .unwrap()
                .is_none()
        );
        assert_eq!(ids(&c, "kwk", View::Pinned).unwrap(), ["d0", "d2"]);
        assert!(ids(&c, "jm", View::Pinned).unwrap().is_empty());
        crate::tx::undo(&mut c, &mut NoFileOps, &actor("kwk"), &done.tx_id).unwrap();
        assert!(ids(&c, "kwk", View::Pinned).unwrap().is_empty());

        record_open(&mut c, &actor("kwk"), "d1").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        record_open(&mut c, &actor("kwk"), "d0").unwrap();
        assert_eq!(ids(&c, "kwk", View::Recent).unwrap(), ["d0", "d1"]);

        let f = add_favorite(
            &mut c,
            &actor("kwk"),
            "c1",
            Some("2025"),
            Some("x"),
            "A · 2025 · x",
        )
        .unwrap();
        assert!(add_favorite(&mut c, &actor("kwk"), "c1", Some("2025"), Some("x"), "dup").is_err());
        assert_eq!(favorites(&c, "kwk").unwrap().len(), 1);
        assert!(favorites(&c, "jm").unwrap().is_empty());
        let id = favorites(&c, "kwk").unwrap()[0].id.clone();
        remove_favorite(&mut c, &actor("kwk"), &id).unwrap();
        assert!(favorites(&c, "kwk").unwrap().is_empty());
        let _ = f;
        assert!(crate::audit::verify(&c).unwrap().ok());
    }
}
