-- P13 step 2: crash safety. A Transaction with file changes records them here (committed on its
-- own) before it touches any file; its catalog commit deletes the row. A row found at startup
-- means the process died between the two: the file changes that landed are reversed
-- (`vf_fs::fileops::recover`), and the catalog was never changed (SQLite rolled it back).
CREATE TABLE tx_journal (
    tx_id TEXT PRIMARY KEY,
    ts    TEXT NOT NULL,
    label TEXT NOT NULL,
    ops   TEXT NOT NULL      -- JSON [{op: FileOp, existed: bool}]: whether its target was there before
);
