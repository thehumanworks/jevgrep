# ADR 0007: Tests as the specification

- Status: Accepted
- Date: 2026-09-20
- Baseline: existing `tests/` tables, the seeded generator in `tests/results.rs`, ADR 0001 option inventory
- Scope: how requirements for filters, globs, display rules, answer decoding, and CLI contracts
  are recorded. Not a new test runner. Not a formal verifier.

## Context

The properties that decide whether `jg` is correct — display rules, filter polarity, glob
matching, CLI validation, answer decoding — are ordinary Rust. Comments in `results.rs` and
`filters.rs` already read like a spec; they are not executable. One-off regressions (“this
query printed a file with no location”) catch a single story. They do not say what must stay
true for every input.

Current Rust practice for this class of code is: types first, then example tables for the
cases a human decided, then property tests for the rules that must hold everywhere. The
usual crate is `proptest` (strategies, shrinking, `.proptest-regressions`). This repository
already has a seeded LCG generator in `tests/results.rs` that is deterministic, has no extra
crate, and finishes in the isolated `scripts/check.sh` budget. Adding `proptest` would
compile a large graph on every CI isolate for shrinking we can live without: the inputs here
are small (filter terms, globs without a full language, in-memory `FileResult`s).

A formal verifier was considered separately and rejected for this product. Tests remain the
spec agents keep.

## Decision

State requirements as tests. A module comment may explain *why*; it is not a substitute.

1. **Tables** for decided examples: `parse_filter` phrases, `fnmatch` / character classes,
   `is_definition` openers, CLI probability and integer bounds (already in `cli::args::tests`),
   option inventory vs generated help, request question-id shapes.
2. **Property-style tests** with a seeded generator (the existing LCG) for rules that must
   hold on arbitrary inputs:
   - `Rules::passes` is in `[0, 1]`, empty rules pass, polarity is monotonic.
   - `fnmatch` agrees with a star/`?` oracle on generated patterns (classes stay tabular).
   - `select` / `view_file` obey the six display rules for random answers; `--top` and the
     weak cap hold; `select_files` is gated and sorted; `filtered_out` counts only strong
     rows the filter removed.
   - `chunk_lines` / `split_blocks` cover every non-blank line at most once and keep
     `askable` inside the window.
   - Answer `score` is the probability-weighted mean of level indices; `decode_answers`
     is a bijection on wire ids.
3. **No new test framework.** Do not add `proptest`, `quickcheck`, or `rstest` unless a
   future change needs shrinking or fixtures that the LCG cannot express. Commit any
   handwritten regression that a generator finds next to the property.
4. **Question wording is behaviour.** Tests may lock the documented instruction templates
   (`directly answer` vs `relevant to`, positive filter questions). Changing those strings
   is a product change; re-run the live benchmark before a release, as `AGENTS.md` already
   requires.

Prefer asserting a relationship (`decode(encode(x))`, `min`/`max` polarity, “no line
printed twice”) over reimplementing the function in the test. When the test *is* an oracle
(star/`?` glob, weighted mean), keep the oracle smaller and obvious.

## Alternatives

- Comments-only specs: rejected; agents and refactors will drift.
- `proptest` now: rejected; compile cost in isolated CI, and the current generator already
  covers the input space we can name.
- Snapshot-only CLI tests: kept where they exist (`tests/fixtures/help.txt`); they do not
  replace the option-inventory and validation tables.

## Consequences

A change to display rules, filter polarity, globs, decoding, or the CLI grammar must update
a table or a property, not only a comment. Properties use fixed seeds so CI is deterministic.
New one-off bugs still get a named regression test (the existing style). No user-visible
binary change is required by this decision; if a property finds a bug, fix it and note it.

## Tests and verification

Implementation: `tests/filters.rs`, `tests/files.rs`, `tests/results.rs`,
`src/answers.rs` `tests`, `src/cli/args.rs` `tests`, `tests/client_search.rs`.
`cargo test --locked --all-targets --all-features` is the executable spec. ADR 0008 runs
that command on commit; ADR 0003 still runs it inside isolated `scripts/check.sh`.

References: existing `tests/results.rs` generator; Clippy/testing practice above; the
proptest book (consulted, not adopted).
