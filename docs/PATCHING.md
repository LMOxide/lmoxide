# Patching: `lmoxide-patch-server` and `lmoxide-patcher`

`lmoxide-patch-server` serves everything a client needs over HTTP before it connects to the game
server:

- the **stock client endpoints** (what Funcom's launcher and the in-client patcher use, see
  [PROTOCOL.md](PROTOCOL.md) "Boot sequence" and "HTTP patch layout"): `/control/…`, `/patch/…`,
  `/track`, `/rest/chat_filter`, `/auth/`, `/client-kit.tar.gz`;
- the **LMOxide patcher protocol** under `/lmoxide/v1/`: signed manifests of complete Windows
  and Linux installs, already patched for this server, so `lmoxide-patcher` (or a GUI built on
  the `lmoxide-patch` crate) only has to sync files.

Crates: `lmoxide-patch` (protocol types, signing, hash cache, client sync engine),
`lmoxide-patch-server` (binary), `lmoxide-patcher` (CLI).

## Server-side patching

Patchers only copy files; the server prepares them. In `LMO.exe` (Windows) and `LMO` (Linux),
same length and in place:

1. **Login public key.** Funcom's `session` key (`lmoxide_net::login::FUNCOM_SESSION_KEY`, 512
   hex characters) is replaced with the game server's key from `--login-public-key-file`. A
   binary that already carries some other key is recognised too (the one 512-hex-digit string
   next to the Diffie-Hellman `prime`), so stock and previously patched binaries both work.
2. **Chat filter URL.** `http://chatfilter.funcom.com/rest/%s` becomes
   `http://<chat-filter-host>/rest/%s`, NUL-padded, exactly as `patches/patch-client.py` does
   (default host: the public HTTP address; at most 21 characters for `host:port`).

The patched executables, the generated `LocalConfig.xml` (identical to
`/control/lmo_live/LocalConfig.xml`) and, for Linux installs with the kit's `lmo` launcher, an
`lmo-kit.conf` are written to `<data-dir>/blobs/<sha256>` and listed in the manifests. The stock
`/patch` endpoints describe and serve the same patched `LMO.exe` (`FileList.bin` is rebuilt from
the served files), so the in-client patcher doesn't undo an LMOxide install, and a stock client
picks up the patched executable through its own patcher.

The login key file is polled (`--key-poll-secs`, default 10): when the game server writes a new
key, binaries are re-patched and manifests re-signed. Without the file the binaries keep Funcom's
key (`/lmoxide/v1/info` lists a warning, and so does the log); clients can't then log in to an
LMOxide game server.

## Install sources

Per platform: a client dir, an optional overlay dir and a remove list.

| | Windows | Linux |
|---|---|---|
| client dir | `--client-dir` (also serves the stock endpoints) | `--linux-client-dir` (stock `LMO_Linux_v1.5.557666`) |
| overlay | `--windows-overlay-dir` | `--linux-overlay-dir`: the client kit's prebuilt `libgconf-2.so.4`, `liblmofixes.so` and the `lmo` launcher (`make-kit.sh` builds them into `prebuilt/`) |
| remove | `--windows-remove` (default none) | `--linux-remove` (default `libstdc++.so.6`, which `apply.sh` moves aside) |

Rules, in order:

1. Every regular file of the client dir (symlinked directories are not followed).
2. `<file>.orig` replaces `<file>` when both exist: `patch-client.py`, `apply.sh` and the dev
   tools keep the stock copy there. `--no-orig` turns this off.
3. `--exclude` globs drop files (a pattern without `/` matches a file name anywhere). The default
   list drops logs, crash dumps, `*.orig`, `*.bundled`, `OldLogs/**`, `LocalConfig.xml`,
   `lmo-kit.conf`, the uninstaller and `.lmoxide/**`. Setting `--exclude` replaces the list.
4. Overlay files are added on top, replacing same-named files.
5. Remove-list paths are dropped and listed in the manifest's `remove` (patchers delete them).
6. Patched executable, `LocalConfig.xml`, `lmo-kit.conf` as above.

Hashes of client files are cached in `<data-dir>/hash-cache.json` by absolute path, size and
mtime: the first start hashes ~3.6 GB (both clients; seconds to minutes depending on the disk,
`--hash-jobs` threads), later starts take well under a second. The stock endpoints are up
immediately; `/lmoxide/v1/manifest/*` answers `503` with `Retry-After` until hashing is done.
After changing a client or overlay dir, send the server `SIGHUP` (or restart it) to rescan.

