#!/usr/bin/env bash
# Fails if any tracked or untracked-but-not-ignored file contains a term from
# the local, untracked .scrub-list (internal names that must never be published).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
if [ ! -f .scrub-list ]; then
  echo "check-scrub: no .scrub-list, skipping"
  exit 0
fi
matches=$(git ls-files -co --exclude-standard -z | xargs -0 -r grep -n -i -F -f .scrub-list -- 2>/dev/null || true)
if [ -n "$matches" ]; then
  echo "check-scrub: internal names found:"
  echo "$matches"
  exit 1
fi
echo "check-scrub: clean"
