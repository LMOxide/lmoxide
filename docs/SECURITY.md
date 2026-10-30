# Security

What protects an LMOxide server that is reachable from the internet, and what can't be
protected because the 2016 client doesn't allow it.

## The game protocol (ports 7100-7102, 7200)

The client speaks Funcom's binary RPC protocol over plain TCP, and IRC to the chat server. It has
no transport encryption and we can't add any without rewriting the client, so:

- **Game traffic is unencrypted.** Someone on the network path can read chat and game state and
  could inject traffic into a session. Use a VPN (WireGuard, Tailscale) between players and the
  server when that matters.
- **Passwords are encrypted.** The client encrypts the login (`user|challenge|game id|password`)
  with Diffie-Hellman (2048-bit group, compiled into the client) and TEA, against the server's
  public key (`lmoxide_net::login`). Funcom's private key is lost, so every LMOxide server makes
  its own key pair (`lmoxide-server login-key`; the private half stays in the database) and the
  patch server writes the public half into the client binaries it serves. A client patched for
  another server can't log in. The challenge is random per login, so a recorded answer can't be
  replayed.
- **Passwords are stored as argon2id hashes.** Only `account create`/`account passwd`, the API,
  or `--auto-register` set them.
- **Session cookies are random.** After a password check the universe agent hands out a random
  32-bit cookie tied to the account (`game_sessions`, 24 h, replaced by the next login). The
  territory, game and chat servers accept a client only with its account's live cookie.
  (Before LMOxide the cookie was derived from the account id, so anyone could log in as anyone.)
- **Throttling.** 10 failed logins (or forged cookies) per address or username within 15 minutes
  lock it out for the rest of that window; at most `--max-connections-per-ip` connections per
  address and listener; frames over 1 MiB and chat lines over 8 KiB close the connection; a
  connection must authenticate within 30 s.
- **Bans and maintenance** are checked at login and again by the game server; a ban through the
  API also kicks the player and ends their sessions.
- **GM levels are per account** (`account gm`) and ordered: see "Access levels" below.
  `--default-gm-level` gives everyone a level: only for LAN play.
- **`--auto-register`** lets anyone create an account by logging in. Leave it off on a public
  server and create accounts with `lmoxide-server account create` or the API (optionally with
  `--registration open`, or `discord` so only the Discord bot registers members of its server).

## Access levels

An account's GM level decides what it may do, in game and through the API. The defaults:

| Level | Who | What |
|---|---|---|
| 0 | player | plays (unless `--play-gm-level` is 1), its own account in the web UI |
| 1 | alpha tester | the Progression and Maxxed modes even with `--play-gm-level 1`; the testers' test commands, in chat or from the dev UI's debugger on their own character: teleporting (`!tp`, `!goto`, `!move`, ...), test scenarios, `!own` any minifigure, `!dialog`, achievements (`!ach`, `!count`, `!defeat`), `!anim`, `!skin`, `!state` and the Quick section (`!revive`, `!smash`, `!xp`, `!stars`, puzzles) |
| 2 | contributor | the World Editor mode and its API routes; every test command; the client's full GM stat (god camera, flying and the other GM hotkeys) |
| 3 | moderator | the API's `moderate` scope: kick, ban, broadcast, instances |
| 4 | admin | the `admin` scope: accounts, keys, the audit log, commands on other players |

The thresholds are options: game server `--tester-gm-level` (1), `--world-editor-gm-level` (2),
`--devcmd-gm-level` (2), `--client-gm-level` (2), `--play-gm-level` (0); API
`--editor-gm-level` (2, `lmoxide` takes the game's), `--moderator-gm-level` (3),
`--admin-gm-level` (4). The testers' commands are devcmd.rs `TESTER_COMMANDS`;
`GET /v1/catalog/test-commands` gives each command's level. The Discord bot sets levels from
Discord roles (its `gm_roles`: Alpha Tester 1, Contributor 2, Admin 4).

The client's GmLevel stat is a bitmask, not this ladder (docs/PROTOCOL.md "Client hotkey GM
gates"): accounts from `--client-gm-level` get every bit, others none. Migration 0015 moved
older databases' levels onto this ladder (1 → 3, 2 and up → 4).

## The control interface (port 7300)

The game server's internal API for lmoxide-api. It is bound to 127.0.0.1 by default (in Docker,
to the private compose network, never published) and requires a shared bearer token of at least
32 characters (`--control-token` / `--control-token-file`, compared in constant time). Without a
token it doesn't start.

## The REST API (lmoxide-api)

- **TLS**: built in (`--tls-cert`/`--tls-key`, rustls) or behind a TLS-terminating reverse proxy
  (then `--trust-forwarded-for` so rate limits see client addresses). It warns when it serves
  plain HTTP on a non-loopback address.
- **Credentials**: API keys and login session tokens are 256-bit random secrets (`lmox_...`); only
  their SHA-256 is stored. Keys have a scope (`read` < `moderate` < `discord` < `admin`); a login
  session's scope follows the account's GM level (`--moderator-gm-level`, `--admin-gm-level`), and
  so does an account key's (the keys the Discord bot holds for linked accounts; they stop working
  while the account is banned or once it is unlinked). Moderators can't kick or ban staff. A
  password change ends the account's sessions.
- **Discord codes**: the one-time codes the bot sends in DMs (link an account, sign in to reset
  a password) are 8 characters from a 31-letter alphabet, last 10 minutes, work once, and only
  their SHA-256 is stored; wrong ones count as failed logins. A code sign-in may set a new
  password without the old one, so whoever controls the linked Discord account controls the
  game account.
- **Throttling**: a token bucket per address (10 requests/s, bursts of 60), and the same login
  lockout as the game. Registration (`--registration`, `closed` by default) is limited per address.
- **Hardening**: 64 KiB request bodies, 30 s timeouts, `Cache-Control: no-store`, CORS only for
  `--cors-origins`. Every administrative action goes to the audit log (`GET /v1/audit`), with the
  `X-On-Behalf-Of` a bot sends for the person behind a command (the bot's claim).
- `/v1/status` and `/v1/online` are public (in-game names only, no usernames or account ids)
  unless `--private-status`.

## The patch server (lmoxide-patch-server)

Serves client files to the stock client's patcher (plain HTTP: that's what it speaks) and to
LMOxide patchers. LMOxide manifests are signed with the patch server's ed25519 key, so patchers
detect tampering even over plain HTTP; HTTPS can be enabled as well. See
[PATCHING.md](PATCHING.md).

## Deployment checklist

- Run the containers as provided (non-root user, read-only client mounts).
- Publish only the game ports and Caddy's 80/443 (HTTPS for the API and the patch server). In
  `lmoxide` the control interface listens on a loopback port with a token made at every start;
  running the programs apart, never publish 7300 and generate the control token with
  `openssl rand -hex 32` into `secrets/control_token`.
- Back up the `data` volume (the SQLite database holds accounts, characters and the login key).
- Create an admin account (`account create <name> --gm-level 4`) instead of using `--default-gm-level`.
- During an alpha, `play-gm-level = 1` keeps the game to alpha testers and staff.
