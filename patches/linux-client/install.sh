#!/usr/bin/env bash
# Install the native Linux LEGO Minifigures Online client for an lmoxide-server: download the stock
# client (once; cached), verify it, extract it, check system libraries, apply the client kit.
# Re-running updates an existing install (only the kit is applied again).
#   install.sh [--server <ip>] [--dir <install-dir>] [--zip <LMO_Linux_v1.5.557666.zip>]
# Defaults: server 127.0.0.1, dir ~/Games/LMO, zip downloaded to ~/.cache/lmo-client/.
set -euo pipefail
KIT=$(cd "$(dirname "$0")" && pwd)
URL=https://lmo.files.aronwk.com/LMO_Clients/Linux/LMO_Linux_v1.5.557666.zip
ZIP_SHA256=f8cebe2f78410ff0c38c0f4386a9c1777ca0133558e7cb985003e067a6cdb4b4

SERVER= DIR=$HOME/Games/LMO ZIP=
while [[ $# -gt 0 ]]; do
  case $1 in
    --server) SERVER=${2:?}; shift 2 ;;
    --dir) DIR=${2:?}; shift 2 ;;
    --zip) ZIP=${2:?}; shift 2 ;;
    -h | --help) sed -n '2,7p' "$0"; exit 0 ;;
    *) echo "unknown option $1 (see --help)" >&2; exit 2 ;;
  esac
done

for tool in curl unzip python3 sha256sum; do
  command -v $tool >/dev/null || { echo "missing: $tool (install it first)" >&2; exit 1; }
done

if [[ -f $DIR/LMO || -f $DIR/LMO.orig ]]; then
  echo "client already in $DIR; applying the kit again"
else
  if [[ -z $ZIP ]]; then
    ZIP=$HOME/.cache/lmo-client/LMO_Linux_v1.5.557666.zip
    mkdir -p "$(dirname "$ZIP")"
    if [[ ! -f $ZIP ]] || [[ $(sha256sum "$ZIP" | cut -d' ' -f1) != "$ZIP_SHA256" ]]; then
      echo "downloading the client (1.2 GB) from $URL"
      curl -fL -C - -o "$ZIP" "$URL"
    fi
  fi
  echo "verifying $ZIP"
  [[ $(sha256sum "$ZIP" | cut -d' ' -f1) == "$ZIP_SHA256" ]] || { echo "checksum mismatch: not the expected client zip" >&2; exit 1; }
  echo "extracting to $DIR"
  tmp=$(mktemp -d "$(dirname "$DIR")/.lmo-extract.XXXXXX" 2>/dev/null || { mkdir -p "$(dirname "$DIR")"; mktemp -d "$(dirname "$DIR")/.lmo-extract.XXXXXX"; })
  trap 'rm -rf "$tmp"' EXIT
  unzip -q "$ZIP" -d "$tmp"
  chmod -R u+w "$tmp"
  mv "$tmp/LMO_Linux_v1.5.557666" "$DIR"
fi

"$KIT/apply.sh" "$DIR" ${SERVER:+--server "$SERVER"}

# system libraries the client needs (64-bit): report what's missing
missing=$(cd "$DIR" && LD_LIBRARY_PATH=. ldd ./LMO ./libcef.so 2>/dev/null | awk '/not found/ {print $1}' | sort -u)
if [[ -n $missing ]]; then
  echo
  echo "missing system libraries:" $missing
  echo "Arch: sudo pacman -S --needed sdl2-compat nss libcups libxtst libxcomposite libxrandr alsa-lib pango"
  echo "Debian/Ubuntu: sudo apt install libsdl2-2.0-0 libnss3 libcups2 libxtst6 libxcomposite1 libxrandr2 libasound2 libpango-1.0-0"
fi
echo
echo "done. Start the server, then: $DIR/lmo <username>"
