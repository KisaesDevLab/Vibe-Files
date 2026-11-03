-- P07: disposition decisions and runs. A document's retention ends (vf_core::retention); a
-- reviewer approves it (destroyed after a 30-day wait) or defers it (shown again later).
CREATE TABLE disposition (
    doc_id         TEXT PRIMARY KEY REFERENCES document(id),
    class_id       TEXT,
    eligible_on    TEXT NOT NULL,         -- yyyy-mm-dd
    decision       TEXT NOT NULL CHECK (decision IN ('approved', 'deferred')),
    decided_by     TEXT,
    decided_at     TEXT NOT NULL,
    destroy_after  TEXT,                  -- approved: yyyy-mm-dd, the end of the wait
    deferred_until TEXT,                  -- deferred: yyyy-mm-dd
    run_id         TEXT,                  -- the run that destroyed it
    destroyed_at   TEXT,
    device_id      TEXT NOT NULL
);

CREATE TABLE disposition_run (
    id          TEXT PRIMARY KEY,
    ts          TEXT NOT NULL,
    by_user     TEXT,
    device_id   TEXT NOT NULL,
    documents   INTEGER NOT NULL,
    certificate TEXT NOT NULL             -- cabinet-relative path of the PDF
);
