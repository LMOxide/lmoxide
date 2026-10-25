-- Minifigure upgrades bought in the Item Shop ("MinifigureUpgrade<n>"): a bit per upgrade
-- position. 0 = none bought (older saves: the server derives them from the rank).
ALTER TABLE character_minifigures ADD COLUMN purchased INTEGER NOT NULL DEFAULT 0;
