//! Search: words in a document's path (its file name and its folders), narrowed by catalog
//! metadata. The text inside documents is not read or searched (owner 2026-11-13, PA-65: at
//! ~487,000 documents on a share, reading every file cost far more than it gave).

use serde::{Deserialize, Serialize};
use vf_core::db::Connection;

/// Where to look.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum Scope {
    #[default]
    Cabinet,
    /// One client (catalog id).
    Client(String),
    /// A folder (cabinet-relative, forward slashes) and everything below it.
    Folder(String),
}

/// Tax-year filter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Year {
    Year(i32),
    /// Permanent file (no tax year).
    Permanent,
}

/// Doc-type filter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocType {
    /// Catalog id.
    Is(String),
    /// No doc type yet ("Needs doc type").
    Needed,
}

/// A search. Saved searches store this as JSON. A search saved before PA-65 may carry
/// `has_ssn` (a filter on the documents' text): it is read and ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Query {
    /// Words; every word must appear in the document's path (any case, any order).
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub scope: Scope,
    /// Client (catalog id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<Year>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_type: Option<DocType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Filed on or after (`YYYY-MM-DD`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filed_from: Option<String>,
    /// Filed on or before (`YYYY-MM-DD`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filed_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filed_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

impl Query {
    pub fn is_empty(&self) -> bool {
        *self == Query::default()
    }
}

/// One document found.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub doc_id: String,
    pub rel_path: String,
    pub client: Option<String>,
    pub tax_year: Option<i32>,
    pub doc_type: Option<String>,
}

/// The words of `text` (quotes around a word are dropped).
fn words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| w.trim_matches(|c: char| c == '"'))
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `s` with LIKE's wildcards and the escape character escaped (for `ESCAPE '\'`).
fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn like(s: &str) -> String {
    format!("%{}%", escape_like(s))
}

