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
(a sound layer pf+1/pf+2 is read into its world; a playable pf+1/pf+2 such as 1632 Dragoon Dwelling
or 1643 Monkey Cave is a world of its own):

| Role | From | What the server does (triggers.rs) |
|---|---|---|
| `trigger` | RDB 1000053 server bounded areas not in content before, `category` by name (hazard, speed_or_jump, teleport, spawn_ambush, plate_puzzle, door_wall_shield, encounter, quest_story, hate_res, other), `scry` = their Scry script | what the triggers on them say |
| `kill` | `FallThroughWorld` (49 worlds) | trigger: back at the nearest respawn point; everything below the volume's top counts |
| `story` | cinematic, `Story_*` and talking-head areas (162) | story.rs: cinematics, story beats, talking heads, story goals (tools/content/import_story.py writes their `trigger/<pf>/story-*` triggers) |
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

**Registered commands.** `crate::triggers::register_command(name, Arc<dyn Fn(&Fired, &args)>)`:
a trigger's `{ "command": { "name": <name>, "args": {...} } }` then calls it with
`Fired { trigger, area, player, player_name, pf, pos }`. `cinematic` (`{ "cinematic": { "name" } }`)
and `dialogue` (`{ "dialogue": { "dialogue" } }`) actions call the commands registered under those
names with args `name` / `dialogue`. story.rs registers (each checked against the character, so
once per character also across logins):

| Action | Does |
|---|---|
| `{ "cinematic": { "name": <tunables story.cinematics key> } }` | plays that narrated cinematic if the player hasn't seen it and has its `requires` / none of its `unless` tags (one at a time; the first of several that may) |
| `{ "dialogue": { "dialogue": "dialogue/..." } }` | starts that recorded dialogue unless the player has every tag it gives |
| `command` `tag` `{ "tag": n }` | adds lore tag n (a pusher tag: the client shows the talking head) |
| `command` `story_goal` `{ "goal": <hash>, "requires": [tags] }` | resolves that story goal |
| `command` `story_beat` `{ "text": n, "requires": [tags] }` | runs dialogue text n's server side (tags, goals, spawns, teleports, script steps; no window) unless the player has its tags |

Test: `!areas`, `!wall open|close <name>`, `!goto area|wall|fallzone|speedpad|story [n]`,
`!cinematic`, `!story` (docs/DEVELOPMENT.md).

Not imported: the resources `0x40000000 | pf` (e.g. 4500's `BA_SpawnTentacle1/2`,
`BA_SpawnTikiBoss`, 2500's `BossEncounter_Start_Hakin/Rhys`, 6300's `BA_TikiRoom_Plate31-48`). No
code in the Linux client builds such an id (the constant appears only in library and debug-print
code), so the client never loads them; they look like editor or work-in-progress copies, and
whether live's server read them is unknown.

## Not implemented yet

What phases 2 and 3 brought in as data only carries the `todo` tag and a `todo` note
`<kind>: what is missing` on its area (`content/worlds/*/areas.json`; `todo` is a field of areas and
triggers). `extract_server_areas.py` writes them on every run, so they stay up to date: an area of
role `trigger`, `story` or `territory` with no trigger on it that no server module runs by name
(encounter.rs `BA_SpawnChampion`, plates.rs, pistons.rs, and the tutorial and Pirate World story
that tutorial.rs, cannon.rs and knubot.rs run), and the client walls live must have opened. A
trigger added on an area clears its mark at the next run. Find them with
`grep -l '"todo"' content/worlds/*/areas.json` or the world editor's tag filter.

