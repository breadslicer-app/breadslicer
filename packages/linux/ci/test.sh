#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 SynrgStudio
# SPDX-License-Identifier: AGPL-3.0-only
#
# Builds the workspace and runs every test, inside the Arch image
# (docker/arch.Dockerfile). Run by ../build.sh. The GUI and 3D view tests get
# a virtual X display (Xvfb) and Mesa's software Vulkan driver (lavapipe).
# The whole log goes to /out/test-log.txt.
set -uo pipefail

cd /src
log=/out/test-log.txt
start=$(date +%s)
# The UI crate (Slint's generated code) alone takes ~6 GB to compile. Built
# next to five others it ran a container capped at 8 GB into swap for hours,
# so it goes first, alone, with the features the tests unify on it.
cargo build --locked --profile quick-release -p bread-gui-ui -p i-slint-backend-testing 2>&1 | tail -n 5
cargo build --workspace --all-targets --locked --profile quick-release 2>&1 | tail -n 40
build=${PIPESTATUS[0]}
echo "== build took $(( $(date +%s) - start )) s (exit $build)"
[ "$build" -eq 0 ] || exit "$build"

start=$(date +%s)
xvfb-run -a -s "-screen 0 1920x1080x24" \
    cargo test --workspace --locked --no-fail-fast --profile quick-release >"$log" 2>&1
status=$?
echo "== tests took $(( $(date +%s) - start )) s (exit $status)"

grep -E '^test result:' "$log" | awk '
    { passed += $4; failed += $6; ignored += $8 }
    END { printf "== passed %d, failed %d, ignored %d\n", passed, failed, ignored }'
grep -E '^(test .* FAILED|failures:$|    [a-z_:]+$)' "$log" | grep -v '^failures:$' | sort -u | head -n 80
exit "$status"
