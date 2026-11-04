-- QA 2026-11-03: more kinds of sync conflicts ('deleted_here': deleted on this PC while another
-- changed it). SQLite cannot change a CHECK constraint in place: the table is rebuilt.
CREATE TABLE sync_conflict_new (
    id           TEXT PRIMARY KEY,
    doc_id       TEXT NOT NULL,
    copy_doc_id  TEXT,
    kind         TEXT NOT NULL,      -- both_changed | deleted_there | deleted_here
    from_device  TEXT NOT NULL,
    at           TEXT NOT NULL,
    resolved     TEXT,
    resolved_at  TEXT
);
INSERT INTO sync_conflict_new SELECT id, doc_id, copy_doc_id, kind, from_device, at, resolved,
    resolved_at FROM sync_conflict;
DROP TABLE sync_conflict;
ALTER TABLE sync_conflict_new RENAME TO sync_conflict;
