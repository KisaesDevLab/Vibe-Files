//! Search (P03 step 4): words in the page text (FTS5 trigram, so `3456` finds `12-3456789`)
//! and in file names, narrowed by catalog metadata. One hit per document: its best page, with
//! a snippet.

use serde::{Deserialize, Serialize};
use vf_core::db::Connection;

/// Snippet highlight markers (start, end): control characters the UI turns into styling.
pub const MARK_START: char = '\u{2}';
pub const MARK_END: char = '\u{3}';

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

/// A search. Saved searches store this as JSON (step 6).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Query {
    /// Words; every word must appear (in the text or the file name).
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
    /// Text that looks like an SSN (`123-45-6789`). P06's PII detector refines this.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_ssn: bool,
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
    /// Best matching page (0-based); `None` when only the name or metadata matched.
    pub page: Option<u32>,
    /// Pages that match.
    pub pages: usize,
    /// Text around the match, with [`MARK_START`] / [`MARK_END`] around the words.
    pub snippet: String,
}

/// Words of `text` as FTS5 phrases. Trigram needs three characters; shorter words are
/// matched with LIKE instead.
fn terms(text: &str) -> (Vec<String>, Vec<String>) {
    let mut long = Vec::new();
    let mut short = Vec::new();
    for w in text.split_whitespace() {
        let w = w.trim_matches(|c: char| c == '"');
        if w.is_empty() {
            continue;
        }
        if w.chars().count() >= 3 {
            long.push(w.to_string());
        } else {
            short.push(w.to_string());
        }
    }
    (long, short)
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

/// Runs `q`, best first, at most `limit` documents.
pub fn search(conn: &Connection, q: &Query, limit: usize) -> vf_core::Result<Vec<Hit>> {
    use vf_core::db::SqlValue as V;
    let (long, short) = terms(&q.text);
    let mut params: Vec<V> = Vec::new();
    let mut p = |v: V| {
        params.push(v);
        format!("?{}", params.len())
    };

    // Matching pages: every long word in the page text (FTS), or in the file name.
    let mut sql = String::new();
    let text_match = if long.is_empty() {
        None
    } else {
        let phrase = long
            .iter()
            .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" AND ");
        Some(p(V::Text(phrase)))
    };
    sql.push_str("WITH m AS (");
    match &text_match {
        Some(m) => sql.push_str(&format!(
            "SELECT doc_id, page, rank, snippet(doc_fts, 0, char(2), char(3), '…', 12) AS snip
             FROM doc_fts WHERE doc_fts MATCH {m}"
        )),
        None => {
            sql.push_str("SELECT NULL AS doc_id, NULL AS page, NULL AS rank, NULL AS snip WHERE 0")
        }
    }
    sql.push_str(
        "), best AS (
            SELECT doc_id, page, rank, snip, count(*) OVER (PARTITION BY doc_id) AS n,
                   row_number() OVER (PARTITION BY doc_id ORDER BY rank, page) AS rn FROM m)
         SELECT d.id, d.rel_path, c.name, d.tax_year, t.name, b.page, coalesce(b.n, 0),
                coalesce(b.snip, '')
         FROM document d
         LEFT JOIN best b ON b.doc_id = d.id AND b.rn = 1
         LEFT JOIN client c ON c.id = d.client_id
         LEFT JOIN doc_type t ON t.id = d.doc_type_id
         WHERE d.deleted_at IS NULL",
    );
    let mut wh: Vec<String> = Vec::new();
    // Long words: a text hit, or every long word in the file name.
    if !long.is_empty() {
        let names: Vec<String> = long
            .iter()
            .map(|w| format!("d.rel_path LIKE {} ESCAPE '\\'", p(V::Text(like(w)))))
            .collect();
        wh.push(format!(
            "(b.doc_id IS NOT NULL OR ({}))",
            names.join(" AND ")
        ));
    }
    // Short words (1–2 characters): in the name or any page's text.
    for w in &short {
        let l = p(V::Text(like(w)));
        wh.push(format!(
            "(d.rel_path LIKE {l} ESCAPE '\\' OR EXISTS (SELECT 1 FROM doc_fts f
              WHERE f.doc_id = d.id AND f.text LIKE {l} ESCAPE '\\'))"
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
    if q.has_ssn {
        wh.push(
            "EXISTS (SELECT 1 FROM page_text x WHERE x.hash = d.hash AND x.text GLOB
              '*[0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9][0-9][0-9]*')"
                .into(),
        );
    }
    for w in wh {
        sql.push_str(" AND ");
        sql.push_str(&w);
    }
    sql.push_str(" ORDER BY b.rank IS NULL, b.rank, d.filed_at DESC, d.rel_path");
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
            page: r.get(5)?,
            pages: r.get::<_, i64>(6)? as usize,
            snippet: r.get(7)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// `snippet` split into (text, highlighted) runs.
