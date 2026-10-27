# Survey mode

A server mode for recording where things were on live, in game, together. Nothing moves or attacks.
Surveyors place enemies, NPCs, smashables and buildies by standing where they belong. They confirm,
edit or delete the guessed content, and mark areas (missions, achievements, dialogues, scripts,
comments) for someone to review later. Each surveyor's work is saved in files that others can
import and merge.

Status: being planned. The `lmoxide-survey` crate (file formats, ids, merging, applying to the
existing content) is the first step; the steps after it are below.

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
- **Edit** a guess (move it, turn it, change its look or level): it becomes surveyed where you left it.
- **Delete** a guess: it is not spawned any more.
- **Hide** anything: it isn't rendered for you until you unhide it, or until you use "unhide all" in
  the web UI. Hiding is never saved. It is for objects in the way.

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
  name, Flags stat 0x8000), so the category shows in game.
- Hides are per surveyor and held in memory.

## Steps

1. **`lmoxide-survey` crate** (no server changes): formats, ids, load/save, merge and its report,
   and `apply` (base content + sets → what to spawn, with categories; what waves spawn). Unit tests.
2. **Survey mode in the server**: `--mode`, frozen AI, no damage, maxed survey characters, GM,
   the categories in names, select by attack, selection marker, hide/unhide. Wait for the
   live-capture import (other session: `world_content.json`, `extract_world_content.py`,
   `npc_fights.json`) to land first, since this changes `npc.rs` spawning.
3. **Placing**: `!skin` to wear enemy and NPC looks. A ghost object for buildies and
   interactables, which can't be worn. Place / confirm / edit / delete. Moving NPCs live;
   simple dynels respawn under a new id (the client needs about 1.5 s before an id can be reused).
   The server saves to the active set as it goes.
4. **Control routes, API and debug browser**: `/v1/survey/...` on the control interface, proxied
   by `lmoxide-api` (`openapi.json`). A **Placement** view in `lmoxide-debug-browser`: catalog,
   this playfield's objects by category, the selection editor (x/y/z/heading, nudge, snap to me,
   drop to navmesh), hide/unhide all. The server pushes in-game selections to the page as events.
5. **Notes, scripts and waves** in the web UI. Areas are walked out from the surveyor's positions.
   "Preview wave" spawns one wave.
6. **Import / merge UI** with the match distance and angle, and the preview described above.
7. **Play servers use accepted sets**: `data/survey/<set>/` is applied over the base content at
   startup. Placements with a wave aren't spawned until their (reviewed, implemented) trigger.
8. **Hosting**: `lan-server.sh --modes play,survey`, docker-compose services per mode.

Open research, separate from the steps: whether the client lets GMs zoom freely (Linux symbols),
and the client's dimension list as a server picker (not needed now).
