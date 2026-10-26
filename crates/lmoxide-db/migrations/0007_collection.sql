-- Smashed minifigures and the minifigures a character owns.

-- Smashed minifigures: rebuilt at this unix time in milliseconds (LockTemplate on the client).
CREATE TABLE character_smashed (
    character_id  INTEGER NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    template      INTEGER NOT NULL,
    until_ms      INTEGER NOT NULL,
    PRIMARY KEY (character_id, template)
);

-- 1: the character owns only the minifigures in character_collection (created with
-- progression on); 0: it owns every minifigure (older saves, progression off).
ALTER TABLE characters ADD COLUMN limited_collection INTEGER NOT NULL DEFAULT 0;

-- Owned minifigures of a character with a limited collection, and when they were obtained.
CREATE TABLE character_collection (
    character_id  INTEGER NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    template      INTEGER NOT NULL,
    obtained_at   INTEGER NOT NULL,
    PRIMARY KEY (character_id, template)
);
