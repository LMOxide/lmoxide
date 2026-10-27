#!/usr/bin/env bash
# A survey server (docs/SURVEY.md) next to the LAN server (tools/lan-server.sh): its own game
# server (--mode survey), patch server, REST API and survey web page, on its own ports, with
# its own characters (data in ../var/survey-data) and the survey set in ../var/survey-set.
#   tools/survey-server.sh [--release] [lan-ip]
#   tools/survey-server.sh --restart [--release]   (rebuild and restart the game server and API)
# Ports: TCP 8081 (patch server: clients connect with `lmo <name> <lan-ip>:8081`), 7110-7112
# (universe, territory, game), 7210 (chat), 8091 (REST API), 5175 (the dev UI's world editor:
# http://<lan-ip>:5175/editor/, from LMOXIDE_SURVEY_WEB_DIR, default ../lmoxide-dev-ui; the LAN
# server's dev UI on :5174 reaches this server's API too). The control interface is 127.0.0.1:7310.
# It shares the LAN server's login key and logins (--share-accounts ../var/data/lmoxide.db):
# clients patched for the LAN server log in here too, with the same usernames and passwords (the
# web page as well); characters are the survey server's own; every account is a GM (test commands, teleports)
# and is created on its first login. `!server play` in game (or the dev UI's switch) takes a player to
# the LAN server's game with their character there, `!server editor` there back here. Every account may use the world editor (the API gives logins
# the admin scope from GM level 0): keep it to the LAN.
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
  pid=$(cat "$ROOT/logs/survey-server.pid" 2>/dev/null) || { echo "no survey server running (logs/survey-server.pid)"; exit 1; }
  kill "$pid" && echo "survey game server $pid stopped; keep-listening starts the new build"
  if pid=$(cat "$ROOT/logs/survey-api.pid" 2>/dev/null) && kill "$pid" 2>/dev/null; then
    echo "survey API $pid stopped; it starts again with the new build"
  fi
  exit 0
fi
IP=${1:-$(ip -4 route get 1.1.1.1 | sed -nE 's/.* src ([0-9.]+).*/\1/p')}
# the main checkout's folder, also from a git worktree of it (var/, clients/ and the web apps are next to it)
MAIN=$(cd "$(git -C "$ROOT" rev-parse --path-format=absolute --git-common-dir)/.." && pwd)
VAR=$(realpath -m "$MAIN/../var")
DATA=${LMOXIDE_SURVEY_DATA_DIR:-$VAR/survey-data}
SET=${LMOXIDE_SURVEY_SET:-$VAR/survey-set}
# the content the world editor edits: a git checkout of lmoxide-content (docs/WORLD-EDITOR-CONTENT.md),
# made from content/ the first time, its origin the content repository
WORK=${LMOXIDE_CONTENT_WORK:-$VAR/content-work}
if [[ ! -d $WORK/.git ]]; then
  git clone -q "$MAIN/content" "$WORK"
  git -C "$WORK" remote set-url origin "$(git -C "$MAIN/content" remote get-url origin)"
  echo "content working copy: $WORK (cloned from content/)"
fi
SHARED_DB=${LMOXIDE_SHARE_ACCOUNTS-$VAR/data/lmoxide.db}
CLIENT=${LMOXIDE_CLIENT_DIR:-$(realpath -m "$MAIN/../clients/windows/LEGO Minifigures Online")}
WEB_DIR=${LMOXIDE_SURVEY_WEB_DIR:-$(realpath -m "$MAIN/../lmoxide-dev-ui")}
# the native Linux client for patchers, as the LAN server offers it (tools/lan-server.sh builds the overlay)
CLIENTS=$(realpath -m "$MAIN/../clients")
LINUX_CLIENT=${LMOXIDE_LINUX_CLIENT_DIR-$CLIENTS/linux-stock/LMO_Linux_v1.5.557666}
LINUX_ARGS=()
if [[ -n $LINUX_CLIENT && -x $LINUX_CLIENT/LMO && -f $CLIENTS/linux-overlay/liblmofixes.so ]]; then
  LINUX_ARGS=(--linux-client-dir "$LINUX_CLIENT" --linux-overlay-dir "$CLIENTS/linux-overlay")
else
  echo "no Linux client (run tools/lan-server.sh once): patchers get Windows only"
