#!/usr/bin/env bash
# Pack the client kit into a small tarball to hand to another PC (no repo or compiler needed
# there): dist/lmo-client-kit-v<N>.tar.gz with install.sh, apply.sh, the launcher, the sources,
# prebuilt libraries and patch-client.py. On the other PC:
#   tar xzf lmo-client-kit-v<N>.tar.gz && lmo-client-kit-v<N>/install.sh --server <host-ip>
set -euo pipefail
KIT=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$KIT/../.." && pwd)
VERSION=$(cat "$KIT/KIT_VERSION")
NAME=lmo-client-kit-v$VERSION
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/$NAME/prebuilt" "$ROOT/dist"
cp "$KIT"/{install.sh,apply.sh,lmo,KIT_VERSION,README.md,gconf-stub.c,lmo-fixes.c} "$ROOT/patches/patch-client.py" "$stage/$NAME/"
cc -shared -fPIC -O2 -o "$stage/$NAME/prebuilt/libgconf-2.so.4" "$KIT/gconf-stub.c"
cc -shared -fPIC -O2 -o "$stage/$NAME/prebuilt/liblmofixes.so" "$KIT/lmo-fixes.c" -lpthread
tar czf "$ROOT/dist/$NAME.tar.gz" -C "$stage" "$NAME"
echo "$ROOT/dist/$NAME.tar.gz ($(du -h "$ROOT/dist/$NAME.tar.gz" | cut -f1))"
