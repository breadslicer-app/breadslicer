#!/bin/sh
# Purges Weblate "approval-only" commits from the current branch.
#
# When a translation gets approved on Weblate, it pushes a commit authored
# by the reviewer whose only change is the `Last-Translator:` header. That
# gives the reviewer undeserved attribution for someone else's work.
#
# This script inspects the new commits on top of the base branch (default:
# `main`, assumed to already be a strict ancestor of HEAD) and drops every
# commit whose entire diff consists of `Last-Translator:` header changes.
# A backup ref is saved at refs/backups/<branch>-<timestamp> before the
# current branch is rewritten.

set -eu

BASE_BRANCH="main"
DRY_RUN=0
ASSUME_YES=0

usage() {
    cat <<EOF
Usage: $0 [--base <branch>] [--dry-run] [--yes]

  --base <branch>  Base branch to compare against (default: main).
  --dry-run        List approval-only commits without rewriting history.
  --yes            Skip the interactive confirmation before rewriting.
  -h, --help       Show this help text.
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --base) shift; BASE_BRANCH="${1:?missing value for --base}" ;;
        --dry-run) DRY_RUN=1 ;;
        --yes|-y) ASSUME_YES=1 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
    shift
done

current_branch=$(git symbolic-ref --short HEAD 2>/dev/null || true)
if [ -z "$current_branch" ]; then
    echo "error: HEAD is detached; refusing to rewrite history" >&2
    exit 1
fi

if [ "$current_branch" = "$BASE_BRANCH" ]; then
    echo "error: refusing to operate on the base branch ($BASE_BRANCH)" >&2
    exit 1
fi

if ! git diff --quiet || ! git diff --cached --quiet; then
    echo "error: working tree has uncommitted changes" >&2
    exit 1
fi

if ! git rev-parse --verify --quiet "${BASE_BRANCH}^{commit}" >/dev/null; then
    echo "error: base branch '$BASE_BRANCH' does not exist" >&2
    exit 1
fi

BASE_TIP=$(git rev-parse "${BASE_BRANCH}^{commit}")
MERGE_BASE=$(git merge-base HEAD "$BASE_TIP")
if [ "$MERGE_BASE" != "$BASE_TIP" ]; then
    echo "error: HEAD is not rebased onto $BASE_BRANCH" >&2
    echo "       merge-base: $MERGE_BASE" >&2
    echo "       $BASE_BRANCH: $BASE_TIP" >&2
    echo "       rebase first, then re-run this script." >&2
    exit 1
fi

# A commit is "approval-only" when every added/removed line in its diff
# is a `Last-Translator:` PO header (and there is at least one such line).
is_approval_only() {
    commit=$1
    # -U0 strips context lines; --pretty=format: strips the commit header.
    diff_body=$(git show --no-color --pretty=format: -U0 "$commit" \
        | awk '/^(\+\+\+|---) /{next} /^[+-]/{print}')
    [ -n "$diff_body" ] || return 1
    non_matching=$(printf '%s\n' "$diff_body" \
        | grep -Ev '^[+-]"Last-Translator: ' || true)
    [ -z "$non_matching" ]
}

kept_file=$(mktemp)
drop_file=$(mktemp)
trap 'rm -f "$kept_file" "$drop_file"' EXIT INT TERM HUP

total=0
for commit in $(git rev-list --reverse "${BASE_TIP}..HEAD"); do
    total=$((total + 1))
    if is_approval_only "$commit"; then
        printf '%s\n' "$commit" >> "$drop_file"
    else
        printf '%s\n' "$commit" >> "$kept_file"
    fi
done

if [ "$total" -eq 0 ]; then
    echo "no commits on top of $BASE_BRANCH"
    exit 0
fi

drop_count=$(wc -l < "$drop_file" | tr -d ' ')
kept_count=$(wc -l < "$kept_file" | tr -d ' ')

if [ "$drop_count" -eq 0 ]; then
    echo "no approval-only commits found (kept: $kept_count)"
    exit 0
fi

echo "commits to drop ($drop_count):"
while IFS= read -r commit; do
    short=$(git rev-parse --short "$commit")
    line=$(git log -1 --no-show-signature --format='%an <%ae> | %s' "$commit")
    printf '  %s  %s\n' "$short" "$line"
done < "$drop_file"

echo ""
echo "commits to keep ($kept_count):"
while IFS= read -r commit; do
    short=$(git rev-parse --short "$commit")
    line=$(git log -1 --no-show-signature --format='%an <%ae> | %s' "$commit")
    printf '  %s  %s\n' "$short" "$line"
done < "$kept_file"

if [ "$DRY_RUN" -eq 1 ]; then
    echo ""
    echo "(dry run; re-run without --dry-run to rewrite $current_branch)"
    exit 0
fi

if [ "$ASSUME_YES" -ne 1 ]; then
    printf '\nrewrite %s? [y/N] ' "$current_branch"
    read -r answer
    case "$answer" in
        y|Y|yes|YES) ;;
        *) echo "aborted"; exit 1 ;;
    esac
fi

backup_ref="refs/backups/${current_branch}-$(date -u +%Y%m%dT%H%M%SZ)"
git update-ref "$backup_ref" HEAD
echo "backup ref: $backup_ref"

git reset --hard "$BASE_TIP"

while IFS= read -r commit; do
    if ! git cherry-pick "$commit"; then
        cat <<EOF >&2

cherry-pick failed for $commit.

Resolve the conflict and continue with:
  git cherry-pick --continue

Or abort the rewrite and restore the original branch with:
  git cherry-pick --abort
  git reset --hard $backup_ref
EOF
        exit 1
    fi
done < "$kept_file"

echo ""
echo "done. $drop_count commit(s) dropped, $kept_count kept."
echo "backup: $backup_ref  (remove with: git update-ref -d $backup_ref)"