fi
HTTP=8081 UNIVERSE=7110 TERRITORY=7111 GAME=7112 CHAT=7210 CONTROL=7310 API_PORT=8091 WEB_PORT=5175
# a survey server already running: its supervisors first (else they start the servers again)
[[ -s $ROOT/logs/survey-holder.pid ]] && { kill "$(cat "$ROOT/logs/survey-holder.pid")" 2>/dev/null || true; sleep 2; }
[[ -s $ROOT/logs/survey-api-loop.pid ]] && { kill "$(cat "$ROOT/logs/survey-api-loop.pid")" 2>/dev/null || true; }
survey_pids() { ss -ltnpH "( sport = :$HTTP or sport = :$UNIVERSE or sport = :$TERRITORY or sport = :$GAME or sport = :$CHAT or sport = :$CONTROL or sport = :$API_PORT or sport = :$WEB_PORT )" | grep -oE 'pid=[0-9]+' | cut -d= -f2 | sort -u; }
for p in $(survey_pids); do kill "$p" 2>/dev/null || true; done
for _ in $(seq 30); do [[ -z $(survey_pids) ]] && break; sleep 0.2; done
cargo build --manifest-path "$ROOT/Cargo.toml" -q -p lmoxide-server -p lmoxide-patch-server -p lmoxide-api $([[ $PROFILE == release ]] && echo --release)
BIN=$ROOT/target/$PROFILE
mkdir -p "$ROOT/logs" "$DATA"
export RUST_LOG=${RUST_LOG:-info}
[[ -s $DATA/control_token ]] || { head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' >"$DATA/control_token"; chmod 600 "$DATA/control_token"; }
SHARE=()
if [[ -n $SHARED_DB && -f $SHARED_DB ]]; then
  SHARE=(--share-accounts "$SHARED_DB")
else
  echo "no $SHARED_DB: the survey server has its own login key and accounts (clients need patching for it)"
fi
# the LAN patch server's manifest key: patchers trust both alike (one install, two server profiles)
mkdir -p "$VAR/patch-data-survey"
[[ -f $VAR/patch-data/manifest-signing.key ]] && ! cmp -s "$VAR/patch-data/manifest-signing.key" "$VAR/patch-data-survey/manifest-signing.key" \
  && cp "$VAR/patch-data/manifest-signing.key" "$VAR/patch-data-survey/manifest-signing.key"
rm -f "$ROOT/logs/survey-server.log"
python3 "$ROOT/tools/keep-listening.py" --bind 0.0.0.0 --ports "$UNIVERSE,$TERRITORY,$GAME,$CHAT" \
  --pidfile "$ROOT/logs/survey-server.pid" --holder-pidfile "$ROOT/logs/survey-holder.pid" -- \
  "$BIN/lmoxide-server" --mode survey --data-dir "$DATA" --survey-set "$SET" --content-dir "$WORK" --bind 0.0.0.0 --public-host "$IP" \
  --universe-port $UNIVERSE --territory-port $TERRITORY --game-port $GAME --chat-port $CHAT \
  --control-bind 127.0.0.1:$CONTROL --control-token-file "$DATA/control_token" --auto-register "${SHARE[@]}" \
  --peer-name play --peer-data-dir "$VAR/data" --peer-game-addr "$IP:7102" --peer-chat-addr "$IP:7200" \
  ${LMOXIDE_SERVER_ARGS:-} >"$ROOT/logs/survey-server.log" 2>&1 &
SERVER=$!
# the patch server serves the login key the game server wrote
for _ in $(seq 150); do grep -qs "control interface listening" "$ROOT/logs/survey-server.log" && break; sleep 0.2; done
"$BIN/lmoxide-patch-server" --bind 0.0.0.0 --public-host "$IP" --http-port $HTTP --universe-port $UNIVERSE \
  --client-dir "$CLIENT" --data-dir "$VAR/patch-data-survey" --login-public-key-file "$DATA/login-public-key.txt" \
  --upstream "${LMOXIDE_UPSTREAM-http://update.lego.funcom.com/legoupm}" --cache-dir "$VAR/rdb-cache" \
  "${LINUX_ARGS[@]}" --server-name "LMOxide world editor" --server-kind world-editor --other-server "http://$IP:8080" \
  ${LMOXIDE_PATCH_ARGS:-} >"$ROOT/logs/survey-patch-server.log" 2>&1 &
PATCH=$!
(
  while true; do
    "$BIN/lmoxide-api" --data-dir "$DATA" --bind "0.0.0.0:$API_PORT" --control-url http://127.0.0.1:$CONTROL \
      --control-token-file "$DATA/control_token" --client-dir "$CLIENT" --admin-gm-level 0 --moderator-gm-level 0 ${LMOXIDE_API_ARGS:-} &
    echo $! >"$ROOT/logs/survey-api.pid"
    wait $! || true
    sleep 1
  done
) >"$ROOT/logs/survey-api.log" 2>&1 &
API=$!
echo $API >"$ROOT/logs/survey-api-loop.pid"
WEB=
if [[ -f $WEB_DIR/package.json ]]; then
  [[ -d $WEB_DIR/node_modules ]] || pnpm -C "$WEB_DIR" install --frozen-lockfile >"$ROOT/logs/survey-web.log" 2>&1
  (cd "$WEB_DIR" && LMOXIDE_EDITOR_API=http://127.0.0.1:$API_PORT exec node_modules/.bin/vite --host 0.0.0.0 --port $WEB_PORT --strictPort) \
    >"$ROOT/logs/survey-web.log" 2>&1 &
  WEB=$!
fi
echo "survey server ($PROFILE build) on $IP; set: $SET"
echo "  join:   lmo <name> $IP:$HTTP   (every account is a GM; a new name makes a new survey character)"
echo "  editor: http://$IP:$WEB_PORT/editor/   (sign in with that account; the lmo launcher's password is x)"
echo "  logs:   logs/survey-server.log, logs/survey-api.log, logs/survey-patch-server.log, logs/survey-web.log"
trap 'kill $PATCH $API ${WEB:-} 2>/dev/null; [[ -s $ROOT/logs/survey-api.pid ]] && kill "$(cat "$ROOT/logs/survey-api.pid")" 2>/dev/null' EXIT
wait $SERVER
