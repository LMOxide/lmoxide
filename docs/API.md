# REST API (lmoxide-api)

For a web UI, a Discord bot, scripts. The complete, machine-readable description is the OpenAPI
document at `GET /v1/openapi.json` (or `lmoxide-api openapi`); generate clients from it.

## Running

```sh
lmoxide-api --data-dir lmoxide-data --bind 127.0.0.1:8090 \
  --control-url http://127.0.0.1:7300 --control-token-file secrets/control_token
```

It opens the game server's SQLite database (same `--data-dir`) for accounts and reaches the
running game server through its control interface for live data (players online, kicks,
broadcasts, maintenance). Without a control token those routes answer 503. Flags (each also an
environment variable, see `--help`): `--tls-cert/--tls-key`, `--private-status`,
`--open-registration`, `--session-ttl-hours`, `--moderator-gm-level` (1), `--admin-gm-level`
(2), `--cors-origins`, `--trust-forwarded-for`.

## Authentication

`Authorization: Bearer <secret>` with either

- an **API key** (bots, scripts): `lmoxide-api key create discord-bot --scope read` prints the
  secret once; admins can also `POST /v1/api-keys`. Scopes: `read` (status, players, accounts,
  characters), `moderate` (+ kick, ban, unban, broadcast), `admin` (+ create/rename/delete
  accounts, passwords, GM levels, maintenance, API keys, audit log).
- a **session token** from `POST /v1/auth/login` with a game account's username and password
  (web UI). Its scope follows the account's GM level; ordinary players get no scope and can only
  use `/v1/me` (their account, password change). Sessions last `--session-ttl-hours`.

Errors are `{"error": {"code": "...", "message": "..."}}` with codes such as `unauthorized`,
`forbidden`, `not_found`, `invalid`, `conflict`, `rate_limited`, `game_server_unavailable`.

## Routes

| Method | Path | Scope | |
|---|---|---|---|
| GET | `/v1/status` | public* | game server up, version, players online, maintenance |
| GET | `/v1/online` | public* | characters in play (in-game name, playfield) |
| GET | `/v1/playfields` | public | playfield ids with names (for the ids in other answers) |
| POST | `/v1/auth/login` | - | `{username, password}` → `{token, expires_at, scope, account}` |
| POST | `/v1/auth/logout` | any | ends the session |
| POST | `/v1/auth/register` | - | `{username, password}`, only with `--open-registration` |
| GET | `/v1/me` | session | the logged-in account |
| POST | `/v1/me/password` | session | `{current_password, new_password}` |
| GET | `/v1/players` | read | online players with account ids and usernames |
| POST | `/v1/players/{account_id}/kick` | moderate | `{reason?}` |
| POST | `/v1/broadcast` | moderate | `{message}`: a chat line from "Server" to everyone |
| POST | `/v1/maintenance` | admin | `{enabled, message?}`: only GMs may log in |
| GET | `/v1/accounts?search=&offset=&limit=` | read | |
| POST | `/v1/accounts` | admin | `{username, password?, gm_level?}` |
| GET | `/v1/accounts/{id}` | read | with characters; `last_login_ip` for admins only |
| PATCH | `/v1/accounts/{id}` | admin | `{username?, gm_level?}` |
| DELETE | `/v1/accounts/{id}` | admin | kicks, then deletes with character and friendships |
| POST | `/v1/accounts/{id}/password` | admin | `{password}` (null removes it) |
| POST | `/v1/accounts/{id}/ban` | moderate | `{days?, reason?}`; kicks; staff need admin |
| POST | `/v1/accounts/{id}/unban` | moderate | |
| GET | `/v1/characters/{id}` | read | team, stars, upgrades, tokens |
| GET/POST | `/v1/api-keys` | admin | list / `{name, scope}` → secret once |
| DELETE | `/v1/api-keys/{id}` | admin | revoke |
| GET | `/v1/audit?limit=&before=` | admin | administrative actions, newest first |

\* public unless `--private-status`, then `read`.

Example (a Discord bot's status command):

```sh
curl -s https://lmo.example.org/api/v1/status
# {"game_server_online":true,"version":"0.1.0","started_at":1792828074,"players_online":3,...}
```
