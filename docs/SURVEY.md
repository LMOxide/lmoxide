# Survey mode

A server mode for recording where things were on live, in game, together. Nothing moves or attacks.
Surveyors place enemies, NPCs, smashables and buildies by standing where they belong. They confirm,
edit or delete the guessed content, and mark areas (missions, achievements, dialogues, scripts,
comments) for someone to review later. Everything they do is content records (docs/CONTENT.md)
in the world editor's copy of the content repository, `var/content-work`; publishing queues it for
an admin to approve (docs/WORLD-CONTENT.md).

Status: steps 1–4 work (tested with the Linux client and the world editor). The world editor is
the **World Editor game mode** of the one server (docs/SERVER-ARCHITECTURE.md); the web page is
the world editor of lmoxide-dev-ui (`/editor/`).

## Running

`lmoxide-server --modes ...,world-editor:7104 --editor-content-dir <a git checkout of
lmoxide-content>`: the World Editor mode's characters are its own (an account logs in to it
through its universe port, the patcher's Game Mode, with GM level 2 (contributor) and up:
`--world-editor-gm-level`); nothing moves or attacks in its copies of the worlds (copy 2); there
everyone has the test commands. The play modes run beside it in the same process with the published
content; what the editor saves lives in its working copy until published.
`tools/lan-server.sh` runs it all (World Editor on 7104; the content: `var/content-work`, cloned
from `content/` the first time). `--survey-set` (`var/survey-set`) names the survey set of
before content records: it is taken into the working copy once (survey/migrate.rs; its
`migrated.json` says what was done) and otherwise left alone.

`!preview` (World Editor) plays the world one stands in as the working copy has it, with gameplay
on, in a copy of one's own; `!preview end` goes back (preview.rs). The dev UI's "Play this" button
calls `POST /v1/survey/preview` `{end}`: the caller's own character only, so a contributor's level is enough. The old separate survey server
(`tools/survey-server.sh`, `!server`) is retired; `lmoxide-server import-characters` copied its
characters over.

## Content categories

Every enemy, NPC, smashable, buildie and bouncer a server spawns has a category, by its record's
`source`:

| Category | `source` | In survey mode |
|---|---|---|
| **live** | `live`: recorded from the live server (the 2016 capture) | hide only |
| **demo** | `demo`: the recorded session in the client folder | hide only |
| **client** | `client` (or none): the client's own data (RDB dynels, buildies, cinematic points) | hide only |
| **guessed** | `proposed`, `server`, `guess`: proposed by a tool or guessed | confirm, edit, delete, hide |
| **surveyed** | `surveyed`, `footage`: placed, confirmed or edited by a surveyor | edit, delete, hide |

- **Confirm** a guess: it is right as it is. It becomes surveyed at the same place.
- **Edit** a guess (move it, turn it, change its level, give it a patrol route, or change it into
  any other spawn: another enemy, an NPC, a smashable, ...): it becomes surveyed as you left it.
- **Delete** a guess: it is not spawned any more.
- **Hide** anything: it isn't rendered for you until you unhide it, or until you use "unhide all" in
  the web UI. Hiding is never saved. It is for objects in the way.
- **One wave at a time**: of a script with several waves, only one wave's placements are shown,
  wave 1 unless you step to another (or choose "all waves"). Each surveyor steps each script on
  their own. Placing something into a wave shows that wave. Placements of other scripts and the
  playfield's start spawns aren't affected.

Confirming a guess makes its record `source: surveyed` with the tag `verified`; editing one
makes it surveyed as you left it (a level other than its template's goes in `overrides`, with its
health); deleting deletes the record (refused while another record names it: a bouncer that needs
it built, a wave that starts when it is hurt). **Forget** sets a record back to its last commit
(a new one goes, a deleted one comes back): undo of placing, deleting and editing. The guesses
are content records like any other; tools that regenerate them write the content tree.

**Bouncers** (jump pads: mortars, wind fans, catapults, planes, lilies, invisible jumpers;
jumppad.rs) are objects too (`kind: "bouncer"`): a pad whose landing point is exact (the recorded
worlds, `target_by: "demo"`: the Tutorial 1300 and the pirate prison 4600) is **demo**; every
other pad's landing point was guessed (docs/TUNABLES.md "Landing points"), so the pad is a
**guess** for an editor to check: confirming or editing it (pos, heading, `target`, look) writes
its bouncer record, as for spawns (a recorded quaternion rotation stays unless the heading
changes; a new look is an object of kind bouncer with that visual).

