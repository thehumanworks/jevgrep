# ADR 0008: Pre-commit hook for the assertions that must hold

- Status: Accepted
- Date: 2026-09-20
- Baseline: ADR 0003 (`scripts/check.sh` isolated quality gate) and ADR 0006 (shared Clippy)
- Scope: the local Git hook and how it is installed. Not a replacement for CI.

## Context

`scripts/check.sh` is the required local and CI gate. It isolates HOME/XDG/Git/credentials,
pins actionlint and ShellCheck, formats, runs Clippy, tests, doctests, rustdoc, a host
release build, and the fake-API/PTY harness. That is correct for CI and too slow, and too
tool-heavy, to run on every commit. Contributors without the QA-tool installer should still
be able to commit.

The assertions that must be true before a commit succeeds are the ones a local edit can
break in seconds: rustfmt, Clippy with the Cargo policy, and `cargo test` (ignored live
tests stay ignored). Workflow lint, rustdoc, packaging, and PTY need extra tools or a
release binary; they stay in `check.sh`.

Python `pre-commit`, cargo-husky, and lefthook would add a second installer and a policy
file that can drift from `check.sh`. This repo already speaks bash and pinned Rust.

## Decision

Ship a versioned hook and an installer. No secret-store tools, no network, no extra
binaries beyond the Rust pin and Python 3.11+ already required to develop here.

- `scripts/pre-commit.sh` is the assertion script. It runs, in order:
  1. `python3 scripts/check-toolchain.py --print-rust` (pin drift fails the commit)
  2. `cargo +"$pin" fmt --all -- --check`
  3. `cargo +"$pin" clippy --locked --all-targets --all-features -- -D warnings`
  4. `cargo +"$pin" test --locked --all-targets --all-features`
- Those three cargo lines are the Rust subset of `scripts/check.sh` quality, with the same
  Clippy flag. Lint *levels* live in `Cargo.toml` (ADR 0006).
- `scripts/githooks/pre-commit` execs that script. `scripts/install-git-hooks.sh` sets
  `core.hooksPath` to `scripts/githooks` for this repository only and marks the scripts
  executable.
- Skip one commit with `JG_SKIP_HOOKS=1` or `SKIP=1`. Skipping is for a broken hook or an
  emergency; CI still runs the full isolated gate.
- Document installation in the README Development section. Do not mention maintainer
  secret tools or agent-only paths there.

The hook checks the working tree (what Cargo sees), not a stash of the index. Unstaged
Rust edits must also pass; that is simpler and safer than `git stash -k`.

Do not run ignored live tests. Do not call `fnox`, a secret manager, or the QA-tool
installer from the hook.

## Alternatives

- Running full `scripts/check.sh` on commit: rejected; isolation plus release plus PTY is
  a CI job, not a commit hook.
- fmt-only hook: rejected; it would let a red Clippy or a broken invariant be pushed.
- Python `pre-commit` / lefthook: rejected; second installer, easy drift from `check.sh`.
- `cargo-husky` writing into `.git/hooks` from a build.rs: rejected; surprising compile
  side effects.

## Consequences

After `scripts/install-git-hooks.sh`, a commit that fails fmt, Clippy, or tests is
refused locally. First-run cost is a full compile; later commits are incremental. People
who skip the installer still have CI. Changing Clippy flags requires editing both
`check.sh` and `pre-commit.sh` (enforced by `scripts/test_quality.py`). No user-visible
binary change; no release.

## Tests and verification

- `scripts/test_quality.py` checks the shared Clippy invocation, installer `hooksPath`,
  README mention, and the absence of secret-tool names in the hook scripts.
- `scripts/check.sh` runs ShellCheck on `scripts/*.sh` and `scripts/githooks/*`.
- Manual: `scripts/install-git-hooks.sh` then a no-op commit path, and `JG_SKIP_HOOKS=1`.

References: ADR 0003; Git `core.hooksPath`; README Development.
