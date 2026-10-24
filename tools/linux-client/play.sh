#!/usr/bin/env bash
# Start the native Linux client against an lmo-server (this PC or another on the LAN).
#   tools/linux-client/play.sh <client-dir> <server-host> <username> [password]
# Any username works (a new one gets a new character); the password is not checked.
# Run setup-linux-client.sh <client-dir> <server-host>:8080 once first.
set -euo pipefail
CLIENT=${1:?usage: play.sh <client-dir> <server-host> <username> [password]}
HOST=${2:?server host}
USER_NAME=${3:?username}
PASS=${4:-x}
cd "$CLIENT"
# What the launcher normally fetches: the server's addresses for every service.
curl -sf --max-time 5 "http://$HOST:8080/control/lmo_live/LocalConfig.xml" >LocalConfig.xml.new \
  || { echo "can't reach lmo-server at http://$HOST:8080 (running? firewall?)" >&2; rm -f LocalConfig.xml.new; exit 1; }
mv LocalConfig.xml.new LocalConfig.xml
# SDL_VIDEODRIVER=x11: the 2016 build needs GLX; sdl2-compat would otherwise pick Wayland/EGL
# and the renderer crashes in gl3wInit.
SDL_VIDEODRIVER=x11 LD_LIBRARY_PATH=. exec ./LMO -verified -username "$USER_NAME" -password "$PASS"
