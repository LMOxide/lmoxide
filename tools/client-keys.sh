#!/usr/bin/env bash
# Type into the running LMO client window (works on a hidden workspace) via Hyprland.
#   tools/client-keys.sh key Return      press one key (xkb name)
#   tools/client-keys.sh hold w 2        hold a key for N seconds
#   tools/client-keys.sh text "hello"    type lowercase letters, digits and spaces
# LMO_WINDOW picks the window (default class:lmo.exe, the Wine client), e.g. address:0x... for
# the Linux test client, whose class "LMO" the user's own client shares.
set -euo pipefail
send() { hyprctl dispatch "hl.dsp.send_key_state({mods=\"\", key=\"$1\", state=\"$2\", window=\"${LMO_WINDOW:-class:lmo.exe}\"})" >/dev/null; }
press() { send "$1" down; sleep 0.08; send "$1" up; sleep 0.12; }
case $1 in
  key) press "$2" ;;
  hold) send "$2" down; sleep "${3:-1}"; send "$2" up ;;
  text) s=$2; for ((i = 0; i < ${#s}; i++)); do c=${s:i:1}; [[ $c == " " ]] && c=space; press "$c"; done ;;
esac
