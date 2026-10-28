# Development

Working on the server: running it against a test client, the in-game test commands, and the
scripts in `tools/` and `patches/`. For setup, see the [README](../README.md).

- [Test runs](#test-runs)
- [Test commands](#test-commands)
- [Scripts](#scripts)
- [Reverse-engineering tools](#reverse-engineering-tools)

## Test runs

```sh
LMO_AUTOPLAY=1 tools/test-run.sh 60      # server + client, wait 60 s, print a summary, screenshot
tools/lan-server.sh --restart            # rebuild and restart the game server and API of a LAN run
```

`tools/test-run.sh` logs in as `tester` / `tester-password`, prints the RPCs, warnings and client
progress, and saves a screenshot of the game window to `logs/screen.png`. `LMO_RUNNER=wine|proton|linux`
picks how the client runs (`linux` = the native Linux build).

Scripted runs: `LMOXIDE_DEV_COMMANDS="!test crowd;!xp 10"` (server environment) types test
commands 5 s after the first entry into the world.

## Test commands

Typed into the normal chat in game (free text needs a GM level; `tools/lan-server.sh` sets 1).
A line starting with `!` (or `/`, if the client lets it through) goes to the server, not to other
players, in the local chat or any channel tab (e.g. Global); answers come back as local chat lines
from "Server" (`crates/lmoxide-server/src/devcmd.rs`).

| Command | Does |
|---|---|
| `!where` | current playfield and position |
| `!instance [salt]` | instance spike: re-enter this world under another playfield key (`salt << 24 \| playfield`; everyone in this world; 0 = the playfield id) |
| `!tp <playfield or name>` | go to a playfield's arrival point (`!tp 5200`, `!tp kingdom`) |
| `!list <kind>` | numbered test targets of a kind: `buildie`, `pad`, `lever`, `plate`, `gate`, `mirror`, `collectible`, `boombox`, `portal`, `trigger`, `arena`, `ambush`, `explore`, `boss`, `champion`, `enemy`, `npc`, `fighter`, `patrol`, `dialogue`, `smashable`, `script`, `quest` (area quests' areas) |
| `!goto <kind> [n]` | go next to target n (default 1), e.g. `!goto buildie 2` |
| `!dialog [n]` | play recorded NPC dialogue n (without n: list them) |
| `!script [list]` | this playfield's approved, active scripts (world editor) with their waves, triggers, placements and what is running (`!goto script n` goes to one) |
| `!script start <script> [wave]` | start a script (name, part of it, id or number from the list) at a wave (name, id or number; default its first), as a trigger wall does; while it runs, naming a waiting wave starts that wave (`!script start ufo 2`) |
| `!script stop [script]` | end this playfield's running scripts (or that one); their spawns go |
| `!window <id>` | open teleport window `<id>` (RDB 1000644) as a portal would; 73 and 74 ask "Do you want to travel to …?" like their portals (`!goto portal n` lands next to a portal: walk 8 m away, then step in) |
| `!aura <0-4>` | set the active minifigure's aura tier |
| `!plates <solve\|reset>` | light every plate of the Gauntlet's plate room (`!test plates`), or none |
| `!mirrors <solve\|reset>` | turn this playfield's Medusa mirrors (Armory, Gauntlet) to the solution or back to the start (`!test mirrors`, `!test mirrors2` go there) |
| `!team <name>` | team up with that online character at once (you lead), without the invite window: for testing what teams share (kills, smashes, builds, clears) |
| `!revive` | bring every minifigure (team and collection) back from smashed, at full health |
| `!test [n\|label]` | list the manual test setups, or set one up: go somewhere, switch the team, spawn enemies around you for 15 minutes (`data/test_scenarios.json`; add your own there) |
| `!spawn [count] <name>` | spawn count (default 1, at most 30) of an enemy or smashable around you for 15 minutes, as `!test` does (`!spawn 3 Crate`, `!spawn Captain Dreadleg`) |
| `!despawn [all]` | remove the test spawns (`!spawn`, `!test`) within 30 m, or all of this playfield's |
| `!fx <id> [hook] [@near\|@ground]` | play effect package `<id>` on yourself, from a code hook (`onhit`, `charge`, `execute`, ...), with the nearest enemy or smashable as its target (`@near`) or at your feet (`@ground`); `!fx next\|prev\|stop` steps through every package or ends it (data/effect_catalog.json) |
| `!anim <attribute>` | play an animation attribute (`limbostart`, `chainattack`, `get_hit`, ...: data/animations.json picks a package that sets it) |
| `!skin <monster data\|reset>` | look like any monster data (data/monster_data.json), or your minifigure again |
| `!state <n>` | set your character state (stat 20: 1 Idle, 4 Fighting, 7 Flying, 29 Limbo, 69 Evade, 104 Building; data/bcc_states.json) |
| `!aim [shots\|lobs] [current\|lead\|homing]` | how players' straight shots and lobbed shots (both without `shots`/`lobs`) are aimed at the enemy they target, for everyone: `current` (where it is at the shot) or `lead` (where it will be when the shot gets there); `homing` isn't implemented. Start values: `--shot-aim` (current), `--lob-aim` (lead). Without a mode: the current ones |
| `!bounce [bounce\|spin\|<knockback 0/1> <launched 0/1> <direction>]` | throw yourself 6 m along +x as a bouncer (default) or a cannon/piston does, or with raw animation settings |
| `!smash` | smash the nearest smashable (or defeat the nearest enemy) within 15 m |
| `!sleep <secs>` | in `LMOXIDE_DEV_COMMANDS` only: wait before the next command |
| `!ach` | achievements earned, kill and smash counters, most defeated enemies, playfields visited, finds |
| `!quests [reset]` | area quests started, their progress and cooldowns; reset: clear them so they start again |
| `!unach <id ...>` / `!unach all` | take achievements away to earn them again (the "all of" ones above go too; `all` also clears the journal and counters); story and tutorial progress stays |
| `!count <kills\|smashes\|builds> [n] [name]` | count n defeats, smashes or builds for achievements (e.g. `!count kills 1000`, `!count kills 50 Witch`) |
| `!xp [n]` | gain n XP (with `--progression`) and show level and XP |
| `!stars <n>` | gain n stars (to rank minifigures up with `--progression`) |
| `!defeat <boss name>` | count a boss defeat (`!defeat Hades`) for the boss and the active minifigure's achievements |
| `!lc <level>` | complete Lost Creations level `<level>` with the active minifigure (its "Lost Creations_Get to Level 5/10/15" achievements; a stub until the Lost Creations run) |
| `!cleared` | count this playfield as cleared, as when its last enemy falls (pocket adventure achievements, also the active minifigure's "Conquer the X as Y") |

New features add their targets to `devcmd.rs` `targets`, and new commands to `COMMANDS` (`!help`,
and `GET /v1/catalog/test-commands` for tools such as the effect browser).

Through the API, `POST /v1/players/{id}/command` runs a command as an online player and answers
with the chat lines it produced (`tools/dev-command.sh <account> '<cmd>'` from a shell).

## Scripts

### Servers and clients

| Command | What it does |
|---|---|
| `tools/servers.sh [up\|restart\|down\|status\|open\|logs] [play\|editor\|both] [--debug] [--no-open]` | starts `lan-server.sh` (play, progression) and/or `survey-server.sh` (world editor) in the background (own process group, outlives the terminal; output in `logs/servers-*.log`), waits until they answer and opens the debugger (`:5174/debugger/`) and world editor (`:5174/editor/`, `:5175` without the play server) in the browser; release build unless `--debug`. Servers already running are left alone; `down` stops only the ones it started (SIGINT, as Ctrl-C: characters are saved); `restart` is the scripts' `--restart` |
| `tools/lan-server.sh [--release] [lan-ip]` | game server + patch server (Windows and, from `clients/downloads/LMO_Linux_v1.5.557666.zip`, Linux installs) + REST API on :8090 for LAN play (binds 0.0.0.0, advertises the LAN IP; `--auto-register`, GM level 1); logs in `logs/lan-*.log`. Also the dev servers of `../lmoxide-webui` (:5173) and `../lmoxide-dev-ui` (:5174, debugger and world editor) on all interfaces, their `/api` proxied to the API (`LMOXIDE_WEB_APPS=` picks them). `--restart [--release]` rebuilds and restarts the game server and the API |
| `tools/lan-party.sh [--reset] [lan-ip]` | `lan-server.sh --release` with open registration + the web UI (`../lmoxide-webui` built, served on :4173); accounts `host`, `player1`..`player8` with password `password` and GM level 2 (admin); `--reset` deletes every character but host's |
| `tools/test-run.sh [secs]` | restart the servers + client, wait, print a summary, screenshot (see [Test runs](#test-runs)); `LMO_AUTOPLAY=1` also presses "start" via Lua |
| `tools/dev-run.sh <client-dir> <log-dir>` | restart game server, patch server and client (patched with the login key by `patches/patch-client.py`) and leave them running; `LMO_RUNNER=wine\|proton\|linux` picks how the client runs (`linux` = native build, `<client-dir>` is then the Linux client) |
| `tools/web-dev-backend.sh` | a local API with demo accounts and bot keys, for web UI and bot development |
| `tools/keep-listening.py --ports P,.. -- CMD` | keeps the game server's ports listening across restarts (inherited by lmoxide-server via `LMOXIDE_LISTEN_FDS`) and starts the server again when it exits: a client pressing "relocate? Yes" during a restart waits instead of failing. `lan-server.sh` uses it; `LMOXIDE_KEEP_LISTENING=1` for dev-run |
| `tools/tour.sh <kind> [secs] [dir]` | screenshot tour: the server (`LMOXIDE_DEV_TOUR`) teleports the test character to each target of a test-command kind (`boss`, `buildie`, ...) and the script screenshots each one into `logs/tour-<kind>/` with `labels.txt` |
| `tools/skin-shots.sh [dir] [filter]` | preview pictures of every look (`!skin`, data/monster_data.json): the Linux test client (tester) joins the running LAN server, goes to PvP - Builder Wars (1100), zooms in once, faces the camera, then wears each look in turn and saves a square crop to `../var/skin-shots/<id>-<name>.png` with `index.tsv` (about 50 minutes for all 672) |
| `tools/client-mouse.sh click\|scroll\|drag\|info ...` | click/scroll/drag at window-local coordinates in the LMO client (or any window via `LMO_WINDOW=class:RE\|title:RE`) on a hidden workspace, without moving the real cursor or focus, through a Hyprland plugin it builds and loads on demand (`tools/hypr-target-pointer/`) |
| `tools/proton-run.sh /abs/LMO.exe [args]` | run the Windows client under Steam's Proton without Steam (prefix `~/.local/share/lmo-proton`) |
| `cargo run -p lmoxide-net --example login_probe -- host:7100 user pass key` | log in like the client, without starting the game |

### Client installs

| Command | What it does |
|---|---|
| `lmoxide-patcher install --server http://host:8080 --dir d` | install or update a client from a patch server (signed manifests; `verify`, `info`): [PATCHING.md](PATCHING.md) |
| `patches/linux-client/install.sh [--server ip] [--dir d]` | install the native Linux client (download, verify, extract, apply the kit); `~/Games/LMO/lmo <username>` starts it. See [patches/linux-client/README.md](../patches/linux-client/README.md) |
| `patches/linux-client/apply.sh <dir> [--server ip]` | the client patch kit: apply (again) to an extracted client |
| `patches/linux-client/make-kit.sh` | pack the kit (prebuilt libraries) as `dist/lmo-client-kit-v<N>.tar.gz` for another PC |
| `patches/linux-client/play.sh <dir> <server-ip> <username>` | start a kit-installed client from another folder |

## Reverse-engineering tools

Setup for the Python tools: `python3 -m venv .venv && .venv/bin/pip install pefile capstone protobuf`.
They read `../clients/windows/LEGO Minifigures Online/LMO.exe` by default, or `$LMO_EXE`. Ghidra comes
from `/opt/ghidra` (Arch `ghidra` package) or `$GHIDRA_HOME`. See also [PROTOCOL.md](PROTOCOL.md)
"Tooling".

### Lookups

| Command | What it does |
|---|---|
| `tools/re/py method.py <name\|0xID\|Iface::>` | RPC lookup: ID, signature, client dispatch/sender function, argument read order |
| `tools/re/py xref.py <regex>` | search exe strings, with referencing functions |
| `tools/re/py d.py <va> [bytes]` | disassemble with string annotations |
| `tools/re/py gameactions.py SPELL...` | a spell's game action lists |
| `tools/ghidra/analyze.sh` | one-time Ghidra import + analysis into `../research/ghidra/` |
| `tools/ghidra/decompile.sh <va...>` | decompiled C of the functions containing each address |
| `tools/ghidra/xrefs.sh <va...>` | references to each address (code and data, one level through vtables) |
| `tools/ghidra/symbols.sh <regex> [n]` | symbols matching a regex; for vtables, the first n slots |

### Regenerating `data/` and `proto/`

Extractors that write to `data/archive/` feed only `lmoxide-content import` (which rebuilds the whole
content tree): the game reads `content/` (docs/CONTENT.md), so new findings go into the content tree.

| Command | What it does |
|---|---|
| `tools/re/py extract_rpc.py <out.json>` | `data/rpc_methods.json` |
| `tools/re/py extract_protos.py <exe> <dir>` | `proto/` |
| `tools/re/py extract_iir.py <out.json>` | `data/iir_types.json` (IIR names, type ids, classes, protobufs) |
| `tools/re/py extract_progression.py` | `data/archive/progression.json` (minifigure rank costs, rank level requirements) |
| `tools/re/py extract_achievements.py` | `data/archive/achievements.json` (the lore tree's achievements, with inferred rules) |
| `tools/re/py extract_encounters.py` | `data/archive/encounters.json` (boss arenas, ambushes, champion spots from the bounded areas, RDB 1000053 of the Linux client) |
| `tools/re/py extract_spell_actions.py` | `data/archive/spell_actions.json` (action lists of the channelled abilities and the nanos they cast) |
| `tools/re/py extract_return_points.py` | `data/archive/return_points.json` (arrival next to the portal back to where the player came from) |
| `tools/re/py extract_effect_catalog.py [--no-refs]` | `data/effect_catalog.json`, `data/effect_package_ids.json` (effect packages: timelines, code hooks, particles and sounds with RDB ids; `used_by` takes ~10 min) |
| `tools/re/py extract_animations.py` | `data/animations.json` (motion sets, anim graphs, animation attributes and the package `!anim` plays; after the effect catalog) |
| `tools/re/py extract_monster_data.py` | `data/monster_data.json` (every look, RDB 1010514 binary XML: `lmxb.py`) |
| `tools/re/py extract_bcc_states.py` | `data/bcc_states.json` (character states and substates from the client's behaviour tree) |
| `tools/re/py extract_playfields.py` | `data/archive/playfields.json` (every playfield's name) |
| `tools/re/py rdb_names.py TYPE [NAME]` | the client's resource name index (RDB 1020004): names of sounds, particles, ... |
