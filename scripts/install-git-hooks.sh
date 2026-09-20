#!/usr/bin/env bash
# Install the versioned Git hooks for this repository (no extra tools, no secret helpers).
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
git rev-parse --is-inside-work-tree >/dev/null
chmod +x "$root/scripts/pre-commit.sh" "$root/scripts/githooks/pre-commit"
git config core.hooksPath scripts/githooks
printf 'Git hooks path is scripts/githooks (pre-commit runs scripts/pre-commit.sh).\n'
printf 'Skip one commit with JG_SKIP_HOOKS=1 or SKIP=1.\n'
