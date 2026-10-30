-- GM levels become an ordered ladder (docs/SECURITY.md "Access levels"): 1 alpha tester,
-- 2 contributor, 3 moderator, 4 admin. Before, 1 was a moderator (test commands, world editor)
-- and 2 and up an admin (higher values were client GM bitmasks such as 6 for the god camera,
-- which contributors now get from their level). Existing accounts keep what they could do.
UPDATE accounts SET gm_level = CASE WHEN gm_level >= 2 THEN 4 WHEN gm_level = 1 THEN 3 ELSE 0 END;
