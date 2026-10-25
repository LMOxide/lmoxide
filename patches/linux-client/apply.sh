#!/usr/bin/env bash
# The client patch kit: makes an extracted stock native Linux client
# (LMO_Linux_v1.5.557666.zip from lmo.files.aronwk.com) run today and play on an lmoxide-server.
# Idempotent: every run starts again from the verified stock files.
#   apply.sh <client-dir> [--server <ip>]     (default server: the one applied before, else 127.0.0.1)
#
# What it changes (README.md):
#   - libstdc++.so.6 (2016, too old for today's system libraries) -> libstdc++.so.6.bundled
#   - libgconf-2.so.4: a stub (libcef.so needs it; distributions no longer ship it)
#   - liblmofixes.so: client bug fixes, preloaded by the launcher (lmo-fixes.c)
#   - LMO: the chat filter URL and the login key are the server's (patch-client.py; stock copy
#     kept as LMO.orig); the key comes from http://<server>:8080/lmoxide/v1/login-key; private
#     messages work again (DoTell); Data/Gui/Default/Modules.xml shows chat to everyone
#   - lmo: the launcher; lmo-kit.conf: kit version and server
set -euo pipefail
KIT=$(cd "$(dirname "$0")" && pwd)
STOCK_LMO_SHA256=2f09831aff5587e7dee889f4c0e2aebd4eb80ca67fa5716b6d5fc8821784e7ad

CLIENT=${1:?usage: apply.sh <client-dir> [--server <ip>]}
shift
SERVER=
while [[ $# -gt 0 ]]; do
  case $1 in
    --server) SERVER=${2:?--server needs an address}; shift 2 ;;
    *) echo "unknown option $1" >&2; exit 2 ;;
  esac
done
CLIENT=$(cd "$CLIENT" && pwd)
cd "$CLIENT"
[[ -f lmo-kit.conf ]] && source ./lmo-kit.conf
SERVER=${SERVER:-${LMO_SERVER:-127.0.0.1}}
VERSION=$(cat "$KIT/KIT_VERSION")

step() { printf '  %s\n' "$*"; }
echo "lmo client kit v$VERSION -> $CLIENT (server $SERVER)"

# the executable, from the stock copy
stock=LMO
[[ -f LMO.orig ]] && stock=LMO.orig
if [[ $(sha256sum "$stock" | cut -d' ' -f1) != "$STOCK_LMO_SHA256" ]]; then
  echo "$CLIENT/$stock is not the stock 1.5.557666 Linux client; not patching" >&2
  exit 1
fi
[[ $stock == LMO.orig ]] && cp LMO.orig LMO
chmod +x LMO Launcher DWWebHelper

if [[ -f libstdc++.so.6 ]]; then
  mv libstdc++.so.6 libstdc++.so.6.bundled
  step "moved the bundled libstdc++.so.6 aside"
fi

# prebuilt libraries ship in the kit tarball (make-kit.sh); in the repo they're compiled here
for lib in libgconf-2.so.4:gconf-stub.c liblmofixes.so:lmo-fixes.c; do
  so=${lib%%:*} src=${lib#*:}
  if [[ -f $KIT/prebuilt/$so ]]; then
    cp "$KIT/prebuilt/$so" "$so"
  else
    command -v cc >/dev/null || { echo "need a C compiler (gcc) to build $so" >&2; exit 1; }
    cc -shared -fPIC -O2 -o "$so" "$KIT/$src" -lpthread
  fi
  step "installed $so"
done

patcher=$KIT/patch-client.py
[[ -f $patcher ]] || patcher=$KIT/../patch-client.py
python3 "$patcher" --exe "$PWD/LMO" --host "$SERVER:8080" \
  --login-key-url "http://$SERVER:8080/lmoxide/v1/login-key" >/dev/null \
  || { echo "can't patch LMO (is lmoxide-patch-server running on $SERVER:8080?)" >&2; exit 1; }
step "chat filter and login key -> $SERVER"

# leftovers of the developer test harness (tools/dev-run.sh LMO_AUTOPLAY=1)
if [[ -f Data/startup.lua.orig ]]; then
  cp Data/startup.lua.orig Data/startup.lua
fi

install -m 755 "$KIT/lmo" lmo
cat >lmo-kit.conf <<EOF
# written by the lmo client kit (apply.sh)
LMO_KIT_VERSION=$VERSION
LMO_SERVER=$SERVER
LMO_KIT_APPLIED="$(date -Iseconds)"
EOF
step "launcher: $CLIENT/lmo <username>"
