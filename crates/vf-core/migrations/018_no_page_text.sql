-- PA-65 (owner 2026-11-13, Q-101): the text inside documents is no longer read, stored or
-- searched. At ~487,000 documents on a share, reading every file cost far more than it gave,
-- and the stored text was most of the catalog's size. Search matches file names, folders and
-- details. The tables go, with what they held; `migrate()` gives the space back afterwards.
DROP TRIGGER IF EXISTS doc_fts_doc_gone;
DROP TABLE IF EXISTS doc_fts_doc;
DROP TABLE IF EXISTS doc_fts;
DROP TABLE IF EXISTS page_text;
DROP TABLE IF EXISTS ocr_job;
DELETE FROM local_state WHERE k IN ('ocr_reads', 'ocr_reads_owner');
