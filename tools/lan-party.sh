#!/usr/bin/env bash
# LAN party: tools/lan-server.sh --release (game, patch server, REST API with open registration,
# --progression) plus the web UI (../lmoxide-webui, production build served by `vite preview`).
# Ctrl-C stops everything.
#   tools/lan-party.sh [--reset] [lan-ip]
# Accounts host and player1..player8, password "password", GM level 4 (admin: the highest anything
# checks: admin scope in the web UI, GM chat and test commands in game). Every other account
# gets GM level 4 in game too (--default-gm-level).
# --reset: delete every character except host's; the others start over at character creation.
# Env: LMOXIDE_DATA_DIR (as lan-server.sh), LMOXIDE_WEBUI_DIR (../lmoxide-webui), WEBUI_PORT (4173).
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
RESET=
if [[ ${1:-} == --reset ]]; then
  RESET=1
  shift
fi
DATA=${LMOXIDE_DATA_DIR:-$(realpath -m "$ROOT/../var/data")}
WEBUI=${LMOXIDE_WEBUI_DIR:-$(realpath -m "$ROOT/../lmoxide-webui")}
WEBUI_PORT=${WEBUI_PORT:-4173}
GM=4
PASSWORD=password
ACCOUNTS=(host player{1..8})

# Stop a LAN server still running on this data dir first: it saves its players' characters on
# the way out, which would undo the reset.
for pid in $(pgrep -x lmoxide-server || true); do
  if tr '\0' ' ' </proc/$pid/cmdline 2>/dev/null | grep -qF -- "--data-dir $DATA "; then
    kill "$pid" && while kill -0 "$pid" 2>/dev/null; do sleep 0.2; done
  fi
done

cargo build --manifest-path "$ROOT/Cargo.toml" -q --release -p lmoxide-server -p lmoxide-patch-server -p lmoxide-api
SERVER=$ROOT/target/release/lmoxide-server
mkdir -p "$DATA"
for name in "${ACCOUNTS[@]}"; do
  if ! echo "$PASSWORD" | "$SERVER" --data-dir "$DATA" account create "$name" --gm-level "$GM" >/dev/null 2>&1; then
    echo "$PASSWORD" | "$SERVER" --data-dir "$DATA" account passwd "$name" >/dev/null
    "$SERVER" --data-dir "$DATA" account gm "$name" "$GM" >/dev/null
  fi
done
echo "accounts ${ACCOUNTS[*]}: password \"$PASSWORD\", GM level $GM"

if [[ -n $RESET ]]; then
  n=$(sqlite3 "$DATA/lmoxide.db" "PRAGMA foreign_keys = ON;
    DELETE FROM characters WHERE account_id NOT IN (SELECT id FROM accounts WHERE username = 'host');
    SELECT changes();")
  echo "reset: deleted $n characters (kept host's)"
fi

if [[ ! -d $WEBUI ]]; then
  echo "no web UI at $WEBUI (LMOXIDE_WEBUI_DIR=): starting without it"
else
  [[ -d $WEBUI/node_modules ]] || pnpm -C "$WEBUI" install --frozen-lockfile
  mkdir -p "$ROOT/logs"
  pnpm -C "$WEBUI" -s build >"$ROOT/logs/lan-webui-build.log" 2>&1 || { cat "$ROOT/logs/lan-webui-build.log"; exit 1; }
  # preview serves dist/ and proxies /api/ to the API on :8090 (vite.config.ts)
  (cd "$WEBUI" && exec node_modules/.bin/vite preview --host 0.0.0.0 --port "$WEBUI_PORT" --strictPort) \
    >"$ROOT/logs/lan-webui.log" 2>&1 &
  WEB=$!
  trap 'kill $WEB 2>/dev/null' EXIT
  IP=${1:-$(ip -4 route get 1.1.1.1 | sed -nE 's/.* src ([0-9.]+).*/\1/p')}
  echo "web UI: http://$IP:$WEBUI_PORT (log logs/lan-webui.log)"
fi

# (the web UI is served built above; lan-server.sh adds the dev UI's dev server)
LMOXIDE_DATA_DIR=$DATA LMOXIDE_DEFAULT_GM_LEVEL=$GM LMOXIDE_WEB_APPS=${LMOXIDE_WEB_APPS-dev-ui:5174} \
  LMOXIDE_API_ARGS="--open-registration ${LMOXIDE_API_ARGS:-}" \
  "$ROOT/tools/lan-server.sh" --release "$@"
