#!/usr/bin/env bash
# Builds and runs the hollowing oracle (DragonFruit's own Rust, see
# hollow-harness/src/main.rs) and writes its fixtures. The build goes to
# .local/ (ignored); DragonFruit is read from its checkout, never modified.
#
# usage: run_hollow_harness.sh [OUT] [REAL_OUT]
#   SC_BENCH_STL=path.stl [SC_BENCH_HEIGHT=90] also writes REAL_OUT, the real
#   model's hashes and DragonFruit's timings (kept in .local/, not committed:
#   the model is private).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
OUT="${1:-$ROOT/libs/sc-geom/hollow-df/tests/fixtures/df_hollow.json}"
REAL_OUT="${2:-$ROOT/.local/df-oracle/df_hollow_real.json}"
mkdir -p "$(dirname "$REAL_OUT")"
export CARGO_TARGET_DIR="$ROOT/.local/df-oracle/hollow-harness-target"
cargo run --offline --release --manifest-path "$HERE/hollow-harness/Cargo.toml" -- "$OUT" "$REAL_OUT"
