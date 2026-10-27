#!/usr/bin/env bash
# One command for the servers: the play server (tools/lan-server.sh: progression, REST API, web UI,
# debugger) and the world editor server (tools/survey-server.sh), started in the background
# (they keep running after this terminal closes) and the debugger / world editor opened in the
# browser once they answer.
#   tools/servers.sh [up] [play|editor|both] [--debug] [--no-open]   start (default: both, release build)
#   tools/servers.sh restart [play|editor|both] [--debug]   rebuild, restart game server + API (players stay connected)
#   tools/servers.sh down [play|editor|both]                stop (characters of players online are saved)
#   tools/servers.sh status
#   tools/servers.sh open [debugger|editor]                 open the page in the browser
#   tools/servers.sh logs [play|editor]                     follow the game server's log
# A server already running (also one started by hand) is left as it is by `up`; `down` stops only
# the ones started here. The editor server shares the play server's logins (and its client
# overlay), so `both` starts play first. Env as the two scripts (LMOXIDE_*); LAN ip: LMOXIDE_IP.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
LOGS=$ROOT/logs
mkdir -p "$LOGS"

CMD=up
case ${1:-} in up | restart | down | stop | status | open | logs) CMD=$1; shift ;; esac
WHICH=both PROFILE=--release OPEN=1 PAGE=
for a in "$@"; do
  case $a in
    play | editor | both) WHICH=$a ;;
    debugger) PAGE=debugger ;;
    --debug) PROFILE= ;;
    --release) PROFILE=--release ;;
    --no-open) OPEN= ;;
    *) sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
  esac
done
[[ $CMD == open && $WHICH == editor ]] && PAGE=editor
servers() { case $WHICH in both) echo play editor ;; *) echo "$WHICH" ;; esac; }

# per server: its script, game port (universe), web port and the log that says it is up
script() { [[ $1 == play ]] && echo lan-server.sh || echo survey-server.sh; }
port() { [[ $1 == play ]] && echo 7100 || echo 7110; }
web_port() { [[ $1 == play ]] && echo 5174 || echo 5175; }
game_log() { [[ $1 == play ]] && echo "$LOGS/lan-server.log" || echo "$LOGS/survey-server.log"; }
runner_log() { echo "$LOGS/servers-$1.log"; }
group_file() { echo "$LOGS/servers-$1.pgid"; }
listening() { [[ -n $(ss -ltnH "sport = :$1") ]]; }
running() { listening "$(port "$1")"; }
# the process group started here, if it still runs
group() { local g; g=$(cat "$(group_file "$1")" 2>/dev/null) && kill -0 -- "-$g" 2>/dev/null && echo "$g"; }

lan_ip() { echo "${LMOXIDE_IP:-$(ip -4 route get 1.1.1.1 | sed -nE 's/.* src ([0-9.]+).*/\1/p')}"; }
url() {
  # the play server's dev UI reaches both APIs (one sign-in for both tools); alone, the editor's own
  case $1 in
    debugger) echo "http://localhost:5174/debugger/" ;;
    editor) listening 5174 && echo "http://localhost:5174/editor/" || echo "http://localhost:5175/editor/" ;;
  esac
}
open_page() {
  local u; u=$(url "$1")
  echo "$1: $u"
  [[ -n $OPEN ]] && command -v xdg-open >/dev/null && (setsid xdg-open "$u" >/dev/null 2>&1 &)
  return 0
}

start() {
  local s=$1 log g
  if running "$s"; then
    echo "$s server: already running (port $(port "$s"))"
    return
  fi
  log=$(runner_log "$s")
  echo "$s server: building and starting ($([[ -n $PROFILE ]] && echo release || echo debug) build; log $log)"
  # its own session and process group: it outlives this terminal, and `down` stops the whole group
  setsid bash -c 'echo $$ >"$1"; shift; exec "$@"' _ "$(group_file "$s")" \
    "$ROOT/tools/$(script "$s")" $PROFILE ${LMOXIDE_IP:-} >"$log" 2>&1 </dev/null &
  local t=0
  rm -f "$(game_log "$s")"
  until grep -qs "control interface listening" "$(game_log "$s")"; do
    sleep 1
    t=$((t + 1))
    if ! g=$(group "$s"); then
      echo "$s server: stopped while starting; the end of $log:"
      tail -n 20 "$log"
      exit 1
    fi
    (( t % 30 == 0 )) && echo "  still starting ($t s): $(tail -n 1 "$log" | cut -c1-120)"
    (( t > 1800 )) && { echo "$s server: not up after 30 min, see $log"; exit 1; }
  done
  # the dev UI (vite) comes right after, if the scripts found ../lmoxide-dev-ui
  for _ in $(seq 60); do listening "$(web_port "$s")" && break; sleep 0.5; done
  echo "$s server: up"
}

stop() {
  local s=$1 g
  if ! g=$(group "$s"); then
    running "$s" && echo "$s server: not started by this script (stop it where it runs, Ctrl-C)" || echo "$s server: not running"
    return
  fi
  # as Ctrl-C in its terminal: the game server saves the characters of players online
  kill -INT -- "-$g" 2>/dev/null || true
  for _ in $(seq 60); do kill -0 -- "-$g" 2>/dev/null || break; sleep 0.5; done
  kill -TERM -- "-$g" 2>/dev/null && sleep 1 && kill -KILL -- "-$g" 2>/dev/null || true
  rm -f "$(group_file "$s")"
  echo "$s server: stopped"
}

status() {
  local s ip; ip=$(lan_ip)
  for s in play editor; do
    if running "$s"; then
      echo "$s server: running$(group "$s" >/dev/null && echo " (started by tools/servers.sh)" || echo " (started by hand)")"
    else
      echo "$s server: stopped"
    fi
  done
  running play && echo "  join: lmo <name> $ip       web UI http://$ip:5173  debugger $(url debugger)"
  running editor && echo "  join: lmo <name> $ip:8081  world editor $(url editor)"
  return 0
}

case $CMD in
  up)
    for s in $(servers); do start "$s"; done
    status
    for s in $(servers); do open_page "$([[ $s == play ]] && echo debugger || echo editor)"; done
    ;;
  restart)
    for s in $(servers); do
      if running "$s"; then "$ROOT/tools/$(script "$s")" --restart $PROFILE; else start "$s"; fi
    done
    ;;
  down | stop)
    # editor first: it reads the play server's logins
    for s in $(servers | tr ' ' '\n' | tac); do stop "$s"; done
    ;;
  status) status ;;
  open)
    OPEN=1
    if [[ -n $PAGE ]]; then open_page "$PAGE"; else
      running play && open_page debugger
      running editor && open_page editor
    fi
    ;;
  logs) [[ $WHICH == both ]] && WHICH=play; tail -F "$(game_log "$WHICH")" ;;
esac
