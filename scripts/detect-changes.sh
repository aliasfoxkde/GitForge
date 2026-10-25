#!/usr/bin/env bash
# detect-changes.sh — populate CI_CHANGE_* vars for downstream skip_if evaluation.
#
# Runs in the CI container at the commit being tested. Uses git to enumerate
# changed files relative to the merge-base with origin/main (the last commit
# on main that this branch was based on). All files touched by the push are
# classified, and the resulting CI_CHANGE_* vars are echoed in a form the
# runner can capture and return as job result JSON.
#
# CI_CHANGE_INCLUDE_CODE     — non-doc Rust/source files changed
# CI_CHANGE_INCLUDE_DOCS    — docs/, *.md at repo root changed
# CI_CHANGE_INCLUDE_CRATES  — specific crates/ gitforge-* dirs changed
# CI_CHANGE_INCLUDE_TESTS   — *_test.rs, tests/ dirs, test code
# CI_CHANGE_INCLUDE_INFRA   — infrastructure/, docker/, scripts/, .github/ changed
# CI_CHANGE_FILE_COUNT      — total number of changed files

set -euo pipefail

# Ensure origin/main is available for the merge-base calculation
git fetch --quiet origin main 2>/dev/null || true

# Get the list of files changed vs the merge-base with main
# (all commits on this branch since it diverged from main)
if ! BASE=$(git merge-base origin/main HEAD 2>/dev/null); then
    # Fallback: parent commit if merge-base fails (detached HEAD or shallow clone)
    BASE=HEAD~1
fi

CHANGED_FILES=$(git diff --name-only "$BASE" HEAD 2>/dev/null || echo "")

# A push to main itself (or any branch already merged into main) makes
# merge-base == HEAD, so the diff above is empty even though the push
# carries real changes. The observed change set is then the tip commit's
# own diff (first parent for merges). Observed live 2026-10-24: the
# d227f05a main push reported FILE_COUNT=0 and skipped every cargo job
# while an earlier run of the same SHA ran them all.
if [[ -z "$CHANGED_FILES" ]]; then
    CHANGED_FILES=$(git diff --name-only HEAD^1 HEAD 2>/dev/null || echo "")
fi

# Empty push — no files changed
if [[ -z "$CHANGED_FILES" ]]; then
    echo "CI_CHANGE_FILE_COUNT=0"
    echo "CI_CHANGE_INCLUDE_CODE=0"
    echo "CI_CHANGE_INCLUDE_DOCS=0"
    echo "CI_CHANGE_INCLUDE_CRATES=0"
    echo "CI_CHANGE_INCLUDE_TESTS=0"
    echo "CI_CHANGE_INCLUDE_INFRA=0"
    exit 0
fi

FILE_COUNT=$(echo "$CHANGED_FILES" | wc -l)

# Classify each file
CODE=0
DOCS=0
CRATES=0
TESTS=0
INFRA=0

while IFS= read -r file; do
    [[ -z "$file" ]] && continue

    # Docs: docs/ directory or top-level *.md files (not .github/, not Cargo.toml, etc.)
    if [[ "$file" =~ ^docs/ || ( "$file" =~ \.md$ && "$file" != "Cargo.lock" && "$file" != "*.tom" ) ]]; then
        # Exclude files that are not documentation
        if [[ "$file" =~ \.md$ ]] || [[ "$file" == docs/* ]]; then
            DOCS=1
            continue
        fi
    fi

    # Infrastructure: infrastructure/, docker/, scripts/, .github/
    if [[ "$file" =~ ^(infrastructure|docker|scripts)/ || "$file" =~ ^\.github/ ]]; then
        INFRA=1
        continue
    fi

    # Crates: specific crates/ directories (gitforge-* packages)
    if [[ "$file" =~ ^crates/gitforge- ]]; then
        CRATES=1
        # Rust source files count as code too
        if [[ "$file" =~ \.rs$ ]]; then
            CODE=1
        fi
        continue
    fi

    # Tests: *_test.rs files, tests/ directories (check before generic .rs to avoid
    # the buggy "! /tests/" pattern which doesn't do what it says on the tin)
    if [[ "$file" =~ (_test\.rs|/tests/|\.fuzz$) ]]; then
        TESTS=1
        continue
    fi

    # Rust source
    if [[ "$file" =~ \.rs$ ]]; then
        CODE=1
        continue
    fi

    # Anything else that looks like source
    if [[ "$file" =~ \.(rs|toml|yaml|yml|toml|cargo)$ ]]; then
        CODE=1
    fi

done <<< "$CHANGED_FILES"

echo "CI_CHANGE_FILE_COUNT=$FILE_COUNT"
echo "CI_CHANGE_INCLUDE_CODE=$CODE"
echo "CI_CHANGE_INCLUDE_DOCS=$DOCS"
echo "CI_CHANGE_INCLUDE_CRATES=$CRATES"
echo "CI_CHANGE_INCLUDE_TESTS=$TESTS"
echo "CI_CHANGE_INCLUDE_INFRA=$INFRA"
