-- A character's id is its account's, and the client treats character instances up to
-- 16,000,000 as NPCs (n3Dynel::IsNPC): such a player's keyboard movement never shows for the
-- others. Accounts there move up by 1,000,000,000 (new accounts are created above it).
PRAGMA defer_foreign_keys = ON;

CREATE TEMP TABLE renumber AS
SELECT id AS old, id + 1000000000 AS new FROM accounts
WHERE id <= 16000000 AND id + 1000000000 NOT IN (SELECT id FROM accounts);

UPDATE accounts SET id = (SELECT new FROM renumber WHERE old = accounts.id)
WHERE id IN (SELECT old FROM renumber);
UPDATE characters SET account_id = (SELECT new FROM renumber WHERE old = characters.account_id)
WHERE account_id IN (SELECT old FROM renumber);
UPDATE characters SET id = (SELECT new FROM renumber WHERE old = characters.id)
WHERE id IN (SELECT old FROM renumber);
UPDATE character_team SET character_id = (SELECT new FROM renumber WHERE old = character_id)
WHERE character_id IN (SELECT old FROM renumber);
UPDATE character_upgrades SET character_id = (SELECT new FROM renumber WHERE old = character_id)
WHERE character_id IN (SELECT old FROM renumber);
UPDATE character_tokens SET character_id = (SELECT new FROM renumber WHERE old = character_id)
WHERE character_id IN (SELECT old FROM renumber);
UPDATE character_minifigures SET character_id = (SELECT new FROM renumber WHERE old = character_id)
WHERE character_id IN (SELECT old FROM renumber);
UPDATE character_tags SET character_id = (SELECT new FROM renumber WHERE old = character_id)
WHERE character_id IN (SELECT old FROM renumber);
UPDATE character_counters SET character_id = (SELECT new FROM renumber WHERE old = character_id)
WHERE character_id IN (SELECT old FROM renumber);
UPDATE api_sessions SET account_id = (SELECT new FROM renumber WHERE old = account_id)
WHERE account_id IN (SELECT old FROM renumber);
UPDATE game_sessions SET account_id = (SELECT new FROM renumber WHERE old = account_id)
WHERE account_id IN (SELECT old FROM renumber);
UPDATE audit_log SET target_account = (SELECT new FROM renumber WHERE old = target_account)
WHERE target_account IN (SELECT old FROM renumber);

-- friendships keep low < high, which a new id can reverse: re-insert them
CREATE TEMP TABLE moved_friendships AS
SELECT coalesce((SELECT new FROM renumber WHERE old = low), low) AS a,
       coalesce((SELECT new FROM renumber WHERE old = high), high) AS b,
       created_at
FROM friendships
WHERE low IN (SELECT old FROM renumber) OR high IN (SELECT old FROM renumber);
DELETE FROM friendships WHERE low IN (SELECT old FROM renumber) OR high IN (SELECT old FROM renumber);
INSERT INTO friendships (low, high, created_at) SELECT min(a, b), max(a, b), created_at FROM moved_friendships;

DROP TABLE moved_friendships;
DROP TABLE renumber;
