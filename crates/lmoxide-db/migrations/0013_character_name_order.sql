-- Character creation stored its three names as "nick first last"; they are "first nick last"
-- (reading order, as the launcher shows them). Only creation makes three-word names, so every
-- three-word name swaps its first two words: "Sparky Fantastic Burrito" -> "Fantastic Sparky
-- Burrito".
UPDATE characters SET name =
    substr(substr(name, instr(name, ' ') + 1), 1, instr(substr(name, instr(name, ' ') + 1), ' ') - 1)
    || ' ' || substr(name, 1, instr(name, ' ') - 1)
    || ' ' || substr(substr(name, instr(name, ' ') + 1), instr(substr(name, instr(name, ' ') + 1), ' ') + 1)
WHERE instr(name, ' ') > 1
  AND instr(substr(name, instr(name, ' ') + 1), ' ') > 1
  AND instr(substr(substr(name, instr(name, ' ') + 1), instr(substr(name, instr(name, ' ') + 1), ' ') + 1), ' ') = 0
  AND substr(substr(name, instr(name, ' ') + 1), instr(substr(name, instr(name, ' ') + 1), ' ') + 1) <> '';