## Protocol (`/lmoxide/v1/`)

| Route | Answer |
|---|---|
| `GET /lmoxide/v1/info` | JSON `Info`: `protocol` (1), `name`, `universe_addr`, `http_addr`, `https_addr`, `platforms`, `ready` (platforms whose manifest is built), `manifest_key` (ed25519, hex), `login_public_key` (hex or null), `warnings`, `revisions` (platform → manifest revision, see below), `install_bytes` (platform → size of a complete install), `api_url` (the public lmoxide-api base URL, or null), `website` (or null), `patchers` (patcher name → platform → newest version offered), `kind` (`progression`, `adventure`, `world-editor` or another word; null: not said), `other_servers` (patch server URLs of the other game servers on this host, so patchers can offer to switch). The newer fields may be missing from older servers. |
| `GET /lmoxide/v1/manifest/{windows\|linux}` | JSON `SignedManifest` (below), with `ETag: "<revision>"`; `If-None-Match` with the installed manifest's revision answers `304` while nothing changed; `404` if the platform isn't configured, `503` + `Retry-After` while indexing |
| `GET /lmoxide/v1/files/{sha256}` | file body by lower-case sha256, streamed from disk; supports `Range` (`206`), `Cache-Control: immutable`; `404` for anything not in a current manifest |
| `GET /lmoxide/v1/patchers/{windows\|linux}` | JSON list of `SignedRelease` (below): the newest build of each patcher for that platform, `[]` if none; `404` without `--patcher-dir` |
| `GET /lmoxide/v1/login-key` | the game server's login public key, `text/plain`, 512 hex characters + newline (read from the key file on each request; for hand-patched clients: `patch-client.py --login-key-url`); `404` without `--login-public-key-file`, `503` while the file is missing or invalid |

### Manifest

```json
{
  "format": 1,
  "server": "LMOxide",
  "platform": "linux",
  "version": "<sha256 over the file list>",
  "generated_at": 1792828800,
  "login_public_key": "<512 hex> or null",
  "universe_addr": "lmo.example.org:7100",
  "files": [
    { "path": "LMO", "size": 55465880, "sha256": "54e8…", "executable": true },
    { "path": "LocalConfig.xml", "size": 859, "sha256": "8cef…", "executable": false }
  ],
  "remove": ["libstdc++.so.6"],
  "launch": {
    "executable": "LMO",
    "args": ["-verified"],
    "login_args": ["-username", "{username}", "-password", "{password}"],
    "env": { "LD_LIBRARY_PATH": ".", "LD_PRELOAD": "liblmofixes.so", "SDL_VIDEODRIVER": "x11" }
  }
}
```

- `files` is sorted by path and lists the complete install; paths are relative and
  `/`-separated, never absolute, never with empty, `.` or `..` components, `\`, `:` or control
  characters, and never under `.lmoxide/` (patchers reject such manifests even if signed).
  Windows manifests never contain two paths differing only in case.
- `version` = sha256 of the concatenated lines `path \0 sha256 \0 size \0 executable(0|1) \n`.
- Files not in the manifest are left alone by patchers (logs, settings), except `remove`.

A manifest's **revision** (`Manifest::revision`) is the sha256 of its JSON with `generated_at`
set to 0: it changes with any file, the launch info or the address, but not when the server
merely rebuilds the same manifest. Patchers check for updates cheaply by comparing `revisions`
in `info` (or by `If-None-Match`) with the revision of their saved manifest, and only then fetch
and verify the signed manifest, which stays authoritative.

### Patcher releases (self-update)

With `--patcher-dir`, the server hands out builds of the patchers themselves. Files in that
directory named `<name>-<version>-<platform>[.exe]` (e.g. `lmoxide-patcher-0.2.0-linux`,
`lmoxide-patcher-gui-0.2.0-windows.exe`; the version is dotted numbers) are hashed and the
newest of each name and platform is listed as a `Release` `{format, name, version, platform,
size, sha256, generated_at}` inside a `SignedRelease` `{format, key, signature, release}`
envelope. The body is downloaded from `/lmoxide/v1/files/{sha256}`. Releases are signed with
the manifest key over the context `lmoxide-release-v1\0` (a manifest signature can't pass for
a release), so a patcher accepts one only if it is signed by the key an install already
trusts. Drop a new build in the directory and send SIGHUP to publish it.

Updating is optional and up to the patcher: `lmoxide-patcher self-update --server URL`
(trusting the key of `--dir`, default `~/Games/LMO`, or `--trust`) replaces the running
binary; the GUI offers it when the server has a newer `lmoxide-patcher-gui` (Settings can turn
the check off). The new build is downloaded next to the program, checked against the signed
sha256 and renamed over it (on Windows the old one moves to `<exe>.old`, removed at the next
start), so a failed update leaves the old build working.

### Signature

```json
{ "format": 1, "key": "<ed25519 public key, 64 hex>", "signature": "<128 hex>", "manifest": "<the manifest JSON, as a string>" }
```

The signature is ed25519 (`verify_strict`) over the bytes `lmoxide-manifest-v1\0` followed by the
UTF-8 bytes of the `manifest` string, so verifiers never re-serialize JSON. The `key` field is
only a claim: patchers check the signature against a key they trust (pinned or remembered) and
fail if `key` differs from it. Integrity of file bodies follows from the signed sha256, so plain
HTTP is fine; the optional HTTPS port only adds privacy.

The signing key is generated on first start as `<data-dir>/manifest-signing.key` (hex seed,
mode 0600) and logged as `manifest signing key (patchers pin this)`. Keep the data dir: a new
key makes every installed patcher refuse the server until users accept it with `--trust`.

## Running the patch server

```sh
lmoxide-patch-server \
  --bind 0.0.0.0 --http-port 8080 --public-host lmo.example.org \
  --universe-port 7100 \
  --client-dir "/srv/lmo/windows/LEGO Minifigures Online" \
  --linux-client-dir "/srv/lmo/linux/LMO_Linux_v1.5.557666" \
  --linux-overlay-dir /srv/lmo/linux-overlay \
  --login-public-key-file /srv/lmoxide/game/login-public-key.txt \
  --data-dir /srv/lmoxide/patch
