-- A client's partner (owner 2026-11-05): who at the firm looks after the client. Optional;
-- set in Browse (clients.vf.json) or from a client CSV column named Partner.
ALTER TABLE client ADD COLUMN partner TEXT;
