-- The character's journal: what it defeated, where it went, what it found. Achievements are
-- earned from these; the API shows them.

-- Enemies, bosses, champions and smashables defeated, by name.
CREATE TABLE character_defeats (
    character_id  INTEGER NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    name          TEXT NOT NULL,
    -- enemy, champion, boss or smashable
    kind          TEXT NOT NULL,
    count         INTEGER NOT NULL,
    first_at      INTEGER NOT NULL,
    last_at       INTEGER NOT NULL,
    PRIMARY KEY (character_id, name)
);

-- Playfields entered.
CREATE TABLE character_visits (
    character_id  INTEGER NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    playfield     INTEGER NOT NULL,
    count         INTEGER NOT NULL,
    first_at      INTEGER NOT NULL,
    last_at       INTEGER NOT NULL,
    PRIMARY KEY (character_id, playfield)
);

-- Collectibles and places found ("<playfield>:<world object name>").
CREATE TABLE character_finds (
    character_id  INTEGER NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    find          TEXT NOT NULL,
    found_at      INTEGER NOT NULL,
    PRIMARY KEY (character_id, find)
);
