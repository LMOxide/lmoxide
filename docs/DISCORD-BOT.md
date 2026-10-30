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

API on `http://127.0.0.1:8090` with its own database and three bot keys, printed at startup
(also in `../var/web-dev/demo-*-key.txt`): `demo-bot` (scope `read`), `demo-mod-bot` (scope
`moderate`) and `demo-discord-bot` (scope `discord`, for registering and linking). Demo accounts: `admin`, `moderator`, `player`, `banned` (password
`<name>-password`). Nobody is online unless a game client connects: `/v1/online` returns `[]`,
kicks answer `{"affected": 0}`, broadcasts reach nobody.

## Credentials

The bot uses **API keys**, not account logins: `Authorization: Bearer lmox_...`. A server admin
creates them:

```sh
lmoxide-api key create discord-bot --scope discord         # or in Docker:
docker compose exec lmoxide lmoxide api key create discord-bot --scope discord
```

The secret is printed once; the bot reads it from its environment or a secret file, never from
its repository. Keys are revoked with `lmoxide-api key revoke <id>` or `DELETE /v1/api-keys/{id}`.

- **`read`**: status, players online with usernames, account and character lookups.
- **`moderate`**: plus kick, ban, unban, broadcast. Moderate keys can't kick or ban staff
  (accounts with a GM level): 403.
- **`discord`**: plus the `/v1/discord` routes: register and link Discord accounts, set the GM
  level of linked accounts, make their account keys and one-time codes. The bot's own key.
- **`admin`**: don't give a bot this unless it really manages accounts.

A key without an account is shared by everyone who can run the bot's commands, so with it **the
bot must check Discord roles itself** before calling moderation routes. Better: act with the
person's **account key** (below), and the API decides from their GM level.

## Accounts from Discord

A game account can be linked to one Discord account (`Account.discord_id`, a string). The bot's
`discord` key manages that:

- **Register**: `POST /v1/discord/accounts {discord_id, username, password, gm_level?}` makes a
  linked account and returns `{link, key}`. Check first that the person is a member of the
  Discord server. Allowed while registration is `open` or `discord` (`GET /v1/config`
  `registration`); with `--registration discord` this is the only way to register.
- **Account keys**: `key.secret` (and `POST /v1/discord/links/{discord_id}/api-key`, which
  replaces it) is a key that **acts as that account**: its scope follows the account's GM level,
  it stops working while the account is banned or after it is unlinked, and the audit log names
  the account (`account:<name> via api-key:discord-bot`). Keep it secret and use it for that
  person's commands (`/kick`, `/announce`, ...). Keep using the `discord` key for lookups that
  ordinary players can't make (`/whois`).
- **Link an existing account**: `POST /v1/discord/codes {discord_id, purpose: "link"}` →
  `{code, expires_at}` (`XXXX-XXXX`, 10 minutes, works once). DM it; the person enters it in
  the web UI while signed in (`POST /v1/me/discord {code}`). There is no event: poll
  `GET /v1/discord/links/{discord_id}` until it is linked or the code expires, then make the
  account key.
- **Forgotten password**: `POST /v1/discord/codes {discord_id, purpose: "login"}` (linked
  accounts only). DM it; the web UI signs in with it (`POST /v1/auth/code`) and the session may
  set a new password without the old one.
- **Roles → GM levels**: `GET /v1/discord/links` lists every linked account with its GM level;
  `PATCH /v1/discord/links/{discord_id} {gm_level}` sets it (linked accounts only; from the next
  game login). `DELETE /v1/discord/links/{discord_id}` unlinks and revokes the account's keys.

**Attribution** (with a shared key): send `X-On-Behalf-Of: discord:<user name> (<user id>)` with every moderation
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