```

Every flag has an `LMOXIDE_…` environment variable (`--help` shows them):

| Flag | Env | Default |
|---|---|---|
| `--bind` | `LMOXIDE_BIND` | `127.0.0.1` |
| `--http-port` | `LMOXIDE_HTTP_PORT` | `8080` |
| `--public-host` | `LMOXIDE_PUBLIC_HOST` | `127.0.0.1` |
| `--public-http-port` | `LMOXIDE_PUBLIC_HTTP_PORT` | `--http-port` |
| `--universe-host` | `LMOXIDE_UNIVERSE_HOST` | `--public-host` |
| `--universe-port` | `LMOXIDE_UNIVERSE_PORT` | `7100` |
| `--chat-filter-host` | `LMOXIDE_CHAT_FILTER_HOST` | public HTTP `host:port` |
| `--https-port` | `LMOXIDE_HTTPS_PORT` | off (needs `--tls-cert`, `--tls-key`) |
| `--public-https-port` | `LMOXIDE_PUBLIC_HTTPS_PORT` | `--https-port` |
| `--tls-cert`, `--tls-key` | `LMOXIDE_TLS_CERT`, `LMOXIDE_TLS_KEY` | PEM files |
| `--server-name` | `LMOXIDE_SERVER_NAME` | `LMOxide` |
| `--api-url` | `LMOXIDE_PUBLIC_API_URL` | none (announced in `info` for patchers' status line) |
| `--website` | `LMOXIDE_WEBSITE` | none (announced in `info`) |
| `--server-kind` | `LMOXIDE_SERVER_KIND` | none (announced in `info` as `kind`) |
| `--other-server <url>` (repeatable) | `LMOXIDE_OTHER_SERVERS` (comma-separated) | none (announced in `info` as `other_servers`) |
| `--patcher-dir` | `LMOXIDE_PATCHER_DIR` | none (no patcher self-updates) |
| `--client-dir` | `LMOXIDE_CLIENT_DIR` | (Windows client) |
| `--linux-client-dir` | `LMOXIDE_LINUX_CLIENT_DIR` | |
| `--windows-overlay-dir`, `--linux-overlay-dir` | `LMOXIDE_WINDOWS_OVERLAY_DIR`, `LMOXIDE_LINUX_OVERLAY_DIR` | |
| `--windows-remove`, `--linux-remove` | `LMOXIDE_WINDOWS_REMOVE`, `LMOXIDE_LINUX_REMOVE` | none, `libstdc++.so.6` (comma-separated) |
| `--exclude` | `LMOXIDE_EXCLUDE` | see above (comma-separated) |
| `--no-orig` | `LMOXIDE_NO_ORIG` | off |
| `--login-public-key-file` | `LMOXIDE_LOGIN_PUBLIC_KEY_FILE` | unset: unpatched |
| `--key-poll-secs` | `LMOXIDE_KEY_POLL_SECS` | `10` |
| `--data-dir` | `LMOXIDE_DATA_DIR` | `lmoxide-patch-data` |
| `--hash-jobs` | `LMOXIDE_HASH_JOBS` | `4` |
| `--upstream` | `LMOXIDE_UPSTREAM` | off (e.g. `http://update.lego.funcom.com/legoupm`) |
| `--cache-dir` | `LMOXIDE_CACHE_DIR` | `<data-dir>/rdb-cache` |
| `--no-filter-download-list` | `LMOXIDE_NO_FILTER_DOWNLOAD_LIST` | off |
| `--client-kit` | `LMOXIDE_CLIENT_KIT` | off |

