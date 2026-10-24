#!/usr/bin/env bash
# Screenshot tour: start the servers and the Wine test client (like test-run.sh), let the server
# teleport the test character through a test-command kind's targets (LMOXIDE_DEV_TOUR, see
# game.rs) and screenshot the game window at each one.
#   tools/tour.sh <kind> [seconds per target=20] [out dir=logs/tour-<kind>]
# Env as for test-run.sh (LMOXIDE_DATA_DIR, LMO_CLIENT_DIR...). Writes NN.png and labels.txt.
set -uo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
KIND=${1:?usage: tour.sh <kind> [secs] [outdir]}
SECS=${2:-20}
OUT=${3:-$ROOT/logs/tour-$KIND}
CLIENT=${LMO_CLIENT_DIR:-$(cd "$ROOT/../clients/windows/LEGO Minifigures Online" && pwd)}
export LMO_CLIENT_ARGS=${LMO_CLIENT_ARGS--username tester -password tester-password}
export LMO_AUTOPLAY=1 LMOXIDE_DEV_TOUR=$KIND LMOXIDE_DEV_TOUR_SECS=$SECS
mkdir -p "$OUT" && rm -f "$OUT"/*.png "$OUT/labels.txt"
"$ROOT/tools/dev-run.sh" "$CLIENT" "$ROOT/logs" >/dev/null || exit 1
# the test client's window only (a native "LMO" window may be the user's own game)
CLASS=$([[ ${LMO_RUNNER:-wine} == linux ]] && echo lmo || echo lmo.exe)
window() { hyprctl clients -j | CLASS=$CLASS python3 -c 'import json,os,sys; print(next((c["stableId"] for c in json.load(sys.stdin) if c["class"].lower() == os.environ["CLASS"]), ""))'; }
seen=0; idle=0
# up to 4 minutes for the login, then until no target came for 3 rounds
while (( seen == 0 ? idle < 240 : idle < SECS * 3 )); do
  sleep 1
  n=$(grep -c "dev tour" "$ROOT/logs/server.log" 2>/dev/null)
  if (( n > seen )); then
    seen=$n; idle=0
    label=$(grep "dev tour" "$ROOT/logs/server.log" | tail -1 | sed 's/\x1b\[[0-9;]*m//g; s/.*label=//')
    # the new playfield loads; shoot just before the next teleport
    sleep $(( SECS > 6 ? SECS - 4 : 2 ))
    id=$(window)
    file=$(printf "%s/%02d.png" "$OUT" "$seen")
    [[ -n $id ]] && timeout 10 grim -T "$id" "$file"
    echo "$(basename "$file") $label" | tee -a "$OUT/labels.txt"
  else
    idle=$((idle + 1))
  fi
done
pkill -x LMO.exe; sleep 2
WINEPREFIX=${WINEPREFIX:-$HOME/.local/share/lmo-wine} wineserver -k 2>/dev/null
[[ -f $ROOT/logs/dev-run.pids ]] && { kill $(cat "$ROOT/logs/dev-run.pids") 2>/dev/null; rm -f "$ROOT/logs/dev-run.pids"; }
echo "screenshots in $OUT"
