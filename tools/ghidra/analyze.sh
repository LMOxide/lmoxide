#!/usr/bin/env bash
# One-time: import LMO.exe into a Ghidra project and run auto-analysis (slow: tens of minutes).
#   tools/ghidra/analyze.sh            -> project at ../ghidra/LMO (outside the repo)
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
GHIDRA=${GHIDRA_HOME:-/opt/ghidra}
# The 2G default heap is tight for this 27 MB binary.
export GHIDRA_HEADLESS_MAXMEM=${GHIDRA_HEADLESS_MAXMEM:-8G}
EXE=$(realpath "${LMO_EXE:-$ROOT/../client/LEGO Minifigures Online/LMO.exe}")
PROJ=$(realpath -m "${LMO_GHIDRA_PROJECT:-$ROOT/../ghidra}")
mkdir -p "$PROJ"
exec "$GHIDRA/support/analyzeHeadless" "$PROJ" LMO -import "$EXE" -overwrite \
  -analysisTimeoutPerFile 14400 -max-cpu "$(nproc)"
