#!/usr/bin/env bash
# Restart lmoxide-server, lmoxide-patch-server and the client (under Wine) for a test run.
#   tools/dev-run.sh <client-dir> [log-dir]
# <client-dir> is an extracted "LEGO Minifigures Online" folder (with RDB/).
set -euo pipefail
CLIENT=${1:?usage: dev-run.sh <client-dir> [log-dir]}
LOGS=${2:-/tmp}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
export WINEPREFIX=${WINEPREFIX:-$HOME/.local/share/lmo-wine}
VAR=$(realpath -m "$ROOT/../var")
# Fetch files the archived client lacks from Funcom's patch server (set LMOXIDE_UPSTREAM= to disable).
UPSTREAM=${LMOXIDE_UPSTREAM-http://update.lego.funcom.com/legoupm}
# The database with accounts and characters (outside the repo).
DATA=${LMOXIDE_DATA_DIR:-$VAR/data}

pkill -x LMO.exe || true
pkill -x LMO || true
# Only the servers an earlier dev-run started (other sessions may run their own lmoxide-server).
PIDS=$LOGS/dev-run.pids
stop_servers() {
  [[ -f $PIDS ]] || return 0
  local pids; pids=$(cat "$PIDS"); rm -f "$PIDS"
  kill $pids 2>/dev/null || true
  # a server that doesn't stop (stuck) is killed after 3 s
  for _ in $(seq 30); do kill -0 $pids 2>/dev/null || return 0; sleep 0.1; done
  kill -9 $pids 2>/dev/null || true
}
stop_servers
wineserver -k 2>/dev/null || true
WINEPREFIX=${LMO_PROTON_PREFIX:-$HOME/.local/share/lmo-proton}/pfx \
  "${PROTON:-$HOME/.local/share/Steam/steamapps/common/Proton - Experimental}/files/bin/wineserver" -k 2>/dev/null || true
sleep 1

cargo build --manifest-path "$ROOT/Cargo.toml" -q -p lmoxide-server -p lmoxide-patch-server
# Development accounts: created by their first login, and GMs (free-text chat, test commands).
RUST_LOG=${RUST_LOG:-info,lmoxide_server=debug} \
  "$ROOT/target/debug/lmoxide-server" --data-dir "$DATA" --auto-register --default-gm-level "${LMOXIDE_DEFAULT_GM_LEVEL:-1}" \
  ${LMOXIDE_SERVER_ARGS:-} >"$LOGS/server.log" 2>&1 &
echo $! >>"$PIDS"
# The patch server patches the login key into the LMO.exe it serves (and derives PatchVersion
# from it): start it once the game server has written the key, else the client's patcher sees
# a different exe and tries to "repair" it.
for _ in $(seq 100); do [[ -s $DATA/login-public-key.txt ]] && break; sleep 0.2; done
# the Windows client tree, even when running the Linux client (its LocalConfig.xml is the same)
PATCH_CLIENT=${LMOXIDE_PATCH_CLIENT_DIR:-$([[ ${LMO_RUNNER:-wine} == linux ]] && realpath -m "$ROOT/../clients/windows/LEGO Minifigures Online" || echo "$CLIENT")}
RUST_LOG=${RUST_LOG:-info} "$ROOT/target/debug/lmoxide-patch-server" --client-dir "$PATCH_CLIENT" \
  --data-dir "$VAR/patch-data" --login-public-key-file "$DATA/login-public-key.txt" \
  --upstream "$UPSTREAM" --cache-dir "$VAR/rdb-cache" ${LMOXIDE_PATCH_ARGS:-} >"$LOGS/patch-server.log" 2>&1 &
echo $! >>"$PIDS"
for _ in $(seq 100); do curl -sf http://127.0.0.1:8080/lmoxide/v1/login-key >/dev/null && break; sleep 0.2; done

# The client must carry the server's login key (and chat filter URL).
EXE=LMO.exe
[[ ${LMO_RUNNER:-wine} == linux ]] && EXE=LMO
python3 "$ROOT/patches/patch-client.py" --exe "$CLIENT/$EXE" --host 127.0.0.1:8080 \
  --login-key-url http://127.0.0.1:8080/lmoxide/v1/login-key >/dev/null

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
case ${LMO_RUNNER:-wine} in
  wine) (cd "$CLIENT" && WINEDEBUG=-all wine LMO.exe -verified ${LMO_CLIENT_ARGS:-} >"$LOGS/wine.log" 2>&1 &) ;;
  # Valve's Proton outside the Steam client (own prefix in ~/.local/share/lmo-proton).
  proton) (cd "$CLIENT" && "$ROOT/tools/proton-run.sh" "$CLIENT/LMO.exe" -verified ${LMO_CLIENT_ARGS:-} >"$LOGS/wine.log" 2>&1 &) ;;
  # Native Linux build: <client-dir> is the extracted Linux client.
  linux) (cd "$CLIENT" && SDL_VIDEODRIVER=x11 LD_LIBRARY_PATH=. LD_PRELOAD=$([[ -f liblmofixes.so ]] && echo liblmofixes.so) ./LMO -verified ${LMO_CLIENT_ARGS:-} >"$LOGS/wine.log" 2>&1 &) ;;
  *) echo "unknown LMO_RUNNER=$LMO_RUNNER (wine|proton|linux)" >&2; exit 1 ;;
esac
echo "server log: $LOGS/server.log, client log: $CLIENT/ClientLog.txt"
