-- Indexes for P03's background lookups (duplicate checks, OCR queue): content hashes kept as
-- versions, and documents by filing / change time.
CREATE INDEX version_hash ON version(hash);
CREATE INDEX document_filed_at ON document(filed_at);
CREATE INDEX document_updated_at ON document(updated_at);
