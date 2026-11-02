-- Vibe Files catalog, migration 002 (PHASES P01 step 7): per-user pins, favorites, recent opens.
-- Sync-aware like 001: changeable rows carry device_id + rev; removals are tombstones.

CREATE TABLE user_pin (
    user       TEXT NOT NULL,
    doc_id     TEXT NOT NULL REFERENCES document(id),
    created    TEXT NOT NULL,
    device_id  TEXT NOT NULL,
    rev        INTEGER NOT NULL DEFAULT 1,
    deleted_at TEXT,
    PRIMARY KEY (user, doc_id)
);

-- A favorite is a Browse tree position: a client, one of its buckets ('perm' or a year), or a
-- folder inside a bucket (relative to the client folder).
CREATE TABLE user_favorite (
    id         TEXT PRIMARY KEY,
    user       TEXT NOT NULL,
    client_id  TEXT NOT NULL REFERENCES client(id),
    bucket     TEXT,
    folder     TEXT,
    label      TEXT NOT NULL,
    created    TEXT NOT NULL,
    device_id  TEXT NOT NULL,
    rev        INTEGER NOT NULL DEFAULT 1,
    deleted_at TEXT
);
CREATE INDEX user_favorite_user ON user_favorite(user) WHERE deleted_at IS NULL;

CREATE TABLE user_recent (
    user      TEXT NOT NULL,
    doc_id    TEXT NOT NULL REFERENCES document(id),
    opened_at TEXT NOT NULL,
    PRIMARY KEY (user, doc_id)
);
CREATE INDEX user_recent_time ON user_recent(user, opened_at);
