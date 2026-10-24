#!/usr/bin/env bash
# Symbols matching a regex (vftables: also the first N slots).
#   tools/ghidra/symbols.sh 'GameserverToClientRPC.*vftable' 8
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
GHIDRA=${GHIDRA_HOME:-/opt/ghidra}
export GHIDRA_HEADLESS_MAXMEM=${GHIDRA_HEADLESS_MAXMEM:-8G}
PROJ=$(realpath -m "${LMO_GHIDRA_PROJECT:-$ROOT/../research/ghidra}")
"$GHIDRA/support/analyzeHeadless" "$PROJ" LMO -process "${LMO_GHIDRA_PROGRAM:-LMO.exe}" -noanalysis -readOnly \
  -scriptPath "$ROOT/tools/ghidra" -postScript Symbols.java "$@" 2>&1 \
  | grep -E '^INFO  Symbols\.java> ' | sed -E 's/^INFO  Symbols\.java> //; s/ \(GhidraScript\) *$//'
