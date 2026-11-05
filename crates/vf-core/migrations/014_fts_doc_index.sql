-- Which document each page-index row belongs to, indexed (PA-34). doc_fts.doc_id is an
-- UNINDEXED column: finding a document's pages there reads every page of the cabinet
-- (~900K documents). Every doc_fts row has its row here; deleting a row here deletes the
-- page-index row.
CREATE TABLE doc_fts_doc (
    fts_row INTEGER PRIMARY KEY,   -- doc_fts.rowid
    doc_id  TEXT NOT NULL
);
CREATE INDEX doc_fts_doc_id ON doc_fts_doc(doc_id);
INSERT INTO doc_fts_doc (fts_row, doc_id) SELECT rowid, doc_id FROM doc_fts;
CREATE TRIGGER doc_fts_doc_gone AFTER DELETE ON doc_fts_doc BEGIN
    DELETE FROM doc_fts WHERE rowid = old.fts_row;
END;