### Ids

Objects are named by their record ids: `spawn/4200/scallywag-7`, `bouncer/1200/jump-pad-1`,
`trigger/1612/gate`; notes and scripts by theirs (`area/4200/note-captain`,
`script/4200/ambush`). New ones get ids from their names (numbered when taken). Ids hold
slashes: in a URL path they are percent-encoded (`spawn%2F4200%2Fscallywag-7`), which lmoxide-api
passes on as they are.

## Where the work is

| In the editor | Content record |
|---|---|
| enemy, NPC, smashable (placed or guessed) | `spawn` in `worlds/<world>/spawns.json`, its look a `template` (`npc/...`, `object/...`; placing a look nothing uses makes one) |
| bouncer | `bouncer` in `bouncers.json` |
| trigger wall or radius | `trigger` in `triggers.json` (shape wall or circle, `look` its colour, `once: per_player`, one action) |
| note | `area` in `areas.json` with `role: note` and `note: {kind, reference, status}` |
| script and its waves | `script` in `scripts/<slug>.json` (its area an `area` record of its own) |
| smash rule | an achievement's `rule` (an achievement by lore id), or a rule-only `achievement` record named by `client_name` (`category:`, `drop:`, `find:`, `minifigure:` keys) |
| feature | `features.json` (the registry; records name their feature) |

Saves write the record's file at once (canonical form). The world editor's Definitions tab edits
every other kind (NPCs, objects, skills, buffs, loot, items, minifigures, dialogues,
achievements, regions, factions).

## Server modes

Each mode is its own server process on its own ports and data dir (`tools/lan-server.sh` will
start several at once):

- `--mode play` (today's default), `--mode progression` (today's `--progression`),
  `--mode survey`.
- Survey characters live in the survey server's own data dir, so they are separate from play
  characters. They start at the level cap with every minifigure, and every account is GM.
- Players connect to the port of the server they want (a patcher GUI profile per server).

In survey mode:

- NPC AI doesn't run: no aggro, attacks, walking, respawns, loot or encounters, and gates are open.
  Players take no damage.
- Attacking an object **selects** it instead of hurting it; interacting with a buildie selects
  it instead of building it.
