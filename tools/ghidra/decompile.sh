#!/usr/bin/env bash
# Decompile the functions containing the given addresses (needs analyze.sh first).
#   tools/ghidra/decompile.sh 0xb4b580 0x7c0000
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
GHIDRA=${GHIDRA_HOME:-/opt/ghidra}
# The 2G default heap is tight for this 27 MB binary.
export GHIDRA_HEADLESS_MAXMEM=${GHIDRA_HEADLESS_MAXMEM:-8G}
PROJ=$(realpath -m "${LMO_GHIDRA_PROJECT:-$ROOT/../ghidra}")
"$GHIDRA/support/analyzeHeadless" "$PROJ" LMO -process LMO.exe -noanalysis -readOnly \
  -scriptPath "$ROOT/tools/ghidra" -postScript Decompile.java "$@" 2>&1 \
  | awk '/@@@ /{p=1} /^INFO  .*(REPORT|ANALYZING|Decompile.java)/{next} p' \
  | sed -E 's/^INFO  Decompile\.java> //; s/ \(GhidraScript\)$//'
