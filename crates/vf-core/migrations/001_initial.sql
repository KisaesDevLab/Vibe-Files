-- Vibe Files catalog, migration 001 (PHASES P00 step 4).
-- Sync-aware from day one (CLAUDE.md principle 6): every row that can change carries
-- device_id + rev; deletes are tombstones (deleted_at), never row removal.
-- Ids are uuid v7 text unless noted. Times are RFC 3339 UTC text.

CREATE TABLE cabinet (
    id          TEXT PRIMARY KEY,
    firm_id     TEXT NOT NULL,             -- = SentinelV1.firm_id
    root_path   TEXT NOT NULL,
    schema_ver  INTEGER NOT NULL,
    layout_ver  INTEGER NOT NULL,          -- docs/BUCKET_LAYOUT.md version
    created     TEXT NOT NULL
);

CREATE TABLE device (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    first_seen  TEXT NOT NULL,
    last_seen   TEXT NOT NULL,
    mode        TEXT NOT NULL CHECK (mode IN ('local', 'lan', 'remote'))
);

CREATE TABLE client (
    id            TEXT PRIMARY KEY,        -- = SentinelV1.client_id
    ext_id        TEXT NOT NULL,           -- = SentinelV1.tax_software_id (CSV / tax software ID)
    name          TEXT NOT NULL,
    entity_type   TEXT,                    -- individual, s_corp, partnership, c_corp, trust, ...
    folder_rel    TEXT,                    -- client folder relative to the cabinet root
    sentinel_hash TEXT,                    -- SHA-256 of _Vibe/client.json bytes
    extra         TEXT NOT NULL DEFAULT '{}',  -- JSON: CSV extra columns (Filer template tokens)
    device_id     TEXT NOT NULL,
    rev           INTEGER NOT NULL DEFAULT 1,
    updated_at    TEXT NOT NULL,
    deleted_at    TEXT
);
CREATE UNIQUE INDEX client_ext_id ON client(ext_id) WHERE deleted_at IS NULL;

CREATE TABLE client_relationship (
    parent_id  TEXT NOT NULL REFERENCES client(id),
    child_id   TEXT NOT NULL REFERENCES client(id),
    kind       TEXT NOT NULL,              -- shareholder / partner / beneficiary / ... (Filer role)
    share_pct  REAL,
    note       TEXT,
    device_id  TEXT NOT NULL,
    rev        INTEGER NOT NULL DEFAULT 1,
    updated_at TEXT NOT NULL,
    deleted_at TEXT,
    PRIMARY KEY (parent_id, child_id, kind)
);

CREATE TABLE retention_class (
    id                 TEXT PRIMARY KEY,
    name               TEXT NOT NULL UNIQUE,
    years              INTEGER,            -- NULL with anchor 'never'
    anchor             TEXT NOT NULL CHECK (anchor IN ('tax_year_end', 'engagement_close', 'filed_at', 'never')),
    legal_hold_default INTEGER NOT NULL DEFAULT 0,
    device_id          TEXT NOT NULL,
    rev                INTEGER NOT NULL DEFAULT 1,
    updated_at         TEXT NOT NULL,
    deleted_at         TEXT
);

CREATE TABLE doc_type (
    id                 TEXT PRIMARY KEY,
    name               TEXT NOT NULL UNIQUE,
    default_folder_tpl TEXT,               -- Filer template path, e.g. 'Income Tax/{Year}'
    retention_class_id TEXT REFERENCES retention_class(id),
    client_visible     INTEGER NOT NULL DEFAULT 0,
    sort               INTEGER NOT NULL DEFAULT 0,
    device_id          TEXT NOT NULL,
    rev                INTEGER NOT NULL DEFAULT 1,
    updated_at         TEXT NOT NULL,
    deleted_at         TEXT
);

CREATE TABLE document (
    id               TEXT PRIMARY KEY,
    client_id        TEXT REFERENCES client(id),     -- NULL: cabinet-level / unassigned
    doc_type_id      TEXT REFERENCES doc_type(id),
    tax_year         INTEGER,                         -- NULL = permanent file
    rel_path         TEXT NOT NULL,                   -- relative to the cabinet root
    hash             TEXT NOT NULL,                   -- SHA-256 hex of current content
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
    tombstone_reason TEXT
);
CREATE UNIQUE INDEX document_rel_path ON document(rel_path) WHERE deleted_at IS NULL;
CREATE INDEX document_client ON document(client_id, tax_year);
CREATE INDEX document_hash ON document(hash);

CREATE TABLE document_tag (
    doc_id TEXT NOT NULL REFERENCES document(id),
    tag    TEXT NOT NULL,
    PRIMARY KEY (doc_id, tag)
);

