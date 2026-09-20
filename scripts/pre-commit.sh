#!/usr/bin/env bash
# Fast assertions that must hold before a commit succeeds.
# Same Clippy rules as CI: levels live in Cargo.toml; this command passes -D warnings.
# The full isolated gate remains scripts/check.sh (workflow lint, rustdoc, release, PTY).
set -euo pipefail

if [[ ${JG_SKIP_HOOKS:-0} == 1 || ${SKIP:-0} == 1 ]]; then
  printf 'pre-commit: skipped\n'
  exit 0
fi

root=$(git rev-parse --show-toplevel)
cd "$root"

pin=$(python3 scripts/check-toolchain.py --print-rust)
step() { printf '==> %s\n' "$*"; }

step 'Formatting'
cargo +"$pin" fmt --all -- --check

step 'Clippy'
cargo +"$pin" clippy --locked --all-targets --all-features -- -D warnings

step 'Tests (ignored live tests stay ignored)'
cargo +"$pin" test --locked --all-targets --all-features

printf 'pre-commit: ok. Full isolated gate: scripts/check.sh\n'