/// Runs `q`: the documents whose path holds every word and that pass the filters, newest
/// filing first, at most `limit`.
pub fn search(conn: &Connection, q: &Query, limit: usize) -> vf_core::Result<Vec<Hit>> {
    use vf_core::db::SqlValue as V;
    let mut params: Vec<V> = Vec::new();
    let mut p = |v: V| {
        params.push(v);
        format!("?{}", params.len())
    };
    let mut wh: Vec<String> = Vec::new();
    for w in words(&q.text) {
        wh.push(format!(
            "d.rel_path LIKE {} ESCAPE '\\'",
            p(V::Text(like(&w)))
        ));
    }
    match &q.scope {
        Scope::Cabinet => {}
        Scope::Client(id) => wh.push(format!("d.client_id = {}", p(V::Text(id.clone())))),
        Scope::Folder(f) => {
            let f = f.trim_end_matches('/');
            wh.push(format!(
                "d.rel_path LIKE {} ESCAPE '\\'",
                p(V::Text(format!("{}/%", escape_like(f))))
            ));
        }
    }
    if let Some(c) = &q.client {
        wh.push(format!("d.client_id = {}", p(V::Text(c.clone()))));
    }
    match &q.year {
        Some(Year::Year(y)) => wh.push(format!("d.tax_year = {}", p(V::Integer(i64::from(*y))))),
        Some(Year::Permanent) => wh.push("d.tax_year IS NULL".into()),
        None => {}
    }
    match &q.doc_type {
        Some(DocType::Is(t)) => wh.push(format!("d.doc_type_id = {}", p(V::Text(t.clone())))),
        Some(DocType::Needed) => wh.push("d.doc_type_id IS NULL".into()),
        None => {}
    }
    for (col, v) in [
        ("d.status", &q.status),
        ("d.source", &q.source),
        ("d.filed_by", &q.filed_by),
    ] {
        if let Some(v) = v {
            wh.push(format!("{col} = {}", p(V::Text(v.clone()))));
        }
    }
    if let Some(f) = &q.filed_from {
        wh.push(format!(
            "substr(d.filed_at, 1, 10) >= {}",
            p(V::Text(f.clone()))
        ));
    }
    if let Some(t) = &q.filed_to {
        wh.push(format!(
            "substr(d.filed_at, 1, 10) <= {}",
            p(V::Text(t.clone()))
        ));
    }
    if let Some(t) = &q.tag {
        wh.push(format!(
            "EXISTS (SELECT 1 FROM document_tag g WHERE g.doc_id = d.id AND g.tag = {})",
            p(V::Text(t.clone()))
        ));
    }
    let mut sql = String::from(
        "SELECT d.id, d.rel_path, c.name, d.tax_year, t.name
         FROM document d
         LEFT JOIN client c ON c.id = d.client_id
         LEFT JOIN doc_type t ON t.id = d.doc_type_id
         WHERE d.deleted_at IS NULL",
    );
    for w in wh {
        sql.push_str(" AND ");
        sql.push_str(&w);
    }
    sql.push_str(" ORDER BY d.filed_at DESC, d.rel_path");
    let lim = p(V::Integer(limit as i64));
    sql.push_str(&format!(" LIMIT {lim}"));
    let mut st = conn.prepare(&sql)?;
    let rows = st.query_map(vf_core::db::params_from_iter(params), |r| {
        Ok(Hit {
            doc_id: r.get(0)?,
            rel_path: r.get(1)?,
            client: r.get(2)?,
            tax_year: r.get(3)?,
            doc_type: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_core::db::Storage;

    fn db() -> (tempfile::TempDir, Connection) {
        let t = tempfile::tempdir().unwrap();
        let c = vf_core::db::open(&t.path().join("c.db"), Storage::Local).unwrap();
        c.execute_batch(
            "INSERT INTO client (id, ext_id, name, device_id, updated_at) VALUES
               ('c1', '1001', 'Acme LLC', 'd', 't'), ('c2', '1002', 'Smith, John', 'd', 't');
             INSERT INTO doc_type (id, name, device_id, updated_at) VALUES ('w2', 'W-2', 'd', 't');
             INSERT INTO document (id, client_id, doc_type_id, tax_year, rel_path, hash, size,
                                   status, source, filed_by, filed_at, device_id, updated_at) VALUES
               ('a', 'c1', NULL, 2025, 'Acme LLC (1001)/Income Tax/2025/scan.pdf', 'ha', 1,
                'received', 'client', 'intake', '2026-02-01T10:00:00Z', 'd', 't'),
               ('b', 'c2', 'w2', 2024, 'Smith, John (1002)/Income Tax/2024/W-2 Sample Payroll.pdf', 'hb', 1,
                'final', 'client', 'kurt', '2025-02-01T10:00:00Z', 'd', 't'),
               ('c', 'c2', NULL, NULL, 'Smith, John (1002)/Permanent/Setup/EIN letter.pdf', NULL, 1,
                'received', 'government', 'intake', '2024-05-01T10:00:00Z', 'd', 't'),
               ('gone', 'c2', NULL, 2024, 'Smith, John (1002)/Income Tax/2024/old payroll.pdf', 'hd', 1,
                'received', 'client', 'intake', '2024-01-01T10:00:00Z', 'd', 't');
             UPDATE document SET deleted_at = 't' WHERE id = 'gone';
             INSERT INTO document_tag (doc_id, tag) VALUES ('b', 'reviewed');",
        )
        .unwrap();
        (t, c)
    }

    fn ids(c: &Connection, q: &Query) -> Vec<String> {
        search(c, q, 50)
            .unwrap()
            .into_iter()
            .map(|h| h.doc_id)
            .collect()
    }

    fn text(t: &str) -> Query {
        Query {
            text: t.into(),
            ..Query::default()
        }
    }

    /// Every word must be in the path, in any order and case; a folder counts; one or two
    /// letters are enough; a deleted document is never found.
    #[test]
    fn words_match_the_path() {
        let (_t, c) = db();
        assert_eq!(ids(&c, &text("payroll w-2")), ["b"]);
        assert_eq!(ids(&c, &text("\"EIN\" letter")), ["c"]);
        assert_eq!(ids(&c, &text("permanent")), ["c"]);
        assert_eq!(ids(&c, &text("ei")), ["c"]);
        assert_eq!(ids(&c, &text("smith 2024")), ["b"]);
        assert!(ids(&c, &text("payroll scan")).is_empty());
        // LIKE's own wildcards are plain characters here.
        assert!(ids(&c, &text("100%")).is_empty());
        assert!(ids(&c, &text("w_2")).is_empty());
    }

    /// No words: the filters alone, newest filing first. A document with no fingerprint yet
    /// (PA-60) is found like any other.
    #[test]
    fn filters_narrow_and_order_by_filing() {
        let (_t, c) = db();
        assert_eq!(ids(&c, &Query::default()), ["a", "b", "c"]);
        let q = |f: fn(&mut Query)| {
            let mut q = Query::default();
            f(&mut q);
            q
        };
        assert_eq!(
            ids(&c, &q(|q| q.scope = Scope::Client("c2".into()))),
            ["b", "c"]
        );
        assert_eq!(
            ids(
                &c,
                &q(|q| q.scope = Scope::Folder("Smith, John (1002)/Permanent".into()))
            ),
            ["c"]
        );
        assert_eq!(ids(&c, &q(|q| q.client = Some("c1".into()))), ["a"]);
        assert_eq!(ids(&c, &q(|q| q.year = Some(Year::Year(2024)))), ["b"]);
        assert_eq!(ids(&c, &q(|q| q.year = Some(Year::Permanent))), ["c"]);
        assert_eq!(
            ids(&c, &q(|q| q.doc_type = Some(DocType::Is("w2".into())))),
            ["b"]
        );
        assert_eq!(
            ids(&c, &q(|q| q.doc_type = Some(DocType::Needed))),
            ["a", "c"]
        );
        assert_eq!(ids(&c, &q(|q| q.status = Some("final".into()))), ["b"]);
        assert_eq!(ids(&c, &q(|q| q.source = Some("government".into()))), ["c"]);
        assert_eq!(ids(&c, &q(|q| q.filed_by = Some("kurt".into()))), ["b"]);
        assert_eq!(
            ids(&c, &q(|q| q.filed_from = Some("2025-01-01".into()))),
            ["a", "b"]
        );
        assert_eq!(
            ids(&c, &q(|q| q.filed_to = Some("2024-12-31".into()))),
            ["c"]
        );
        assert_eq!(ids(&c, &q(|q| q.tag = Some("reviewed".into()))), ["b"]);
        // Words and a filter together; the limit.
        let mut both = text("pdf");
        both.year = Some(Year::Year(2025));
        assert_eq!(ids(&c, &both), ["a"]);
        assert_eq!(search(&c, &Query::default(), 2).unwrap().len(), 2);
        let hit = &search(&c, &text("w-2"), 5).unwrap()[0];
        assert_eq!(hit.client.as_deref(), Some("Smith, John"));
        assert_eq!(
            (hit.tax_year, hit.doc_type.as_deref()),
            (Some(2024), Some("W-2"))
        );
    }

    /// A search saved before PA-65 with the text filter `has_ssn` still loads.
    #[test]
    fn an_older_saved_search_still_reads() {
        let q: Query =
            serde_json::from_str(r#"{"text":"w-2","scope":{"kind":"cabinet"},"has_ssn":true}"#)
                .unwrap();
        assert_eq!(q, text("w-2"));
    }
}
