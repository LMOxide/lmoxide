# Survey mode

A server mode for recording where things were on live, in game, together. Nothing moves or attacks.
Surveyors place enemies, NPCs, smashables and buildies by standing where they belong. They confirm,
edit or delete the guessed content, and mark areas (missions, achievements, dialogues, scripts,
comments) for someone to review later. Each surveyor's work is saved in files that others can
import and merge.

Status: steps 1–4 work (tested with the Linux client and the world editor). Run one next to the
LAN server with `tools/survey-server.sh` (ports below); the web page is the world editor of
lmoxide-dev-ui (`/editor/`).

## Running

`lmoxide-server --mode survey --survey-set <dir>` (default `<data-dir>/survey`): its own data dir,
so survey characters are separate from play characters; every account is GM; nothing moves or
attacks. `--share-accounts <play server's database>` gives it the play server's login key and logins, so
the same patched clients log in with the same usernames and passwords (in the world editor too);
characters stay separate.

`tools/survey-server.sh` does all of it next to `tools/lan-server.sh`: game server 7110–7112,
chat 7210, control 127.0.0.1:7310, patch server 8081 (`lmo <name> <lan-ip>:8081`), API 8091
(every account gets the admin scope there: keep it to the LAN), the page on 5175
(`http://<lan-ip>:5175/editor/`; the LAN server's dev UI on 5174 reaches it too). Data: `var/survey-data`, the set: `var/survey-set`.
`tools/survey-server.sh --restart` rebuilds and restarts the game server and API; the set is
loaded again.

Switching servers in game: `!server editor` on the play server and `!server play` here (also
the dev UI's "→ …" buttons) move the player without logging out: same login, each server's own
character (`switch.rs`; the servers name each other with `--peer-name`, `--peer-data-dir`,
`--peer-game-addr`, `--peer-chat-addr`, which the two scripts pass). The leaving server teleports the
client to where the character is saved on the other one and sends `RedirectToServer`; the other
checks the login cookie in the first one's database, matching accounts by username. Chat stays
with the chat server the client logged in through, which passes it on to the other server's.

## Content categories

Every enemy, NPC, smashable, buildie and bouncer a server spawns has a category:

| Category | Where it comes from | In survey mode |
|---|---|---|
| **live** | recorded from the live server (`world_content.json` `source: "live"`, the 2016 capture) | hide only |
| **demo** | the recorded session in the client folder (`source: "demo"`) | hide only |
| **client** | the client's own data (RDB dynels, buildies, `source: "rdb_*"`, cinematic points) | hide only |
| **guessed** | proposed by `tools/re/extract_world_content.py` (`source: "proposed"`) | confirm, edit, delete, hide |
| **surveyed** | placed, confirmed or edited by a surveyor | edit, delete, hide |

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

Confirming, editing and deleting a guess each save a placement record that **replaces** it (a
deleted guess's record is marked `removed`). The guess itself never changes, so the extractor can
keep regenerating the guesses.

**Bouncers** (jump pads: mortars, wind fans, catapults, planes, lilies, invisible jumpers;
jumppad.rs) are objects too (`kind: "bouncer"`): a pad whose landing point is exact (the recorded
worlds, `target_by: "demo"`: the Tutorial 1300 and the pirate prison 4600) is **demo**; every
other pad's landing point was guessed (docs/TUNABLES.md "Landing points"), so the pad is a
**guess** for an editor to check: confirming or editing it (pos, heading, `target`) makes a
surveyed placement that replaces it, as for spawns.

### Guess ids

A guess's id comes from where it is: `g-` + a hash of its playfield, kind, look and position
(rounded to 10 cm). The extractor doesn't need to change. A placement that replaces a guess also
saves a copy of the guess (kind, look, position). If the extractor moves the guess later, the
id changes, but the replacement still finds it by look within the match distance (default 1 m).
Replacements that find nothing are reported, not dropped.

## Files

One folder per set, with one file per playfield and kind:

```
<set>/
  set.json                 # name, authors, notes about the set
  achievements.json        # smash achievement rules (only when there are some)
  4600.placements.json     # exact: the server spawns these as they are
  4600.notes.json          # areas, scripts and waves: for someone to review and implement
  published/               # the same files: what editors published (play servers read only this)
```

Files are pretty-printed with a fixed order, so diffs and merges stay readable.

**Saving and publishing.** The set is the working copy: every edit is applied at once (in game
too) and written to disk once edits pause for 3 s (at most 30 s after the first unsaved one, and
at shutdown), not on every keystroke, so a crash of the page, the game or the server loses at
most a few seconds. Nothing reaches play servers until it is **published**: `published/` takes
the working version of the chosen changes (the world editor's Publish dialog: all of yours, or one
playfield + feature + author group). A set from before publishing gets `published/` as a copy of
itself at the first start (it was all visible to play servers already).

### Placements (exact)

```json
{
  "format": 1,
  "playfield": 4600,
  "placements": [
    {
      "id": "p-hosttemple-018f3a2c9b41d7e2",
      "kind": "enemy",
      "name": "Skeleton Warrior",
      "monster_data": 82410,
      "level": 2,
      "pos": [334.0, 48.75, 124.0],
      "heading": 1.57,
      "replaces": { "id": "g-6c1e0b5a99f2d317", "kind": "enemy", "monster_data": 82410, "pos": [331.2, 48.75, 120.0] },
      "wave": { "script": "s-hosttemple-…", "wave": "w-hosttemple-…" },
      "patrol": [[330.0, 48.75, 120.0], [342.0, 48.75, 120.0], [342.0, 48.75, 131.0]],
      "comment": "patrols to the dock",
      "author": "hosttemple",
      "updated": 1793000000
    }
  ]
}
```

- `kind`: `enemy`, `npc`, `smashable`, `buildie`, `interactable` or `bouncer`. The look is
  `monster_data` and/or `template`.
- Bouncers: `kind: "bouncer"`, `template` the pad's **visual id** (its look; the catalog's
  bouncer entries), `target` where it throws you (`[x, y, z]`; left out: not set yet, the pad
  doesn't launch), e.g. `{"id": "p-…", "kind": "bouncer", "name": "Mortar", "template": 212668,
  "pos": [329.8, 54.0, 96.7], "heading": 1.2, "target": [331.8, 48.0, 81.8], "replaces": {…}}`.
  `monster_data`, `level`, `wave` and `patrol` aren't used. A surveyed pad keeps the guid of the
  pad it replaces (the tutorial's gated pads stay gated); a new one gets a guid from its id. Where
  a jump goes on from its landing point (pads that throw on: chains, clouds, lilies) isn't
  stored: it follows from the pads there (`chain` in the object list). In review a moved target
  is a change ("target moved 3.2 m", "target set", "target cleared"); merging treats two pads
  as near-duplicates only when their targets are within the distance too.
- `removed: true` (with `replaces`): the replaced guess isn't spawned. A deleted placement of
  your own stays in the file as a `removed` record, so merging someone's older copy doesn't bring
  it back.
- `wave`: the placement is spawned by that script's wave, not when the playfield starts. It names
  the script **and** the wave, because a world can have several scripts.
- `patrol` (enemies and NPCs, optional): the looping route it walks while nobody is fighting it,
  as the play server's patrols (`world_content.json` `patrol`, `npc.rs` `patrol`): two or more
  points, the last leading back to the first; it starts at the point nearest to where it is.
- `tags` (any record: placements, trigger walls, notes, scripts; optional): labels for sorting
  work out, e.g. `approximate` (rough, fix later), `todo`, `review`. Stored lower case without
  the `#` (`lmoxide_survey::tidy_tags`: letters, digits, `-`, `_`, `/`). They change nothing in
  game; the world editor filters, marks and shows them, and the approval queue lists them.
  `GET /v1/survey/tags` counts them (with the suggested ones: approximate, todo, review, guess,
  placeholder, needs-footage, verified).

### Trigger walls

Also in `<playfield>.placements.json`, under `triggers`: walls that do something when a player
walks through them.

```json
{ "id": "t-…", "name": "cove entrance", "pos": [300.0, 48.0, 90.0], "heading": 1.57,
  "size": [6.0, 3.0, 1.0], "color": "green",
  "action": { "script": { "script": "s-…", "wave": null } }, "once": true, "comment": "…" }
```

- `pos` is the bottom middle; `size` is width (across), height and depth (along `heading`).
- `radius` (optional): a **trigger radius** instead of a wall: a cylinder of that radius and
  `size`'s height around `pos`, set off by walking into it from any side. Drawn as a ring of
  the wall look's pieces around the circle (the pressure plates were tried: square, no use).
  Pieces are drawn back to back, since the looks are one-sided.
- `action`: `script` (a script's wave; none: its first), `discover` (`name`: the exploration
  area found, `achievement`: the id it counts toward; both optional, and some achievements need
  several locations, each its own wall), `dialog` (the dialogue's first text id, which names it,
  or a description; optional), `portal` (`window`: the teleport window it opens, `to`: where it
  leads; with only `to`, a trip there), `message` (`text`), `other` (the comment says what). `once`: only
  each player's first crossing counts. `GET /v1/survey/trigger-targets` lists what the world
  editor suggests: the exploration areas (with the achievements each counts toward, and the
  `title` players know them by: "Volcano Summit" for 6200's `Achievement_Volcano`), the
  exploration achievements (with every location each needs, and as the game shows them:
  `title` "Fiery Explorer", `text` "Find the Volcano Summit in Dinosaur Rise.", `group`
  "Paleontologist"; content/achievements from TDB 55700/55701), the recorded dialogues and the
  portals' teleport windows (`portals`: window, destination, whether it asks Yes/No, the
  playfields with such a portal).
- `color`: how the survey server draws it, from the arena doors and forcefields: `green` (the
  default), `shield`, `low shield`, `forcefield` (`GET /v1/survey/wall-looks`). Not the red arena
  door: it is solid, so the wall couldn't be walked through (walls saved as `red` show green). The look is scaled to the
  height and repeated across the width; crossing is checked against the exact box.
- In survey mode crossing one shows what it would do: a script's wave pops in (as "Preview"),
  `dialog` with a number opens it, `portal` opens its window (`!window`; only `to`: `!tp`), the
  others say in chat what happens.
- `collect` (`{"collect": {"drop": "<key>"}}`): gives the player one collectible: `drop` is a
  collect achievement's key (its id, e.g. `"2431"` for Cerberus' Collar; `/smash-achievements`
  `kind: "collect"`) or a `drop:<name>` rule's key. On play servers it raises the achievement's
  counter by one (a `drop:` key with an `item` drops that item at the player), and a wall counts
  **at most once per player, ever**, whatever its `once` (remembered in the character's journal
  finds as `collect:<key>:wall:<wall id>`), so walking back and forth doesn't fill the counter.

### Notes (for review)

```json
{
  "format": 1,
  "playfield": 4600,
  "notes": [
    {
      "id": "n-…", "kind": "achievement",
      "area": { "circle": { "center": [300.0, 48.0, 90.0], "radius": 12.0 } },
      "reference": "Treasure Hunter",
      "comment": "pops when you step on the X",
      "status": "unreviewed", "author": "…", "updated": 0
    }
  ],
  "scripts": [
    {
      "id": "s-…", "name": "Dock ambush",
      "area": { "polygon": { "points": [[…], […], […]] } },
      "reference": null, "comment": "…",
      "waves": [
        { "id": "w-…", "name": "first skeletons", "trigger": { "enter_area": { "note": "n-…" } }, "comment": "…" },
        { "id": "w-…", "name": "captain", "trigger": { "health_below": { "placement": "p-…", "percent": 50 } }, "comment": "…" }
      ],
      "status": "unreviewed", "author": "…", "updated": 0
    }
  ]
}
```

- Note `kind`: `mission_area`, `achievement`, `dialog`, `script`, `comment`. `reference` (the
  name or id of the achievement, dialogue, script) is optional. The comment is always allowed.
- `area`: a point, a circle, or a polygon walked out point by point (from the surveyor's
  positions).
- Triggers (all optional; the comment says the rest): `after` (seconds after the script starts or
  the previous wave), `health_below` (a placement below n% health), `enter_area` (a note, or the
  script's own area), `wave_cleared` (a wave's placements are all defeated), `other`. A script with
  one wave is the simple case.
- `status`: `unreviewed`, `accepted`, `rejected`. A reviewer sets it.
- Waves can carry `tags` too.

### Smash achievement rules

```json
{
  "format": 1,
  "smash": [
    { "id": "minifigure:kills_or_smashes:witch:50", "enemies": ["Witch", "Swamp Hag"],
      "comment": "…", "tags": ["review"], "author": "…", "updated": 0 },
    { "id": "find:4200:Achievement_Mermaid", "enemies": [], "tags": ["todo"], "author": "…", "updated": 0 }
  ]
}
```

Which enemies (or smashables, as defeats name them) count toward a smash achievement. `id` is the
achievement's key (`GET /smash-achievements`): its id, or `minifigure:<what>:<match>:<n>` for
the per-minifigure copies of one achievement ("Smash 50 Witches as Judge", "… as Cyclops"), which
share one rule. A key `find:<playfield>:<location>` (a discover location as `trigger-targets`
names it) holds only tags and a comment, for the editor's tree.

Editors' own records use the same file:

- `category:<name>` (any name): an **enemy category**, a named list of enemies (e.g.
  `category:Skeletons` with live's skeleton names from `/enemy-names`). A rule's `enemies` may
  name a category as `@<name>` (`"@Skeletons"`): play servers count every enemy of that category
  (one level deep: `@` inside a category is ignored; an unknown category counts nothing).
- `drop:<name>`: a **mission item enemies drop** ("Collect" in the editor): `enemies` drop it
  (categories allowed), `item` (optional) its item template, `once` (optional) each enemy
  (name) drops it only once per player.
- A collect achievement's rule (its id, e.g. `2431`): which enemies give one of its
  collectibles (`once`: each enemy name only once per player).

`once` (default false) and `item` (default none) are left out when not set. Rules have a `feature` (default
`live`, left out) like every record, and are published and approved like the others: their
changes go under playfield 0, named "Achievements". On play servers see docs/WORLD-CONTENT.md.

## Merging

The web UI imports a set (or single files) into yours and shows a preview first:

- **Duplicates** (same id, same content) are skipped.
- **Near-duplicates** are skipped: same kind, same look, position within the match distance and
  heading within the match angle. The distance (default 1 m) and angle (default 20°) can be
  changed for each import. The preview lists them so you can take one anyway.
  - Two replacements of the same guess are near-duplicates when they're within the distance.
    Otherwise they're a **conflict**.
- **Conflicts** (same id, or the same guess replaced in two places) are listed. You keep yours or
  theirs (default: the newer `updated`).
- **Removals** are merged like placements, so a delete is shared too.
- **Notes**: same kind, same reference, same comment and centre within the distance = a near-duplicate.
- **Scripts**: same name (ignoring case) and area centre within the distance = the same script. Their
  waves are merged into yours by id, then by name, and their placements are pointed at your
  script and wave ids.
- **Smash rules**: by id (their achievement) only: the same content is a duplicate, else a
  conflict (playfield 0).

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
- Bouncers are the play servers' pads (they launch here too), with the working set's bouncer
  placements applied at once: an edited pad leaves the world and comes back ~2 s later (the
  client needs the id free). A selected bouncer gets the selection marker at the pad and a second
  one at its landing point (both on invisible helpers, shown only to that surveyor). The
  hops of a chain after the first aren't marked. Interacting with a pad selects it (if the client
  sends it: pads have no interaction).
- Not yet: placing, wearing or editing buildies and interactables (they aren't NPCs; the catalog lists
  enemies, NPCs and smashables), showing patrol routes in game, play servers using a set.

## API

lmoxide-dev-ui's world editor (`/editor/`, its `src/editor/backend.ts`) uses these
routes of `lmoxide-api` under `/v1/survey` (admin scope), passed to the survey server's control
interface. `{account}` is the surveyor: a player online on the survey server. Shapes are the
crate's (`Placement`, `Note`, `Script`, `merge::Report`, ...); `SurveyObject` is any spawned object
with its `category`, `playfield` and, for a placement replacing a guess, `replaces`.

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
| GET | `/tags` | | `[{tag, count, suggested}]`: every tag in the working set (waves' and smash rules' too), most used first, and the suggested ones |
| GET | `/playfields` | | `[{id, name, base, placements, triggers, scripts, notes}]` by id: every playfield the client names (the content tree's worlds) or the set has records for; `base`: base content objects, `placements`: the set's (deletions of guesses left out), `triggers`: trigger walls. Cheap enough to poll |
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
| PATCH | `/objects/{id}` | any of `pos`, `heading`, `level`, `name`, `comment`, `tags`, `feature`, `wave`, `patrol`, and `kind` + `monster_data` + `template` (change it into another spawn); trigger walls: `size`, `color`, `action`, `once`; bouncers: `pos`, `heading`, `name`, `comment`, `tags`, `feature`, `target` (`[x,y,z]` or null), `template` (another pad look) | the `SurveyObject` (a guess comes back surveyed, under a new id, still selected) |
| POST | `/objects/{id}/confirm` | | the surveyed `SurveyObject` |
| DELETE | `/objects/{id}` | | `{id}`: the record that keeps it deleted (a deleted guess's is a new placement) |
| POST | `/objects/{id}/forget` | | drops a placement's or trigger wall's record entirely (no deletion record stays); a guess it replaced comes back: its `SurveyObject`, else `null`. Undoing a placement, undoing a delete (forget its record), resetting an edited guess |
| PUT | `/objects/{id}` | a `SurveyObject` snapshot (with `playfield`; bouncers: with `template` and `target`) | makes it exactly so, spawned again; a guess it replaces goes away. Redo, undoing a reset or a delete |
| POST | `/playfields/{pf}/objects` | a snapshot as for `PUT /objects/{id}` (its `playfield` and `replaces` are ignored) | a new record in `pf` (paste): new id (`t-…` for `kind: "trigger"`, else `p-…`), author from the request, feature checked, a `wave` must be one of `pf`'s (else 422); saved and spawned; nobody's selection changes. The new `SurveyObject` |
| GET | `/unpublished` | | changes not published yet, the caller's first: `[{playfield, playfield_name, feature, feature_path, author, mine, summary, items: [{kind, id, name, op, details, updated}]}]` |
| POST | `/publish` | `{playfield?, feature?, author?, ids?}` (author: default the caller, `*` everyone) | `{records, unpublished}` |
| GET / PUT | `/set` | `SetInfo` | `SetInfo` |
| GET | `/export[?playfield=]` | | a bundle: `{format, info, playfields: {"<pf>": {placements, notes, scripts}}, smash_rules?}` (`smash_rules` with the whole set or `playfield=0`) |
| POST | `/import` | `{bundle, options: {distance, heading, prefer, take}, dry_run}` | `merge::Report` |

The page polls `state` every second and reloads objects and notes when `revision` or the playfield
changes, so changes made in game (a selection by attacking, someone else's edit) show up.

## Content editing API

Survey servers also edit the content tree itself (docs/WORLD-EDITOR-CONTENT.md): a git checkout
of lmoxide-content, `var/content-work` (`--content-dir`; `tools/survey-server.sh` clones it from
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
   comment, wave, patrol, change into another spawn), delete; the set is saved as it goes.
4. **Control routes and API**: the routes above (survey.rs `routes`), passed on by `lmoxide-api`.
5. **Notes, scripts and waves** stored and shown per wave; "Preview" spawns a wave again.
6. **Import / export** with the merge preview.
7. **Hosting**: `tools/survey-server.sh` next to the LAN server; `lmo <name> <ip>:<port>`.
8. **Trigger walls**: placed, sized, coloured, moved, deleted and hidden like the rest; crossing
   one shows its action.

Next:

9. **Buildies and interactables**: placing them (ghosts to wear) and editing them (they are
   buildie.rs / scenery.rs objects, not NPCs); buildies can already be selected and hidden.
10. **Patrol routes in game**: markers at the selected patroller's points.
11. **Play servers use accepted sets**: done for placements without a wave (docs/WORLD-CONTENT.md:
    approval queue, rules, features, live reload), trigger walls (walls.rs) and script waves
    (scripts.rs).
12. **docker-compose** service for a survey server.

Open research, separate from the steps: whether the client lets GMs zoom freely (Linux symbols),
and the client's dimension list as a server picker (not needed now).
