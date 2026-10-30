<p align="center"><img src="assets/logo/lmoxide-128.png" alt="LMOxide logo" width="96"></p>

# LMOxide

A server for **LEGO Minifigures Online**, written in Rust. Funcom shut the game down in 2016,
and the client can't be played without a server. LMOxide replaces that server, so the last
released client (1.5.557666) works again on Windows and Linux. Everything was reverse-engineered
from the client.

![In game on an LMOxide server](docs/img/first-inplay.png)

<!-- Feature screenshots go here. -->

## Status

These work: logging in with real accounts, the starting worlds and every dungeon, enemies and
combat, the minifigure collection and team, loot, achievements, friends, teams, chat and
multiplayer. Values that the client data doesn't contain are guessed and listed in
[docs/TUNABLES.md](docs/TUNABLES.md).

Characters level up from level 1 as on live (progression); the Maxxed mode has everything
unlocked. [docs/GAME-OPTIONS.md](docs/GAME-OPTIONS.md) has the game modes and the options for
aiming and story windows.

## How it works

```
 game client ──TCP 7100-7102, 7200──▶ lmoxide-server ◀──control 7300 (private)── lmoxide-api ◀── web UI, bots
      │                                     │  SQLite (lmoxide.db)    ───────────┘   HTTPS 8090
      └──HTTP 8080──▶ lmoxide-patch-server ◀┘  login-public-key.txt
 LMOxide patcher ──────────┘ (signed manifests)
```

- **`lmoxide-server`** is the game. It runs the universe, territory, game and chat servers that
  the client connects to, speaks the client's binary RPC protocol, and stores accounts and
  characters in SQLite.
- **`lmoxide-patch-server`** serves what the client needs before it connects: its configuration,
  the login page, the chat filter, and complete client installs already patched for this server.
