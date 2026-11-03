-- P11: B2 sync. What this workstation last exchanged with the bucket, per document and per
-- manifest segment, so a push sends only what changed (any change bumps `document.rev`, whatever
-- wrote it) and a pull can tell "changed here" from "changed there".
CREATE TABLE sync_base (
    doc_id    TEXT PRIMARY KEY,
    client_id TEXT,                 -- NULL: a document outside client folders
    rel_path  TEXT NOT NULL,        -- cabinet-relative, as last exchanged
    hash      TEXT NOT NULL,
    rev       INTEGER NOT NULL,     -- document.rev right after the exchange
    deleted   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX sync_base_client ON sync_base(client_id);

-- Manifest segments as last exchanged: '' is the cabinet segment.
CREATE TABLE sync_segment (
    client_id     TEXT PRIMARY KEY,
    rev           INTEGER NOT NULL,
    folder_rel    TEXT,
    sentinel_hash TEXT,
    files_hash    TEXT              -- cabinet segment: hash of the master files sent
);

-- Contents (by SHA-256) known to be in the bucket.
CREATE TABLE sync_object (
    hash TEXT PRIMARY KEY
);

-- Unlocked collisions (P11 step 6): the remote content took the document's path, this
-- workstation's content was kept beside it as a new document.
CREATE TABLE sync_conflict (
    id           TEXT PRIMARY KEY,
    doc_id       TEXT NOT NULL,     -- the canonical document (remote content)
    copy_doc_id  TEXT,              -- the conflict copy (this workstation's content)
    kind         TEXT NOT NULL CHECK (kind IN ('both_changed', 'deleted_there')),
    from_device  TEXT NOT NULL,     -- device that wrote the remote change
    at           TEXT NOT NULL,
    resolved     TEXT,              -- mine | theirs | both
    resolved_at  TEXT
);
