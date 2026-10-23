#!/usr/bin/env bash
# References to the given addresses (code + data, one level through data such as vtables).
#   tools/ghidra/xrefs.sh 0xb52870
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
GHIDRA=${GHIDRA_HOME:-/opt/ghidra}
export GHIDRA_HEADLESS_MAXMEM=${GHIDRA_HEADLESS_MAXMEM:-8G}
PROJ=$(realpath -m "${LMO_GHIDRA_PROJECT:-$ROOT/../ghidra}")
"$GHIDRA/support/analyzeHeadless" "$PROJ" LMO -process "${LMO_GHIDRA_PROGRAM:-LMO.exe}" -noanalysis -readOnly \
  -scriptPath "$ROOT/tools/ghidra" -postScript Xrefs.java "$@" 2>&1 \
  | grep -E '^INFO  Xrefs\.java> ' | sed -E 's/^INFO  Xrefs\.java> //; s/ \(GhidraScript\) *$//'
