-- The game mode a login is for (the universe port it came through).
ALTER TABLE game_sessions ADD COLUMN mode TEXT NOT NULL DEFAULT 'progression';
