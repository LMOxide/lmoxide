-- The character's movement speed factor (`!speed`, a test command): 1 normal, 0.5 half, 2 double.
ALTER TABLE characters ADD COLUMN speed REAL NOT NULL DEFAULT 1.0;
