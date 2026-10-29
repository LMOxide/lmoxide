-- Story progress the client's lore tree has no flag for (story.rs): cinematics seen
-- ("cinematic/<name>" -> unix time), story goals resolved ("goal/<hash>" -> unix time) and
-- counted ("count/<hash>" -> the count).
CREATE TABLE character_story (
    character_id    INTEGER NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    key             TEXT NOT NULL,
    value           INTEGER NOT NULL,
    PRIMARY KEY (character_id, key)
);
