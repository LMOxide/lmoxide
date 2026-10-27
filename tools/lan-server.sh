#!/usr/bin/env bash
# Run LMOxide for LAN play: lmoxide-server and lmoxide-patch-server (foreground; Ctrl-C stops
# both and saves the characters of players still online).
#   tools/lan-server.sh [--release] [lan-ip]   (default ip: this PC's address on the default route)
# --release: optimized build (target/release; faster, slower to compile). Logging is the same.
# Listens on all interfaces and tells clients to connect to <lan-ip>. Ports: TCP 8080 (patch
# server), 7100 (universe), 7101 (territory), 7102 (game), 7200 (chat), 8090 (REST API for the
# web UI and bots; LMOXIDE_API_PORT=, empty, leaves it off).
# LAN conveniences: --auto-register (the first login creates the account or sets the password of
# an account without one). Everyone can chat (the patched client shows the chat window without a
# GM level); test commands need one: LMOXIDE_DEFAULT_GM_LEVEL=1 gives it to every account.
# Progression is on (--progression: XP, levels and minifigure ranks, as live).
# The game server runs under tools/keep-listening.py: its ports stay open across restarts (a
# client pressing "relocate? Yes" meanwhile waits instead of failing), and a server that exits
# is started again; the API is started again too. Rebuild and restart both while this runs:
#   tools/lan-server.sh --restart [--release]
# Web apps: the dev servers (vite, live reload) of ../lmoxide-webui on :5173 and
# ../lmoxide-dev-ui on :5174 (debugger /debugger/, world editor /editor/; the editor reaches a
# survey server's API on :8091, tools/survey-server.sh), on all interfaces (this PC and others); their /api goes to
# this API, so they need no CORS setup. One whose port is taken is left as it is (already
# running). LMOXIDE_WEB_APPS="webui:5173 dev-ui:5174" picks them (empty: none).
# Map data the clients lack comes from Funcom's CDN through the patch server (LMOXIDE_UPSTREAM);
# LMOXIDE_LOCAL_CDN=1 uses the local mirror instead (tools/funcom-mirror.py, served on :8070 by
# tools/funcom-cdn.sh, started here unless already running). Clients see no difference.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
RESTART=0
if [[ ${1:-} == --restart ]]; then
  RESTART=1
  shift
fi
PROFILE=debug
if [[ ${1:-} == --release ]]; then
  PROFILE=release
  shift
fi
if (( RESTART )); then
  cargo build --manifest-path "$ROOT/Cargo.toml" -q -p lmoxide-server -p lmoxide-api $([[ $PROFILE == release ]] && echo --release)
  pid=$(cat "$ROOT/logs/lan-server.pid" 2>/dev/null) || { echo "no LAN game server running (logs/lan-server.pid)"; exit 1; }
  kill "$pid" && echo "game server $pid stopped; keep-listening starts the new build"
  if pid=$(cat "$ROOT/logs/lan-api.pid" 2>/dev/null) && kill "$pid" 2>/dev/null; then
    echo "API $pid stopped; it starts again with the new build"
  fi
  exit 0