pub fn runs(snippet: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut on = false;
    for c in snippet.chars() {
        if c == MARK_START || c == MARK_END {
            if !cur.is_empty() {
                out.push((std::mem::take(&mut cur), on));
            }
            on = c == MARK_START;
        } else {
            cur.push(if c == '\n' { ' ' } else { c });
        }
    }
    if !cur.is_empty() {
        out.push((cur, on));
    }
    out
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
               ('c', 'c2', NULL, NULL, 'Smith, John (1002)/Permanent/Setup/EIN letter.pdf', 'hc', 1,
                'received', 'government', 'intake', '2024-05-01T10:00:00Z', 'd', 't');
             INSERT INTO document_tag (doc_id, tag) VALUES ('b', 'reviewed');
             INSERT INTO page_text (hash, page, source, text) VALUES
               ('hb', 0, 'pdf', 'SSN 987-65-4321 wages');",
        )
        .unwrap();
        crate::fts::replace_doc(
            &c,
            "a",
            &[
                (0, "Cover page".into()),
                (1, "Payer EIN 12-3456789 Anderson".into()),
                (2, "Totals EIN 12-3456789".into()),
            ],
        )
        .unwrap();
        crate::fts::replace_doc(&c, "b", &[(0, "Wages, tips 52,000 Sample Payroll".into())])
            .unwrap();
        (t, c)
    }

    fn ids(h: &[Hit]) -> Vec<&str> {
        h.iter().map(|h| h.doc_id.as_str()).collect()
    }

    fn q(text: &str) -> Query {
        Query {
            text: text.into(),
            ..Query::default()
        }
    }

    #[test]
    fn partial_words_find_the_best_page_with_a_snippet() {
        let (_t, c) = db();
        let h = search(&c, &q("3456789"), 50).unwrap();
        assert_eq!(ids(&h), ["a"]);
        assert_eq!(h[0].pages, 2);
        assert!(matches!(h[0].page, Some(1 | 2)), "{:?}", h[0].page);
        assert!(
            h[0].snippet.contains("\u{2}3456789\u{3}"),
            "{:?}",
            h[0].snippet
        );
        assert_eq!(
            runs(&h[0].snippet).iter().filter(|r| r.1).count(),
            1,
            "one highlighted run"
        );
        // Every word must appear.
        assert_eq!(ids(&search(&c, &q("EIN Anderson"), 50).unwrap()), ["a"]);
        assert!(search(&c, &q("EIN Anderson Smith"), 50).unwrap().is_empty());
        // Both words in the path: Smith's "EIN letter.pdf".
        assert_eq!(ids(&search(&c, &q("EIN Smith"), 50).unwrap()), ["c"]);
    }

    #[test]
    fn file_names_and_short_words_match_too() {
        let (_t, c) = db();
        let h = search(&c, &q("letter"), 50).unwrap();
        assert_eq!(ids(&h), ["c"]);
        assert_eq!((h[0].page, h[0].pages), (None, 0));
        assert_eq!(ids(&search(&c, &q("W-2"), 50).unwrap()), ["b"]);
        assert_eq!(ids(&search(&c, &q("tips 52"), 50).unwrap()), ["b"]);
    }

    #[test]
    fn metadata_filters_and_scope() {
        let (_t, c) = db();
        let all = |q: Query| ids(&search(&c, &q, 50).unwrap()).join(",");
        assert_eq!(all(Query::default()), "a,b,c");
        assert_eq!(
            all(Query {
                scope: Scope::Client("c2".into()),
                ..Query::default()
            }),
            "b,c"
        );
        assert_eq!(
            all(Query {
                scope: Scope::Folder("Smith, John (1002)/Permanent".into()),
                ..Query::default()
            }),
            "c"
        );
        assert_eq!(
            all(Query {
                year: Some(Year::Permanent),
                ..Query::default()
            }),
            "c"
        );
        assert_eq!(
            all(Query {
                year: Some(Year::Year(2025)),
                ..Query::default()
            }),
            "a"
        );
        assert_eq!(
            all(Query {
                doc_type: Some(DocType::Needed),
                ..Query::default()
            }),
            "a,c"
        );
        assert_eq!(
            all(Query {
                status: Some("final".into()),
                tag: Some("reviewed".into()),
                filed_by: Some("kurt".into()),
                ..Query::default()
            }),
            "b"
        );
        assert_eq!(
            all(Query {
                source: Some("government".into()),
                ..Query::default()
            }),
            "c"
        );
        assert_eq!(
            all(Query {
                filed_from: Some("2025-01-01".into()),
                filed_to: Some("2025-12-31".into()),
                ..Query::default()
            }),
            "b"
        );
        assert_eq!(
            all(Query {
                has_ssn: true,
                ..Query::default()
            }),
            "b"
        );
    }

    #[test]
    fn deleted_documents_are_not_found_and_queries_round_trip() {
        let (_t, c) = db();
        c.execute("UPDATE document SET deleted_at = 't' WHERE id = 'a'", [])
            .unwrap();
        assert!(search(&c, &q("3456789"), 50).unwrap().is_empty());
        let saved = Query {
            text: "EIN".into(),
            year: Some(Year::Permanent),
            scope: Scope::Client("c1".into()),
            ..Query::default()
        };
        let json = serde_json::to_string(&saved).unwrap();
        assert_eq!(serde_json::from_str::<Query>(&json).unwrap(), saved);
        assert!(Query::default().is_empty());
        assert!(!saved.is_empty());
    }
}
