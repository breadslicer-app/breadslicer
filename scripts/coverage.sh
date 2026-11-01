#!/bin/sh

# Re-runnable coverage measurement for the workspace.
# Writes the per-file coverage table to stdout (sorted ascending so the
# worst-covered files surface first); full reports land under
# target/coverage/ (Stdout log at target/coverage/tarpaulin-run.log,
# JSON at target/coverage/tarpaulin-report.json).
#
# Requires `cargo-tarpaulin` >= 0.34 (for --follow-exec). Install with:
#     cargo install cargo-tarpaulin
#
# On a machine without a wgpu adapter, tests in
# apps/slicer-gui/crates/render/src/mesh_render/ soft-skip and that area's numbers
# read lower than on a GPU-equipped machine.

set -eu

if ! command -v cargo-tarpaulin >/dev/null 2>&1; then
    echo "cargo-tarpaulin not installed. Install with:" >&2
    echo "    cargo install cargo-tarpaulin" >&2
    exit 1
fi

mkdir -p target/coverage

cargo tarpaulin \
    --config tarpaulin.toml \
    > target/coverage/tarpaulin-run.log 2>&1

# Per-file table for hand-written workspace code, sorted ascending.
# The grep filters belt-and-braces over tarpaulin.toml's exclude-files
# in case a generated path slips through (e.g. new build artefacts).
# Tarpaulin appends a delta suffix (` +X.XX%` or ` -X.XX%`) when a
# previous report exists; the regex accepts both shapes.
grep -E "^\|\| (apps|libs)/.*\.rs: [0-9]+/[0-9]+( [+-][0-9.]+%)?\$" \
    target/coverage/tarpaulin-run.log \
    | grep -v "/tests/" \
    | grep -v "target/package" \
    | awk -F: '{
        split($2, nums, "/");
        covered = nums[1]+0; total = nums[2]+0;
        if (total > 0) {
            pct = 100.0 * covered / total;
            printf "%6.1f%% %4d/%-4d %s\n", pct, covered, total, $1;
        }
    }' | sort -n

echo ""
echo "Full report: target/coverage/tarpaulin-run.log"
echo "JSON:        target/coverage/tarpaulin-report.json"
