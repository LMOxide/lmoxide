#!/usr/bin/env bash
# Run a Windows program under Steam's Proton without the Steam client.
#   tools/proton-run.sh /abs/path/LMO.exe [args...]   (run from the client folder; Proton needs the full path)
# Env: PROTON (proton install dir; default Proton - Experimental), LMO_PROTON_PREFIX
#      (default ~/.local/share/lmo-proton; Proton creates the Wine prefix in <it>/pfx).
set -euo pipefail
STEAM=${STEAM_ROOT:-$HOME/.local/share/Steam}
PROTON=${PROTON:-$STEAM/steamapps/common/Proton - Experimental}
export STEAM_COMPAT_CLIENT_INSTALL_PATH=$STEAM
export STEAM_COMPAT_DATA_PATH=${LMO_PROTON_PREFIX:-$HOME/.local/share/lmo-proton}
mkdir -p "$STEAM_COMPAT_DATA_PATH"
# Proton reads the game id for per-game fixes; 0 = none.
export SteamAppId=${SteamAppId:-0} SteamGameId=${SteamGameId:-0}
exec "$PROTON/proton" run "$@"
