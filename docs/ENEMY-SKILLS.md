# Reconstructing an enemy's skills

How an enemy's abilities are rebuilt from the client and the live capture (the workflow used for the
spawners, the Alien Mindslicer, the Overlord, the Hurler, the artillery and battle bots, the
grenadier, 2026-10-30). The enemy NPC templates (which monster casts what) were server-side; the
client has the **spells** (RDB 1000621), their effects and the server Lua, but not who cast them.
So a kit is a match between a monster and client spells, checked against the live capture and the
user's memory. Read `docs/SPAWN-RESEARCH.md` for SpawnMonster specifics.

## 1. Find the candidate spells

All commands run from `lmoxide/tools/re` with `./py`.

- **By name**: `./py spell_dump.py -n 'grenadier|bomb'`. Enemy spells are named `XX_<monster>
  <ability> castable` (`XX_alien overlord bolt castable`), bosses `XX_<area> boss <name> ...`
  (`XX_space pd policeHQ boss mech stomp castable`), generic ones `XX_Slash`, `XX_Shot`, `XX_Bomb`.
- **By neighbouring ids**: a monster's spells were made together, so their ids sit side by side
  (the Hurler's smash 319946 beside its throw 319947; the grenadier's grenade 190612 near its bomb
  190796). `./py spell_dump.py $(seq 190790 190800)`.
- **By effect**: the look's particles name the monster. Search `data/effect_catalog.json` for
  particles with the look's name (`bot_battle_merc`, `ogre_hurler`); each package lists the spells
  that use it (`used_by`). `./py spell_dump.py -f <package>` lists spells by EffectPackage.
- **Monster data names** (looks): RDB 1010514 names (`space_buzzer_hive`, `bot_artillery_merc`);
  a dump is in `extract_npc_spells.py` `monsterdata_names`.
- **NPC template names** (TDB 51000) for spawned monsters: `./py -c "import tdc;
  n=tdc.texts(51000); ..."` ("Summoned Gremlin" #1164, "Abducted Medic" #1480, "Vespid Colony"
  #1453). Templates next to each other belong together.
- **Boss variants vs regular**: a dungeon boss (`policeHQ boss mech`) has a bigger kit than the
  regular monster with the same look. Don't give the regular one the boss's spells wholesale; ask
  the user which it had.

## 2. Read them

`./py spell_dump.py <ids>` prints each spell's stats and game action lists:

- Stats: `TargetingMode` (5 a character, 7 ground, 8 self), `AttackRange` (cm),
  `RechargeDelay` / `ItemDelay` / `TimeExist` (cs), `AISpellCategory` (1024 ranged, 512 melee,
  256 / 128 / 16384 spawn / buff / on-combat), `AIPowerrating` (the AI prefers higher),
  `EffectPackage` (the cast's animation and particles), `MaxBuffCounters`.
- Lists by event: 0 on use, 33 each channel tick, 20 on buff end, 37 (a buff's end, client side).
  `C` marks actions the client runs itself (Projectile, StartEffect, Set, SetSpellRecastTime).
- Criteria `(x, y, op)`: op 0 = 1 < 2 > 7 ≤ 8 ≥ 24 ≠ compare stat x with y (stat 520 Rnd rolls
  1–100, 522 LastRnd rereads it: random picks); 152 / 153 caster health above / below y %;
  159 / 160 / 166 buff counter x more / less / equal y; 18 / 19 switch to target / caster; 4
  joins. Decoded from the Linux build (`CriteriaOperator::RegisterCriteriaOperator`,
  `StatCheckScriptCommand_t::Compare`'s jump table); the server runs them (`npc_spells.rs`
  `criteria_met`).
- Projectile items: `./py spell_dump.py -i <item>` (what they cast where they land).
- Server Lua (a spell's `LuaScriptID`, e.g. the boss mech's missile volley 885): dump all with
  `research/lmoxide-notes/client-survey/scripts/dump_lua.py <out dir>` and read `<id>_t11.lua`.
- Effect packages: `data/effect_catalog.json` (particles, animation attributes, timelines:
  `movingmesh` at `target` is a beam between caster and target).

## 3. Check against live

The live capture (`../LMO_Captures.cap`, 13 playfields) shows what the server really sent:
`python3 tools/re/livecap.py ../LMO_Captures.cap --jsonl <scratch>/live.jsonl`, then search the
PassBlob records for the spell id and the instances
(`data/live-capture/lmo-captures/playfield-objects.json`). BehaviorControlCenterIIR_c carries a
cast's spell id; MagicBuffIIR_c a buff (who cast it, how long, the level); positions give
distances (a Summoned Wolf 4.6 m from its Alpha). Many monsters aren't in it (no Overlord, no
grenadier casts): then the client data and the user's memory decide.

## 4. Import

`./py import_spells.py --comment "<why>" <spell ids>` writes each spell, every nano it reaches and
its projectile items to `content/skills` / `content/items` (existing ones are left). Rename
generic slugs (`spawn`, `throw`, `grenade-190612`) to say whose they are, and fix references.
Where the client gives no AI stats (the artillery shells, the missile), add them to the skill's
`client.stats` and say so in its `comment` (they're guesses).

## 4b. Check the look can animate them

A cast's effect package sets the `ability` attribute N; the look's motion set must have
`ability_N`, else the client holds the model stiff (the grenadier's XX_Slash wanted 5).
`tools/re/py $PWD/tools/content/anim_audit.py` lists every enemy skill that doesn't fit;
`tools/re/py $PWD/tools/content/anim_fit.py <monster data> '<name regex>'` lists client spells and
marks those the look can play (OK). Bosses whose motion sets list only idles (Hades, Charon) are
false positives.

## 5. Give them to the monster

In `content/npcs/<monster>.json`: `skills` (the AI casts the strongest ready one in range), and
`ai` (`stationary`, `fixed_facing` for objects, `hatch_after` for spawned eggs). A template whose
skills differ from others of its look casts its own (`skills.rs` `template_spells`). Spawned
monsters need `spawn_hash` and maybe `lifetime` (SPAWN-RESEARCH.md). Say in `comment` what is
client data, live, the user's memory or a guess; `source: guess` when the match is.

## 5b. Actions

Enemies see each cast through (strike, animation, channel) before the next, with the cooldown
counted from its end; only a player's stun or knockback interrupts. A skill an enemy uses
alongside its other actions gets `"parallel": true` in its skill record.

## 6. Check and test

- `cargo run -p lmoxide-content -- fmt` and `-- check --warnings`; `cargo test -p lmoxide-server`
  (`skills::tests::same_as_the_old_files` lists deliberate changes to the old monster kits:
  `ADDED`, `REPLACED`).
- A `!test` scenario in `data/test_scenarios.json` (or `!spawn <name>`: any enemy template, placed
  or not); `docs/TUNABLES.md` for the guesses.
- Restart the LAN server (`tools/lan-server.sh --restart --release`) and give the user the test
  commands. The log shows `enemy cast`, `spawned monster`, `hatches`.
