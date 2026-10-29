# One server, several game modes (plan)

Status: built (2026-10-28), steps below. Decisions still open are marked **Open**.

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

Decided (2026-10-28): **one account, one character set per mode; the patcher picks the mode.**

- One DB, one login. A `characters.mode` column (`progression` | `maxed` | `world-editor`). The
  character screen lists only the characters of the mode the client connected to; a new
  character is made in that mode.
- **How the mode reaches the server: one universe port per mode, one process.** The client only
  knows `UniverseAddr` and its login screen has no mode choice, so the patcher writes a different
  port per mode (e.g. progression `:7100`, maxed `:7103`, world-editor `:7104`). The server
  listens on all of them; the universe port a client logs in through sets its session's mode,
  carried by the login cookie. Territory, game and chat ports stay shared. Launching the client
  without the patcher keeps the last mode written.
- Characters never move between modes (no carrying maxed items into progression).
- World-editor characters need an editor permission on the account (today every account is admin
  on the survey server; that has to become a real permission once it is public). The universe
  agent refuses the login on that port without it.
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
- Dynel ids: NPCs, smashables, loot bags and projectiles already get server-wide unique ids (one
  counter each). Fixed-id objects (portal doors = their guid, buildies `0x00b0_0000 + n`, gates,
  mirror beams, scenery, jump pads) exist once, in the main copy; spawning them in other copies
  needs ids per copy (step 5).

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
  (or forwarded by a TCP proxy). Host names work for all of them (live used
  `um.lmo_live.funcom.com:7100` etc.; see step 0).
- **Docker compose**: `lmoxide` + `caddy` (+ `discord-bot`). One volume for data.
- **Discord bot**: its own small service, API key auth as in DISCORD-BOT.md. Give the API an event
  stream (`GET /v1/events`, server-sent events: logins, achievements, broadcasts, reports) so the
  bot reacts instead of polling.
- Before going public: real editor permission (above), rate limits already exist; the content
  repo push token stays on the server only.

## What changes in the other repos

- **lmoxide-patcher**: one profile per host and a mode picker next to Play. `/lmoxide/v1/info`
  replaces `kind` / `other_servers` (PATCHING.md) with `modes: [{id, name, description,
  universe_addr, requires}]`; the patcher remembers the choice per profile.
  **Decided (2026-10-28): a `LocalConfig.xml` per mode, each pointing at its own control path.**
  The client's built-in patcher fetches `<ControlHttpPatchAddr>/lmo_live/LocalConfig.xml`
  (stock.rs serves `/control/{folder}/LocalConfig.xml`), and that address is inside the file. So
  the patch server serves `/control/mode/<id>/lmo_live/LocalConfig.xml` with that mode's
  `UniverseAddr` and `ControlHttpPatchAddr` = `<host>/control/mode/<id>`: once a mode's file is
  installed, the built-in patcher keeps fetching that same file and never reverts it. The signed
  manifest lists each mode's file (`modes: {id: LocalConfig blob}`), and lmoxide-patcher installs
  the chosen one, so it stays verified. Rejected: taking `LocalConfig.xml` out of the manifest. It
  loses verification and doesn't help, because the built-in patcher would still put the default
  mode's file back from `/control/lmo_live/LocalConfig.xml`.

- **lmoxide-dev-ui**: one API, one sign-in; debugger and editor are tools of the same server.
  Editor gets "Play this (preview)" and instance pickers; debugger lists instances and their
  counters and follows players across instances.
- **lmoxide-webui**: characters grouped by mode, instance list for admins, mode-aware stats.
- **Scripts**: `lan-server.sh`, `survey-server.sh`, `servers.sh` become one start command.

## Steps

Each step ends with a runnable server and the client verified.

0. **Spike: done 2026-10-28, instances work.** `teleport::client_key` + `!instance <salt>` (per playfield,
   key = `salt << 24 | playfield`). Linux client, Isle of Yarr 4200:
   - key ≠ model id: the world, NPCs, crates/barrels, smashing, loot and star pickup all work under
     `0x02001068`. The client log shows `Model=[51100:4200] R=33558632`.
   - Re-entering the same world under a new key (0 → 2 → 3): the client destroys the playfield and
     builds it again, cleanly. Leaving (1200) and coming back, and logging in straight into a keyed
     world, also work. No assertions.
   - Live did the same: the 2016 capture enters 4200 three times with three keys (23135290,
     23135571, 23135566), while open worlds keep one key per server (1200: 23134633). Its
     PlayfieldFIIR_t record id was `[0, 0]` (we send `[51100, playfield]`; harmless so far).
   - Host names: Funcom's stock `LocalConfig.xml` has `UniverseAddr` `um.lmo_live.funcom.com:7100`,
     and live's LoginConnectReady / ChatServerConnectReady carried `gs007-nj4.lego.funcom.com:7102`
     and `chat.lmo_live.funcom.com:7001`, so names work everywhere (the client uses getaddrinfo).
   - Found on the way: the client's own in-game patcher puts `LocalConfig.xml` back from the
     manifest at launch (a hand-edited UniverseAddr was reverted). Choosing a mode by address
     therefore needs the patch server to serve a per-mode LocalConfig, or the file has to leave the
     manifest. See the lmoxide-patcher bullet above.
