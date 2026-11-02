-- Vibe Files catalog, migration 003: the inspector's "Recent activity" reads a document's last
-- audit rows (WHERE doc_id = ? ORDER BY seq DESC); without this it scans the audit table.
CREATE INDEX audit_doc ON audit(doc_id, seq);
