-- Indexes for a large cabinet (PA-34: ~900K documents, years of audit rows).
-- The trash list and tombstone lookups.
CREATE INDEX document_deleted ON document(deleted_at) WHERE deleted_at IS NOT NULL;
-- What Intake (or a merge, an import) filed last.
CREATE INDEX document_filed_by ON document(filed_by, filed_at);
-- Documents with a tag (the guessed-metadata tag, a search filter).
CREATE INDEX document_tag_tag ON document_tag(tag);
-- The undo stack (per login or workstation) and Intake's batch records.
CREATE INDEX undo_record_owner ON undo_record(user, device_id, undone);
CREATE INDEX undo_record_label ON undo_record(label);
-- Audit rows of one Transaction (the undo stack's summaries) and of one document.
CREATE INDEX audit_tx ON audit(json_extract(detail, '$.tx'));
CREATE INDEX audit_doc ON audit(doc_id);
