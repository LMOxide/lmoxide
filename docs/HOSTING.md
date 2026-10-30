# Hosting a server

[← README](../README.md)

This runs a public LMOxide server from the published images, with Docker. Nothing is built, so
you don't need Rust or Node.

| Service | Image | What it is |
|---|---|---|
| `lmoxide` | `ghcr.io/lmoxide/lmoxide` | the game server (every game mode), the REST API and the patch server, in one process |
| `caddy` | `caddy:2` | HTTPS in front of everything on the HTTP side (certificates from Let's Encrypt) |
| `webui` | `ghcr.io/lmoxide/lmoxide-webui` | the website: server status, registration, accounts, the admin console |
| `dev-ui` | `ghcr.io/lmoxide/lmoxide-dev-ui` | the debugger and the world editor |
| `discord-bot` | `ghcr.io/lmoxide/lmoxide-discord-bot` | optional (the `discord` profile): accounts and GM levels through Discord |

## What you need

- A Linux machine with a public IP address, Docker and the Docker Compose plugin.
- A host name (e.g. `lmo.example.org`) with a DNS record pointing at the machine. Players,
  browsers and Caddy's certificate all use it.
- **Your own copies of the game client**, extracted: the Windows client and the Linux client.
  LMOxide can't distribute them. The patch server builds player installs from them (patched for
  your server), so players don't need a copy of their own.

## Ports

| Port | Protocol | What |
|---|---|---|
| 80 | TCP | HTTP: certificates, the redirect to HTTPS, and the patch server for the game client (it speaks only plain HTTP) |
| 443 | TCP, UDP | HTTPS: the web UI, the dev UI, the API (`/api/`), the patch server for LMOxide patchers |
| 7100, 7103, 7104 | TCP | the game modes' universe ports: Progression, Maxxed, World Editor |
| 7101, 7102 | TCP | territory and game |
| 7200 | TCP | chat |

Open these in the firewall. Nothing else needs to be reachable: the API and the patch server
listen on port 8080 inside the compose network only. To leave a game mode out, remove it from
`modes` in `docker/lmoxide.toml` and its port from `docker-compose.yml`.

## Addresses

Everything on the HTTP side shares the one host name (`docker/Caddyfile`):