- Buildies (buildie.rs: the recorded ones are `demo`, Medusa's mirrors `client`) are listed with
  the objects (`kind: "buildie"`, id `buildie-<instance>`, never saved) and can be selected,
  hidden and walked to, not changed.
- The selected NPC shows an aura. A selected smashable or buildie gets a ground effect at its
  position, because simple dynels can't show effects.
- Guessed objects are named `[guess] <name>` and surveyed ones `[surveyed] <name>` (a custom NPC
  name, Flags stat 0x8000), so the category shows in game. Friendly NPCs get CARS group 3 so they
  can be attacked (selected), and everything shows its recorded facing.
- The selected character wears the champions' feet aura (effect 162435), shown only to the
  surveyor who selected it. Smashables and trigger walls can't show effects themselves: an
  invisible character ("invisible_monsterdata" 51057, sent only to that surveyor) stands at their
  spot wearing it.
- Marks (`PUT /v1/survey/players/{account}/marks`, `[{by: author|feature|tag, value, look}]`; looks
  from `GET /v1/survey/mark-looks`: achievement auras, champion icons): an effect on every object
  an author changed last, a feature holds or a tag is on, shown only to that surveyor (helpers for simple
  dynels, as the selection's), refreshed after changes and playfield moves; never saved.
- Hides are per surveyor and held in memory (`HIDDEN` in survey.rs: what isn't sent to them).
- Edits respawn the object under a new NPC id (the client needs ~1.5 s before an id is reused).
- Bouncers are the working copy's bouncer records (they launch here too), put in again as they
  change: an edited pad leaves the world and comes back ~2 s later (the
  client needs the id free). A selected bouncer gets the selection marker at the pad and a second
  one at its landing point (both on invisible helpers, shown only to that surveyor). The
  hops of a chain after the first aren't marked. Interacting with a pad selects it (if the client
  sends it: pads have no interaction).
- Not yet: placing, wearing or editing buildies and interactables (they aren't NPCs; the catalog lists
  enemies, NPCs and smashables), showing patrol routes in game.

## API

lmoxide-dev-ui's world editor (`/editor/`, its `src/editor/backend.ts`) uses these
routes of `lmoxide-api` under `/v1/survey` (admin scope), passed to the survey server's control
interface. `{account}` is the surveyor: a player online on the survey server. Shapes are the
crate's (`Note`, `Script`, `TriggerWall`, ...), written into content records
(lmoxide_content::views); `SurveyObject` is any spawned object with its `category` and
`playfield`, named by its record id.

| Method | Path | Body | Answer |
|---|---|---|---|
| GET | `/surveyors` | | players online: `account_id`, `username`, `character_name`, `playfield` |
| GET | `/catalog` | | what can be placed: `kind`, `name`, `monster_data`, `template`, `level`, `used`. Bouncers: one entry per pad look, `{kind: "bouncer", name: "Mortar (a)", monster_data: null, template: <visual id>, level: null, used}`; `place` puts one at the surveyor (facing their way) with no `target` yet |
| GET | `/players/{account}/state` | | `PlayerState`: playfield, position, heading, selection, hidden ids, `shown_waves` (script id → wave id or `"all"`; a script that isn't there shows its first wave), what they wear, the playfield's `revision` |
| POST | `/players/{account}/wear` | `{what: Placeable \| null}` | `PlayerState` |
| POST | `/players/{account}/place` | `{what, wave: WaveRef \| null, feature?, tags?}` | the new `SurveyObject` (selected; its wave is shown) |
| POST | `/players/{account}/place-trigger` | `{name, size, color, action, once, feature?, tags?}` | the new trigger wall as a `SurveyObject` (`kind: "trigger"`, selected) |
| GET | `/wall-looks` | | `[{color}]`: the looks trigger walls can have |
| GET | `/trigger-targets` | | `{locations, achievements, dialogs, portals}`: what trigger walls can discover or open |
| GET | `/tags` | | `[{tag, count, suggested}]`: every tag on a record of the working copy (and on script waves), most used first, and the suggested ones |
| GET | `/playfields` | | `[{id, name, base, placements, triggers, scripts, notes}]` by id: every playfield the client names (the content tree's worlds); `base`: what the content spawned at start, `placements`: surveyed objects, `triggers`: trigger walls. Cheap enough to poll |
| GET | `/smash-achievements` | | `[SmashEntry]`: every kill or smash counter (not builds; the global "Smash 1000 Monsters" ones too), every defeat achievement and, last, the collectible counters (`kind: "collect"`: goblets 2421, meteorites 2424, pies 2426, crystal crates 2428, Cerberus' Collar 2431; `key` the id, `n` the counter's target, `what` and `match` null; their rules say which enemies drop them): `{key, ids, title, text, group, kind: "counter" \| "defeat" \| "collect", what: "kills" \| "smashes" \| "kills_or_smashes" \| null, match, n, minifigures, rule: SmashRule \| null}`. The per-minifigure copies are one entry: `key` `minifigure:<what>:<match lowercase>:<n>`, `ids` all of them, `title` their shared part ("Witch Hunter"), `text` without " as <minifigure>", `group` "Minifigures", `minifigures` how many. Others: `key` the id, `group` the parent category's title, `minifigures` 0. `match`: what the name must contain, or the defeat target; `n`: the counter's target, 1 for a defeat |
| PUT / DELETE | `/smash-achievements/{key}` | `{enemies, comment?, tags?, feature?}` | the `SmashEntry` with its rule (404 for an unknown key) / 204. `{key}` may be `find:<playfield>:<location>` (URL-encoded; the location must be one of `trigger-targets`): its rule has no enemies (else 422), and the entry has `kind: "find"`, `ids` the location's achievements, `title` its title, `group` its playfield's name, `match` its name. `{key}` may be `category:<name>` or `drop:<name>` (URL-encoded, any non-empty name, else 422): the entry has `kind: "category"` / `"drop"`, `title` the name, `text` "", `ids` [], `group`, `what`, `match` null, `n` 0, `minifigures` 0. The body may also have `once` (bool) and `item` (an item template), for any key |
| GET | `/achievement-rules` | | `[SmashRule]`: every rule, discover locations', categories and drops too |
| GET | `/enemy-names` | | `[{name, kind: "enemy" \| "smashable" \| "boss" \| "champion", playfields: [u32], source: "live" \| "demo" \| "client" \| "achievement"}]` by name: every enemy and smashable name a defeat can carry: every playfield's base content and the catalog (boss / champion by health over a regular enemy's of its level, as npc.rs ranks them), the encounter champions (named after their achievements' targets, e.g. "Uggluk Gatekeeper", with the side dungeons they appear in) and the targets of defeat and per-minifigure target achievements that aren't places (`source: "achievement"` when nothing else has them). One entry per name: playfields merged, the biggest kind and the best source kept |
| POST | `/players/{account}/select` | `{id}` or `{nearest: true}` | `PlayerState` |
| POST | `/players/{account}/hide` | `{ids, hidden}` | `PlayerState` |
| POST | `/players/{account}/unhide-all` | | `PlayerState` |
| POST | `/players/{account}/shown-wave` | `{script, wave: <wave id> \| "all"}` | `PlayerState` |
| POST | `/players/{account}/preview-wave` | `{wave}` | |
| POST | `/players/{account}/teleport` | `{pos}` | `PlayerState` |
| GET | `/playfields/{pf}/objects` | | `{revision, objects: SurveyObject[]}`. Bouncers: `kind: "bouncer"`, `template` the visual id, `name` from the look ("Mortar", "Wind fan", "Catapult", "Plane", "Lily pad", "Invisible jumper", else the pad's data name, or a placement's own name), `target: [x,y,z] \| null`, and `chain: [[x,y,z], …]` (left out when empty): the further landing points when the jump goes on through pads that throw on |
| GET | `/playfields/{pf}/notes` | | `{notes, scripts}` |
| PUT / DELETE | `/playfields/{pf}/notes/{id}` | `Note` | `Note` |
| PUT / DELETE | `/playfields/{pf}/scripts/{id}` | `Script` | `Script` |
| PATCH | `/objects/{id}` | any of `pos`, `heading`, `level`, `name`, `comment`, `tags`, `feature`, `wave`, `patrol`, and `kind` + `monster_data` + `template` (change it into another spawn); trigger walls: `size`, `color`, `action`, `once`; bouncers: `pos`, `heading`, `name`, `comment`, `tags`, `feature`, `target` (`[x,y,z]` or null), `template` (another pad look) | the `SurveyObject` (a guess comes back surveyed; `feature` moves the record to that feature's version) |
| POST | `/objects/{id}/confirm` | | the surveyed `SurveyObject` |
| DELETE | `/objects/{id}` | | `{id}`: its record is deleted (forget it to bring it back) |
| POST | `/objects/{id}/forget` | | its record back to its last commit (a new one goes, a deleted one comes back, where it was in its file): its `SurveyObject`, else `null`. Undoing a placement, a delete or an edit |
| PUT | `/objects/{id}` | a `SurveyObject` snapshot (with `playfield`; bouncers: with `template` and `target`) | writes the record exactly so, spawned again. Redo, undoing a delete |
| POST | `/playfields/{pf}/objects` | a snapshot as for `PUT /objects/{id}` (its `playfield` is ignored) | a new record in `pf` (paste): an id from its name, author from the request, feature checked, a `wave` must be one of `pf`'s (else 422); saved and spawned; nobody's selection changes. The new `SurveyObject` |
| GET | `/unpublished` | | the working copy's changes since its last commit not in the publish queue, the caller's first: `[{playfield (0: definitions), playfield_name, feature, feature_path, author, mine, summary, items: [{kind (placement, trigger, note, script, smash_rule, record), id, name, op, details, updated, tags}]}]` |
| POST | `/publish` | `{playfield?, feature?, author?, ids?}` (author: default the caller, `*` everyone) | `{records, unpublished}`: the chosen changes become one publish queue entry (docs/WORLD-CONTENT.md) |

The page polls `state` every second and reloads objects and notes when `revision` or the playfield
changes, so changes made in game (a selection by attacking, someone else's edit) show up.

## Content editing API

Survey servers also edit the content tree itself (docs/WORLD-EDITOR-CONTENT.md): a git checkout
of lmoxide-content, `var/content-work` (`--editor-content-dir`; `tools/lan-server.sh` clones it from
`content/` the first time, its `origin` the content repository). Saves write the record's file
at once (canonical form); nothing is committed here. Routes under `/v1/editor` on the control
interface, passed on by lmoxide-api like `/v1/survey` (admin; the caller is the author).
Records are named by query (`?id=npc/pirate&feature=live`; ids hold slashes).

| Route | |
|---|---|
| `GET /v1/editor/kinds` | `index` (schemas/index.json) and every kind's `schemas` |
| `GET /v1/editor/records?kind=&world=&feature=&tag=&source=&author=&q=&changed=true&problems=true&limit=&offset=` | `{ total, records: [{ id, feature, kind, name, world, file, source, tags, author, rev, fields, problems, changed }] }` (`fields`: the kind's list fields) |
| `POST /v1/editor/records` | create: `{ kind, name, world (playfield), feature, fields }` or `{ kind, copy_of: { id, feature }, name? }`; the id comes from the name (numbered when taken) |
| `GET /v1/editor/record?id=&feature=&with=` | `record` (as written), `resolved` (features in `with`, `extends`) with `origin` per field, a spawn's `template` (overrides applied), `problems`, `used_by`, `versions` (its features), `committed` (at the last commit), `changed` |
| `PUT /v1/editor/record?id=&feature=` | `{ record, base }`: `base` is the version the edit started from; saved since, the two merge field by field. 409 `conflict` with `conflicts: [{ path, base, ours, theirs }]` (nothing saved), 422 `invalid` with `problems` (doesn't parse, references missing or of the wrong kind) |
| `DELETE /v1/editor/record?id=&feature=&replace_with=` | 409 `referenced` with `used_by` while others name it, unless `replace_with` (they're changed to name it) |
| `GET /v1/editor/history?id=&feature=&limit=` | commits that changed it: `hash`, `date`, `author` (`Content-Author:`), `approved_by`, `subject`, `before`, `after` |
| `GET /v1/editor/problems` | the whole tree's check |
| `GET /v1/editor/changes?author=` | records changed since the last commit: `change` (added, changed, removed), `author`, `before`, `after` |
| `GET /v1/editor/catalog/{name}?q=&ids=&limit=` | client ids with names (`monster_data`, `effect_package`, `spell`, `nano`, `item`, `text`, `lore`, `playfield`, `window`, `npc_template`, `stat`); `record` names the content record it belongs to |
| `POST /v1/editor/loot/simulate?id=&rolls=&playfield=` | rolls a loot table as saved (lmoxide_content::roll, the server's rules): `rates: [{ what, chance, per_roll }]`, `nothing`, `warnings` |
| `POST /v1/editor/reload` | reads the working copy again (after a pull or an edit by hand) |

## Steps

Done:

1. **`lmoxide-survey` crate**: formats, ids, load/save, merge and its report, and `apply` (base
   content + sets → what to spawn, with categories; what waves spawn). Unit tests.
2. **Survey mode in the server** (`survey.rs`, hooks in `npc.rs` and `world.rs`): `--mode`,
   frozen AI, no damage, maxed survey characters, GM, categories in names, select by attack,
   selection markers, per-surveyor hiding and shown waves, `--share-accounts`.
3. **Placing**: wear a look (`!skin`), place, confirm, edit (position, heading, level, name,
   comment, wave, patrol, change into another spawn), delete; saved as it goes (since
   2026-10-28 as content records, docs/WORLD-EDITOR-CONTENT.md).
4. **Control routes and API**: the routes above (survey.rs `routes`), passed on by `lmoxide-api`.
5. **Notes, scripts and waves** stored and shown per wave; "Preview" spawns a wave again.
6. **Import / export** with the merge preview (gone with content records: git and the content
   repository's pull requests do it).
7. **Hosting**: the World Editor game mode of the one server (tools/lan-server.sh).
8. **Trigger walls**: placed, sized, coloured, moved, deleted and hidden like the rest; crossing
   one shows its action.

Next:

9. **Buildies and interactables**: placing them (ghosts to wear) and editing them (they are
   buildie.rs / scenery.rs objects, not NPCs); buildies can already be selected and hidden.
10. **Patrol routes in game**: markers at the selected patroller's points.
11. **Play servers use the editor's work**: done; it is content records play servers run from
    their content tree (docs/WORLD-CONTENT.md). Approval (committing to the content repository)
    isn't built yet.
12. **docker-compose** service for a survey server.

Open research, separate from the steps: whether the client lets GMs zoom freely (Linux symbols),
and the client's dimension list as a server picker (not needed now).