| Kind | Areas | Worlds | What is known | What is missing |
|---|---|---|---|---|
| `hazard` | 90 | 12 (1410, 2210, 2320, 3210, 4210, 5210, 5310, 5500, 6200, 6300, 7200, 7202) | lava bricks / fireballs (4210; candidate spells 600554 / 600594 "lava summon ball / explosion buff", lava balls 1-7 m around the target), grills (5310 `GrillVolumne1-3`; 318445 / 463750 / 463751, a 144 s ground fire, nano 319122: likely the boss fight's), snowballs (1410; 579191 / 580992 "XX_Snowball"), tar, swamps, slow fields, spider jumps, cracked floors, falling bricks, traps, ritual poison (Scry 176480 `boundedarea_castspellonself`) | which spell each casts and when (the Scry parameters are lost); the snowball time trial's rules |
| `cannon_catapult` | 12 | 3 (4300, 5200, 5310) | `BA_CannonTraining*`, `BA_CannonDodge*`, `BA_Catapult*`, `BA_SiegeCatapult*`, `BA_MountainCatapult*` | what fires, where to and how often (cannon.rs runs only 4600's fort cannon, jumppad.rs the catapult pads) |
| `jump` | 15 | 3 (1690, 3210, 4200) | jump sound areas, `BA_FakeJumpieTrigger*`, `BA_JumpEntry*` | what they set off |
| `teleport` | 47 | 42 (most dungeons' `BA_TeleportEntrance` / `Exit`, 4310 `TeleportToSkull*`, 5200 `BA_Teleport_OutofThroneRoom1`, ...) | Scry 18602 `teleportplayerboundedarea_wazu` (or the name) | the destination (a script parameter, lost); only 4600 `BA_TeleportJail` runs |
| `res` | 95 | 23 | `Res_*` (Scry 174337 `boundedarea_discoverresurrectionpoint`) | nothing to do while every respawn point is known; whether live limited respawns to discovered ones |
| `setxp` | 2 | 1300 | `BA_FixXP1`, `BA_ResetXP1` (Scry 276794 `ba_setxp`) | the XP value (lost) |
| `escort` | 15 | 2 (1300, 4610) | the tutorial escort chain (`BA_Start_Escort1`, `BA_TriggerEscortStage*`, Scry 597467 `genericsendmessagetoself`), 4610 `BA_EscortEngageBoss1` / `BA_EscortGoToExit1` | the escort scripts' messages (tutorial.rs runs the tutorial escort without them) |
| `ambush` | 49 | 16 | spawn and ambush areas extract_world_content.py didn't make (`BA_VolcanoAmbush*`, `BA_HarpySpawn_*`, patrols, counters, ...) | which enemies, how many, which waves |
| `puzzle` | 32 | 8 (1399, 1675, 2310, 2320, 4300, 5300, 5500, 6300) | plates, statues, braziers, bone cages, whack-a-pirate, Tiki buildie triggers | the rules (plates.rs, mirrors.rs, tiki.rs run the Gauntlet's and Medusa's) |
| `door` | 20 | 9 | `BA_FirstBarrier*`, `BA_DoorCollision*`, `BA_PirateBayShield`, `BA_TutorialBossShield1`, room collisions | when they switch (the walls under them are client walls, or doors encounter.rs / gates.rs open) |
| `encounter` | 38 | 18 | boss and event helpers (`*_Outside`, `*_NoLogin`, `BossArea*`, `Res_Boss`) | what they did (leaving or logging in mid-fight) |
| `quest_counter` | 28 | 13 | player counters, GUI popups, `BA_AskCommoner*`, `Achievement_TimedRun(_End)` | what they counted or showed |
| `other` | 114 | 34 | names only (`BA_Alarm*`, `BA_SlowField*` siblings, `BA_CatchDCPlayers`, ...) | everything |
| `territory` | 4 | 2 (1300, 4500) | RDB 1000050 territories with roaming paths | NPCs don't wander by territories |
| `wall_opening` | 9 | 3 (4500, 5210, 6300) | client walls `kraken_blocker0` (Kraken bay), `Collision_Barrier1-4` (Enchanted Forest, by `BA_FirstBarrier1/2`), `Mirror_Collision_1-4` (Gauntlet mirror room) | what opened them on live and how the server tells the client (no RPC or IIR toggles collision) |
| `story` | 53 | 23 | story areas phase 3 left out: hand matches from story_triggers.json (the note names the tag, text or goal guessed), unmatched cinematic areas, the King's Court (5200 `BA_Cine_TheKingsCourt1`: tag 425, text 276 resolves CPIJ, then teleports out of the throne room), the Funhouse `BA_TalkingHead1` (8000, 8060: one of pushers 4725 / 4726 / 4728 / 4729; 8010's -> 4727 and 8020 / 8070 aren't playable worlds) | which beat, head or goal each is, and the throne room teleport's destination |

Story leftovers without an area of their own:

- dialogue 266 (`BecomeAPirate`, resolves "Complete the Pirate Trials", tag 312): nothing starts
  it (its NPC and trigger are unknown); the `becoming-a-pirate` cinematic (tunables) waits on it;
- texts 21000 (area hints such as "You need to join a pirate crew ...") and 50018 (scry FIFO test
  messages): not sent; the hints belong to portals whose conditions aren't known.

The quarry crystals (quest/medieval-story goal 3UX1, items 464315-464318) drop from enemies and
smashables defeated in the Quarry (`drops_in: area/5210/quest-quarry`) while the player is on that
step (2026-10-30: nothing dropped them before).

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
