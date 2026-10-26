#!/usr/bin/env bash
# A local backend for developing the web UI or the Discord bot: lmoxide-server + lmoxide-api on
# localhost with their own database, demo accounts, and CORS open to local dev servers.
# No game client needed. Ctrl-C stops both.
#   tools/web-dev-backend.sh [--reset]
# --reset: start over with a fresh database.
# Env: WEB_ORIGINS (default the usual Vite/Next/CRA dev URLs), API_PORT (8090),
#      DATA (default ../var/web-dev, outside the repo).
#
# Demo accounts (password = "<name>-password"):
#   admin (GM 2, admin scope), moderator (GM 1, moderate scope), player (no scope),
#   banned (banned for 7 days)
# API keys for bots, created once and printed: demo-bot (read), demo-mod-bot (moderate).
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
DATA=${DATA:-$(realpath -m "$ROOT/../var/web-dev")}
API_PORT=${API_PORT:-8090}
WEB_ORIGINS=${WEB_ORIGINS:-http://localhost:5173,http://127.0.0.1:5173,http://localhost:3000,http://127.0.0.1:3000,http://localhost:4321}
# ports clear of a LAN server on the defaults
GAME_PORTS=(--universe-port 27100 --territory-port 27101 --game-port 27102 --chat-port 27200)
CONTROL=127.0.0.1:27300
# the client's RDB, for the sounds of effect packages (GET /v1/catalog/sounds/{id}), if it's there
CLIENT=${LMOXIDE_CLIENT_DIR:-$(realpath -m "$ROOT/../clients/windows/LEGO Minifigures Online")}
CLIENT_ARGS=()
[[ -f $CLIENT/RDB/RDBHashIndex.bin ]] && CLIENT_ARGS=(--client-dir "$CLIENT")

[[ ${1:-} == --reset ]] && rm -rf "$DATA"
mkdir -p "$DATA"
cargo build --manifest-path "$ROOT/Cargo.toml" -q -p lmoxide-server -p lmoxide-api
SERVER=$ROOT/target/debug/lmoxide-server
API=$ROOT/target/debug/lmoxide-api
[[ -f $DATA/control_token ]] || { head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' >"$DATA/control_token"; chmod 600 "$DATA/control_token"; }

if [[ ! -f $DATA/lmoxide.db ]]; then
  echo "creating demo accounts"
  for a in admin:2 moderator:1 player:0 banned:0; do
    name=${a%:*}
    echo "$name-password" | "$SERVER" --data-dir "$DATA" account create "$name" --gm-level "${a#*:}" 2>/dev/null
  done
  "$SERVER" --data-dir "$DATA" account ban banned --days 7 --reason "demo ban" 2>/dev/null >/dev/null
  "$API" --data-dir "$DATA" key create demo-bot --scope read 2>/dev/null | tee "$DATA/demo-bot-key.txt"
  "$API" --data-dir "$DATA" key create demo-mod-bot --scope moderate 2>/dev/null | tee "$DATA/demo-mod-bot-key.txt"
fi

export RUST_LOG=${RUST_LOG:-info}
"$SERVER" --data-dir "$DATA" "${GAME_PORTS[@]}" --control-bind "$CONTROL" \
  --control-token-file "$DATA/control_token" >"$DATA/server.log" 2>&1 &
GAME=$!
trap 'kill $GAME 2>/dev/null' EXIT
echo
echo "API:      http://127.0.0.1:$API_PORT  (OpenAPI: /v1/openapi.json, also docs/openapi.json)"
echo "Accounts: admin / moderator / player / banned, password <name>-password"
echo "Bot keys: read $(grep -o 'lmox_[0-9a-f]*' "$DATA/demo-bot-key.txt" 2>/dev/null || echo "(see $DATA)")"
echo "          moderate $(grep -o 'lmox_[0-9a-f]*' "$DATA/demo-mod-bot-key.txt" 2>/dev/null || echo "(see $DATA; --reset to create)")"
echo "CORS:     $WEB_ORIGINS"
echo "logs:     $DATA/server.log (game), API below"
echo
"$API" --data-dir "$DATA" --bind "127.0.0.1:$API_PORT" --control-url "http://$CONTROL" \
  --control-token-file "$DATA/control_token" --cors-origins "$WEB_ORIGINS" --open-registration "${CLIENT_ARGS[@]}"
