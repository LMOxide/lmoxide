#!/usr/bin/env bash
# Retired: the world editor runs in the same server as the other game modes now
# (tools/lan-server.sh, the World Editor game mode on universe port 7104; the patcher's Game
# Mode picks it). Its characters were the old ../var/survey-data's; copy them over once with
#   target/release/lmoxide-server --data-dir ../var/data import-characters ../var/survey-data/lmoxide.db
# (docs/SERVER-ARCHITECTURE.md).
echo "tools/survey-server.sh is retired: the world editor runs in tools/lan-server.sh (World Editor game mode)." >&2
sed -n '4,5p' "$0" | sed 's/^# \{0,1\}//' >&2
exit 1
