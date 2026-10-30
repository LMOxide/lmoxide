-- Character names are picked on the creation screen and belong to one account: no other
-- account's character may have the same name (case-insensitive); an account's characters in
-- different game modes may share theirs. A character still on the creation screen has no name
-- yet ('').
UPDATE characters SET name = '' WHERE playfield = 4000;

-- Two accounts already sharing a name stop the migration (rename one of them first, e.g. with
-- an older build's API).
CREATE TEMP TABLE character_name_check (names_used_by_two_accounts INTEGER CHECK (names_used_by_two_accounts = 0));
INSERT INTO character_name_check
SELECT count(*) FROM (
    SELECT lower(name) FROM characters WHERE name <> '' GROUP BY lower(name) HAVING count(DISTINCT account_id) > 1
);
DROP TABLE character_name_check;

CREATE INDEX characters_name ON characters(name COLLATE NOCASE);

-- (the insert check skips a save that keeps the character's name: an upsert runs it too)
CREATE TRIGGER characters_name_insert BEFORE INSERT ON characters
WHEN NEW.name <> ''
  AND NOT EXISTS (SELECT 1 FROM characters WHERE id = NEW.id AND name = NEW.name)
  AND EXISTS (SELECT 1 FROM characters WHERE name = NEW.name COLLATE NOCASE AND account_id <> NEW.account_id)
BEGIN
    SELECT RAISE(ABORT, 'character name taken');
END;

CREATE TRIGGER characters_name_update BEFORE UPDATE OF name ON characters
WHEN NEW.name <> '' AND NEW.name <> OLD.name
  AND EXISTS (SELECT 1 FROM characters WHERE name = NEW.name COLLATE NOCASE AND account_id <> NEW.account_id)
BEGIN
    SELECT RAISE(ABORT, 'character name taken');
END;

-- When the account last changed its own username (the cooldown); NULL = never.
ALTER TABLE accounts ADD COLUMN username_changed_at INTEGER;