1. **Copies of worlds: done 2026-10-28.** `instance.rs`: `Pf` = one running copy, key
   `copy << 16 | model` (copy 0 = the playfield id, so nothing changed for players). No `Display`,
   so `pf.to_string()` can't silently miss game data for a copy; data is looked up by
   `pf.model()`. The rule used throughout: where something is (members, NPCs, loot, projectiles,
   encounters, scripts, buildie state) is a `Pf`; game data and destinations (`teleport::
   destination`, spawns, portals, dialogues, achievements' events, the saved character) are
   playfield ids. The session keeps `place: Pf` beside the saved `me.playfield`.
   `instance::arrival(model)` picks the copy a player arriving in a world joins (always the main
   copy for now; LoginConnectReady and the game connection both use it), and `instance::towards`
   keeps a player in their copy within the same world. `!instance [n]` shows or changes the copy;
   `!where` names it. Verified with the Linux client in 4200: copy 1 has its own state (empty of
   residents, its own test crates, smashing, loot), copy 0 is unchanged, travel and login land in
   the main copy. Per copy already: arenas, ambushes, champions, scripts, the tutorial's spawns.
   Still main-copy only (step 5): residents spawn only in the main copy; fixtures (portal doors,
   scenery, jump pads, buildies, gates, mirror/plate/piston/ballista/tiki/cannon puzzles) and so
   the arena doors' looks. The world editor works on main copies.

2. **Rules per mode: done (c01a3f0).** `instance::Mode` (progression, maxed, world-editor) and
   `Rules` (progression, editor, min GM level); characters carry their mode (`characters.mode`,
   migration 0009); each mode has a main copy of every world (copy 0/1/2), extras from 3. Copies
   spawn when their first player arrives (worlds.rs): residents and fixtures, puzzles and arena
   doors with state per copy; fixture ids carry the copy in their top byte (`instance::fixture`).
3. **One server, characters per mode: done (c3911ea).** `--modes progression:7100,maxed:7103,
   world-editor:7104`: the universe port sets the login's mode (`game_sessions.mode`, 0010);
   territory lists that mode's characters; the world editor needs GM 1. The survey code runs
   beside the play content (`--editor-content-dir`, its own pad table). switch.rs, the chat proxy,
   `--share-accounts` and the peer flags are gone; `import-characters` brought the old editor
   server's characters over. The patch server's `--modes`, the per-mode `LocalConfig.xml`
   (decided above) and lmoxide-patch's `for_mode` / `Options::mode` / `.lmoxide/mode` too.
4. **Previews: done (a59b1f2).** `!preview` loads the editor's working copy (leniently: records
   with errors are left out and counted) into a private extra copy with the maxed rules: its
   residents, trigger walls, scripts and pads; definitions stay published. Not done: a personal
   draft layer per editor (the working copy is shared, autosaved), definitions in previews.
5. **More copies: done (3e9bf13).** `instance::arrival`: back to the copy left in the last 3 min,
   a teammate's copy, a new copy per pocket world (team), open worlds up to `--open-world-cap`
   (40). Empty extra copies are dropped after 3 min (worlds.rs).
6. **Performance: done as far as measured (747d8fd).** Members indexed per copy (broadcasts no
   longer scan every member); `GET /v1/instances` (players, NPCs per copy, AI tick time). NPCS is
   still one map scanned per tick (fine at LAN scale: split it per copy if the tick time grows).
7. **One binary: done (858d3d4).** `lmoxide` runs the game server, API and patch server from one
   TOML file (docker/lmoxide.example.toml) with one HTTP listener; docker-compose = lmoxide +
   Caddy. Then the patcher's Game Mode dropdown (lmoxide-patcher 1e3b466), the web UI (modes,
   Instances page; lmoxide-webui 3cbd1f3) and the dev UI (one API, previews).
