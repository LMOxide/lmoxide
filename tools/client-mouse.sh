#!/usr/bin/env bash
# Click, scroll and drag inside a window on any (hidden) workspace without moving the real
# cursor or changing focus. Coordinates are window-local logical pixels.
#   tools/client-mouse.sh click X Y [left|right|middle] [count]
#   tools/client-mouse.sh scroll X Y STEPS            (positive = down, 1..20)
#   tools/client-mouse.sh drag X1 Y1 X2 Y2 [button]
#   tools/client-mouse.sh info                        (address, size, X11 or Wayland)
# Target: LMO_WINDOW (Hyprland selector: class:RE, title:RE, address:0x..), default the LMO
# client (native class LMO, Wine lmo.exe, Proton steam_app_0).
# Uses the same-session-target-pointer Hyprland plugin (hyprctl cutarget): it gives the target
# surface the seat's pointer focus, sends spaced-out wl_pointer events (XWayland turns them into
# real X input, like a physical mouse) and then hands the focus back. The plugin source is
# github.com/Gabriel-Kahen/hyprland-codex-background-computer-use plus
# hypr-target-pointer/hyprland-0.56.patch; it is built and loaded on demand.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
export HYPRLAND_INSTANCE_SIGNATURE=${HYPRLAND_INSTANCE_SIGNATURE:-$(ls -t "$XDG_RUNTIME_DIR/hypr" | head -1)}
selector=${LMO_WINDOW:-'class:^(LMO|lmo\.exe|steam_app_0)$'}
plugin_version=0.3.0-restack

# address size_w size_h xwayland pid title
win=$(hyprctl clients -j | python3 -c '
import json, re, sys
kind, _, pat = sys.argv[1].partition(":")
for c in json.load(sys.stdin):
    v = c["address"] if kind == "address" else c.get(kind, "")
    if (v.lower() == pat.lower()) if kind == "address" else re.search(pat, v):
        print(c["address"], *c["size"], int(c["xwayland"]), c["pid"], c["title"]); break
' "$selector")
[[ -n $win ]] || { echo "no window matches $selector" >&2; exit 1; }
read -r addr w h xw pid title <<<"$win"

plugin() {
    hyprctl plugin list | grep -q "Version: $plugin_version" && return
    local src=$HOME/src/hyprland-codex-background-computer-use dir=$HOME/.local/lib/hyprland-plugins ver so old
    ver=$(hyprctl version -j | python3 -c 'import json,sys; print(json.load(sys.stdin)["commit"][:12])')
    so=$dir/same-session-target-pointer-$ver-$plugin_version.so
    if [[ ! -f $so ]]; then
        [[ -d $src ]] || git clone -q https://github.com/Gabriel-Kahen/hyprland-codex-background-computer-use "$src"
        git -C "$src" checkout -q -- hyprland/target-pointer.cpp
        git -C "$src" apply "$here/hypr-target-pointer/hyprland-0.56.patch"
        make -s -C "$src/hyprland" clean all >&2
        mkdir -p "$dir" && cp "$src/hyprland/same-session-target-pointer.so" "$so"
    fi
    for old in "$dir"/*.so; do hyprctl plugin unload "$old" >/dev/null 2>&1 || true; done
    hyprctl plugin load "$so" >/dev/null
}

# Run one cutarget transaction and wait until the plugin has played it out.
cutarget() {
    local out
    plugin
    out=$(hyprctl cutarget "$@")
    echo "$out"
    [[ $out == *'"ok":true'* ]] || exit 1
    sleep "$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["duration_ms"] / 1000 + 0.05)' "$out")"
}

cmd=${1:-info}; shift || true
case $cmd in
  info) echo "$addr ${w}x$h $([[ $xw == 1 ]] && echo xwayland || echo wayland) pid=$pid $title" ;;
  click) cutarget click "$addr" "$1" "$2" "${3:-left}" "${4:-1}" ;;
  scroll) cutarget scroll "$addr" "$1" "$2" "$3" ;;
  drag) cutarget drag "$addr" "$1" "$2" "$3" "$4" "${5:-left}" ;;
  *) sed -n '2,7p' "$0"; exit 1 ;;
esac