CREATE TABLE version (
    doc_id     TEXT NOT NULL REFERENCES document(id),
    hash       TEXT NOT NULL,              -- content in _Vibe/versions/<hash[0..2]>/<hash>
    size       INTEGER NOT NULL,
    created    TEXT NOT NULL,
    created_by TEXT,
    device_id  TEXT NOT NULL,
    reason     TEXT,
    PRIMARY KEY (doc_id, hash, created)
);

CREATE TABLE lock (
    doc_id      TEXT PRIMARY KEY REFERENCES document(id),
    user        TEXT NOT NULL,
    device_id   TEXT NOT NULL,
    since       TEXT NOT NULL,
    lease_until TEXT,
    scope       TEXT NOT NULL DEFAULT 'edit'
);

CREATE TABLE audit (
    seq       INTEGER PRIMARY KEY,         -- gapless, starts at 1
    ts        TEXT NOT NULL,
    user      TEXT,
    device_id TEXT NOT NULL,
    action    TEXT NOT NULL,
    doc_id    TEXT,
    detail    TEXT NOT NULL DEFAULT '{}',  -- JSON
    prev_hash TEXT NOT NULL,
    hash      TEXT NOT NULL
);
-- Append-only: refuse edits and deletes at the database level too.
CREATE TRIGGER audit_no_update BEFORE UPDATE ON audit BEGIN SELECT RAISE(ABORT, 'audit is append-only'); END;
CREATE TRIGGER audit_no_delete BEFORE DELETE ON audit BEGIN SELECT RAISE(ABORT, 'audit is append-only'); END;

CREATE TABLE legal_hold (
    id         TEXT PRIMARY KEY,
    client_id  TEXT REFERENCES client(id),
    doc_id     TEXT REFERENCES document(id),
    reason     TEXT NOT NULL,
    since      TEXT NOT NULL,
    released   TEXT,
    device_id  TEXT NOT NULL,
    rev        INTEGER NOT NULL DEFAULT 1,
    updated_at TEXT NOT NULL,
    CHECK (client_id IS NOT NULL OR doc_id IS NOT NULL)
);

CREATE TABLE consent_7216 (
    id         TEXT PRIMARY KEY,
    client_id  TEXT NOT NULL REFERENCES client(id),
    scope      TEXT NOT NULL,
    signed_at  TEXT NOT NULL,
    expires_at TEXT,
    doc_id     TEXT REFERENCES document(id),
    device_id  TEXT NOT NULL,
    rev        INTEGER NOT NULL DEFAULT 1,
    updated_at TEXT NOT NULL,
    deleted_at TEXT
);

CREATE TABLE user (
    id          TEXT PRIMARY KEY,
    username    TEXT NOT NULL UNIQUE,
    display     TEXT NOT NULL,
    role        TEXT NOT NULL CHECK (role IN ('admin', 'preparer', 'reviewer', 'front_desk', 'read_only')),
    windows_sid TEXT UNIQUE,
    pw_hash     TEXT,                      -- Argon2 PHC string
    totp        TEXT,                      -- wrapped secret
    active      INTEGER NOT NULL DEFAULT 1,
    device_id   TEXT NOT NULL,
    rev         INTEGER NOT NULL DEFAULT 1,
    updated_at  TEXT NOT NULL
);

CREATE TABLE user_client_acl (
    user_id   TEXT NOT NULL REFERENCES user(id),
    client_id TEXT NOT NULL REFERENCES client(id),
    allow     INTEGER NOT NULL,            -- 1 allow, 0 deny (default allow when no row)
    device_id TEXT NOT NULL,
    rev       INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (user_id, client_id)
);

CREATE TABLE saved_search (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    query      TEXT NOT NULL,              -- JSON
    owner      TEXT REFERENCES user(id),
    device_id  TEXT NOT NULL,
    rev        INTEGER NOT NULL DEFAULT 1,
    updated_at TEXT NOT NULL,
    deleted_at TEXT
);

CREATE TABLE sync_state (
    device_id         TEXT PRIMARY KEY,
    last_manifest_rev INTEGER NOT NULL DEFAULT 0,
    last_push         TEXT,
    last_pull         TEXT
);

CREATE TABLE sync_pending (
    doc_id TEXT NOT NULL,
    op     TEXT NOT NULL CHECK (op IN ('put', 'meta', 'delete', 'lock', 'unlock')),
    since  TEXT NOT NULL,
    PRIMARY KEY (doc_id, op)
);

-- Undo records for Transactions (P00 step 5; per-user stack in P05).
CREATE TABLE undo_record (
    tx_id     TEXT PRIMARY KEY,
    user      TEXT,
    device_id TEXT NOT NULL,
    ts        TEXT NOT NULL,
    label     TEXT NOT NULL,
    record    TEXT NOT NULL,               -- JSON
    undone    INTEGER NOT NULL DEFAULT 0
);
