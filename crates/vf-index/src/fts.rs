//! The page index: one `doc_fts` row per document page (migration 002, trigram tokenizer).

use vf_core::db::{Connection, params};

/// Replaces every indexed page of `doc_id`.
pub fn replace_doc(
    conn: &Connection,
    doc_id: &str,
    pages: &[(u32, String)],
) -> vf_core::Result<()> {
    let tx = vf_core::db::begin_write(conn)?;
    // By the indexed doc_fts_doc (its trigger deletes the doc_fts rows), PA-34.
    tx.execute("DELETE FROM doc_fts_doc WHERE doc_id = ?1", [doc_id])?;
    {
        let mut ins = tx.prepare("INSERT INTO doc_fts (text, doc_id, page) VALUES (?1, ?2, ?3)")?;
        let mut row = tx.prepare("INSERT INTO doc_fts_doc (fts_row, doc_id) VALUES (?1, ?2)")?;
        for (page, text) in pages {
            if !text.trim().is_empty() {
                ins.execute(params![text, doc_id, page])?;
                row.execute(params![tx.last_insert_rowid(), doc_id])?;
            }
        }
    }
    tx.commit()?;
    Ok(())
}

/// Indexed pages of `doc_id`, in page order.
pub fn pages_of(conn: &Connection, doc_id: &str) -> vf_core::Result<Vec<u32>> {
    let mut st = conn.prepare(
        "SELECT f.page FROM doc_fts_doc x JOIN doc_fts f ON f.rowid = x.fts_row
         WHERE x.doc_id = ?1 ORDER BY f.page",
    )?;
    let v = st
        .query_map([doc_id], |r| r.get(0))?
        .collect::<Result<Vec<u32>, _>>()?;
    Ok(v)
}
