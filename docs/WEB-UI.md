# Building the web UI

Context for the LMOxide web UI, which lives in its own repository and talks only to
`lmoxide-api` over HTTP. The API is developed here; if the UI needs something the API lacks,
change it in this repo (and regenerate `docs/openapi.json`).

## What to read

| | |
|---|---|
| [`docs/openapi.json`](openapi.json) | the contract: every route, request and response schema (OpenAPI 3.1). Generate a typed client from it (e.g. `openapi-typescript`, `orval`, `openapi-fetch`). A test keeps it in sync with the code. |
| [`docs/API.md`](API.md) | authentication, scopes, route table, error format |
| [`docs/SECURITY.md`](SECURITY.md) | "The REST API": tokens, rate limits, CORS, what is public |

## A backend to develop against

From this repo (needs Rust and `protoc`; no game client):

```sh
tools/web-dev-backend.sh          # --reset for a fresh database
```

It serves the API on `http://127.0.0.1:8090` with its own database (`../var/web-dev`), demo
accounts and CORS open to `localhost:5173`, `:3000` and `:4321` (`WEB_ORIGINS=...` to change):

| Login | Password | Scope |
|---|---|---|
| `admin` | `admin-password` | `admin` (GM level 2) |
| `moderator` | `moderator-password` | `moderate` (GM level 1) |
| `player` | `player-password` | none: only `/v1/me` |
| `banned` | `banned-password` | can't log in |

It also prints a `read`-scope API key (`demo-bot`) and has `--open-registration` on. Nobody is
online unless a game client connects, so `/v1/online` and `/v1/players` return `[]`; kicks answer
`{"affected": 0}`.

## Authentication flow

1. `POST /v1/auth/login` `{username, password}` → `{token, expires_at, scope, account}`.
   Wrong credentials and banned accounts both get 401 `bad_credentials`; 429 `rate_limited`
   after 10 failures in 15 minutes.
2. Send `Authorization: Bearer <token>` on every call. Tokens last 24 h by default (`expires_at`
   is unix seconds); afterwards calls answer 401 `unauthorized`: send the user to login.
3. `POST /v1/auth/logout` ends it. Changing the password (`POST /v1/me/password`) ends all of
   the account's sessions.

There are no cookies: keep the token in memory or `sessionStorage` (and avoid third-party
scripts on the admin pages, since any script on the page could read it).

## What to show, by scope

Each scope includes the ones before it; the server enforces all of this, the UI just hides what
would answer 403.

| Scope | Pages |
|---|---|
| public (no login) | server status (`/v1/status`), who's online (`/v1/online`), registration if enabled |
| none (players) | own account (`/v1/me`), change password |
| `read` | players online with accounts (`/v1/players`), account search and details (`/v1/accounts`, `/v1/accounts/{id}`), characters (`/v1/characters/{id}`) |
| `moderate` | kick, ban (days, reason), unban, broadcast a chat message |
| `admin` | create/rename/delete accounts, set passwords and GM levels, maintenance mode, API keys (secret shown once at creation), audit log, **world content** (below) |

**World content** (admin; docs/WORLD-CONTENT.md, routes `/v1/content/...`, not in openapi.json):
a queue of what world editors changed (`GET /v1/content/queue`: per playfield, feature and
author, with a one-line summary and each record's details), with Approve / Reject for a whole
change or ticked records; the rules (`GET/PUT /v1/content/rules`): which features are live on the
server (`active`) and which are approved without asking, by feature/category (`auto_features`,
e.g. `live` = "mimic live") or by author (`auto_authors`); pick features from
`GET /v1/content/features` (show `path`, e.g. "New content > Christmas Event > 2026"); and a
summary of the whole approved (and working) content (`GET /v1/content/summary?of=approved|working`).
Also Preview on a waiting change (`POST /v1/content/preview`: in the game for everyone for a
while) and the history of approvals (`GET /v1/content/history`) with each entry's `actions`
(undo, redo, keep, revert: `POST /v1/content/history/{seq}/{action}`).
`GET /v1/content/state` gives the badge count (`pending`) and the source folder's error, if any.

Moderators get 403 when they kick or ban staff (accounts with a GM level); only admins can.

## Data notes

- Times are unix seconds (UTC). Ids are u32 (API key ids i64).
- Playfields are numeric ids; `GET /v1/playfields` maps them to names (public, cache it).
- `GET /v1/config` (always public): whether registration is open, and the GM levels from which
  sessions get the moderate and admin scopes.
- `ban` on an account is the ban in force (`until: null` = permanent), absent otherwise.
- `last_login_ip` is only filled in for admins.
- Character `team`, `upgrades` and `minifigure_ranks` use minifigure item template ids:
  `GET /v1/minifigures` has their names, attack names and upgrade names by bit; `GET /v1/tokens`
  names the token ids of `tokens`. No images yet.
- `GET /v1/accounts` sends the number of matches in `X-Total-Count`.
- Errors are always `{"error": {"code", "message"}}`; codes: `unauthorized`, `bad_credentials`,
  `forbidden`, `not_found`, `invalid`, `conflict`, `rate_limited`, `registration_closed`,
  `game_server_unavailable` (502, game server down: show the status as offline), `no_control`
  (503, the API runs without a game server connection).
- `GET /v1/status` with `game_server_online: false` and the other fields `null` means the game
  server is down; the account pages still work (they read the database).

## Deployment

The UI is static files or its own server; either way the API has to allow its origin:
`LMOXIDE_API_CORS_ORIGINS=https://admin.example.org` (see `docker-compose.yml`, service `api`).
Serving both from one domain behind a reverse proxy (UI at `/`, API at `/api/` → `api:8090`)
avoids CORS entirely; then set `LMOXIDE_API_TRUST_FORWARDED_FOR=true`. The API must sit behind
TLS in production.
