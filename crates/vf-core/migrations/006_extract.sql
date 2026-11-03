-- P08: client TIN last 4 (Q-27: never the full number) and what the form recognizers read.
ALTER TABLE client ADD COLUMN tin_last4 TEXT;

-- One row per document: the form found on its first page, as read (rebuildable: re-run the
-- recognizers). TINs are last 4 digits only.
CREATE TABLE document_extract (
    doc_id          TEXT PRIMARY KEY REFERENCES document(id),
    hash            TEXT NOT NULL,           -- content it was read from
    form            TEXT NOT NULL,           -- w2, 1099-int, k1-1065, ...
    tax_year        INTEGER,
    payer_name      TEXT,
    payer_tin4      TEXT,
    recipient_name  TEXT,
    recipient_tin4  TEXT,
    confidence      REAL NOT NULL,
    proposed_client TEXT,                    -- client uuid the matcher proposed
    detail          TEXT NOT NULL DEFAULT '{}',
    read_at         TEXT NOT NULL
);
