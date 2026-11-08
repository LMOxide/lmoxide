#!/usr/bin/env bash
# Shrink Cargo target/ dirs to just the final release binaries.
# Keeps: <target>/release/<exe> and <target>/<triple>/release/<exe>(.exe)
# Deletes: everything else (debug/, deps/, build/, incremental/, flycheck*, ...).
# Next build recompiles from scratch.
# Usage: tools/slim-target.sh [-n] <target-dir>...   (-n = dry run)
set -euo pipefail
dry=0; [[ ${1:-} == -n ]] && { dry=1; shift; }
for t in "$@"; do
  t=${t%/}
  [[ -f $t/CACHEDIR.TAG || -d $t/debug || -d $t/release ]] || { echo "skip $t (not a cargo target dir)"; continue; }
  before=$(du -sh "$t" | cut -f1)
  keep=$(find "$t" -mindepth 2 -maxdepth 3 -type f -path '*/release/*' \
           \( -perm -u+x -o -name '*.exe' \) ! -name '*.so' ! -name '*.d' \
           ! -path '*/deps/*' ! -path '*/build/*' ! -path '*/examples/*' ! -path '*/incremental/*')
  echo "$t ($before) keeps:"; printf '  %s\n' $keep
  ((dry)) && continue
  stash=$(mktemp -d "$t.keep.XXXX")
  for f in $keep; do mkdir -p "$stash/$(dirname "${f#$t/}")"; mv "$f" "$stash/${f#$t/}"; done
  rm -rf "$t"
  mv "$stash" "$t"
  printf 'Signature: 8a477f597d28d172789f06886806bc55\n' > "$t/CACHEDIR.TAG"
  echo "  -> $(du -sh "$t" | cut -f1)"
done
