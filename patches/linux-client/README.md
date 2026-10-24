# LMO native Linux client kit

Installs and patches the native Linux LEGO Minifigures Online client (1.5.557666, the last live
build) to play on an lmoxide-server. The kit never ships the client itself: it downloads the stock
archive from lmo.files.aronwk.com, checks it against a known checksum, and applies its changes
to that, so every install is the same and can be re-done at any time.

## Install

On a PC with the repo:

```sh
patches/linux-client/install.sh --server 192.168.5.69      # the server PC's LAN address
~/Games/LMO/lmo <username>
```

On another PC, from a running server (`tools/lan-server.sh` builds the kit and the server serves
it at `/client-kit.tar.gz`; the same command updates an install later):

```sh
d=$(mktemp -d) && curl -fsSL http://192.168.5.69:8080/client-kit.tar.gz | tar xz -C $d && $d/*/install.sh --server 192.168.5.69
~/Games/LMO/lmo <username>
```

Or with the kit tarball (`patches/linux-client/make-kit.sh` builds
`dist/lmo-client-kit-v<N>.tar.gz`, about 12 KB; no repo or compiler needed there):

```sh
tar xzf lmo-client-kit-v1.tar.gz
lmo-client-kit-v1/install.sh --server 192.168.5.69
~/Games/LMO/lmo <username>
```

Options: `--dir <install dir>` (default `~/Games/LMO`), `--zip <file>` (an already downloaded
`LMO_Linux_v1.5.557666.zip`; otherwise it is downloaded once to `~/.cache/lmo-client/`).
Needs `curl`, `unzip`, `python3`; the installer lists missing system libraries.

Any username works and creates its own character; there is no password.

## Update

Run `install.sh` again (from a newer repo or kit tarball): an existing client is kept and the
kit applied again from the stock files. `lmo-kit.conf` in the client folder records the kit
version and server. To point a client at another server: `apply.sh <client dir> --server <ip>`.

## What the kit changes (`apply.sh`)

| Change | Why |
|---|---|
| `libstdc++.so.6` → `libstdc++.so.6.bundled` | The bundled 2016 C++ library is older than what today's system libraries (loaded through `LD_LIBRARY_PATH=.`) need; the system one is backward compatible. |
| `libgconf-2.so.4` (stub, `gconf-stub.c`) | `libcef.so` (the embedded browser) links GNOME 2's config library, which distributions dropped; CEF only asks it for proxy settings. |
| `liblmofixes.so` (`lmo-fixes.c`), preloaded by `lmo` | Client bug fix: its animation name table (`MCore::StringIDGenerator`) is filled from several threads without a lock and corrupts memory (crashes while loading worlds with many monster types). |
| `LMO`: chat filter URL (`patch-client.py`; stock copy `LMO.orig`) | Chat lines are only sent after a hard-coded Funcom filter URL approves them; it now asks the server. |
| `lmo` launcher, `lmo-kit.conf` | Fetches `LocalConfig.xml` (the server's addresses) like Funcom's launcher did, forces SDL's X11 driver (the 2016 renderer needs GLX; Wayland/EGL crashes) and preloads the fixes. |

Nothing else in the client is modified. Bump `KIT_VERSION` when a change is added.
