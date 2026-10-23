#!/usr/bin/env bash
# Disassemble functions of the Linux LMO build (same version as LMO.exe, with ~143k named
# dynamic symbols) by demangled-name regex, or list matches.
#   tools/re/linux_fn.sh -l 'CharDCMoveIIR_t::'          list matching symbols (addr size name)
#   tools/re/linux_fn.sh 'CharDCMoveIIR_t::Activate\(\)'  disassemble every match
# Calls are annotated with demangled target names by objdump.
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
BIN=${LMO_LINUX_BIN:-$ROOT/../downloads/linux_557666/LMO}
SYMS=$ROOT/logs/linux_syms_sized.txt
[[ -s $SYMS ]] || nm -DC -S --defined-only "$BIN" | awk 'NF>=4' > "$SYMS"
list=0; [[ ${1:-} == -l ]] && { list=1; shift; }
pat=$1
grep -E " [TtWw] .*$pat" "$SYMS" | sort -u -k1,1 | while read -r addr size _type name; do
  if (( list )); then echo "$addr $size $name"; continue; fi
  start=$((16#$addr)); stop=$((start + 16#$size))
  echo "== $name @ 0x$addr"
  objdump -d -C --no-show-raw-insn -M intel --start-address=$start --stop-address=$stop "$BIN" | tail -n +8
done