fi
IP=${1:-$(ip -4 route get 1.1.1.1 | sed -nE 's/.* src ([0-9.]+).*/\1/p')}
VAR=$(realpath -m "$ROOT/../var")
DATA=${LMOXIDE_DATA_DIR:-$VAR/data}
CLIENT=${LMOXIDE_CLIENT_DIR:-$(realpath -m "$ROOT/../clients/windows/LEGO Minifigures Online")}
# The native Linux client for patchers (the `linux` manifest): the stock build, extracted once
# from aronwk's zip, plus the client kit's prebuilt libraries and launcher as overlay. Both live
# under clients/ (not synced by Dropbox). LMOXIDE_LINUX_CLIENT_DIR= turns Linux installs off.
CLIENTS=$(realpath -m "$ROOT/../clients")
LINUX_ZIP=$CLIENTS/downloads/LMO_Linux_v1.5.557666.zip
LINUX_CLIENT=${LMOXIDE_LINUX_CLIENT_DIR-$CLIENTS/linux-stock/LMO_Linux_v1.5.557666}
LINUX_ARGS=()
if [[ -n $LINUX_CLIENT ]]; then
  if [[ ! -x $LINUX_CLIENT/LMO && -f $LINUX_ZIP ]]; then
    echo "extracting the Linux client to $LINUX_CLIENT (once, 2.3 GB)..."
    mkdir -p "$(dirname "$LINUX_CLIENT")" && unzip -q -o "$LINUX_ZIP" -d "$(dirname "$LINUX_CLIENT")"
  fi
  if [[ -x $LINUX_CLIENT/LMO ]]; then
    OVERLAY=$CLIENTS/linux-overlay
    mkdir -p "$OVERLAY"
    cc -shared -fPIC -O2 -o "$OVERLAY/libgconf-2.so.4" "$ROOT/patches/linux-client/gconf-stub.c"
    cc -shared -fPIC -O2 -o "$OVERLAY/liblmofixes.so" "$ROOT/patches/linux-client/lmo-fixes.c" -lpthread
    cp "$ROOT/patches/linux-client/lmo" "$OVERLAY/lmo"
    LINUX_ARGS=(--linux-client-dir "$LINUX_CLIENT" --linux-overlay-dir "$OVERLAY")
  else
    echo "no Linux client (put LMO_Linux_v1.5.557666.zip in $CLIENTS/downloads/): patchers get Windows only"
  fi
fi
# the client kit, served at http://<ip>:8080/client-kit.tar.gz for other PCs (patches/linux-client/README.md)
KIT=$("$ROOT/patches/linux-client/make-kit.sh" | cut -d' ' -f1)
# a LAN server already running: its supervisors first (else they start the servers again)
[[ -s $ROOT/logs/lan-holder.pid ]] && { kill "$(cat "$ROOT/logs/lan-holder.pid")" 2>/dev/null || true; sleep 2; }
[[ -s $ROOT/logs/lan-api-loop.pid ]] && { kill "$(cat "$ROOT/logs/lan-api-loop.pid")" 2>/dev/null || true; }
API_PORT=${LMOXIDE_API_PORT-8090}
# only the servers on the LAN ports (other checkouts or sessions may run their own on side ports)
lan_pids() { ss -ltnpH "( sport = :8080 or sport = :7100 or sport = :7101 or sport = :7102 or sport = :7200 or sport = :7300${API_PORT:+ or sport = :$API_PORT} )" | grep -oE 'pid=[0-9]+' | cut -d= -f2 | sort -u; }
for p in $(lan_pids); do kill "$p" 2>/dev/null || true; done
for _ in $(seq 30); do [[ -z $(lan_pids) ]] && break; sleep 0.2; done
cargo build --manifest-path "$ROOT/Cargo.toml" -q -p lmoxide-server -p lmoxide-patch-server -p lmoxide-api $([[ $PROFILE == release ]] && echo --release)
BIN=$ROOT/target/$PROFILE
mkdir -p "$ROOT/logs"
echo "LAN server ($PROFILE build) on $IP. Install or update the client on another PC:"
echo "  d=\$(mktemp -d) && curl -fsSL http://$IP:8080/client-kit.tar.gz | tar xz -C \$d && \$d/*/install.sh --server $IP"
echo "logs: $ROOT/logs/lan-server.log, $ROOT/logs/lan-patch-server.log"

