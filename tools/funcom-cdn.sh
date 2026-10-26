#!/usr/bin/env bash
# Serve the local mirror of Funcom's LMO patch CDN (made by tools/funcom-mirror.py) at the same
# URL layout as http://update.lego.funcom.com/legoupm (foreground; Ctrl-C stops it).
#   tools/funcom-cdn.sh [--bind ADDR] [--port PORT] [--dir DIR]
# Defaults: 127.0.0.1:8070, ../clients/funcom-cdn. The CDN is then http://127.0.0.1:8070/legoupm:
#   LMOXIDE_UPSTREAM=http://127.0.0.1:8070/legoupm tools/lan-server.sh --release
# (dev-run.sh reads LMOXIDE_UPSTREAM too.) A stock client can use it directly as HttpPatchAddr.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BIND=127.0.0.1
PORT=8070
DIR="$ROOT/../clients/funcom-cdn"
while (( $# )); do
  case $1 in
    --bind) BIND=$2; shift 2 ;;
    --port) PORT=$2; shift 2 ;;
    --dir) DIR=$2; shift 2 ;;
    *) sed -n '2,8p' "$0"; exit 1 ;;
  esac
done
[[ -d $DIR/legoupm ]] || { echo "no mirror in $DIR/legoupm: run tools/funcom-mirror.py first"; exit 1; }
echo "Funcom CDN mirror: http://$BIND:$PORT/legoupm (from $DIR)"
exec python3 -I -m http.server --bind "$BIND" --directory "$DIR" "$PORT"
