#!/usr/bin/env bash
# Start a kit-installed client (apply.sh / install.sh) against a server: the client's own
# launcher, for clients outside the default install dir.
#   patches/linux-client/play.sh <client-dir> <server-ip> <username>
set -euo pipefail
CLIENT=${1:?usage: play.sh <client-dir> <server-ip> <username>}
[[ -x $CLIENT/lmo ]] || { echo "no launcher in $CLIENT: run $(dirname "$0")/apply.sh \"$CLIENT\" first" >&2; exit 1; }
exec "$CLIENT/lmo" "${3:?username}" "${2:?server}"
