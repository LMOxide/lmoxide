#!/usr/bin/env bash
# Run a test command (devcmd.rs) as a logged-in player, as if typed in chat, through the game
# server's control interface. The player's GM level still applies.
#   tools/dev-command.sh <account name> '!loot potion'
# LMOXIDE_CONTROL (default http://127.0.0.1:7300) and LMOXIDE_CONTROL_TOKEN_FILE (default
# ../var/data/control_token, as tools/lan-server.sh writes it) pick the server.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
[[ $# -eq 2 ]] || { echo "usage: $0 <account name> '<!command>'" >&2; exit 2; }
CONTROL=${LMOXIDE_CONTROL:-http://127.0.0.1:7300}
TOKEN=$(cat "${LMOXIDE_CONTROL_TOKEN_FILE:-$ROOT/../var/data/control_token}")
account=$(curl -sf -H "Authorization: Bearer $TOKEN" "$CONTROL/v1/players" |
  python3 -c 'import json,sys; print(next((p["account_id"] for p in json.load(sys.stdin) if p["username"] == sys.argv[1]), ""))' "$1")
[[ -n $account ]] || { echo "$1 is not online" >&2; exit 1; }
curl -sf -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d "$(python3 -c 'import json,sys; print(json.dumps({"command": sys.argv[1]}))' "$2")" "$CONTROL/v1/players/$account/command"
echo
