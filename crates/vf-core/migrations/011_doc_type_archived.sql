-- Doc types can be archived (owner 2026-11-04): hidden from pick lists, kept for the
-- documents that have them.
ALTER TABLE doc_type ADD COLUMN archived INTEGER NOT NULL DEFAULT 0;
