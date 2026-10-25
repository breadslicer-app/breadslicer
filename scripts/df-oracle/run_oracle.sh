#!/usr/bin/env bash
# Runs DragonFruit's own Auto pipeline (scripts/bench-auto-supports.ts) over a
# corpus of posed STLs, as a parity oracle for the native SC port.
#
# DF runs from a disposable copy under .local/df-oracle/; the reference checkout
# is only read (its node_modules are resolved through NODE_PATH). Nothing from
# DF is linked into SC.
#
# usage: run_oracle.sh CORPUS_DIR OUT_JSON [extra bench args, e.g. --only NAME]
set -euo pipefail
DF="${DF_REPO:-C:/Users/dnaon/.openclaw/workspace/repos/DragonFruit}"
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
corpus="$(cd "$1" && pwd)"; out="$2"; shift 2

sandbox="$ROOT/.local/df-oracle/sandbox-$(git -C "$DF" rev-parse --short HEAD)"
if [ ! -d "$sandbox" ]; then
  mkdir -p "$sandbox"
  (cd "$DF" && cp -r src scripts profiles package.json tsconfig.json "$sandbox/")
  [ -d "$DF/plugins" ] && cp -r "$DF/plugins" "$sandbox/"
  cp "$HERE/register.mjs" "$sandbox/"
  git -C "$DF" rev-parse HEAD > "$sandbox/DF_REVISION"
fi

export DF_ORACLE_PACKAGE_URL="file:///$DF/package.json"
export NODE_PATH="$DF/node_modules"
cd "$sandbox"
node --max-old-space-size=16384 \
  --import "file:///$DF/node_modules/tsx/dist/loader.mjs" --import ./register.mjs \
  scripts/bench-auto-supports.ts --corpus "$corpus" --detect --json "$@" > "$out"