- **`lmoxide-patcher`** installs and updates a client from a patch server. A
  [GUI patcher](https://github.com/LMOxide/lmoxide-patcher) is built on the same library.
- **`lmoxide-api`** is a REST API for administration and status. The web UI and the Discord bot
  use it ([lmoxide-webui](https://github.com/LMOxide/lmoxide-webui)).

LMO runs on Funcom's Dreamworld/N3 engine (the Anarchy Online and Age of Conan lineage), not on
LEGO Universe's, so LEGO Universe server projects don't apply.

Passwords are encrypted with the client's Diffie-Hellman/TEA scheme. They use this server's own
login key, which the patch server writes into the clients it serves. Game traffic can't be
encrypted because the client doesn't support it. See [docs/SECURITY.md](docs/SECURITY.md).

<details>
<summary>Repository layout</summary>

| Path | |
|---|---|
| `crates/lmoxide-server` | the game server: universe agent, territory manager, game and chat servers, control interface; admin subcommands (`account ...`, `login-key`) |
| `crates/lmoxide-api` | REST API for administration and status |
| `crates/lmoxide-patch-server` | client configuration, patching, login page, chat filter, LMOxide patcher protocol |
| `crates/lmoxide-patch`, `crates/lmoxide-patcher` | patcher library (for a GUI) and command-line patcher |
| `crates/lmoxide-db` | storage (SQLite): accounts, characters, friendships, API keys, audit log |
| `crates/lmoxide-control` | the control interface between the API and the game server |
| `crates/lmoxide-net` | the client's wire protocol and login encryption |
| `patches/` | client patches: `patch-client.py`, the native Linux client kit |
| `data/`, `proto/` | game data and protobuf schemas extracted from the client |
| `tools/` | development and reverse-engineering scripts |
| `docs/` | documentation (below) |

</details>

<details>
<summary>About <code>data/</code> and <code>proto/</code></summary>

These were extracted from the client: game data such as spawns, loot and stats, and the
protobuf schemas and RPC method signatures from `LMO.exe`. The server is built from them, so
they are kept in the repository. They are included for interoperability with a client that can't
be played without a server. The `tools/re/extract_*.py` scripts regenerate them from a client.
No client files are included. You need your own copy of the client.
</details>

## Setup

You need an extracted copy of the LMO client on the server. The patch server builds player
installs from it. The native Linux client is downloaded automatically by
`patches/linux-client/install.sh`.

The game's data (worlds, NPCs, skills, loot, minifigures, achievements) is its own repository,
[LMOxide/lmoxide-content](https://github.com/LMOxide/lmoxide-content), checked out at
`content/` as a git submodule ([docs/CONTENT.md](docs/CONTENT.md)). Clone with
`git clone --recurse-submodules`, or run `git submodule update --init` in an existing clone.

### Deploy with Docker

One Docker Compose file runs everything from the published images, without Rust or Node:

| Service | Image | Where |
|---|---|---|
| game server, REST API and patch server (one process, every game mode) | `ghcr.io/lmoxide/lmoxide` | game ports 7100-7104 and 7200; HTTP under `/api/` and the patch paths |
| web UI (accounts, characters, moderation, players' own accounts) | `ghcr.io/lmoxide/lmoxide-webui` | `https://<host>/` |
| dev UI (debugger and world editor) | `ghcr.io/lmoxide/lmoxide-dev-ui` | `https://<host>/dev/` |
| Discord bot (optional) | `ghcr.io/lmoxide/lmoxide-discord-bot` | no ports |
| Caddy (HTTPS, certificates from Let's Encrypt) | `caddy:2` | 80, 443 |

You need Docker, a host name pointing at the machine, and your own extracted copies of the
Windows and Linux game clients (LMOxide doesn't ship them). Open TCP 80, 443, 7100-7104 and 7200.

```sh
git clone https://github.com/LMOxide/lmoxide.git && cd lmoxide   # only the compose files are used
cp docker/.env.example .env                           # host name, client folders, image versions
cp docker/lmoxide.example.toml docker/lmoxide.toml    # public_host, server name, registration
docker compose up -d
docker compose exec lmoxide lmoxide game account create <you> --gm-level 4   # your admin account
```

Then sign in at `https://<host>/`; players point the
[LMOxide patcher](https://github.com/LMOxide/lmoxide-patcher/releases) at `https://<host>`.

The Discord bot (registration through Discord, roles to GM levels, moderation commands):

```sh
docker compose exec lmoxide lmoxide api key create discord-bot --scope discord   # printed once
mkdir -p secrets && printf '%s' '<bot token>' > secrets/discord_token && printf '%s' 'lmox_...' > secrets/discord_bot_key
sudo chown 10002 secrets/* && sudo chmod 400 secrets/*                   # readable by the bot's user
cp docker/discord-bot.example.toml docker/discord-bot.toml   # your Discord server, roles, admins
echo COMPOSE_PROFILES=discord >> .env && docker compose up -d
```

Updating is `docker compose pull && docker compose up -d`; the database and login key are in the
`data` volume (back it up). [docs/HOSTING.md](docs/HOSTING.md) goes through every step in detail
(file ownership for the secrets, registration modes, patcher self-updates, backups), and
[docs/SECURITY.md](docs/SECURITY.md) has the access levels and a checklist to read before you
open the server to the internet. To build the images from source instead:
`docker compose -f docker-compose.yml -f docker-compose.build.yml up -d --build`.

### Release files

For other setups, every version tag publishes builds on the [Releases](https://github.com/LMOxide/lmoxide/releases)
page and images on GitHub's container registry:

| | |
|---|---|
| `ghcr.io/lmoxide/lmoxide` | the server image (`latest` or a version) |
| `lmoxide-<version>-x86_64-linux.tar.gz` | the server binaries, `content/` and an example config, for Linux without Docker |
| `lmoxide-patcher-<version>-linux`, `-windows.exe`, `-macos.tar.gz` | the command-line patcher |

The web UI, the dev UI, the Discord bot and the GUI patcher publish theirs in their own
repositories.

### LAN play from source

Needs Rust and `protoc`. The scripts expect the client at
`../clients/windows/LEGO Minifigures Online`; set `LMOXIDE_CLIENT_DIR` to use another folder.

```sh
tools/servers.sh                        # the server in the background (every game mode), opens the debugger and editor
tools/servers.sh status | restart | down
tools/lan-server.sh --release           # the same in the foreground (game, patch server, API)
```

Accounts are created at first login (`--auto-register`), and every account gets GM level 2 (contributor) for
the [test commands](docs/DEVELOPMENT.md#test-commands). `tools/lan-party.sh` also starts the web
UI and creates player accounts.

<details>
<summary>Running the server binaries yourself</summary>

```sh
cargo run --release -p lmoxide-server -- account create <name> --gm-level 4
cargo run --release -p lmoxide-server -- --help
cargo run --release -p lmoxide-patch-server -- --help
cargo run --release -p lmoxide-api -- --help
```

| Port | Service |
|---|---|
| 8080 | patch server (HTTP) |
| 7100, 7101, 7102 | universe, territory, game |
| 7200 | chat |
| 7300 | control (keep private) |
| 8090 | REST API |

Every port can be changed; see `--help`.
</details>

### Players

Players install a client from the server's patch server:

```sh
lmoxide-patcher install --server http://<host>:8080 --dir ~/Games/LMO
```

On Linux, the native client kit can be used instead:

```sh
d=$(mktemp -d) && curl -fsSL http://<host>:8080/client-kit.tar.gz | tar xz -C $d && $d/*/install.sh --server <host>
~/Games/LMO/lmo <username>
```

See [docs/PATCHING.md](docs/PATCHING.md) and
[patches/linux-client/README.md](patches/linux-client/README.md).

## Documentation

| | |
|---|---|
| [GAME-OPTIONS.md](docs/GAME-OPTIONS.md) | the game modes (progression by default) and options for aiming and story windows |
| [HOSTING.md](docs/HOSTING.md) | running a public server from the published images: Docker Compose, Caddy, the web UI, dev UI and Discord bot, updates, backups |
| [DEVELOPMENT.md](docs/DEVELOPMENT.md) | test runs, in-game test commands, scripts, reverse-engineering tools |
| [PROTOCOL.md](docs/PROTOCOL.md) | how the client talks to the server: boot, framing, RPCs, login, game systems |
| [CONTENT.md](docs/CONTENT.md) | the game's data in one editable tree (`content/`): its format, commands, what the import did |
| [TUNABLES.md](docs/TUNABLES.md) | values that are guessed because the client data doesn't contain them |
| [tracker/](docs/tracker/README.md) | checklist of what is verified against live (upgrades, abilities, enemies, achievements, quests, worlds, dialogues); `tools/tracker/build.py` regenerates it, keeping ticks |
| [SECURITY.md](docs/SECURITY.md) | what is protected, what can't be, and a deployment checklist |
| [PATCHING.md](docs/PATCHING.md) | the patch server and the patcher protocol |
| [API.md](docs/API.md) | the REST API ([OpenAPI](docs/openapi.json)) |
| [WEB-UI.md](docs/WEB-UI.md), [DISCORD-BOT.md](docs/DISCORD-BOT.md) | building the web UI and the Discord bot, which live in their own repositories |
| [Linux client kit](patches/linux-client/README.md) | installing and patching the native Linux client |
