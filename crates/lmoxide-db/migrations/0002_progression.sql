-- Character progression: player XP and level, minifigure levels, achievements.

-- Player XP (stat 52) and level (stat 54); level 0 = never set (the server's default applies).
ALTER TABLE characters ADD COLUMN xp INTEGER NOT NULL DEFAULT 0;
ALTER TABLE characters ADD COLUMN level INTEGER NOT NULL DEFAULT 0;

-- Minifigure levels (item stat 3000058); minifigures without a row have the server's default.
CREATE TABLE character_minifigures (
    character_id  INTEGER NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    template      INTEGER NOT NULL,
    level         INTEGER NOT NULL,
    PRIMARY KEY (character_id, template)
);

-- Achievement tags (the client's lore tree, RDB 1000108) and when they were earned.
CREATE TABLE character_tags (
    character_id  INTEGER NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    tag           INTEGER NOT NULL,
    achieved_at   INTEGER NOT NULL,
    PRIMARY KEY (character_id, tag)
);

-- Achievement counters: a stat of the player (template 0) or of one of its minifigures.
CREATE TABLE character_counters (
    character_id  INTEGER NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    stat          INTEGER NOT NULL,
    template      INTEGER NOT NULL,
    value         INTEGER NOT NULL,
    PRIMARY KEY (character_id, stat, template)
);
