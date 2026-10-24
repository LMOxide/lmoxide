#!/usr/bin/env bash
# Run LMOxide for LAN play: lmoxide-server and lmoxide-patch-server (foreground; Ctrl-C stops
# both and saves the characters of players still online).
#   tools/lan-server.sh [--release] [lan-ip]   (default ip: this PC's address on the default route)
# --release: optimized build (target/release; faster, slower to compile). Logging is the same.
# Listens on all interfaces and tells clients to connect to <lan-ip>. Ports: TCP 8080 (patch
# server), 7100 (universe), 7101 (territory), 7102 (game), 7200 (chat).
# LAN conveniences: --auto-register (the first login creates the account or sets the password of
# an account without one) and GM level 1 for everyone (free-text chat and the test commands need it).
# Progression is on (--progression: XP, levels and minifigure ranks, as live).
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PROFILE=debug
if [[ ${1:-} == --release ]]; then
  PROFILE=release
  shift
fi
IP=${1:-$(ip -4 route get 1.1.1.1 | sed -nE 's/.* src ([0-9.]+).*/\1/p')}
VAR=$(realpath -m "$ROOT/../var")
DATA=${LMOXIDE_DATA_DIR:-$VAR/data}
CLIENT=${LMOXIDE_CLIENT_DIR:-$(realpath -m "$ROOT/../clients/windows/LEGO Minifigures Online")}
# the client kit, served at http://<ip>:8080/client-kit.tar.gz for other PCs (patches/linux-client/README.md)
KIT=$("$ROOT/patches/linux-client/make-kit.sh" | cut -d' ' -f1)
pkill -x lmoxide-server || true
pkill -x lmoxide-patch-s || true   # process names are cut to 15 characters
cargo build --manifest-path "$ROOT/Cargo.toml" -q -p lmoxide-server -p lmoxide-patch-server $([[ $PROFILE == release ]] && echo --release)
BIN=$ROOT/target/$PROFILE
mkdir -p "$ROOT/logs"
echo "LAN server ($PROFILE build) on $IP. Install or update the client on another PC:"
echo "  d=\$(mktemp -d) && curl -fsSL http://$IP:8080/client-kit.tar.gz | tar xz -C \$d && \$d/*/install.sh --server $IP"
echo "logs: $ROOT/logs/lan-server.log, $ROOT/logs/lan-patch-server.log"

export RUST_LOG=${RUST_LOG:-info}
"$BIN/lmoxide-patch-server" --bind 0.0.0.0 --public-host "$IP" --client-dir "$CLIENT" \
  --data-dir "$VAR/patch-data" --login-public-key-file "$DATA/login-public-key.txt" \
  --upstream "${LMOXIDE_UPSTREAM-http://update.lego.funcom.com/legoupm}" --cache-dir "$VAR/rdb-cache" \
  --client-kit "$KIT" ${LMOXIDE_PATCH_ARGS:-} >"$ROOT/logs/lan-patch-server.log" 2>&1 &
PATCH=$!
trap 'kill $PATCH 2>/dev/null' EXIT
"$BIN/lmoxide-server" --data-dir "$DATA" --bind 0.0.0.0 --public-host "$IP" \
  --auto-register --default-gm-level "${LMOXIDE_DEFAULT_GM_LEVEL:-1}" --progression ${LMOXIDE_SERVER_ARGS:-} 2>&1 \
  | tee "$ROOT/logs/lan-server.log"
