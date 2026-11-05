-- The running copy of Vibe Files that wrote each journal row (PA-34 review): on a catalog
-- shared by several sessions, recovery at one session's start replays only the rows of copies
-- that are no longer running, never another session's change in progress.
ALTER TABLE tx_journal ADD COLUMN owner TEXT;
