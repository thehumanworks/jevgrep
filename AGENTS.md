# Instructions for coding agents

`jg` (jevgrep) is a single Rust binary that searches code with a natural-language query. The
README is the reference for behaviour, flags and development; `docs/adr/` records why things are
the way they are. This file is what an agent needs to know that those do not say.

## Releasing is the agent's job

When the maintainer asks for a release, a version bump, a new tag or a published version, **you
run `scripts/release.sh` yourself, to the end**. Do not hand the command back to the maintainer,
do not stop after the dry run, and do not ask them to bump `Cargo.toml`, tag or push by hand.
The request is the authorization: it covers the commit to `main`, the tag, both pushes and the
public GitHub release that the tag builds.

1. Decide the level from `git log $(git describe --tags --abbrev=0)..main --oneline` and the
   rules in [`.claude/skills/release/SKILL.md`](.claude/skills/release/SKILL.md): pre-1.0, a
   user-visible change is **minor**, fixes and internals are **patch**, and **major** is only
   ever the maintainer's call. Say which level you chose and why, in one line, then carry on.
2. Verify: `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, then
   `scripts/release.sh <level> --dry-run`, which runs the whole test suite.
3. Cut it: `scripts/release.sh <level> --yes`. `--yes` is required without a terminal. It takes
   several minutes because it waits for the GitHub build; run it in the background and wait for
   it. Do not pipe its output through a filter: a broken filter kills the script and hides its
   exit status. Redirect to a log file if the output is too long.
4. Check the result rather than trusting the exit status: `gh release view vX.Y.Z` shows three
   tarballs and the release is marked latest. Report the release URL.

If the script fails half way, clean up and re-run it with the same number, as the skill's "If it
goes wrong" section describes. A half-published version is worse than none.

The only reasons to stop and hand back are a red test or benchmark, a **major** bump, or a
harness that refuses to let you run the script. In the last case say exactly which command was
blocked and what permission rule would allow it (for Claude Code: `Bash(scripts/release.sh:*)`
under `permissions.allow` in `.claude/settings.json`), so that the next release is not blocked
again. Changes that do not alter the binary (docs, tests, CI, this file) need no release.

## Working in this repository

- `scripts/check.sh` is the quality gate CI runs. Where its pinned tools are not installed, run
  its steps by hand: `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check`.
- `tests/fixtures/help.txt` is the exact `jg --help` output. After changing help text or flags,
  regenerate it with `cargo build --release && target/release/jg --help > tests/fixtures/help.txt`
  and read the diff.
- The wording of the questions and filters sent to Jev is behaviour: changing it changes which
  lines come back. Re-run the benchmark (README, "Development") before releasing such a change.
- Live calls cost money or quota and send code to a third party. Send only public code or this
  repository, never the maintainer's other projects, and run ignored live tests and benchmarks
  only when asked.
- `jg` never launches a secret manager for the `openai` backend's key, and no backend is added
  per service: an OpenAI-compatible service is a base URL, a key and a model (ADR 0005).
- Commit or push only when asked. Releases are the exception above: asking for one asks for its
  commit, tag and pushes.
