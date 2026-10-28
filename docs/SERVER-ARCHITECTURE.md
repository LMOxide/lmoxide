# One server, several game modes (plan)

Status: proposal (2026-10-28), nothing built. Decisions still open are marked **Open**.

## Goal

Replace "one process per mode" (play server with `--progression`, world editor server with
`--mode survey`, each with its own ports, DB, patch server, API and dev UI port) with one server
that runs every mode at once:

- **Game modes**: `progression`, `maxed`, `world-editor` (today's survey mode), more later.
- **Instances**: several running copies of one world. Made automatically when a world is full,
  for every pocket adventure and Lost Creations run (one per team), and on request for previews.
- **Previews**: an editor plays their own unpublished content changes in a private instance.

The server therefore keeps two things apart:

| | comes from | decides |
|---|---|---|
| **Rules** | the game mode (per character) | XP/levels, ranks, drops, smash/respawn, whether NPCs move or attack, GM tools, editor tools |
| **Content** | a content version (published, or one editor's working copy) | what is in the world: spawns, NPCs, loot tables, quests, walls, scripts |

An instance is the combination: `Instance = map × rules × content version × who may enter`.

## Is it a good idea?

Yes, with two caveats.

For it:
- Accounts are already shared by copying (`--share-accounts`) and moving between servers already
  needs a redirect plus a chat proxy (`switch.rs`, `chat.rs pass_on_to`). One server removes both.
- Two of everything (ports 7100/7110, 8080/8081, API 8090/8091, dev UI 5174/5175, two DBs, two
  patcher profiles) goes away.
- The client supports instances natively. `PlayfieldProxyData` has a `model_id` (which map to
  load) and a separate `running_instance` (the key of the client's playfield map). Live sent a
  large running_instance (e.g. `0x02044b4b`) that was the key on every UpdateMessage in the zone
  (research/beta-capture-findings-2026-10-28.md); we currently send the playfield id for both.

Against it:
- **Blast radius.** A panic or deadlock in editor code would stop the game for everyone. Editor
  and preview code is the least tested. Mitigation: run each instance's work under
  `catch_unwind`/its own task so a failing instance is closed and its players are sent to a safe
  playfield, keep `keep-listening.py`, and keep the option to run a second process for the editor
  (same binary, same DB) if this turns out to be a problem.
- **Size of the refactor.** About 40 global statics hold per-world state keyed by the playfield
  id (`MEMBERS`, encounter/mirror/plate/piston state, buildies, projectiles, ...) and ~160
  functions take `playfield: u32`. Modes are process-wide switches (`progression::enabled()` 12
  call sites, `survey::enabled()` 9, `gamedata::get()` 46 for content). All of it is reachable,
  but most of the game was verified by hand in the client, so the change has to go in steps,
  each one runnable.

## Accounts and characters

Recommendation: **one account, characters belong to a mode.**

- One DB, one login. A `characters.mode` column (`progression` | `maxed` | `world-editor`).
- The character screen lists all of an account's characters; picking one picks its mode. A new
  character asks for a mode (**Open:** how — the creation screen has no mode picker. Options: a
  first dialogue/portal in the creation playfield, or the web UI's "new character" + the client
  just lists it, or a chat command).
- **Open:** how the client shows a character's mode in the list. `PlayerCharacterData_t` has a
  dimension field the GUI doesn't show. Fallbacks: a suffix in the (unused) middle/last name
  strings, or the level column (maxed characters show their real level anyway).
- Characters never move between modes (no carrying maxed items into progression).
- World-editor characters need an editor permission on the account (today every account is admin
  on the survey server; that has to become a real permission once it is public).
- Shared across modes: friends, chat, account bans/GM level. Teams only within one mode (a team
  shares one instance, which has one ruleset).

## Rules

`Rules` = a struct loaded from the content tree (`content/rules/<mode>.json`, so rules are
editable data like everything else), held by an `Arc` in each instance. Today's
`progression::enabled()` / `survey::enabled()` checks become `inst.rules.progression` etc. The
`--progression` and `--mode` flags go away.

## Content versions and previews

Layers, newest on top; each is a set of records overriding the one below by record id:

```
personal draft   (an editor's unpublished edits)      -> preview instances, only that editor
shared work      (var/content-work, today's editor)   -> world-editor instances
published        (content repo main / approved queue) -> progression + maxed instances
```

- A content version is an immutable `Arc<Content>`. Overlays share the base (copy-on-write by
  record id), so a preview costs only its changed records, not another 13k-record copy.
- Publishing makes a new published version. Running instances keep theirs until they empty
  (or reload on request, as today's "play-server pull/reload" plan says); new instances get the
  new one.
- Preview = "Play this" in the editor: the server creates a private instance with the editor's
  layer and the mode the editor picks (progression or maxed rules), and teleports them in. No
  more `!server` redirect.
- Only server content can be previewed. Anything the client reads from its own RDB (models,
  effects, map geometry) can't differ per instance.

## Instances

- Key: a server-chosen `running_instance` (u32). The model id stays the playfield id. All
  per-world state is keyed by the instance key; content lookups go through `inst.model`.
- Kinds: `open` (shared; a new copy when it has `n` players, **Open:** `n`, and whether friends
  /teams are steered into the same copy), `team` (pocket adventures, Lost Creations: one per
  team, made on entry), `private` (preview).
- Lifecycle: made on first entry, asleep when empty (already done per playfield: 10 s), dropped
  after a timeout (team instances keep state a few minutes so a disconnected player can come back).
- Dynel ids: NPC ids are made per playfield today, and `MEMBERS` is keyed by `Id32` globally, so
  two copies of a world would collide. NPC ids must be unique per server (or keys become
  `(instance, id)`).
- Persistence: characters save the model id + position, never the instance key. Login puts them
  into a fitting open instance.

## Performance

What matters, roughly in order:

1. **Lock layout.** Global `Mutex<HashMap>`s shared by every world are the first thing that
   stops scaling once there are many instances: every NPC tick in one world contends with every
   other world. Move per-world state into the instance (one task per instance owning its state,
   or at least one lock per instance). This also fixes the class of lock-order deadlocks found
   before (`npc::hit` re-locking `NPCS`, `world::join`).
2. **Only simulate occupied instances.** Already true for playfields; keep it per instance. An
   empty instance should cost memory only, and should be dropped after a while.
3. **Share everything immutable.** Content versions, navmeshes, world data, spell/item tables:
   one `Arc` per version, never per instance. Previews are overlays.
4. **Fan-out.** Each update goes to every player in the instance, so cost grows with players²
   per instance. The open-instance cap `n` is the main knob; a few dozen per world is safe.
5. **DB writes.** SQLite with WAL is fine for this scale if saves are batched (one transaction per
   save interval, not per change) and run off the game tasks.
6. **Startup and spawn time.** Spawning a world takes ~5 s in release today; instances made on
   entry must not block the teleport (spawn in the background, or keep a warm spare copy of busy
   worlds).
7. **Measure.** Add per-instance counters (tick time, players, NPCs, messages/s) to the control
   API so the dev UI can show them; decide `n` from numbers, not guesses.

## Hosting (e.g. lmoxide.dev)

Today: three processes (game, API, patch server), a control port + token between game and API,
and many shell scripts. Proposal:

- **One binary `lmoxide`**, one config file (`lmoxide.toml`), subcommands only for admin tasks.
  It serves the game ports (TCP: universe 7100, territory 7101, game 7102, chat 7200) and one HTTP
  listener for API + patch files + the static web UI/dev UI. The API calls the game in-process
  (no control port or token). `--only api|game|patch` remains for anyone who wants to split.
- **Caddy in front** for HTTPS (automatic certificates) on the HTTP side:
  `lmoxide.dev` (the Astro site, can stay on GitHub Pages), `api.lmoxide.dev`,
  `patch.lmoxide.dev` (manifest + client files: put a CDN in front if downloads get heavy),
  `play.lmoxide.dev` (web UI), `dev.lmoxide.dev` (dev UI). Or one host with path prefixes; fewer
  DNS records, same thing.
- **The game ports can't go through an HTTP proxy**: they are raw TCP and must be open on the host
  (or forwarded by a TCP proxy). **Open:** whether the client's `UniverseAddr` and the territory/
  game addresses we advertise may be host names (`lmoxide.dev:7100`) or must be IPs: needs a test.
- **Docker compose**: `lmoxide` + `caddy` (+ `discord-bot`). One volume for data.
- **Discord bot**: its own small service, API key auth as in DISCORD-BOT.md. Give the API an event
  stream (`GET /v1/events`, server-sent events: logins, achievements, broadcasts, reports) so the
  bot reacts instead of polling.
- Before going public: real editor permission (above), rate limits already exist; the content
  repo push token stays on the server only.

## What changes in the other repos

- **lmoxide-patcher**: one profile per host; drop `kind` / `other_servers` (PATCHING.md). Optional
  later: show the account's characters per mode.
- **lmoxide-dev-ui**: one API, one sign-in; debugger and editor are tools of the same server.
  Editor gets "Play this (preview)" and instance pickers; debugger lists instances and their
  counters and follows players across instances.
- **lmoxide-webui**: characters grouped by mode, instance list for admins, mode-aware stats.
- **Scripts**: `lan-server.sh`, `survey-server.sh`, `servers.sh` become one start command.

## Steps

Each step ends with a runnable server and the client verified.

0. **Spike (small):** send a running_instance ≠ playfield id, and teleport between two copies of
   one world. Decides whether instances work at all. Also test a host name as `UniverseAddr`.
1. **Instance key:** introduce `Instance { key, model, rules, content, kind }`; one instance per
   playfield to begin with (behaviour unchanged). Unique NPC ids.
2. **Rules per instance:** replace the mode switches; both rulesets in one process.
3. **One DB, characters per mode:** merge the survey DB's characters into the play DB with
   `mode = world-editor`; retire `--share-accounts`, `switch.rs` redirect, chat proxy.
4. **Content versions in memory:** published + shared work + overlays; world-editor instances
   use shared work; previews.
5. **More instances:** team instances for pocket adventures and Lost Creations; open-world cap.
6. **Per-instance state and locks** (the performance step; can start during 1–5 world by world).
7. **One binary/config/HTTP listener, Caddy, compose**; then patcher, dev UI, web UI.
