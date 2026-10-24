#!/usr/bin/env bash
# Run lmo-server for LAN play (foreground; Ctrl-C stops it; quit the clients first so characters are saved).
#   tools/lan-server.sh [lan-ip]   (default: this PC's address on the default route)
# Listens on all interfaces and tells clients to connect to <lan-ip>. Ports: TCP 8080 (HTTP),
# 7100 (universe), 7101 (territory), 7102 (game), 7200 (chat).
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
IP=${1:-$(ip -4 route get 1.1.1.1 | sed -nE 's/.* src ([0-9.]+).*/\1/p')}
export LMO_UPSTREAM=${LMO_UPSTREAM-http://update.lego.funcom.com/legoupm}
export LMO_CACHE_DIR=${LMO_CACHE_DIR:-$(realpath -m "$ROOT/../rdb-cache")}
export LMO_DATA_DIR=${LMO_DATA_DIR:-$(realpath -m "$ROOT/../lmo-data")}
# GM level 1 as in dev-run.sh: free-text chat between players needs it.
export LMO_GM_LEVEL=${LMO_GM_LEVEL:-1}
CLIENT=${LMO_CLIENT_DIR:-$(realpath -m "$ROOT/../client/LEGO Minifigures Online")}
pkill -x lmo-server || true
cargo build --manifest-path "$ROOT/Cargo.toml" -q
echo "LAN server on $IP; other PCs: tools/linux-client/play.sh <client-dir> $IP <username>"
RUST_LOG=${RUST_LOG:-info} exec "$ROOT/target/debug/lmo-server" \
  --bind 0.0.0.0 --public-host "$IP" --client-dir "$CLIENT" ${LMO_SERVER_ARGS:-}
