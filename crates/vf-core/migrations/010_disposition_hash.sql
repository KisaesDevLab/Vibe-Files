-- QA 2026-11-03 (P3): an approval is for one content. Destruction re-checks the document (its
-- retention now, its content, that it is there) and skips one that changed since.
ALTER TABLE disposition ADD COLUMN hash TEXT;
