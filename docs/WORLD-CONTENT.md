# World content: from the world editor to play servers

What world editors make (spawns, bouncers, trigger walls, scripts and their waves, notes, smash
rules, and any other record on the editor's Definitions pages) is content records
(docs/CONTENT.md), in the world editor's copy of the content repository. It reaches play servers
through the content repository's `main`, after an admin approved it.

```
world editor (survey server, docs/SURVEY.md)
  var/content-work/            a git checkout of LMOxide/lmoxide-content: every editor's work,
                               saved as they make it (unpublished: differs from its last commit)
        │ Publish (the editor's header)
        ▼
  var/content-queue/<seq>.json one entry per publish: the records it changes, before and after
        │ approve (web UI, admin)            ── not built yet: see "Approval" below
        ▼
  LMOxide/lmoxide-content main  a commit per approved entry, authored as the editor, with
                               Content-Author:, Approved-By:, Lmoxide-Change: (content/README.md)
        │ git pull
        ▼
play servers (content/ or --content-dir): run it from their next start
```

## On play servers

A play server runs its content tree as loaded at start (`--content-dir`, default the checkout's
`content/`), the world editor's records like every other: spawns (surveyed ones too; spawns of a
script wave wait for their wave, scripts.rs), bouncers, trigger walls (walls.rs: invisible,
crossing one does what it says), trigger areas and walls (triggers.rs, below), scripts
(scripts.rs, `!script list|start|stop`), smash rules (achievements.rs). Features: `live` and what is nested under the server's active features
(lmoxide-content `Options::features`).

`--content-queue <dir>` (tools/lan-server.sh passes `../var/content-queue`, the survey server's)
lets admins see and decide on what editors published, on the control interface and through
lmoxide-api (admin):

| Route | |
|---|---|
| `GET /v1/content/state` | `content` (the tree it runs), `queue` (the folder), `queued` (how many wait) |
| `GET /v1/content/queue` | every entry, oldest first: `seq`, `created`, `by` (who published), `summary`, `status` (`queued`, `rejected`), `reason`, `records: [{ id, feature, change, file, author, before, after }]` |
| `GET /v1/content/queue/{seq}` | one entry |
| `POST /v1/content/reject` | `{ seq, reason }`: the entry stays, rejected, with the reason |
| `POST /v1/content/approve` | `{ seq }`: 501 for now (approval commits; below) |

The survey server lists and withdraws entries too (`GET /v1/editor/queue`,
`DELETE /v1/editor/queue/{seq}`), and leaves queued records out of what it calls unpublished.

## Walls and trigger areas

`tools/re/extract_server_areas.py` (rerunnable; it owns only the records whose `comment` names it
and that no editor touched) brings the client's areas into `areas.json` of every playable world
(a sound or quest layer pf+1/pf+2 is read into its world):

| Role | From | What the server does (triggers.rs) |
|---|---|---|
| `trigger` | RDB 1000053 server bounded areas not in content before, `category` by name (hazard, speed_or_jump, teleport, spawn_ambush, plate_puzzle, door_wall_shield, encounter, quest_story, hate_res, other), `scry` = their Scry script | what the triggers on them say |
| `kill` | `FallThroughWorld` (49 worlds) | trigger: back at the nearest respawn point; everything below the volume's top counts |
| `story` | cinematic, `Story_*` and talking-head areas (155) | nothing yet: phase 3 (story) hangs its actions on them |
| `wall` | 1000053 `boundedareas_collision` (`blocks.by` server) and the client's own invisible walls, RDB 1040010 Collision layers of the world's resource (`by` client) | NPC paths stop at walls that block pathing; players crossing a server wall that blocks movement hop back; `set_state` opens/closes one per copy |
| `territory` | RDB 1000050 (1300, 4500) with their roaming `paths` | data only |
| `place` | `Area_*` regions | data only |

The triggers it writes (`triggers.json`, `area` = the area): `FallThroughWorld` → `teleport`
`{ "command": "nearest_respawn" }`; `BA_Speedbuff_*` → `apply_buff` `buff/speed-pad`, `requires`
the pad's speed buildie built (it adds a `proposed` speed buildie spawn where the live capture had
none); `ClearHate_*` → `command` `clear_hate` (`for` 5 s). Guesses: docs/TUNABLES.md "Walls and
trigger areas".

A trigger runs here when it has an `area` or a polygon `shape` (walls.rs keeps the world editor's
one-action walls and radii): every 0.2 s, entering (`on: enter`) or leaving (`exit`), if `requires`
holds and `once` allows. Actions run: `teleport` (`pos`, `point` = a named area or marker of the
target world, `world` alone = its arrival, `command` `nearest_respawn`), `apply_buff`,
`remove_buff`, `heal`, `message`, `effect`, `start_script`, `end_script`, `open_window`,
`discover`, `set_state` (`placement` a wall area, `open` / `closed`), `command`. Others are logged.

**For story actions (phase 3).** `crate::triggers::register_command(name, Arc<dyn Fn(&Fired,
&args)>)`: a trigger's `{ "command": { "name": <name>, "args": {...} } }` then calls it with
`Fired { trigger, area, player, player_name, pf, pos }`. `cinematic` (`{ "cinematic": { "name" } }`)
and `dialogue` (`{ "dialogue": { "dialogue" } }`) actions call the commands registered under those
names with args `name` / `dialogue`; until then they are logged. So a story beat is a trigger
record on a `story` area (`once: per_player` usually) with those actions; nothing in triggers.rs
needs changing.

Test: `!areas`, `!wall open|close <name>`, `!goto area|wall|fallzone|speedpad [n]`
(docs/DEVELOPMENT.md).

Not imported: the resources `0x40000000 | pf` (e.g. 4500's `BA_SpawnTentacle1/2`,
`BA_SpawnTikiBoss`, 2500's `BossEncounter_Start_Hakin/Rhys`, 6300's `BA_TikiRoom_Plate31-48`). No
code in the Linux client builds such an id (the constant appears only in library and debug-print
code), so the client never loads them; they look like editor or work-in-progress copies, and
whether live's server read them is unknown.

## Approval (not built yet)

Decided (docs/WORLD-EDITOR-CONTENT.md): approving an entry commits exactly its records to the
content repository's `main` (the server holds the token, e.g. `LMOXIDE_CONTENT_GIT_TOKEN`; never
the web UI), with the trailers above, pushes, and play servers pull and spawn again what they
can (spawns, walls, scripts, bouncers), the rest on restart; undo is `git revert` of that commit.
Pull requests merged on GitHub arrive the same way. Before approving, an admin can **preview** an
entry: the play server applies its records in memory for a while (15 minutes), then drops them.
On hold at the user's request (they may change the approach), so approve answers 501 and preview
isn't there yet.

## Before content records

Until 2026-10-28 editors worked on a survey set (`var/survey-set`) that play servers approved
into their own copy (`<data-dir>/world-content/`: a ledger of changes, rules, a rejected list).
That is gone: the survey server took its set into the working copy once
(`var/survey-set/migrated.json` says what it did), and play servers no longer read
`world-content/` (it can be deleted).
