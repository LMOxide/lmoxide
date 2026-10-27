# Content: the game's data in one editable tree

Everything the server needs to know about the game lives in `content/`: worlds and what's in them,
NPCs and enemies, objects, skills, buffs, loot tables, items, minifigures, dialogues and
achievements. It is plain JSON, one record per file for definitions and one file per section for
a world. The crate `lmoxide-content` loads it, checks it, and gives the server typed templates,
so the server doesn't hard-code values.

**Status (2026-10-27):** the tree was converted from `data/*.json`, the values hard-coded in
`loot.rs`/`procs.rs`/`combat.rs`/`npc.rs`, and the world editor's published set
(`var/survey-set/published`). The server loads it at start (`--content-dir`, env
`LMOXIDE_CONTENT_DIR`; default the checkout's `content/`; errors stop the server) and is moving
to it one system at a time:

| System | Reads from content | Still from `data/` or code |
|---|---|---|
| Loot (loot.rs) | loot tables, `tunables.loot`, items (stars, orbs, potions, parts, bags, chests), minifigures' parts, bags, `obtain` (regions, rarity), `tunables.rarity` | which source a kill is (enemy/champion/boss by health rank, npc.rs); Resourceful/Professional (procs.rs) |
| Upgrades and procs (procs.rs, combat.rs) | minifigures' stats, attacks (AbilityMods, combo steps) and upgrades; the upgrades' buffs: stat effects (`Life`, `FinalDamageMultiplier`, `MitigationOverrideFlat`, `orb_chance`), `skill_mod` (Empowered Normals), `proc`s and the buffs they apply; `tunables.combat` | `LEVEL` (a constant other constants use); the nanos' client-side Modify actions (the client runs them) |
| Skills (actions.rs, npc_spells.rs, projectile.rs, dash.rs, summons in npc.rs; through skills.rs) | skills' client spells (stats, game action lists), projectiles, dashes, summons and summoned NPCs, ranges; projectile items' on-hit actions and radii; the spells each enemy look casts (its NPC templates' `skills`) | the action types' behaviour (code) |
| NPCs and spawns (npc.rs, progression.rs; through spawns.rs) | each world's enemies, NPCs and smashables (spawns with their templates and overrides; not `removed`, not `surveyed`: those come from the world editor's approved set), NPC fights (`npc_fights`: fighters, armed NPCs, rivals), `tunables.npc_life`; a template's own `loot` table (when it isn't its kind's usual one) replaces the rolled one | spawns that appear during play (`appeared_at`), triggers, actors, scenery, doors and buildies (other modules, still `data/`) |
| Worlds (scenery.rs, jumppad.rs, teleport.rs, encounter.rs, gates.rs, mirrors.rs, buildie.rs, tutorial.rs, discovery.rs, ballista.rs, pistons.rs, plates.rs, tiki.rs; through world_data.rs) | scenery and interactables (spawns with `client.guid`), jump pads (bouncers), teleport destinations (arrival, window, display name), portals, return and respawn points, encounter areas (arenas, ambushes, exploration, champion points), light gates and mirror puzzles, the recorded buildies, cinematic triggers, playfield names, `tunables.story_quests` | how each piece behaves (code) |
| Everything else | | `data/*.json` and constants in the code |

Edits to what the server doesn't read yet have no effect in game. Running the importer again
overwrites the whole tree, edits included: don't, once the tree is edited by hand.

```
content/
  content.json        format version, where the read-only client catalogs are (../data)
  tunables.json       global numbers (combat, loot, NPC regeneration, progression, NPC life, rarity ...)
  texts.json          the client's dialogue texts, answers, text keys and names
  worlds/4200-isle-of-yarr/
    world.json        name, kind, region, level cap, arrival points, respawn points, NPC fights
    spawns.json       enemies, NPCs, smashables, buildies, interactables, doors, scenery, actors
    portals.json  bouncers.json  areas.json  triggers.json  puzzles.json
    scripts/<slug>.json
  regions/ npcs/ factions/ objects/ skills/ buffs/ loot/ items/ minifigures/ dialogues/ achievements/
  schemas/            JSON Schema per kind (generated; for the world editor and editors like VS Code)
```

The client catalogs that are references rather than content stay generated in `data/` and are
never edited: `monster_data.json` (looks), `effect_catalog.json` / `effect_package_ids.json`,
`animations.json`, `stats*.json`, `bcc_states.json`, `iir_types.json`, `rpc_methods.json`,
`gameaction_types.txt`, and `data/live-capture/` (the evidence).

## Commands

```sh
cargo run -p lmoxide-content -- check [--feature feature/christmas] [--warnings]  # every problem, exit 1 on errors
cargo run -p lmoxide-content -- fmt [--check]     # rewrite files in canonical form
cargo run -p lmoxide-content -- schema            # content/schemas/*.schema.json
cargo run -p lmoxide-content -- import --data data --survey ../var/survey-set/published --out content
tools/content/verify_import.py data content       # compares a fresh import with the old files
```

`cargo test -p lmoxide-content` fails when the tree has a problem or isn't formatted.

## Records

Every record has an `id` of the form `<kind>/<slug>`: `npc/scallywag`, `skill/snake-shot`,
`world/isle-of-yarr`. Records in a world's folder include the playfield:
`spawn/4200/grenadier-2`, `portal/4200/to-pirate-storage`. The kind says what the record is,
wherever its file is. Ids are stable: other records refer to them, so rename by adding a new
record and changing the references, not by editing an id.

Client ids (looks, templates, spells, effect packages, text ids) are numbers inside records,
usually in a `render` or `client` block:

```json
{
  "id": "npc/scallywag",
  "name": "Scallywag",
  "kind": "enemy",
  "render": { "monster_data": 207030, "npc_template": 1710, "look": "skeleton_pirate_2", "cars_group": 3 },
  "level": 1,
  "stats": { "health": 139, "life_factor": 2.25 },
  "attack": "skill/spell-197335",
  "skills": [{ "skill": "skill/shot-197338", "weight": 1 }],
  "skills_from": "recorded",
  "loot": "loot/enemy",
  "source": "client"
}
```

Fields every record can have (all optional except `id`):

| Field | |
|---|---|
| `id` | `<kind>/<slug>` |
| `extends` | another record of the same kind: its fields first, this record's on top |
| `name` | |
| `feature` | `live` (default), `feature/...` (new content, events), `other/...` (fixes, experiments) |
| `source` | where the values came from: `client`, `live` (the 2016 capture), `demo` (the 2015 recording), `footage`, `server` (values the server had in code), `proposed` (made up by a tool from client data), `surveyed` (world editor), `guess` |
| `tags` | labels for sorting work (`todo`, `approximate`, `verified`, ...) |
| `comment` | |
| `rev`, `author`, `updated` | written by the tools that save records |

Unknown fields are errors (a typo like `levle` is reported, not ignored).

### Values written by hand

- Durations: `"1.5s"`, `"200ms"`, `"2m"`, `"1h"`, or a number of seconds. Written back as `"1.5s"`.
- Shares: `"20%"` or `0.2`. Written back as `"20%"`.
- Ranges: `3` or `[1, 3]`.

### Inheritance and overrides

`extends` copies another record and changes what this one says. Objects merge field by field,
lists replace, and `null` removes a field:

```json
{ "id": "npc/pirate-specter-elite", "extends": "npc/pirate-specter", "level": 9, "ai": { "prefer_ranged": true } }
```

A spawn changes its template for that spawn only with `overrides`, which takes any fields of the
template:

```json
{ "id": "spawn/4200/scallywag-7", "kind": "enemy", "template": "npc/scallywag", "pos": [154.3, 44.0, 255.7], "overrides": { "level": 5, "stats": { "health": 168 } } }
```

### Features

A record whose `feature` is off is left out. With `--feature feature/christmas` (play servers:
their active features), the records of `feature/christmas` and everything under it are on too.
A record with the same id as one in a parent feature changes it while the feature is on (merged
the same way as `extends`), so an event can reskin an enemy without copying it:

```json
{ "id": "npc/scallywag", "feature": "feature/christmas", "render": { "monster_data": 123456 } }
```

Put such files anywhere, e.g. `features/christmas/npcs/scallywag.json`. New content of a feature
uses new ids.

## The kinds

| Kind | File | What |
|---|---|---|
| `world` | `worlds/<pf>-<slug>/world.json` | playfield id, `kind` (open, pocket, hub, instance, other), `region`, `level.cap`, `arrival` (+ `arrival_guid`, `window`), `arrival_from` (coming back from a world), `respawn_points`, `npc_fights` |
| `spawn` | `spawns.json` | `kind` (enemy, npc, smashable, buildie, interactable, door, scenery, trigger, actor), `template` (`npc/` or `object/`), `pos`, `rot`, `scale`, `overrides`, `respawn`, `patrol`, `wave` (spawned by a script), `anchor`, `fight`, `replaces` + `removed` (world-editor changes), `client` (guid, tag, script group, visual, bounds ...) |
| `portal` | `portals.json` | `pos`, `radius`, `window`, `to` (a destination, below) |
| `bouncer` | `bouncers.json` | `template` (an `object/` of kind bouncer: the look), `pos`, `rot`, `target`, `target_by`, `apex`, `flight`, `then` (next pad), `requires_built` |
| `area` | `areas.json` | `role` (place, arena, ambush, explore, champion, marker, note), `shape`, arena `doors` |
| `trigger` | `triggers.json` | `shape`, `on` (enter, exit, interact), `once` (never, per_player, per_instance, global), `look`, `actions` |
| `puzzle` | `puzzles.json` | Medusa mirrors and light gates (mirrors.rs, gates.rs) |
| `script` | `scripts/<slug>.json` | `area`, `start` (trigger, on_world_load), `params`, `variants`, `waves`, `on_complete` |
| `region` | `regions/` | worlds that share minifigure drops ("Pirate World") |
| `npc` | `npcs/` | `kind` (enemy, npc, boss, champion, summon, trigger), `render`, `level`, `stats`, `attack`, `skills`, `ai`, `loot`, `counts_as`, `dialogue`, `on_defeat`, `lifetime` (summons) |
| `object` | `objects/` | `kind` (smashable, buildie, bouncer, door, interactable, scenery), `render`, `level`, `health`, `loot`, `build`, `launch`, `states`, `on_use`, `on_built`, `on_smashed` |
| `skill` | `skills/` | `client` (the client spell: id, stats, game action lists), `projectile`, `dash`, `summon`, `range`, `area`, `effects`, `target`, `cooldown`, `cast_time`, `ai_category` |
| `buff` | `buffs/` | `nano` (the client buff that shows it), `amount`, `duration`, `ends_on`, `stacking`, `max_stacks`, `effects` |
| `loot` | `loot/` | `rolls`, `entries` |
| `item` | `items/` | `kind` (part, minifig_bag, chest, token, health_orb, projectile, mission_item), `template`, `tokens`, `minifigure`, `chest`, `actions` |
| `minifig` | `minifigures/` | `template`, `series`, `stats`, `normal`, `special`, `upgrades`, `aura_tags`, `parts`, `bag`, `obtain` |
| `dialogue` | `dialogues/` | `texts` (client text ids, `texts.json`), `world`, `npc`, `start`, `pos`, `follows` |
| `achievement` | `achievements/` | the client's lore tree (`lore` id, `parent`, `children`, `kind`, `counter`, ...) and world-editor `rule`s (smash/collect enemies, drops) |
| `faction` | `factions/` | who fights whom (none yet) |

Full field lists with descriptions: `content/schemas/<kind>.schema.json`, or the types in
`crates/lmoxide-content/src/model.rs`.

### Destinations

Portals, triggers and teleports take a destination: `{ "world": "world/isle-of-yarr" }`, with a
named `point` or exact `pos`, a `prompt` ("Do you want to travel to X?"), or a server command
instead of a place: `{ "command": "lost_creations.advance", "args": { "levels": 1 } }`.

### Actions

Triggers, waves, skill hits, buff procs, `on_defeat`, `on_use`, `on_built`, `on_smashed` and
script completion all run the same actions, written `{ "<action>": { ... } }`:

`spawn`, `despawn`, `damage`, `heal`, `apply_buff`, `remove_buff`, `knockback`, `effect`,
`teleport`, `open_window`, `dialogue`, `message`, `music`, `set_state`, `start_script`,
`end_script`, `discover`, `grant_achievement`, `collect`, `drop`, `give_item`, `cinematic`,
`command`, `other`.

Most take a `target`: the action's own target (default), `"self"`, `"trigger_player"`, `"team"`,
`"all_in_area"`, `{ "placement": "spawn/..." }` or `{ "tag": "..." }`. `command` runs server
logic by name with arguments (`upgrade.damage_field`, `upgrade.charge_up_add`, `upgrade.charge_up_release`,
`lost_creations.advance` ...); the server will list the commands it knows. `other` is a
placeholder whose `comment` says what it should do.

### Buffs and their amount

A buff's effects change stats, set a damage type, run procs, change skills, or replace a
shortcut. Its `amount` is a default size that effects use with `"value": "amount"`; whatever
gives the buff (a wave, skill, upgrade) can give its own `amount`, `duration` and `stacks`. One
"Fire" buff then serves waves at 10%, 25% and 40%:

```json
{ "id": "buff/element-fire", "name": "Fire", "amount": "10%",
  "effects": [{ "stat": { "stat": "damage", "op": "mul", "value": "amount" } }, { "damage_type": "fire" }] }
```

```json
"buffs": [{ "buff": "buff/element-fire", "amount": "25%" }]
```

The minifigure upgrades are buffs (`buff/upgrade-*`). Those the server ran in procs.rs carry
their procs, e.g. Berserker:

```json
{ "proc": { "on": "attack", "every": 5, "then": [{ "apply_buff": { "buff": "buff/proc-berserker-strike", "target": "self" } }] } }
```

Proc triggers: `hit`, `normal_hit`, `special_hit`, `attack`, `normal_attack`, `special_attack`,
`damaged`, `kill`, `special_kill`, `health_below` (`below`), `not_hit_for` (`after`),
`interval`, `smash`, `build`; with `every`, `chance`, `lockout`, `channelling`. A `not_hit_for`
proc is a condition: while it holds, the stats of the buffs it applies count (nothing is shown).
The server applies the buff stats `damage`, `defense` and `mitigation`; client stat names
(`MovementMultiplier` ...) are the client's to apply through the buff's nano. Upgrades whose
server effect isn't known are tagged `todo`.

### Loot

A loot table rolls `rolls` times among its weighted entries and rolls every `chance` entry on its
own; entries with neither always drop. Each entry drops a bundle, every part optional:

```json
{ "chance": "6%", "drop": { "minifig_parts": { "from": "world", "count": 1 } } }
```

`stars`, `hearts` (health orbs), `potions`, `minifig_parts` / `minifig_bags` (`"from": "world"`:
what the world's region drops, by rarity; or a list of `minifig/...`), `chest` (one of these
items), `items`, `table` (roll another table). `when` limits an entry (`once_per_player`,
`worlds`, `members_only`, `bonus_stat`). The six tables loot.rs used are
`loot/{smashable,enemy,champion,boss,chest,member-chest}`; every enemy and smashable names its
table, so giving one enemy its own loot is a new table and one changed field.

## Tracking changes

No CRDT: the world editor's survey server applies edits in order, and every record is a small
readable file, so ordinary tools work.

- Files are canonical (`fmt`): field order from the types, short lists on one line, defaults left
  out. A diff shows only what changed.
- One record per file for definitions and sorted records per world file keep git merges small.
- `rev` on each record lets an editor that saves send the revision it started from: different
  fields merge, the same field changed by two people is a conflict to resolve (to be built with
  the editor switch).
- Play servers already keep approval history (docs/WORLD-CONTENT.md); git keeps the rest.

## What the import did

| From | To |
|---|---|
| `playfields.json`, `destinations.json`, `return_points.json`, `resurrection_points.json`, `achievements.json` playfield names | `worlds/*/world.json` (247 worlds) |
| `world_content.json` enemies, npcs, triggers, live_dynamic; `spawns.json`; `npc_fights.json` | spawns + `npcs/` templates (one per name, look and attack; differing level/health as `overrides`) |
| `world_content.json` smashables, buildies, simple dynels, doors, live/demo dynels, client NPCs; `buildies.json`; `loot_items.json` loot tables | spawns + `objects/` |
| `world_content.json` jump pads | `bouncers.json` |
| `world_content.json` static points, `encounters.json` | `areas.json` |
| `world_content.json` + `destinations.json` portals | `portals.json` |
| `mirror_puzzles.json`, `light_gates.json` | `puzzles.json` |
| `minifigure_drops.json` worlds | `regions/` |
| `spell_actions.json`, `npc_spells.json`, `projectiles.json`, `projectile_sizes.json`, `dashes.json`, `summons.json` | `skills/` (+ summoned `npcs/`, projectile `items/`) |
| `minifigure_stats.json` upgrades + procs.rs | `buffs/` |
| `minifigure_stats.json`, `minifig_parts.json`, `minifigure_drops.json`, `minifigure_rarity.json`, `minifigure_auras.json`, `minifigure_monster_data.json`, `minifigure_abilities.json`, `minifigure_spells.json` | `minifigures/` |
| `loot_items.json` tokens, parts, bags, chests | `items/` |
| loot.rs `roll()` | `loot/` |
| `progression.json`, `npc_life.json`, `minifigure_stats.json` model, `story_quests.json`, `minifigure_rarity.json`, constants of combat.rs, loot.rs, npc.rs | `tunables.json` |
| `dialogues.json` | `dialogues/` + `texts.json` |
| `achievements.json` + the world editor's smash rules | `achievements/` |
| the world editor's placements, trigger walls, notes, scripts | spawns, bouncers, triggers, areas, scripts (source `surveyed`) |

Not content (stay as they are): `test_scenarios.json` (dev), `login.html`, `PatchInfoClient.txt`,
the reference catalogs above.

Things the import found: one minifigure bag (598526, Lady Liberty) is for a minifigure that
isn't playable (it never drops; picking one up now gives nothing instead of that minifigure); 30 enemy attack spells have no definition in the client's data
(`tags: ["no-client-definition"]`); client game actions keep their `criteria` (e.g. "has upgrade
448001").

## Next

1. The server reads `content/` instead of `data/*.json`, one system at a time, each giving the
   same results first (loot: same tables, and the same drop rates over 200,000 bags per source):
   loot (done), upgrades and procs (done: same health, damage and mitigation for every minifigure
   and upgrade, the same procs in scripted attack/kill/hit sequences), skills (done: every spell,
   projectile, dash, summon and enemy spell list as the old files had them), NPCs and spawns (done: the
   same spawn list for every playfield), worlds (done: the same scenery, pads, portals, destinations,
   points, areas, puzzles and buildies; the same counts at startup), NPCs and spawns (then each enemy's own `loot` table),
   worlds (portals, bouncers, areas, puzzles), minifigures, achievements. Reloading on change comes
   with the world editor switch.
2. The world editor reads and writes records of this format (forms from `content/schemas`), with
   `rev` merging; its survey set becomes records in `content/` (or a feature folder) instead of a
   separate format.
3. Then the extractors in `tools/re/` stop writing `data/` files that became content, and the
   importer is retired.
