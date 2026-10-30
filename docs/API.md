# REST API (lmoxide-api)

For a web UI, a Discord bot, scripts. The complete, machine-readable description is the OpenAPI
document at `GET /v1/openapi.json` (or `lmoxide-api openapi`); generate clients from it.

## Running

`lmoxide` (one process, docker-compose.yml) serves the API on its HTTP listener with the patch
server; nothing to set up. Run apart:

```sh
lmoxide-api --data-dir lmoxide-data --bind 127.0.0.1:8090 \
  --control-url http://127.0.0.1:7300 --control-token-file secrets/control_token
```

It opens the game server's SQLite database (same `--data-dir`) for accounts and reaches the
running game server through its control interface for live data (players online, kicks,
broadcasts, maintenance). Without a control token those routes answer 503. Flags (each also an
environment variable, see `--help`): `--tls-cert/--tls-key`, `--private-status`,
`--registration open|discord|closed` (default `closed`; `--open-registration` = `open`), `--session-ttl-hours`, `--remember-days` (30), `--moderator-gm-level` (3), `--admin-gm-level`
(4), `--editor-gm-level` (2; docs/SECURITY.md "Access levels"), `--cors-origins`, `--trust-forwarded-for`, `--client-dir` (the Windows client folder:
`/v1/catalog/sounds/{id}` serves the sounds of effect packages from its RDB; 503 without it).

## Authentication

`Authorization: Bearer <secret>` with either

- an **API key** (bots, scripts): `lmoxide-api key create discord-bot --scope read` prints the
  secret once; admins can also `POST /v1/api-keys`. Scopes: `read` (status, players, accounts,
  characters), `moderate` (+ kick, ban, unban, broadcast), `discord` (+ the `/v1/discord` routes,
  for the Discord bot), `admin` (+ create/rename/delete accounts, passwords, GM levels,
  maintenance, API keys, audit log).
- an **account key**: an API key that belongs to an account (`account_id` in `/v1/api-keys`),
  made by the Discord bot for each linked account (`POST /v1/discord/links/{discord_id}/api-key`).
  It acts as that account: its scope follows the GM level as a session's does, `/v1/me` works,
  it stops working while the account is banned, and it is revoked when the account is unlinked.
  Audit actor: `account:<username> via api-key:<name>`.
- a **session token** from `POST /v1/auth/login` with a game account's username and password
  (web UI). Its scope follows the account's GM level; ordinary players get no scope and can only
  use `/v1/me` (their account, password change). Sessions last `--session-ttl-hours`; a login
  with `"remember": true` ("stay signed in") lasts `--remember-days`, and `POST /v1/auth/renew`
  trades a session for a fresh one (new token, old one ends) so a remembered sign-in lasts while
  it's used. Only the token's SHA-256 is stored; logging out, a password change or a ban ends it.
  `POST /v1/auth/code` signs in with a one-time code the Discord bot sent; that session has
  `password_reset: true` and may set a new password without the current one.

Registration (`--registration`): `open`: anyone with `POST /v1/auth/register`, and the Discord
bot; `discord`: only the Discord bot (`POST /v1/discord/accounts`), which checks that the person
is a member of its server; `closed`: only admins. `GET /v1/config` says which.

API keys used by a bot for someone else may send `X-On-Behalf-Of: <who>` (e.g.
`discord:name (id)`); the audit log records it next to the key's name.

Errors are `{"error": {"code": "...", "message": "..."}}` with codes such as `unauthorized`,
`forbidden`, `not_found`, `invalid`, `conflict`, `rate_limited`, `game_server_unavailable`.

## Routes

