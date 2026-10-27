#!/usr/bin/env bash
# Restart lmoxide-server, lmoxide-patch-server and the client (under Wine) for a test run.
#   tools/dev-run.sh <client-dir> [log-dir]
# <client-dir> is an extracted "LEGO Minifigures Online" folder (with RDB/).
# LMOXIDE_LAN_SERVER=1: start only the client, against the running LAN server (tools/lan-server.sh,
# standard ports), so others on the LAN can join the same test world. LMOXIDE_HTTP_PORT=8081: the
# survey server next to it (tools/survey-server.sh) instead.
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
# LMOXIDE_SIDE_PORTS=1: ports 17100-17102, chat 17200, control 17300, HTTP 18080, so a test run
# doesn't collide with a LAN server (tools/lan-server.sh) on the standard ports.
HTTP=${LMOXIDE_HTTP_PORT:-8080}
# The patch server deletes blobs its own manifests don't use, so a test run never shares the LAN
# server's (tools/lan-server.sh) var/patch-data: side-port runs get their own.
PATCH_DATA=$VAR/patch-data
SERVER_PORTS=()
PATCH_PORTS=()
# LMOXIDE_SIDE_PORTS=2: a second set (27100-27102, 27200, 27300, HTTP 28080) for when another
# session's test run holds the first.
if [[ -n ${LMOXIDE_SIDE_PORTS:-} ]]; then
  S=$([[ $LMOXIDE_SIDE_PORTS == 2 ]] && echo 2 || echo 1)
  HTTP=${S}8080
  SERVER_PORTS=(--universe-port ${S}7100 --territory-port ${S}7101 --game-port ${S}7102 --chat-port ${S}7200 --control-bind 127.0.0.1:${S}7300)
  PATCH_PORTS=(--http-port ${S}8080 --universe-port ${S}7100)
  PATCH_DATA=$VAR/patch-data-side$([[ $S == 2 ]] && echo 2 || true)
fi

# the test client only: the native Linux client is also "LMO" and may be the user's own
pkill -x LMO.exe || true
# (SIGKILL: the Linux client ignores SIGTERM)
for p in $(pgrep -x LMO); do [[ $(readlink "/proc/$p/cwd") == "$(realpath "$CLIENT")" ]] && kill -9 "$p"; done
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
[[ -z ${LMOXIDE_LAN_SERVER:-} ]] && stop_servers
wineserver -k 2>/dev/null || true
WINEPREFIX=${LMO_PROTON_PREFIX:-$HOME/.local/share/lmo-proton}/pfx \
  "${PROTON:-$HOME/.local/share/Steam/steamapps/common/Proton - Experimental}/files/bin/wineserver" -k 2>/dev/null || true
sleep 1

if [[ -n ${LMOXIDE_LAN_SERVER:-} ]]; then
  curl -sf http://127.0.0.1:$HTTP/lmoxide/v1/login-key >/dev/null || { echo "no LAN server on :$HTTP (tools/lan-server.sh)" >&2; exit 1; }
else
cargo build --manifest-path "$ROOT/Cargo.toml" -q -p lmoxide-server -p lmoxide-patch-server
# Development accounts: created by their first login, and GMs (free-text chat, test commands).
# LMOXIDE_KEEP_LISTENING=1: under tools/keep-listening.py (ports kept open, the server started
# again when it exits; kill the pid in logs/dev-server.pid to restart it)
KEEP=()
if [[ -n ${LMOXIDE_KEEP_LISTENING:-} ]]; then
  ports=$([[ -n ${LMOXIDE_SIDE_PORTS:-} ]] && echo ${S}7100,${S}7101,${S}7102,${S}7200 || echo 7100,7101,7102,7200)
  KEEP=(python3 "$ROOT/tools/keep-listening.py" --ports "$ports" --pidfile "$LOGS/dev-server.pid" --)
fi
RUST_LOG=${RUST_LOG:-info,lmoxide_server=debug} \
  "${KEEP[@]}" "$ROOT/target/debug/lmoxide-server" --data-dir "$DATA" --auto-register --default-gm-level "${LMOXIDE_DEFAULT_GM_LEVEL:-1}" \
  "${SERVER_PORTS[@]}" ${LMOXIDE_SERVER_ARGS:-} >"$LOGS/server.log" 2>&1 &
echo $! >>"$PIDS"
# The patch server patches the login key into the LMO.exe it serves (and derives PatchVersion
# from it): start it once the game server has written the key, else the client's patcher sees
# a different exe and tries to "repair" it.
for _ in $(seq 100); do [[ -s $DATA/login-public-key.txt ]] && break; sleep 0.2; done
# the Windows client tree, even when running the Linux client (its LocalConfig.xml is the same)
PATCH_CLIENT=${LMOXIDE_PATCH_CLIENT_DIR:-$([[ ${LMO_RUNNER:-wine} == linux ]] && realpath -m "$ROOT/../clients/windows/LEGO Minifigures Online" || echo "$CLIENT")}
RUST_LOG=${RUST_LOG:-info} "$ROOT/target/debug/lmoxide-patch-server" --client-dir "$PATCH_CLIENT" \
  --data-dir "$PATCH_DATA" --login-public-key-file "$DATA/login-public-key.txt" \
  --upstream "$UPSTREAM" --cache-dir "$VAR/rdb-cache" "${PATCH_PORTS[@]}" ${LMOXIDE_PATCH_ARGS:-} >"$LOGS/patch-server.log" 2>&1 &
echo $! >>"$PIDS"
for _ in $(seq 100); do curl -sf http://127.0.0.1:$HTTP/lmoxide/v1/login-key >/dev/null && break; sleep 0.2; done
fi

# The client must carry the server's login key (and chat filter URL).
EXE=LMO.exe
[[ ${LMO_RUNNER:-wine} == linux ]] && EXE=LMO
python3 "$ROOT/patches/patch-client.py" --exe "$CLIENT/$EXE" --host 127.0.0.1:$HTTP \
  --login-key-url http://127.0.0.1:$HTTP/lmoxide/v1/login-key >/dev/null

# The launcher normally fetches this; point the client at our control server.
curl -sf http://127.0.0.1:$HTTP/control/lmo_live/LocalConfig.xml >"$CLIENT/LocalConfig.xml"
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
