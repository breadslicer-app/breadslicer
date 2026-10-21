#!/bin/sh

# Runs cargo-gamma (mutation testing) in a bwrap sandbox. Every command of the
# tool copies the workspace into a scratch directory, so run every command
# through this script, never `cargo gamma` directly on the checkout.
#
# Usage, from any directory:
#     sh scripts/mutation.sh run [options]
#     sh scripts/mutation.sh list mutants
#     sh scripts/mutation.sh <other cargo gamma arguments>
#
# Inside the sandbox:
# - the repository root shows only a generated Cargo.toml (its members are
#   VISIBLE_CRATES, and it has no sc-geom dependency), a copy of Cargo.lock,
#   gamma.toml, the visible crates and the paths of VISIBLE_PATHS (read-only),
#   and target/;
# - the only writable places are target/, a private /tmp (that is,
#   target/mutation/tmp, which holds HOME and the XDG directories), and the
#   cgroup subtree of the systemd manager of the user, where the tool puts the
#   cgroups of its memory limits;
# - CARGO_HOME is target/mutation/cargo-home, a copy of the registry index and
#   archives of the real one;
# - there is no network.
#
# Requires Linux with cgroup v2, a systemd user manager, bwrap, rsync, and
# `cargo install cargo-gamma --version 0.3.0 --locked`.
#
# Reports: target/cargo-gamma/ (gamma-report.html and others).

set -eu

GAMMA_VERSION="cargo-gamma 0.3.0"

# Allow list of the crate directories that the sandbox shows: the crates of
# gamma.toml, and the workspace crates that they need to build and test.
VISIBLE_CRATES="libs/mesh-core libs/app-paths libs/3mf"

# Allow list of the paths outside the crates that the sandbox shows (read-only),
# because the tests of the visible crates read them.
VISIBLE_PATHS="assets packages/shared/it.soulcrafted.slicer.desktop"

# Lists of gamma.toml that must name at least 1 entry: the tool reads an empty
# or missing list as "everything".
REQUIRED_LISTS="packages files include-tests"

# Mutants that `run` tests at the same time, unless the arguments set it.
DEFAULT_JOBS=4

REPO=$(cd "$(dirname "$0")/.." && pwd)
CONFIG="$REPO/gamma.toml"
WORK="$REPO/target/mutation"
REAL_CARGO_HOME=${CARGO_HOME:-"$HOME/.cargo"}
REAL_RUSTUP_HOME=${RUSTUP_HOME:-"$HOME/.rustup"}
PRIVATE_CARGO_HOME="$WORK/cargo-home"
USER_ID=$(id -u)
CGROUP_SUBTREE="/sys/fs/cgroup/user.slice/user-$USER_ID.slice/user@$USER_ID.service"

fail() {
    echo "mutation.sh: $*" >&2
    exit 1
}

require_lists() {
    for key in $REQUIRED_LISTS; do
        awk -v key="$key" '
            $0 ~ "^" key " *= *\\[" { inside = 1 }
            inside && /"/ { found = 1 }
            inside && /\]/ { inside = 0 }
            END { exit (found ? 0 : 1) }
        ' "$CONFIG" || fail "gamma.toml: the list '$key' is missing or empty"
    done
}

generate_manifest() {
    mkdir -p "$WORK/manifest"
    members=""
    for crate in $VISIBLE_CRATES; do
        members="$members\"$crate\", "
    done
    awk -v members="$members" '
        /^members = \[/ {
            print "members = [" members "]"
            if ($0 !~ /\]/) skipping = 1
            next
        }
        skipping { if ($0 ~ /^\]/) skipping = 0; next }
        /^sc-geom = / { next }
        { print }
    ' "$REPO/Cargo.toml" > "$WORK/manifest/Cargo.toml"
    cp "$REPO/Cargo.lock" "$WORK/manifest/Cargo.lock"
}

# Copies only the registry index and the downloaded archives: cargo unpacks the
# sources that it needs, and nothing else of the real CARGO_HOME (for example
# credentials.toml) enters the private one.
refresh_cargo_home() {
    for part in index cache; do
        [ -d "$REAL_CARGO_HOME/registry/$part" ] \
            || fail "no $REAL_CARGO_HOME/registry/$part; build the workspace once first"
        mkdir -p "$PRIVATE_CARGO_HOME/registry/$part"
        rsync -a --delete "$REAL_CARGO_HOME/registry/$part/" \
            "$PRIVATE_CARGO_HOME/registry/$part/"
    done
}

