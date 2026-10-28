-- Quest progress: where a character is in each main quest it started (by the client's main
-- quest id), how often it completed it and until when it is cooling down.
CREATE TABLE character_quests (
    character_id    INTEGER NOT NULL REFERENCES characters(id) ON DELETE CASCADE,
    main_quest      INTEGER NOT NULL,
    -- the step (quest) it is on, and the counted progress of that step's goal
    step            INTEGER NOT NULL,
    progress        INTEGER NOT NULL,
    completions     INTEGER NOT NULL,
    started_at      INTEGER NOT NULL,
    -- unix time of the last completion (0: never)
    completed_at    INTEGER NOT NULL,
    -- unix time it can start again (0: no cooldown)
    cooldown_until  INTEGER NOT NULL,
    PRIMARY KEY (character_id, main_quest)
);
