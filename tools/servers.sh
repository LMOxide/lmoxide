#!/usr/bin/env bash
# One command for the server: tools/lan-server.sh (every game mode: Progression, Maxxed and World
# Editor; REST API, web UI, dev UI with the debugger and the world editor), started in the
# background (it keeps running after this terminal closes), with the debugger and the world
# editor opened in the browser once it answers.
#   tools/servers.sh [up] [--debug] [--no-open]   start (release build)
#   tools/servers.sh restart [--debug]            rebuild, restart game server + API (players stay connected)
#   tools/servers.sh down                         stop (characters of players online are saved)
#   tools/servers.sh status
#   tools/servers.sh open [debugger|editor]       open the page in the browser
#   tools/servers.sh logs                         follow the game server's log
# A server already running (also one started by hand) is left as it is by `up`; `down` stops only
# one started here. The words play, editor and both are still accepted (one server runs every
# mode now). Env as tools/lan-server.sh (LMOXIDE_*); LAN ip: LMOXIDE_IP.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
LOGS=$ROOT/logs
mkdir -p "$LOGS"

CMD=up
case ${1:-} in up | restart | down | stop | status | open | logs) CMD=$1; shift ;; esac
PROFILE=--release OPEN=1 PAGE=
for a in "$@"; do
  case $a in
    play | editor | both) [[ $CMD == open && $a == editor ]] && PAGE=editor ;;
    debugger) PAGE=debugger ;;
    --debug) PROFILE= ;;
    --release) PROFILE=--release ;;
    --no-open) OPEN= ;;
    *) sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
  esac
done

GAME_LOG=$LOGS/lan-server.log RUNNER_LOG=$LOGS/servers-play.log GROUP_FILE=$LOGS/servers-play.pgid
listening() { [[ -n $(ss -ltnH "sport = :$1") ]]; }
running() { listening 7101; }
# the process group started here, if it still runs
group() { local g; g=$(cat "$GROUP_FILE" 2>/dev/null) && kill -0 -- "-$g" 2>/dev/null && echo "$g"; }

lan_ip() { echo "${LMOXIDE_IP:-$(ip -4 route get 1.1.1.1 | sed -nE 's/.* src ([0-9.]+).*/\1/p')}"; }
url() { echo "http://localhost:5174/$1/"; }
open_page() {
  local u; u=$(url "$1")
  echo "$1: $u"
  [[ -n $OPEN ]] && command -v xdg-open >/dev/null && (setsid xdg-open "$u" >/dev/null 2>&1 &)
  return 0
}

start() {
  local g
  if running; then
    echo "server: already running"
    return
  fi
  echo "server: building and starting ($([[ -n $PROFILE ]] && echo release || echo debug) build; log $RUNNER_LOG)"
  # its own session and process group: it outlives this terminal, and `down` stops the whole group
  setsid bash -c 'echo $$ >"$1"; shift; exec "$@"' _ "$GROUP_FILE" \
    "$ROOT/tools/lan-server.sh" $PROFILE ${LMOXIDE_IP:-} >"$RUNNER_LOG" 2>&1 </dev/null &
  local t=0
  rm -f "$GAME_LOG"
  until grep -qs "control interface listening" "$GAME_LOG"; do
    sleep 1
    t=$((t + 1))
    if ! g=$(group); then
      echo "server: stopped while starting; the end of $RUNNER_LOG:"
      tail -n 20 "$RUNNER_LOG"
      exit 1
    fi
    (( t % 30 == 0 )) && echo "  still starting ($t s): $(tail -n 1 "$RUNNER_LOG" | cut -c1-120)"
    (( t > 1800 )) && { echo "server: not up after 30 min, see $RUNNER_LOG"; exit 1; }
  done
  # the dev UI (vite) comes right after, if the script found ../lmoxide-dev-ui
  for _ in $(seq 60); do listening 5174 && break; sleep 0.5; done
  echo "server: up"
}

stop() {
  local g
  if ! g=$(group); then
    running && echo "server: not started by this script (stop it where it runs, Ctrl-C)" || echo "server: not running"
    return
  fi
  # as Ctrl-C in its terminal: the game server saves the characters of players online
  kill -INT -- "-$g" 2>/dev/null || true
  for _ in $(seq 60); do kill -0 -- "-$g" 2>/dev/null || break; sleep 0.5; done
  kill -TERM -- "-$g" 2>/dev/null && sleep 1 && kill -KILL -- "-$g" 2>/dev/null || true
  rm -f "$GROUP_FILE"
  echo "server: stopped"
}

status() {
  local ip; ip=$(lan_ip)
  if running; then
    echo "server: running$(group >/dev/null && echo " (started by tools/servers.sh)" || echo " (started by hand)")"
    echo "  join: lmo <name> $ip (the patcher's Game Mode: Progression, Maxxed, World Editor)"
    echo "  web UI http://$ip:5173  debugger $(url debugger)  world editor $(url editor)"
  else
    echo "server: stopped"
  fi
  return 0
}

case $CMD in
  up)
    start
    status
    open_page debugger
    open_page editor
    ;;
  restart)
    if running; then "$ROOT/tools/lan-server.sh" --restart $PROFILE; else start; fi
    ;;
  down | stop) stop ;;
  status) status ;;
  open)
    OPEN=1
    if [[ -n $PAGE ]]; then open_page "$PAGE"; else open_page debugger && open_page editor; fi
    ;;
  logs) tail -F "$GAME_LOG" ;;
esac
