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
- **GM levels are per account** (`account gm`). The test commands (`!tp`, `!goto`, ...) need
  `--devcmd-gm-level` (default 1). `--default-gm-level` gives everyone a level: only for LAN play.
- **`--auto-register`** lets anyone create an account by logging in. Leave it off on a public
  server and create accounts with `lmoxide-server account create` or the API (optionally with
  `--open-registration`).

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
  their SHA-256 is stored. Keys have a scope (`read` < `moderate` < `admin`); a login session's
  scope follows the account's GM level (`--moderator-gm-level`, `--admin-gm-level`). Moderators
  can't kick or ban staff. A password change ends the account's sessions.
- **Throttling**: a token bucket per address (10 requests/s, bursts of 60), and the same login
  lockout as the game. Registration (off by default) is limited per address.
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
- Publish only the game ports, the patch server and (behind TLS) the API; never 7300.
- Generate the control token with `openssl rand -hex 32` into `secrets/control_token`.
- Back up the `data` volume (the SQLite database holds accounts, characters and the login key).
- Create an admin account (`account create <name> --gm-level 2`) instead of using `--default-gm-level`.
