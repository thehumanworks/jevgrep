---
name: release
description: Release a new version of jevgrep (jg) — decide whether a change needs a version bump, pick the semver level, then bump, tag and publish the binaries. Use after changing jg when the change might warrant a release, when asked to release, tag, publish, or ship a version, or when asked what version a change should get.
---

# Releasing jevgrep

## How a release works here

`version` in `Cargo.toml` is the source of truth. Pushing a `vX.Y.Z` tag runs
`.github/workflows/release.yml`, which builds `jg` for `aarch64-apple-darwin`,
`x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl` and attaches the tarballs to a
GitHub release. Users get that release through `mise exec github:thehumanworks/jevgrep -- jg`,
which always resolves to the newest tag, so **a version is not released until the tag exists and
its build is green**.

Tags are only ever cut from `main`, after the change has landed there. `scripts/release.sh`
enforces that.

## 1. Decide whether this change needs a release

Bump only when the change alters the binary users run:

| Change | Release? |
|---|---|
| Behaviour, output, flags, scoring, prompts, performance, dependency upgrades | Yes |
| Bug fix in `src/` | Yes |
| README, comments, tests, benchmarks, CI, this skill | No |
| Refactor with byte-identical behaviour | No — unless something else is already unreleased |

If `main` has unreleased user-visible commits from earlier work, fold them into the release you
are cutting now; check with `git log $(git describe --tags --abbrev=0)..main --oneline`.

## 2. Pick the level

`jg` is pre-1.0, so the rule is:

- **patch** (`0.2.0` → `0.2.1`) — bug fixes, performance, smaller binary, internal changes. Same
  flags, same output shape, same results.
- **minor** (`0.2.0` → `0.3.0`) — anything users would notice: new or removed flags, changed
  output format or exit codes, changed defaults, and **any change to the wording of the questions
  or filters sent to Jev**, because that changes which lines come back.
- **major** (`1.0.0`) — never cut one on your own. Ask the user; `1.0.0` is their call about
  stability, not a size-of-change judgement.

When torn between patch and minor, pick minor. A surprising result change is worse than a spent
version number.

## 3. Verify before releasing

Releases are public and awkward to retract, so run the checks in README "Development" that the
change touches — at minimum:

```bash
cargo test
cargo clippy --all-targets -- -D warnings && cargo fmt --check
```

If you changed prompt or filter wording, or anything that affects scoring, also re-run the
benchmark (`cargo build --release && fnox exec -- python3 bench/bench.py`, expects 15/15, 15/15,
9/9, zero rows below bar, zero duplicates) and the live tests
(`fnox exec -- cargo test --test live -- --ignored`). Do not release on a red benchmark.

## 4. Cut it

Ask the user before publishing unless they already asked for a release — this pushes to `main`
and creates a public artifact.

```bash
scripts/release.sh minor --dry-run   # bump, run tests, show the plan, change nothing
scripts/release.sh minor             # the real thing
```

The script refuses to run off `main`, with a dirty tree, behind `origin/main`, or on an existing
tag. It bumps `Cargo.toml`, refreshes `Cargo.lock`, runs `cargo test`, commits `release vX.Y.Z`,
pushes `main` and the tag, waits for the build, then checks that all three tarballs are attached,
that the release is marked latest, and that `mise exec ...@X.Y.Z -- jg --version` prints the new
version. Non-interactive callers must pass `--yes`.

It takes `patch`, `minor`, `major` or an explicit `X.Y.Z`. Report the release URL when it
finishes.

## If it goes wrong

A failed build leaves a tag and possibly a release behind; a half-published version is worse than
no version. Delete both, fix the cause, and re-run the script with the same number:

```bash
gh release delete vX.Y.Z --yes --cleanup-tag   # or: git push origin :refs/tags/vX.Y.Z
```

If the tag is fine but the build was flaky, just re-run the workflow instead:
`gh run rerun <run-id> --failed`.
