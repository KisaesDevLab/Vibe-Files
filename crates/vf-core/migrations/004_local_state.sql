-- Small per-catalog facts (P05): e.g. the last audit row mirrored to _Vibe/audit/*.jsonl.
CREATE TABLE local_state (
    k TEXT PRIMARY KEY,
    v TEXT NOT NULL
);
