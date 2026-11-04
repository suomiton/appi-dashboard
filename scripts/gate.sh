#!/usr/bin/env bash
# Local pre-commit gate: format, lint (warnings are errors), test, scrub.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets --quiet
scripts/check-scrub.sh
