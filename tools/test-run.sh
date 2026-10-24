#!/usr/bin/env bash
# One test cycle: restart server + client, wait, print a condensed summary.
#   tools/test-run.sh [seconds=50]
# Env: LMO_AUTOPLAY=1 (scripted EnterGame via tools/client/startup.lua), LMO_CLIENT_DIR (default ../clients/windows/LEGO Minifigures Online),
#      LMO_CLIENT_ARGS (default auto-login as tester), LMO_SERVER_ARGS, KEEP=1 (leave running).
# Full logs: logs/server.log, logs/client.log (copied from the client's ClientLog.txt).
set -uo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WAIT=${1:-50}
CLIENT=${LMO_CLIENT_DIR:-$(cd "$ROOT/../clients/windows/LEGO Minifigures Online" && pwd)}
export LMO_CLIENT_ARGS=${LMO_CLIENT_ARGS--username tester -password test}
LOGS=$ROOT/logs
mkdir -p "$LOGS"

"$ROOT/tools/dev-run.sh" "$CLIENT" "$LOGS" >/dev/null || exit 1
sleep "$WAIT"
cp -f "$CLIENT/ClientLog.txt" "$LOGS/client.log" 2>/dev/null

strip() { sed 's/\x1b\[[0-9;]*m//g' "$1"; }
echo "== server: connections, RPCs, warnings"
strip "$LOGS/server.log" \
  | grep -E 'connected|disconnected|<- |-> |WARN|ERROR|error' \
  | grep -vE 'RDB resource not found|patch file not found' \
  | sed -E 's/^[0-9T:.-]+Z +//; s/caller=ProtoEndpoint \{[^}]*\} //' \
  | cut -c1-260 | awk '{ if ($0 == last) { n++ } else { if (n) print "   (repeated " n "x)"; print; n = 0 } last = $0 }'
echo "   404s: $(strip "$LOGS/server.log" | grep -cE 'not found') (rdb/client files)"

echo "== client: patcher, network, errors"
grep -E 'CorePatcher changing|FlowControl - State|<AUTOPLAY>|Network\.GameClient|Universe\.|FATAL|Exception|Assert' "$LOGS/client.log" 2>/dev/null \
  | grep -vE 'UniversalConfig|LoginStateWaitingToEnterGame' \
  | sed -E 's/^\[[0-9-]+ ([0-9:]+)\.[0-9]+ #[0-9]+\] \[SGID:[0-9]+; TID:[0-9]+\] /\1 /' \
  | cut -c1-220 | tail -25

# Screenshot of the game window (works on hidden workspaces via the foreign-toplevel id).
if command -v hyprctl >/dev/null && command -v grim >/dev/null; then
  id=$(hyprctl clients -j | python3 -c 'import json,sys; print(next((c["stableId"] for c in json.load(sys.stdin) if c["class"].lower() in ("lmo.exe","lmo") or c["title"].startswith("LEGO Minifigures")), ""))')
  [[ -n $id ]] && grim -T "$id" "$LOGS/screen.png" && echo "== screenshot: $LOGS/screen.png"
fi

if [[ -z ${KEEP:-} ]]; then
  pkill -x LMO.exe; pkill -x LMO; pkill -x lmo-server
  WINEPREFIX=${WINEPREFIX:-$HOME/.local/share/lmo-wine} wineserver -k 2>/dev/null
fi
true