| Path | Goes to |
|---|---|
| `/` and every other path | the web UI |
| `/api/...` | the REST API (`/api` is stripped: `/api/v1/status` is the API's `/v1/status`) |
| `/dev/` | the dev UI (`/dev/debugger/`, `/dev/editor/`) |
| `/control/`, `/patch/`, `/auth/`, `/track`, `/rest/`, `/lmoxide/`, `/client-kit.tar.gz` | the patch server, on HTTP (port 80) and HTTPS |

Both UIs call the API at `/api` on their own origin, so there is no CORS to set up. Players
point their patcher at `https://lmo.example.org` (or `http://`).

## First start

1. Get the compose files. Clone the repository, or download `docker-compose.yml`, `docker/` and
   this file from it:

   ```sh
   git clone https://github.com/LMOxide/lmoxide.git && cd lmoxide
   ```

   The rest happens in that folder. `content/` (a submodule) is not needed: the image has it.

2. Copy the settings and fill them in:

   ```sh
   cp docker/.env.example .env
   cp docker/lmoxide.example.toml docker/lmoxide.toml
   mkdir -p patchers     # patcher builds for self-updates (below); Docker would make it root's
   ```

   | File | Set |
   |---|---|
   | `.env` | `LMOXIDE_PUBLIC_HOST` (your host name); `LMOXIDE_WINDOWS_CLIENT` and `LMOXIDE_LINUX_CLIENT` (the client folders on this machine) |
   | `docker/lmoxide.toml` | `public_host`, and the host name in `api-url` and `website`; `server-name`; `registration` (below) |

   The client folders are mounted read-only. They must be readable by the container's user
   (uid 10001).

3. Start it:

   ```sh
   docker compose up -d
   docker compose logs -f lmoxide
   ```

   The first start hashes both clients for the patch server, which takes a minute or two.

4. Create your admin account (GM level 2). The password is asked for:

   ```sh
   docker compose exec lmoxide lmoxide game account create <you> --gm-level 2
   ```

5. Open `https://lmo.example.org` and sign in. The admin console is there. The dev UI is at
   `https://lmo.example.org/dev/`.

Players install the client with the [LMOxide patcher](https://github.com/LMOxide/lmoxide-patcher/releases)
pointed at `https://lmo.example.org`.

### Who may register

`registration` in the `[api]` table of `docker/lmoxide.toml`:

| Value | |
|---|---|
| `closed` | only admins create accounts (web UI, or `lmoxide game account create`) |
| `open` | anyone, from the web UI's register page |
| `discord` | only through the Discord bot, for members of your Discord server |

### Other admin commands

```sh
docker compose exec lmoxide lmoxide game account --help      # create, passwd, gm, ban, unban, list, delete
docker compose exec lmoxide lmoxide game account gm <user> 1  # GM level 1 (moderator)
docker compose exec lmoxide lmoxide api key create <name> --scope read   # an API key for a script
```

## The Discord bot

Optional. It needs a Discord application, an API key and its settings. The bot's
[DEPLOYING.md](https://github.com/LMOxide/lmoxide-discord-bot/blob/main/docs/DEPLOYING.md)
explains the Discord side (the token, the Server Members intent, inviting it, role order).

1. The API key (the secret is printed once):

   ```sh
   docker compose exec lmoxide lmoxide api key create discord-bot --scope discord
   ```

2. The two secrets, readable by the bot's user (uid 10002):

   ```sh
   mkdir -p secrets
   printf '%s' '<Discord bot token>' > secrets/discord_token
   printf '%s' 'lmox_...' > secrets/discord_bot_key
   sudo chown 10002 secrets/discord_token secrets/discord_bot_key
   sudo chmod 400 secrets/discord_token secrets/discord_bot_key
   ```

3. Its settings: `cp docker/discord-bot.example.toml docker/discord-bot.toml`, then fill in
   your Discord server's id, the bot admins, the roles and `webui_url`.

4. Turn it on: uncomment `COMPOSE_PROFILES=discord` in `.env`, then `docker compose up -d`.
   Set `registration = "discord"` in `docker/lmoxide.toml` if the bot should be the only way to
   get an account (`docker compose restart lmoxide`).

The bot talks to the API on the compose network (`http://lmoxide:8080`) and opens no ports. Its
database (the members' account keys) is in the `discord-bot-data` volume.

## Patcher self-updates

The GUI patcher updates itself when the server offers a newer build. The patch server offers
whatever is in the folder `LMOXIDE_PATCHER_DIR` (`./patchers` by default), mounted at
`/patchers` (`patcher-dir` in `docker/lmoxide.toml`). Files must be named
`<name>-<version>-<platform>[.exe]`, and the
[patcher's releases](https://github.com/LMOxide/lmoxide-patcher/releases) already are:

```sh
mkdir -p patchers && cd patchers
v=0.2.0
curl -fLO https://github.com/LMOxide/lmoxide-patcher/releases/download/v$v/lmoxide-patcher-gui-$v-windows.exe
curl -fLO https://github.com/LMOxide/lmoxide-patcher/releases/download/v$v/lmoxide-patcher-gui-$v-linux
cd .. && docker compose kill -s HUP lmoxide     # rescan the folder (or restart lmoxide)
```

`kill -s HUP` doesn't stop the server: the patch server rescans and re-signs (a few seconds).

The newest version of each name and platform is offered, signed with the server's manifest key.
A patcher takes it only if it trusts that key (it has installed from this server before). The
command-line patcher's releases (`lmoxide-patcher-<version>-linux`, `-windows.exe` from
[lmoxide's releases](https://github.com/LMOxide/lmoxide/releases)) work the same way, for
`lmoxide-patcher self-update`. macOS builds don't update themselves.

## Updating

```sh
docker compose pull
docker compose up -d
```

`latest` follows the newest release. To stay on one, set the versions in `.env`
(`LMOXIDE_VERSION=0.2.0`, ...) and change them when you update. Read the release notes first:
a release can need new settings in `docker/lmoxide.toml`, and the compose files may change
(`git pull`).

The game data (`content/`) comes with the `lmoxide` image, so it updates with it.

## Backups

The `data` volume holds everything that matters:

| File | What |
|---|---|
| `lmoxide.db` | the database: accounts, characters, friendships, bans, API keys, the audit log, and the login key (patched into every client the server hands out: a new one means every player has to update) |
| `patch-data/manifest-signing.key` | the patch server's signing key (patchers trust it: a new one makes them ask again) |
| `patch-data/` | the patch server's caches |
| `content-work/`, `content-queue/` | the world editor's working copy and publish queue, if you use it |

Stop the server for a consistent copy:

```sh
docker compose stop lmoxide
docker run --rm --volumes-from "$(docker compose ps -aq lmoxide)" -v "$PWD":/backup \
  debian:bookworm-slim tar czf /backup/lmoxide-data-$(date +%F).tar.gz -C /data .
docker compose start lmoxide
```

Restore by extracting such a file into the volume the same way (`tar xzf ... -C /data`) while
the server is stopped. With the bot, back up the `discord-bot-data` volume as well.

## The world editor

The World Editor mode (port 7104) saves into a git clone of the content repository in the data
volume (`editor-content-dir`). Make it once:

```sh
docker compose exec lmoxide git clone https://github.com/LMOxide/lmoxide-content.git /data/content-work
docker compose restart lmoxide
```

How edits are reviewed and published: [WORLD-EDITOR-CONTENT.md](WORLD-EDITOR-CONTENT.md).

## Variations

<details>
<summary>The dev UI on its own host name</summary>

Instead of `/dev/`, give the dev UI e.g. `dev.lmo.example.org` (a second DNS record) with its own
`/api`, in `docker/Caddyfile`:

```caddy
dev.lmo.example.org {
	encode zstd gzip
	handle_path /api/* {
		reverse_proxy lmoxide:8080
	}
	handle {
		reverse_proxy dev-ui:8080
	}
}
```

</details>

<details>
<summary>Building the images yourself</summary>

With the web UI, dev UI and bot repositories checked out next to this one:

```sh
docker compose -f docker-compose.yml -f docker-compose.build.yml up -d --build
```

The web UI's Discord invite button is set when it is built:
`docker build --build-arg VITE_DISCORD_INVITE=https://discord.gg/... -t lmoxide-webui:latest ../lmoxide-webui`.

</details>

<details>
<summary>Without Docker</summary>

Each release has `lmoxide-<version>-x86_64-linux.tar.gz`: the binaries (`bin/`), `content/`,
the Linux client overlay (`linux-overlay/`) and `lmoxide.example.toml`. It runs on Linux with
glibc 2.36 or newer (Debian 12, Ubuntu 24.04). Set `LMOXIDE_CONTENT_DIR` to the `content/`
folder, change the paths in the config (`data_dir`, the client folders, `linux-overlay-dir`) and
run `bin/lmoxide --config lmoxide.toml`. Put a reverse proxy in front as `docker/Caddyfile`
does, and serve the web UI and dev UI from their release files.

</details>

Before opening the server to the internet, read the deployment checklist in
[SECURITY.md](SECURITY.md).
