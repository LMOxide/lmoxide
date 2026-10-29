# Building the Discord bot

Context for the LMOxide Discord bot, which lives in its own repository and talks only to
`lmoxide-api` over HTTP. The API is developed here; if the bot needs something the API lacks,
change it in this repo (and regenerate `docs/openapi.json`).

## What to read

| | |
|---|---|
| [`docs/openapi.json`](openapi.json) | the contract: every route, request and response schema (OpenAPI 3.1); generate a typed client from it. A test keeps it in sync with the code. |
| [`docs/API.md`](API.md) | authentication, scopes, route table, error format |
| [`docs/SECURITY.md`](SECURITY.md) | "The REST API": keys, rate limits, what is public |

## A backend to develop against

From this repo (needs Rust and `protoc`; no game client):

```sh
tools/web-dev-backend.sh          # --reset for a fresh database
```

API on `http://127.0.0.1:8090` with its own database and two bot keys, printed at startup (also
in `../var/web-dev/demo-*-key.txt`): `demo-bot` (scope `read`) and `demo-mod-bot` (scope
`moderate`). Demo accounts: `admin`, `moderator`, `player`, `banned` (password
`<name>-password`). Nobody is online unless a game client connects: `/v1/online` returns `[]`,
kicks answer `{"affected": 0}`, broadcasts reach nobody.

## Credentials

The bot uses **API keys**, not account logins: `Authorization: Bearer lmox_...`. A server admin
creates them:

```sh
lmoxide-api key create discord-bot --scope read            # or in Docker:
docker compose exec lmoxide lmoxide api key create discord-bot --scope moderate
```

The secret is printed once; the bot reads it from its environment or a secret file, never from
its repository. Keys are revoked with `lmoxide-api key revoke <id>` or `DELETE /v1/api-keys/{id}`.

- **`read`**: status, players online with usernames, account and character lookups.
- **`moderate`**: plus kick, ban, unban, broadcast. Moderate keys can't kick or ban staff
  (accounts with a GM level): 403.
- **`admin`**: don't give a bot this unless it really manages accounts.

The API knows nothing about Discord: the key is shared by everyone who can run the bot's
commands, so **the bot must check Discord roles itself** before calling moderation routes.

**Attribution:** send `X-On-Behalf-Of: discord:<user name> (<user id>)` with every moderation
call. The audit log then records `api-key:discord-bot for discord:<name> (<id>)` instead of just
the key (printable text, at most 100 characters; it is the bot's claim, not authentication).

## Commands, and the routes behind them

| Command idea | Route | Scope |
|---|---|---|
| `/status` | `GET /v1/status` (`game_server_online`, `players_online`, `maintenance`, `started_at`) | public* |
| `/online` | `GET /v1/online` (`character_name`, `playfield`) + `GET /v1/playfields` for names | public* |
| `/whois <username>` | `GET /v1/accounts?search=<name>`, then pick the exact (case-insensitive) match; `GET /v1/accounts/{id}` | read |
| `/character <id>` | `GET /v1/characters/{id}` (team, stars, level, achievements) | read |
| `/kick <username> [reason]` | resolve the account id as above (or from `GET /v1/players`), `POST /v1/players/{account_id}/kick` `{reason}` | moderate |
| `/ban <username> [days] [reason]` | `POST /v1/accounts/{id}/ban` `{days, reason}` (also kicks; no days = permanent) | moderate |
| `/unban <username>` | `POST /v1/accounts/{id}/unban` | moderate |
| `/announce <text>` | `POST /v1/broadcast` `{message}` (1-500 characters, shown in game chat from "Server") | moderate |

\* no key needed unless the server runs the API with `--private-status`; send the key anyway.

Usernames (login names) and character names are different things: `/v1/online` shows only
character names, `/v1/players` (read) shows both with the account id.

## Notifications (polling)

The API has no events or webhooks. For "server is up/down" or "X joined/left" messages, poll:

- `GET /v1/status` every 30-60 s: `game_server_online` flipping is up/down; `maintenance` too.
- `GET /v1/online` (or `/v1/players`) at the same rate: diff the lists for joins and leaves.

Stay well under the rate limit (per client address: 10 requests/s, bursts of 60; over it the API
answers 429 `rate_limited`). If push events become necessary, add them to the API here rather
than polling faster.

## Errors

Always `{"error": {"code", "message"}}`. Codes worth handling: `unauthorized` (bad or revoked
key), `forbidden` (scope too low, or acting on staff), `not_found`, `invalid` (e.g. message
length), `rate_limited`, `game_server_unavailable` (502: game server down, report it as offline),
`no_control` (503: the API isn't connected to a game server). Times are unix seconds; account
and character ids are u32.

## Deployment

The bot only needs to reach the API. Next to the LMOxide compose stack it can join the same
compose project and call `http://api:8090` on the internal network; elsewhere use the public
HTTPS address of the API. No CORS settings are needed (that's only for browsers).
