#!/usr/bin/env bash
# Runs a TypeScript script against DragonFruit's own sources, from the same
# disposable sandbox run_oracle.sh uses. The script is copied into the sandbox
# root, so its imports read `./src/...`.
#
# usage: run_df_script.sh SCRIPT.ts OUT
set -euo pipefail
DF="${DF_REPO:-C:/Users/dnaon/.openclaw/workspace/repos/DragonFruit}"
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
script="$1"; out="$2"

sandbox="$ROOT/.local/df-oracle/sandbox-$(git -C "$DF" rev-parse --short HEAD)"
if [ ! -d "$sandbox" ]; then
  mkdir -p "$sandbox"
  (cd "$DF" && cp -r src scripts profiles package.json tsconfig.json "$sandbox/")
  [ -d "$DF/plugins" ] && cp -r "$DF/plugins" "$sandbox/"
  cp "$HERE/register.mjs" "$sandbox/"
  git -C "$DF" rev-parse HEAD > "$sandbox/DF_REVISION"
fi
cp "$script" "$sandbox/"

export DF_ORACLE_PACKAGE_URL="file:///$DF/package.json"
export NODE_PATH="$DF/node_modules"
cd "$sandbox"
node --import "file:///$DF/node_modules/tsx/dist/loader.mjs" --import ./register.mjs \
  "./$(basename "$script")" > "$out"
