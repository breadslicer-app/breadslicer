#!/usr/bin/env bash
# Builds and runs the native island-scanner oracle (DragonFruit's own Rust,
# see islands-harness/src/main.rs) and writes its fixture. The build goes to
# .local/ (ignored); DragonFruit is read from DF_REPO, never modified.
#
# usage: run_islands_harness.sh [OUT]
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
OUT="${1:-$ROOT/libs/sc-geom/support-gen-df/tests/fixtures/df_native_islands.json}"
export CARGO_TARGET_DIR="$ROOT/.local/df-oracle/islands-harness-target"
cargo run --offline --release --manifest-path "$HERE/islands-harness/Cargo.toml" -- "$OUT"
