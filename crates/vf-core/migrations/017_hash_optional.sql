-- PA-60 (Q-96): a document is cataloged from its folder's listing (path, size, date); its
-- SHA-256 is filled in when something first needs it. `hash` NULL = not fingerprinted yet.
-- SQLite cannot drop NOT NULL in place, so the table is rebuilt with the same columns, in the
-- same order, and its indexes made again. `migrate()` turns foreign keys off around this (six
-- tables reference document(id)) and checks them before it commits.
CREATE TABLE document_new (
    id               TEXT PRIMARY KEY,
    client_id        TEXT REFERENCES client(id),     -- NULL: cabinet-level / unassigned
    doc_type_id      TEXT REFERENCES doc_type(id),
    tax_year         INTEGER,                         -- NULL = permanent file
    rel_path         TEXT NOT NULL,                   -- relative to the cabinet root
    hash             TEXT,                            -- SHA-256 hex of current content; NULL: not read yet
    size             INTEGER NOT NULL,
    mime             TEXT,
    source           TEXT CHECK (source IN ('client', 'firm', 'government', 'third_party')),
    status           TEXT NOT NULL DEFAULT 'received' CHECK (status IN ('received', 'in_work', 'final', 'delivered')),
    filed_by         TEXT,
    filed_at         TEXT NOT NULL,
    device_id        TEXT NOT NULL,
    rev              INTEGER NOT NULL DEFAULT 1,
    updated_at       TEXT NOT NULL,
    deleted_at       TEXT,
    tombstone_reason TEXT,
    file_mtime       INTEGER
);
INSERT INTO document_new
    (id, client_id, doc_type_id, tax_year, rel_path, hash, size, mime, source, status, filed_by,
     filed_at, device_id, rev, updated_at, deleted_at, tombstone_reason, file_mtime)
SELECT id, client_id, doc_type_id, tax_year, rel_path, hash, size, mime, source, status, filed_by,
       filed_at, device_id, rev, updated_at, deleted_at, tombstone_reason, file_mtime
FROM document;
DROP TABLE document;
ALTER TABLE document_new RENAME TO document;

CREATE UNIQUE INDEX document_rel_path ON document(rel_path) WHERE deleted_at IS NULL;
CREATE INDEX document_client ON document(client_id, tax_year);
CREATE INDEX document_hash ON document(hash);
CREATE INDEX document_filed_at ON document(filed_at);
CREATE INDEX document_updated_at ON document(updated_at);
CREATE INDEX document_deleted ON document(deleted_at) WHERE deleted_at IS NOT NULL;
CREATE INDEX document_filed_by ON document(filed_by, filed_at);
-- Documents not fingerprinted yet, by size: a duplicate check reads only those of the new
-- file's size.
CREATE INDEX document_unhashed_size ON document(size) WHERE hash IS NULL;
