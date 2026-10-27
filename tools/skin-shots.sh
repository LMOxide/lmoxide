#!/usr/bin/env bash
# Preview pictures of every look (monster data, data/monster_data.json: enemies, NPCs, smashables,
# buildies, minifigure bodies...): the Linux test client (account tester) logs in to the running LAN
# server (tools/lan-server.sh), parks somewhere quiet, zooms in (Z, once), and for each look wears
# it (!skin <id>), turns to face the camera (a tap of S) and saves a square crop of the window.
#   tools/skin-shots.sh [out dir=../var/skin-shots] [id or name filter (regex)]
# Env: SKIN_PARK (playfield to park in, default 1337 XX_GM Island), SKIN_WAIT (seconds for a look
# to load, default 3), SKIN_CROP (square side as a share of the window height, default 0.8),
# SKIN_SKIP_EXISTING=1 (keep pictures already taken), LMO_CLIENT_DIR (default the Linux test copy).
# Writes <id>-<name>.png and index.tsv (id, name, file, what uses it); the client is closed after.
set -uo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT=$(realpath -m "${1:-$ROOT/../var/skin-shots}")
FILTER=${2:-}
PARK=${SKIN_PARK:-1337}
WAIT=${SKIN_WAIT:-3}
CROP=${SKIN_CROP:-0.8}
CLIENT=$(realpath "${LMO_CLIENT_DIR:-$ROOT/../clients/linux/LEGO Minifigures Online}")
CONTROL=${LMOXIDE_CONTROL:-http://127.0.0.1:7300}
TOKEN=$(cat "${LMOXIDE_CONTROL_TOKEN_FILE:-$ROOT/../var/data/control_token}") || exit 1
USER_NAME=tester
mkdir -p "$OUT"

players() { curl -sf -H "Authorization: Bearer $TOKEN" "$CONTROL/v1/players"; }
# field of the tester's entry in the players list (empty: not online)
me() { players | python3 -c 'import json,sys
p = next((p for p in json.load(sys.stdin) if p["username"] == sys.argv[1]), None)
v = p and p.get(sys.argv[2])
print("" if v is None else " ".join(map(str, v)) if isinstance(v, list) else v)' "$USER_NAME" "$1"; }
cmd() {
  curl -sf -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
    -d "$(python3 -c 'import json,sys; print(json.dumps({"command": sys.argv[1]}))' "$1")" "$CONTROL/v1/players/$ACCOUNT/command" >/dev/null
}
# the test client's window: the LMO process running in $CLIENT (the user's own client is "LMO" too)
window() {
  local pid
  for pid in $(pgrep -x LMO); do
    [[ $(readlink "/proc/$pid/cwd") == "$CLIENT" ]] || continue
    hyprctl clients -j | python3 -c 'import json,sys
c = next((c for c in json.load(sys.stdin) if c["pid"] == int(sys.argv[1])), None)
c and print(c["address"], c["stableId"])' "$pid"
    return
  done
}
key() { LMO_WINDOW=address:$ADDRESS "$ROOT/tools/client-keys.sh" "$@"; }
quit() {
  for pid in $(pgrep -x LMO); do [[ $(readlink "/proc/$pid/cwd") == "$CLIENT" ]] && kill -9 "$pid"; done
}

[[ -n $(me account_id) ]] && { echo "$USER_NAME is already online; close that client first" >&2; exit 1; }
LMOXIDE_LAN_SERVER=1 LMO_RUNNER=linux LMO_AUTOPLAY=1 LMO_CLIENT_ARGS="-username $USER_NAME -password $USER_NAME-password" \
  "$ROOT/tools/dev-run.sh" "$CLIENT" "$ROOT/logs" >/dev/null || exit 1
trap quit EXIT
echo "waiting for $USER_NAME to be in play..."
for _ in $(seq 300); do [[ -n $(me position) ]] && break; sleep 1; done
ACCOUNT=$(me account_id)
[[ -n $ACCOUNT && -n $(me position) ]] || { echo "$USER_NAME didn't get in play" >&2; exit 1; }
read -r ADDRESS SHOT_ID < <(window)
[[ -n ${ADDRESS:-} ]] || { echo "no window for the test client" >&2; exit 1; }

cmd '!skin reset'
if [[ $(me playfield) != "$PARK" ]]; then
  cmd "!tp $PARK"
  for _ in $(seq 60); do [[ $(me playfield) == "$PARK" ]] && break; sleep 1; done
fi
sleep 8 # the playfield loads
POS=$(me position)
echo "parked in $PARK at $POS"
key key z
sleep 1

python3 - "$ROOT/data/monster_data.json" "$FILTER" >"$OUT/.todo" <<'EOF' || exit 1
import json, re, sys
f = sys.argv[2]
for m in json.load(open(sys.argv[1]))["monster_data"]:
    if f and not re.search(f, f"{m['id']} {m['name']}", re.I):
        continue
    users = "; ".join(sorted({f"{u['kind']}: {u['name']}" for u in m["used_by"]} | ({"minifigure"} if m["minifigures"] else set())))
    name = re.sub(r"[^A-Za-z0-9_.-]+", "_", m["name"]).strip("_")
    print(m["id"], f"{m['id']}-{name}.png", m["name"], users, sep="\t")
EOF
total=$(wc -l <"$OUT/.todo")
[[ -f $OUT/index.tsv ]] || printf 'id\tname\tfile\tused by\n' >"$OUT/index.tsv"
n=0
while IFS=$'\t' read -r id file name users; do
  n=$((n + 1))
  [[ -n ${SKIN_SKIP_EXISTING:-} && -s $OUT/$file ]] && continue
  [[ -n $(me position) ]] || { echo "$USER_NAME left the game" >&2; exit 1; }
  # back to the same spot (the S tap walks a little), the look, then face the camera
  cmd "!moveto $POS"
  cmd "!skin $id"
  sleep "$WAIT"
  key hold s 0.15
  sleep 1.5
  raw=$OUT/.raw.ppm
  if timeout 10 grim -T "$SHOT_ID" -t ppm "$raw"; then
    # a centred square, SKIN_CROP of the window height
    side=$(magick identify -format "%h" "$raw" | awk -v c="$CROP" '{ printf "%d", $1 * c }')
    magick "$raw" -gravity center -crop "${side}x${side}+0+0" +repage "$OUT/$file"
    grep -q "^$id	" "$OUT/index.tsv" || printf '%s\t%s\t%s\t%s\n' "$id" "$name" "$file" "$users" >>"$OUT/index.tsv"
    echo "[$n/$total] $file"
  else
    echo "[$n/$total] $id: no screenshot" >&2
  fi
done <"$OUT/.todo"
rm -f "$OUT/.todo" "$OUT/.raw.ppm"
cmd '!skin reset'
echo "pictures in $OUT"
