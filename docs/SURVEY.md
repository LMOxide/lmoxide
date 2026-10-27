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

## Content categories

Every enemy, NPC, smashable and buildie a server spawns has a category:

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
  4600.placements.json     # exact: the server spawns these as they are
  4600.notes.json          # areas, scripts and waves: for someone to review and implement
```

Files are pretty-printed with a fixed order, so diffs and merges stay readable.

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

- `kind`: `enemy`, `npc`, `smashable`, `buildie` or `interactable`. The look is `monster_data`
  and/or `template`.
- `removed: true` (with `replaces`): the replaced guess isn't spawned. A deleted placement of
  your own stays in the file as a `removed` record, so merging someone's older copy doesn't bring
  it back.
- `wave`: the placement is spawned by that script's wave, not when the playfield starts. It names
  the script **and** the wave, because a world can have several scripts.
- `patrol` (enemies and NPCs, optional): the looping route it walks while nobody is fighting it,
  as the play server's patrols (`world_content.json` `patrol`, `npc.rs` `patrol`): two or more
  points, the last leading back to the first; it starts at the point nearest to where it is.

### Trigger walls

Also in `<playfield>.placements.json`, under `triggers`: walls that do something when a player
walks through them.

```json
{ "id": "t-…", "name": "cove entrance", "pos": [300.0, 48.0, 90.0], "heading": 1.57,
  "size": [6.0, 3.0, 1.0], "color": "red",
  "action": { "script": { "script": "s-…", "wave": null } }, "once": true, "comment": "…" }
```

- `pos` is the bottom middle; `size` is width (across), height and depth (along `heading`).
- `action`: `script` (a script's wave; none: its first), `discover` (a location or achievement
  name, optional), `dialog` (a dialogue number for `!dialog`, or a name, optional), `message`
  (`text`), `other` (the comment says what). `once`: only each player's first crossing counts.
- `color`: how the survey server draws it, from the arena doors and forcefields: `red`, `green`,
  `shield`, `low shield`, `forcefield` (`GET /v1/survey/wall-looks`). The look is scaled to the
  height and repeated across the width; crossing is checked against the exact box.
- In survey mode crossing one shows what it would do: a script's wave pops in (as "Preview"),
  `dialog` with a number opens it, the others say in chat what happens.

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
- Attacking an object **selects** it instead of hurting it.
- The selected NPC shows an aura. A selected smashable or buildie gets a ground effect at its
  position, because simple dynels can't show effects.
- Guessed objects are named `[guess] <name>` and surveyed ones `[surveyed] <name>` (a custom NPC
  name, Flags stat 0x8000), so the category shows in game. Friendly NPCs get CARS group 3 so they
  can be attacked (selected), and everything shows its recorded facing.
- The selected character wears the champions' feet aura (effect 162435); a selected smashable
  gets a ground marker at its spot (222321), shown only to the surveyor who selected it.
- Hides are per surveyor and held in memory (`HIDDEN` in survey.rs: what isn't sent to them).
- Edits respawn the object under a new NPC id (the client needs ~1.5 s before an id is reused).
- Not yet: placing or wearing buildies and interactables (they aren't NPCs; the catalog lists
  enemies, NPCs and smashables), showing patrol routes in game, play servers using a set.

## API

lmoxide-dev-ui's world editor (`/editor/`, its `src/editor/backend.ts`) uses these
routes of `lmoxide-api` under `/v1/survey` (admin scope), passed to the survey server's control
interface. `{account}` is the surveyor: a player online on the survey server. Shapes are the
crate's (`Placement`, `Note`, `Script`, `merge::Report`, ...); `SurveyObject` is any spawned object
with its `category`.

| Method | Path | Body | Answer |
|---|---|---|---|
| GET | `/surveyors` | | players online: `account_id`, `username`, `character_name`, `playfield` |
| GET | `/catalog` | | what can be placed: `kind`, `name`, `monster_data`, `template`, `level`, `used` |
| GET | `/players/{account}/state` | | `PlayerState`: playfield, position, heading, selection, hidden ids, `shown_waves` (script id → wave id or `"all"`; a script that isn't there shows its first wave), what they wear, the playfield's `revision` |
| POST | `/players/{account}/wear` | `{what: Placeable \| null}` | `PlayerState` |
| POST | `/players/{account}/place` | `{what, wave: WaveRef \| null}` | the new `SurveyObject` (selected; its wave is shown) |
| POST | `/players/{account}/place-trigger` | `{name, size, color, action, once}` | the new trigger wall as a `SurveyObject` (`kind: "trigger"`, selected) |
| GET | `/wall-looks` | | `[{color}]`: the looks trigger walls can have |
| POST | `/players/{account}/select` | `{id}` or `{nearest: true}` | `PlayerState` |
| POST | `/players/{account}/hide` | `{ids, hidden}` | `PlayerState` |
| POST | `/players/{account}/unhide-all` | | `PlayerState` |
| POST | `/players/{account}/shown-wave` | `{script, wave: <wave id> \| "all"}` | `PlayerState` |
| POST | `/players/{account}/preview-wave` | `{wave}` | |
| POST | `/players/{account}/teleport` | `{pos}` | `PlayerState` |
| GET | `/playfields/{pf}/objects` | | `{revision, objects: SurveyObject[]}` |
| GET | `/playfields/{pf}/notes` | | `{notes, scripts}` |
| PUT / DELETE | `/playfields/{pf}/notes/{id}` | `Note` | `Note` |
| PUT / DELETE | `/playfields/{pf}/scripts/{id}` | `Script` | `Script` |
| PATCH | `/objects/{id}` | any of `pos`, `heading`, `level`, `name`, `comment`, `wave`, `patrol`, and `kind` + `monster_data` + `template` (change it into another spawn); trigger walls: `size`, `color`, `action`, `once` | the `SurveyObject` (a guess comes back surveyed, under a new id, still selected) |
| POST | `/objects/{id}/confirm` | | the surveyed `SurveyObject` |
| DELETE | `/objects/{id}` | | |
| GET / PUT | `/set` | `SetInfo` | `SetInfo` |
| GET | `/export[?playfield=]` | | a bundle: `{format, info, playfields: {"<pf>": {placements, notes, scripts}}}` |
| POST | `/import` | `{bundle, options: {distance, heading, prefer, take}, dry_run}` | `merge::Report` |

The page polls `state` every second and reloads objects and notes when `revision` or the playfield
changes, so changes made in game (a selection by attacking, someone else's edit) show up.

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

9. **Buildies and interactables**: placing them (ghosts to wear), editing and hiding them (they
   are buildie.rs / scenery.rs objects, not NPCs).
10. **Patrol routes in game**: markers at the selected patroller's points.
11. **Play servers use accepted sets** (trigger walls too): a reviewed set (e.g. `data/survey/<set>/`) applied over
    the base content at startup with `lmoxide_survey::apply`. Placements with a wave aren't
    spawned until their (reviewed, implemented) trigger.
12. **docker-compose** service for a survey server.

Open research, separate from the steps: whether the client lets GMs zoom freely (Linux symbols),
and the client's dimension list as a server picker (not needed now).
