#!/usr/bin/env bash
# Make an extracted native Linux client (LMO_Linux_v1.5.557666.zip from lmo.files.aronwk.com)
# run on a current distribution. Idempotent.
#   tools/linux-client/setup-linux-client.sh <client-dir> [server-host:port]
# - moves the bundled 2016 libstdc++.so.6 aside (too old for today's system libraries, which
#   the client loads through LD_LIBRARY_PATH=.; the system one is backward compatible)
# - builds a stub libgconf-2.so.4 (needed by libcef.so, no longer packaged)
# - builds liblmofixes.so (lmo-fixes.c: locks the client's animation name table; play.sh preloads it)
# - points the chat filter URL in LMO at our server (tools/patch-client.py; backup LMO.orig)
set -euo pipefail
CLIENT=${1:?usage: setup-linux-client.sh <client-dir> [server-host:port]}
HOST=${2:-127.0.0.1:8080}
HERE=$(cd "$(dirname "$0")" && pwd)
cd "$CLIENT"
chmod +x LMO Launcher DWWebHelper
[[ -f libstdc++.so.6 ]] && mv libstdc++.so.6 libstdc++.so.6.bundled
[[ -f libgconf-2.so.4 ]] || cc -shared -fPIC -O2 -o libgconf-2.so.4 "$HERE/gconf-stub.c"
cc -shared -fPIC -O2 -o liblmofixes.so "$HERE/lmo-fixes.c" -lpthread
python3 "$HERE/../patch-client.py" --exe "$PWD/LMO" --host "$HOST"
echo "ready: $HERE/play.sh \"$CLIENT\" <server-ip> <username>"