export RUST_LOG=${RUST_LOG:-info}
CDN=
UPSTREAM=${LMOXIDE_UPSTREAM-http://update.lego.funcom.com/legoupm}
if [[ -n ${LMOXIDE_LOCAL_CDN:-} ]]; then
  [[ -d $CLIENTS/funcom-cdn/legoupm ]] || { echo "no CDN mirror in $CLIENTS/funcom-cdn: run tools/funcom-mirror.py"; exit 1; }
  UPSTREAM=http://127.0.0.1:8070/legoupm
  if [[ -z $(ss -ltnH "sport = :8070") ]]; then
    "$ROOT/tools/funcom-cdn.sh" >"$ROOT/logs/funcom-cdn.log" 2>&1 &
    CDN=$!
  fi
  echo "map data from the local Funcom CDN mirror: $UPSTREAM (log logs/funcom-cdn.log)"
fi
"$BIN/lmoxide-patch-server" --bind 0.0.0.0 --public-host "$IP" --client-dir "$CLIENT" \
  --data-dir "$VAR/patch-data" --login-public-key-file "$DATA/login-public-key.txt" \
  --upstream "$UPSTREAM" --cache-dir "$VAR/rdb-cache" \
  --client-kit "$KIT" "${LINUX_ARGS[@]}" ${LMOXIDE_PATCH_ARGS:-} >"$ROOT/logs/lan-patch-server.log" 2>&1 &
PATCH=$!
# the API reaches the game server's control interface (127.0.0.1:7300) with a shared token
CONTROL_ARGS=()
if [[ -n $API_PORT ]]; then
  mkdir -p "$DATA"
  [[ -s $DATA/control_token ]] || { head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' >"$DATA/control_token"; chmod 600 "$DATA/control_token"; }
  CONTROL_ARGS=(--control-token-file "$DATA/control_token")
  # after the game server opened (and migrated) the database: at the same time it's locked
  rm -f "$ROOT/logs/lan-server.log"
  # started again when it exits (--restart stops it after a build)
  (
    until grep -qs "control interface listening" "$ROOT/logs/lan-server.log"; do sleep 1; done
    while true; do
      "$BIN/lmoxide-api" --data-dir "$DATA" --bind "0.0.0.0:$API_PORT" --control-url http://127.0.0.1:7300 \
        --control-token-file "$DATA/control_token" --client-dir "$CLIENT" ${LMOXIDE_API_ARGS:-} &
      echo $! >"$ROOT/logs/lan-api.pid"
      wait $! || true
      sleep 1
    done
  ) >"$ROOT/logs/lan-api.log" 2>&1 &
  API=$!
  echo $API >"$ROOT/logs/lan-api-loop.pid"
  echo "REST API: http://$IP:$API_PORT (log logs/lan-api.log)"
fi
# the web apps' dev servers (see the top)
WEB=()
if [[ -n $API_PORT ]]; then
  for app in ${LMOXIDE_WEB_APPS-webui:5173 dev-ui:5174}; do
    name=${app%:*} port=${app#*:} dir=$(realpath -m "$ROOT/../lmoxide-${app%:*}")
    if [[ ! -f $dir/package.json ]]; then
      echo "$name: not at $dir, skipped"
    elif [[ -n $(ss -ltnH "sport = :$port") ]]; then
      echo "$name: port $port already in use (running already?), left as it is"
    else
      [[ -d $dir/node_modules ]] || pnpm -C "$dir" install --frozen-lockfile >"$ROOT/logs/lan-$name.log" 2>&1
      (cd "$dir" && LMOXIDE_API=http://127.0.0.1:$API_PORT exec node_modules/.bin/vite --host 0.0.0.0 --port "$port" --strictPort) \
        >"$ROOT/logs/lan-$name.log" 2>&1 &
      WEB+=($!)
      echo "$name: http://$IP:$port (log logs/lan-$name.log)"
    fi
  done
fi
trap 'kill $PATCH ${CDN:-} ${API:-} ${WEB[*]} 2>/dev/null; [[ -s $ROOT/logs/lan-api.pid ]] && kill "$(cat "$ROOT/logs/lan-api.pid")" 2>/dev/null' EXIT
python3 "$ROOT/tools/keep-listening.py" --bind 0.0.0.0 --ports 7100,7101,7102,7200 --pidfile "$ROOT/logs/lan-server.pid" \
  --holder-pidfile "$ROOT/logs/lan-holder.pid" -- \
  "$BIN/lmoxide-server" --data-dir "$DATA" --bind 0.0.0.0 --public-host "$IP" "${CONTROL_ARGS[@]}" \
  --auto-register --default-gm-level "${LMOXIDE_DEFAULT_GM_LEVEL:-0}" --progression ${LMOXIDE_SERVER_ARGS:-} 2>&1 \
  | tee "$ROOT/logs/lan-server.log"
