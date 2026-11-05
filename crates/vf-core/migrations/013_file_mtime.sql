-- Each file's last-modified time (seconds since 1970) as last read (PA-34): a rebuild or
-- sweep re-reads a file only when its size or time changed (a cabinet of ~900K files).
ALTER TABLE document ADD COLUMN file_mtime INTEGER;