# Runs "$@" inside the sandbox.
sandboxed() {
    set -- --remount-ro "$REPO" --chdir "$REPO" "$@"
    for crate in $VISIBLE_CRATES; do
        set -- --ro-bind "$REPO/$crate" "$REPO/$crate" "$@"
    done
    for path in $VISIBLE_PATHS; do
        set -- --ro-bind "$REPO/$path" "$REPO/$path" "$@"
    done
    mkdir -p "$WORK/tmp/home/.cache" "$WORK/tmp/home/.config" \
        "$WORK/tmp/home/.local/share" "$WORK/tmp/home/.local/state"
    bwrap \
        --die-with-parent \
        --unshare-net \
        --ro-bind / / \
        --dev /dev \
        --proc /proc \
        --bind "$CGROUP_SUBTREE" "$CGROUP_SUBTREE" \
        --bind "$WORK/tmp" /tmp \
        --tmpfs "$REPO" \
        --ro-bind "$WORK/manifest/Cargo.toml" "$REPO/Cargo.toml" \
        --bind "$WORK/manifest/Cargo.lock" "$REPO/Cargo.lock" \
        --ro-bind "$CONFIG" "$REPO/gamma.toml" \
        --bind "$REPO/target" "$REPO/target" \
        --setenv HOME /tmp/home \
        --setenv XDG_CACHE_HOME /tmp/home/.cache \
        --setenv XDG_CONFIG_HOME /tmp/home/.config \
        --setenv XDG_DATA_HOME /tmp/home/.local/share \
        --setenv XDG_STATE_HOME /tmp/home/.local/state \
        --setenv CARGO_HOME "$PRIVATE_CARGO_HOME" \
        --setenv RUSTUP_HOME "$REAL_RUSTUP_HOME" \
        --setenv CARGO_NET_OFFLINE true \
        "$@"
}

prepare() {
    [ -n "$VISIBLE_CRATES" ] || fail "the list of visible crates is empty"
    require_lists
    [ -d "$CGROUP_SUBTREE" ] \
        || fail "no $CGROUP_SUBTREE; the memory limits need a systemd user manager"
    installed=$(cargo gamma --version 2>/dev/null) || fail "cargo-gamma is not installed"
    [ "$installed" = "$GAMMA_VERSION" ] || fail "found $installed, need $GAMMA_VERSION"
    generate_manifest
    refresh_cargo_home
    metadata=$(sandboxed cargo metadata --format-version 1 --offline) \
        || fail "cargo cannot resolve the sandboxed workspace"
    case $metadata in
        *'"name":"sc-geom"'*) fail "sc-geom is in the dependency graph" ;;
    esac
}

# Succeeds when the options before `--` set the number of jobs, or resume a
# campaign, which keeps the number of jobs that it started with.
sets_jobs() {
    for arg in "$@"; do
        case $arg in
            --) return 1 ;;
            --jobs | --jobs=* | -j | -j* | --resume-testing) return 0 ;;
        esac
    done
    return 1
}

is_dry_run() {
    for arg in "$@"; do
        case $arg in
            --) return 1 ;;
            --dry-run) return 0 ;;
        esac
    done
    return 1
}

# Prints the directory that receives the reports of `run "$@"`.
artifact_dir() {
    dir="target/cargo-gamma"
    while [ $# -gt 0 ]; do
        case $1 in
            --) break ;;
            --artifact-dir)
                if [ $# -gt 1 ]; then
                    dir=$2
                    shift
                fi
                ;;
            --artifact-dir=*) dir=${1#--artifact-dir=} ;;
        esac
        shift
    done
    case $dir in
        /*) printf '%s\n' "$dir" ;;
        *) printf '%s\n' "$REPO/$dir" ;;
    esac
}

run_campaign() {
    sets_jobs "$@" || set -- --jobs "$DEFAULT_JOBS" "$@"
    diagnostics="$(artifact_dir "$@")/gamma-diagnostics.json"
    rm -f "$diagnostics"
    sandboxed cargo gamma run "$@"
    if is_dry_run "$@"; then
        return 0
    fi
    # Without its memory limits, the tool goes on with no warning on the
    # console; only this file says so.
    [ -f "$diagnostics" ] || fail "no $diagnostics, so the memory limits are not confirmed"
    grep -Eq '"metered": *true' "$diagnostics" \
        || fail "the run had no memory limits; see \"unbounded\" in $diagnostics"
}

[ $# -gt 0 ] || fail "usage: sh scripts/mutation.sh <cargo gamma arguments>"
prepare
case $1 in
    run)
        shift
        run_campaign "$@"
        ;;
    --help | -h | --version | -V)
        sandboxed cargo gamma "$@"
        ;;
    -*)
        # The tool reads a first argument that is an option as `run`.
        run_campaign "$@"
        ;;
    *)
        sandboxed cargo gamma "$@"
        ;;
esac
