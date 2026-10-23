#!/usr/bin/env bash
# Restart lmo-server and the client (under Wine) for a test run.
#   tools/dev-run.sh <client-dir> [log-dir]
# <client-dir> is an extracted "LEGO Minifigures Online" folder (with RDB/).
set -euo pipefail
CLIENT=${1:?usage: dev-run.sh <client-dir> [log-dir]}
LOGS=${2:-/tmp}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
export WINEPREFIX=${WINEPREFIX:-$HOME/.local/share/lmo-wine}
# Fetch files the archived client lacks from Funcom's patch server (set LMO_UPSTREAM= to disable).
export LMO_UPSTREAM=${LMO_UPSTREAM-http://update.lego.funcom.com/legoupm}
export LMO_CACHE_DIR=${LMO_CACHE_DIR:-$(realpath -m "$ROOT/../rdb-cache")}

pkill -x LMO.exe || true
pkill -x lmo-server || true
wineserver -k 2>/dev/null || true
sleep 1

cargo build --manifest-path "$ROOT/Cargo.toml" -q
RUST_LOG=${RUST_LOG:-info,lmo_server=debug} \
  "$ROOT/target/debug/lmo-server" --client-dir "$CLIENT" ${LMO_SERVER_ARGS:-} >"$LOGS/server.log" 2>&1 &
for _ in $(seq 50); do curl -sf http://127.0.0.1:8080/track >/dev/null && break; sleep 0.2; done

# The launcher normally fetches this; point the client at our control server.
curl -sf http://127.0.0.1:8080/control/lmo_live/LocalConfig.xml >"$CLIENT/LocalConfig.xml"
rm -f "$CLIENT/ClientLog.txt"
# LMO_AUTOPLAY=1 swaps in tools/client/startup.lua (scripted login flow); otherwise restore stock.
if [[ ! -f "$CLIENT/Data/startup.lua.orig" ]]; then cp "$CLIENT/Data/startup.lua" "$CLIENT/Data/startup.lua.orig"; fi
if [[ -n ${LMO_AUTOPLAY:-} ]]; then
  cp "$ROOT/tools/client/startup.lua" "$CLIENT/Data/startup.lua"
else
  cp "$CLIENT/Data/startup.lua.orig" "$CLIENT/Data/startup.lua"
fi
(cd "$CLIENT" && WINEDEBUG=-all wine LMO.exe -verified ${LMO_CLIENT_ARGS:-} >"$LOGS/wine.log" 2>&1 &)
echo "server log: $LOGS/server.log, client log: $CLIENT/ClientLog.txt"
