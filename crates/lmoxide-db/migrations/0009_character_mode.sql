-- The game mode a character belongs to (docs/SERVER-ARCHITECTURE.md): `progression`, `maxed` or
-- `world-editor`. Each mode has its own character set; a character never changes mode. The
-- characters that existed before modes were the play server's, which ran with progression.
ALTER TABLE characters ADD COLUMN mode TEXT NOT NULL DEFAULT 'progression';
CREATE INDEX characters_account_mode ON characters(account_id, mode);
