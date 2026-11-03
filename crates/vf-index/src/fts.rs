//! The page index: one `doc_fts` row per document page (migration 002, trigram tokenizer).

use vf_core::db::{Connection, params};

/// Replaces every indexed page of `doc_id`.
pub fn replace_doc(
    conn: &Connection,
    doc_id: &str,
    pages: &[(u32, String)],
) -> vf_core::Result<()> {
    // Write lock up front: a read-then-write transaction cannot wait for another connection
    // (SQLITE_BUSY_SNAPSHOT); OCR workers each hold one.
    let tx =
        vf_core::db::Transaction::new_unchecked(conn, vf_core::db::TransactionBehavior::Immediate)?;
    tx.execute("DELETE FROM doc_fts WHERE doc_id = ?1", [doc_id])?;
    {
        let mut ins = tx.prepare("INSERT INTO doc_fts (text, doc_id, page) VALUES (?1, ?2, ?3)")?;
        for (page, text) in pages {
            if !text.trim().is_empty() {
                ins.execute(params![text, doc_id, page])?;
            }
        }
    }
    tx.commit()?;
    Ok(())
}

/// Drops `doc_id` from the index.
pub fn remove_doc(conn: &Connection, doc_id: &str) -> vf_core::Result<()> {
    conn.execute("DELETE FROM doc_fts WHERE doc_id = ?1", [doc_id])?;
    Ok(())
}

/// Indexed pages of `doc_id`, in page order.
pub fn pages_of(conn: &Connection, doc_id: &str) -> vf_core::Result<Vec<u32>> {
    let mut st = conn.prepare("SELECT page FROM doc_fts WHERE doc_id = ?1 ORDER BY page")?;
    let v = st
        .query_map([doc_id], |r| r.get(0))?
        .collect::<Result<Vec<u32>, _>>()?;
    Ok(v)
}
