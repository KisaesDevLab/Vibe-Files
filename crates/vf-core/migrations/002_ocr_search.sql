-- P03: OCR jobs, per-page text, full-text index.
-- All three are rebuildable from the files (a PDF's text layer, or OCR again); none is synced.

-- One job per document, for the content it had when queued.
CREATE TABLE ocr_job (
    doc_id     TEXT PRIMARY KEY REFERENCES document(id),
    hash       TEXT NOT NULL,                 -- content the job reads
    priority   INTEGER NOT NULL DEFAULT 1,    -- 0 intake, 1 backlog
    state      TEXT NOT NULL DEFAULT 'queued'
               CHECK (state IN ('queued', 'running', 'indexed', 'done', 'failed', 'skipped')),
    pages      INTEGER,
    ocr_pages  INTEGER NOT NULL DEFAULT 0,    -- pages read by OCR (need the text layer written)
    attempts   INTEGER NOT NULL DEFAULT 0,
    error      TEXT,
    queued_at  TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX ocr_job_next ON ocr_job(state, priority, queued_at);

-- Text of each page, by content hash, saved as each page finishes (a restart resumes here).
CREATE TABLE page_text (
    hash   TEXT NOT NULL,
    page   INTEGER NOT NULL,                  -- 0-based
    source TEXT NOT NULL CHECK (source IN ('pdf', 'ocr')),
    text   TEXT NOT NULL,
    words  TEXT,                              -- OCR words, JSON (boxes in pixels at `dpi`)
    dpi    INTEGER,
    PRIMARY KEY (hash, page)
);

-- Full text by document page; trigram for partial matches (EIN fragments, names).
CREATE VIRTUAL TABLE doc_fts USING fts5(
    text,
    doc_id UNINDEXED,
    page UNINDEXED,
    tokenize = 'trigram'
);