| Method | Path | Scope | |
|---|---|---|---|
| GET | `/v1/status` | public* | game server up, version, players online, maintenance |
| GET | `/v1/online` | public* | characters in play (in-game name, playfield) |
| GET | `/v1/config` | public | `registration` (`open`/`discord`/`closed`), `registration_open`, `private_status`, `moderator_gm_level`, `admin_gm_level` |
| GET | `/v1/playfields` | public | every playfield id with its name (for the ids in other answers); `teleport`: a `!tp` destination |
| GET | `/v1/minifigures` | public | minifigure template ids with name, series, attack names, upgrade names by bit, parts (tokens) |
| GET | `/v1/tokens` | public | token ids (minifigure parts, as in a character's `tokens`) with name and minifigure |
| POST | `/v1/auth/login` | - | `{username, password, remember?}` → `{token, expires_at, scope, account}` |
| POST | `/v1/auth/renew` | session | `{remember?}` → a new session like the login's; the old token stops working |
| POST | `/v1/auth/logout` | any | ends the session |
| POST | `/v1/auth/register` | - | `{username, password}`, only when registration is `open` |
| POST | `/v1/auth/code` | - | `{code, remember?}`: a `login` code from the Discord bot → a `Session` with `password_reset: true`; 401 `bad_code` (counts as a failed login) |
| GET | `/v1/me` | session | the logged-in account (`discord_id` when linked) |
| POST | `/v1/me/password` | session | `{current_password, new_password}`; `current_password` may be left out in a `password_reset` session |
| GET | `/v1/me/players` | session | the account's characters online (`Player`, with `position` and `heading`) |
| POST | `/v1/me/command` | session | `{command}`: a test command run as one's own character, as if typed in chat; the game server decides by GM level (alpha testers have some, contributors all) → `{affected, replies}` |
| POST | `/v1/me/discord` | session | `{code}`: a `link` code from the Discord bot links the account to that Discord account → `Account`; 401 `bad_code`, 409 when either is linked already |
| GET | `/v1/discord/links` | discord | linked accounts: `[{discord_id, account_id, username, gm_level, banned}]` |
| GET | `/v1/discord/links/{discord_id}` | discord | one of them, or 404 |
| PATCH | `/v1/discord/links/{discord_id}` | discord | `{gm_level}` (linked accounts only) |
| DELETE | `/v1/discord/links/{discord_id}` | discord | unlink; revokes the account's keys, the account stays |
| POST | `/v1/discord/links/{discord_id}/api-key` | discord | a new account key named `discord-bot` (replaces the earlier one) → `CreatedApiKey` |
| POST | `/v1/discord/accounts` | discord | `{discord_id, username, password, gm_level?}` → `{link, key}`: a new linked account with its account key; 403 `registration_closed` when `closed`, 409 when taken or linked |
| POST | `/v1/discord/codes` | discord | `{discord_id, purpose: link\|login}` → `{code, expires_at}`: `XXXX-XXXX`, 10 minutes, once; `link` needs an unlinked Discord account (409), `login` a linked one (404) |
| GET | `/v1/players` | read | online players with account ids, usernames, game mode, playfield and copy |
| GET | `/v1/instances` | moderate | running copies of worlds (each game mode's main copies, team copies, previews): mode, kind, players, NPCs; the AI's tick time |
| POST | `/v1/players/{account_id}/kick` | moderate | `{reason?}` |
| any | `/v1/content/{path}` | admin | a play server's world content (docs/WORLD-CONTENT.md: approval queue, rules, summaries, features), passed to the game server as they are. Not in openapi.json |
| any | `/v1/survey/{path}` | contributor (GM ≥ `--editor-gm-level`) or moderate | the world editor's routes (docs/SURVEY.md "API"), passed to the game server as they are; the caller is the author of what they change. Not in openapi.json; bodies up to 16 MB |
| any | `/v1/editor/{path}` | contributor or moderate | the world editor's content editing routes (docs/WORLD-EDITOR-CONTENT.md: records, schemas, catalogs, changes, history), passed on like `/v1/survey`; the caller is the author |
| POST | `/v1/players/{account_id}/command` | admin | `{command}`: a test command (`!fx 163540`, `!skin 2212`, `!state 29`) run as the player, as if typed in chat (its GM level still applies) → `{affected, replies}`: the chat answers it produced |
| GET | `/v1/catalog/effects` | read | every effect package / particle system: timelines (with the code hook that starts each, `until_stopped`), particles and sounds (with RDB ids), animation attributes, code hooks, items and spells using it |
| GET | `/v1/catalog/sounds/{id}` | read | a sound of an effect package (`sounds[].id`) as Ogg Vorbis; needs `--client-dir` |
| GET | `/v1/catalog/monsters` | read | every monster data (look, `!skin`): designers' name, motion set, anim graph, death effect, what uses it |
| GET | `/v1/catalog/animations` | read | animation attributes (`!anim`), motion sets with their motions, anim graphs |
| GET | `/v1/catalog/bcc-states` | read | character states (`!state`, stat 20) and substates by name |
| GET | `/v1/catalog/test-commands` | session or read | what the test commands accept: commands with usage and the GM level each needs, `!goto` target kinds with numbered targets, `!test` scenarios, `!dialog` dialogues, `!loot` kinds, `!spawn` names, `!skin` looks |
| POST | `/v1/broadcast` | moderate | `{message}`: a chat line from "Server" to everyone |
| POST | `/v1/maintenance` | admin | `{enabled, message?}`: only GMs may log in |
| GET | `/v1/accounts?search=&offset=&limit=` | read | by username; header `X-Total-Count`: matches in all |
| POST | `/v1/accounts` | admin | `{username, password?, gm_level?}` |
| GET | `/v1/accounts/{id}` | read | with characters; `last_login_ip` for admins only |
| PATCH | `/v1/accounts/{id}` | admin | `{username?, gm_level?}` |
| DELETE | `/v1/accounts/{id}` | admin | kicks, then deletes with character and friendships |
| POST | `/v1/accounts/{id}/password` | admin | `{password}` (null removes it) |
| POST | `/v1/accounts/{id}/ban` | moderate | `{days?, reason?}`; kicks; staff need admin |
| POST | `/v1/accounts/{id}/unban` | moderate | |
| GET | `/v1/characters?search=&limit=` | read | characters whose name contains `search` (case-insensitive), by name: `id`, `name`, `mode`, `account_id`, `username` (who plays a character) |
| GET | `/v1/characters/{id}` | read | team, stars, upgrades, tokens, level and XP, minifigure ranks, earned achievements (id, name, title/group/text as the game shows them, points, time; tags with no name are story or tutorial progress), achievement counters, journal (`defeats` by name with kind/count/first/last time, `visits` by playfield, `finds`) |
| POST | `/v1/characters/{id}/achievements/remove` | admin | `{ids: [..]}` or `{all: true}`: takes achievements away (and the earned "all of" ones above them; a find achievement's finds; with `all` also the journal and counters) so they can be earned again; story/tutorial tags, stars and XP stay. Online: the client is updated at once (RemoveTag); offline: the database. Answers `{removed, online}`; audit `achievements-remove` |
| GET/POST | `/v1/api-keys` | admin | list / `{name, scope}` → secret once |
| DELETE | `/v1/api-keys/{id}` | admin | revoke |
| GET | `/v1/audit?limit=&before=` | admin | administrative actions, newest first; for actions on an account `target` is its username and `target_account_id` its id |

\* public unless `--private-status`, then `read`.

The game data routes (`/v1/playfields`, `/v1/minifigures`, `/v1/tokens`) send
`Cache-Control: public, max-age=3600`; the catalogs `private, max-age=3600` with an `ETag` (a
request with `If-None-Match` gets 304 while the data is unchanged); everything else is `no-store`.
Answers are gzip/brotli compressed when the client accepts it. The OpenAPI document gives
the username and password rules (`pattern`, `minLength`, `maxLength`); password lengths are
counted in bytes by the server.

Example (a Discord bot's status command):

```sh
curl -s https://lmo.example.org/api/v1/status
# {"game_server_online":true,"version":"0.1.0","started_at":1792828074,"players_online":3,...}
```