Logging uses `tracing`; set `RUST_LOG` (default `info,lmoxide_patch_server=debug`). Client dirs
can be mounted read-only; only the data dir is written. All routes are GET; request bodies over
16 KiB are refused; there are no directory listings, and `/patch` paths with `..`, `\` or `:`
are rejected.

### Local mirror of Funcom's CDN

`tools/funcom-mirror.py` downloads everything Funcom's patch CDN still serves into
`../clients/funcom-cdn/legoupm/`, byte for byte at the same paths: the files `PatchInfoClient.txt`
names (`RDBHashIndex.bin`, all 20 platform download lists), every platform's `FileList.bin` and
the client files in them, and every resource in the index (84,978; the Android/iOS/Mac lists add
none). Bodies are checked against the md5 in their URL; reruns resume. The Windows and Mac
`PatcherSetupHash` files are already gone (404). `tools/funcom-cdn.sh` serves the folder, so the
patch server can run without Funcom:

```sh
LMOXIDE_LOCAL_CDN=1 tools/lan-server.sh --release         # starts tools/funcom-cdn.sh on :8070
# or by hand, for any patch server:
tools/funcom-cdn.sh &                                     # http://127.0.0.1:8070/legoupm
LMOXIDE_UPSTREAM=http://127.0.0.1:8070/legoupm tools/dev-run.sh
```

Only the patch server talks to the CDN: clients get their patch address (`HttpPatchAddr`) from it
and ask it for missing resources while playing, so the upstream is a server setting; installs made
by the patcher don't change.

Note: the Linux kit's `lmo` launcher fetches `LocalConfig.xml` from `http://$LMO_SERVER:8080`
on every start, so with a public HTTP port other than 8080 use a launcher that doesn't (the
manifest already installs `LocalConfig.xml`).

## Running the patcher

```sh
lmoxide-patcher install --server http://lmo.example.org:8080 --dir ~/Games/LMO   # also: update
lmoxide-patcher verify  --server http://lmo.example.org:8080 --dir ~/Games/LMO [--full]
lmoxide-patcher info    --server http://lmo.example.org:8080
lmoxide-patcher self-update --server http://lmo.example.org:8080
```

Options: `--platform windows|linux` (default: this OS), `--dir` (default `~/Games/LMO`),
`--trust <key>` (pin the manifest key instead of trust-on-first-use), `--jobs N` (parallel
downloads, default 4), `LMOXIDE_SERVER` instead of `--server`. `verify` exits 1 when the install
differs (`--full` rehashes everything instead of trusting size+mtime).

The patcher keeps its state in `<dir>/.lmoxide/`: `server-key` (the trusted key, written on
first use or by `--trust`), `hash-cache.json`, `manifest.json` (the last verified signed
manifest, with launch info) and `partial/` (interrupted downloads, resumed with `Range`). Files
are downloaded there, checked against the signed sha256 and then renamed into place, so an
interrupted update never leaves a half-written file, and nothing outside the install dir is
written (symlinked directories inside it are refused).

Other patchers (such as a GUI) use the same library, `lmoxide-patch`: `client::Client` (HTTP,
`signed_manifest_if_changed`), `client::Trust`, `sync::{plan, apply, sync}` with
`sync::Options::cancel` (an `AtomicBool` that stops hashing and downloads with
`Error::Cancelled`), `sync::load_manifest` (the saved manifest, verified with the stored key),
`Launch::command` (the client's start command, optionally wrapped, e.g. by `wine`),
`default_install_dir()`, `Error::Unreachable` for servers that can't be reached, and
`selfupdate::{check, install, cleanup}` for updating the patcher itself.
