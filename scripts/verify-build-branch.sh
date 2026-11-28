#!/bin/bash
# Refuse a release build from an unrelated branch or stale worktree.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
EXPECTED_BRANCH="${1:-main}"
CURRENT_BRANCH="$(git -C "$ROOT" branch --show-current)"

if [ -z "$CURRENT_BRANCH" ]; then
    echo "release builds require a named branch" >&2
    exit 1
fi

if [ "$CURRENT_BRANCH" != "$EXPECTED_BRANCH" ]; then
    echo "release builds must run from $EXPECTED_BRANCH (current: $CURRENT_BRANCH)" >&2
    exit 1
fi

if ! git -C "$ROOT" diff --quiet --cached; then
    echo "staged changes are not committed; commit them before releasing" >&2
    exit 1
fi

printf 'build branch check passed: %s\n' "$CURRENT_BRANCH"
