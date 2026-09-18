# ADR 0001: Declarative CLI parsing

- Status: Accepted
- Date: 2026-09-18
- Baseline: `72d526d` (jevgrep 0.2.0)
- Scope: parser definitions and normalization; execution, presentation and dependency wiring are separate work.

## Context

The handwritten lexopt parser duplicates defaults and help, accepts nonfinite/out-of-range probabilities, and lets zero jobs/chunk sizes reach later clamps. We need generated help and typed validation without changing search, filter or output selection rules. This decision was recorded before implementing the replacement.

A pre-change probe linked the existing baseline library and confirmed query-only defaults, mandatory positional QUERY despite `-e`, trimming/empty-query removal, leading-hyphen option values, scalar last-value-wins, repeated booleans, `--`, and acceptance of zero jobs/chunks and NaN. `cargo +1.98.1 test --locked --lib` passed but contained **zero unit tests**; it is not evidence of the full baseline suite. Existing `tests/cli.rs` additionally characterizes combined shorts, attached/equals values, options after paths, help/version, query execution and output precedence. Source inspection confirms the zero semantics listed below.

## Decision

Use clap derive for a private typed command definition and normalize it into the existing public `Args`. Preserve `Parsed::{Run(Box<Args>), Print(String)}` and `parse_args(Vec<OsString>) -> Result<Parsed, String>`. Callers continue passing arguments **without** the executable; the parser prepends `jg`. Parsing never exits the process. The parent runner prints `Print` on stdout and returns 0, and prints the already formatted `Err(String)` directly on stderr and returns 2.

Use clap `args_override_self` for repeated scalar and boolean options. Collection options append in encounter order. Value-taking options consume leading-hyphen values, including flag-looking strings, as lexopt did. Positional values beginning with a hyphen require `--`. Keep QUERY required even with `-e`. Normalize positional query first, then extras in encounter order, trim each query and remove empty queries. A supplied but wholly empty query list still produces `Run` with empty queries; execution retains its `jg: empty query` error. Keep paths untouched and use an empty path vector to mean the current directory in discovery.

Resolve only JG_MODEL and JG_BASE_URL after successful parsing, through an explicitly injected lookup function. Precedence is explicit flag (including empty string) > nonempty Unicode environment value > existing client default. Empty/unset/non-Unicode environment values fall back; whitespace-only values remain nonempty, matching the baseline. Unit tests inject lookups and never mutate the process environment. Help, version and errors do not even read application environment. API keys and fnox remain outside the parser.

Keep generated help/errors plain (`ColorChoice::Never`) and use a fixed 100-column command width. Clap must enable both `derive` and `wrap_help`; `term_width(100)` alone does not wrap help without the latter feature. Both help spellings include examples, filters, output and environment sections. `--color` controls later application presentation, not clap. Do not add output-mode conflicts: files mode still wins over region/line output, and JSON wins over flat formatting within the selected mode.

### Complete option inventory

All strings are Unicode; malformed/overflowing integers are errors. `append` means every occurrence is retained; `last` means last value wins; `set` means repeated occurrences remain true. No option has an environment source except those explicitly listed. Numeric defaults are generated from the typed command definition.

| Spelling (all aliases) | Type / default | Repetition / validation / interaction |
| --- | --- | --- |
| `QUERY` | required string | One positional query; trimmed and empty removed, before all extras |
| `PATH ...` | string list / empty (discovery uses `.`) | Positional order retained; no trimming |
| `-e`, `--query QUERY` | string list / empty | append; trimmed and empty removed; does not satisfy QUERY |
| `-l`, `--files` | bool / false | set; files-only ranking, compatible with JSON/flat |
| `-f`, `--filter TEXT` | string list / empty | append; filter parsing remains in execution |
| `--only CATEGORY` | string list / empty | append; every category must hold |
| `--not CATEGORY` | string list / empty | append; exposed as `Args.exclude_terms` |
| `-b`, `--broad` | bool / false | set |
| `-t`, `--threshold P` | f64 / 0.5 | last; finite and in inclusive `[0, 1]` |
| `-T`, `--file-threshold P` | f64 / 0.6 | last; finite and in inclusive `[0, 1]` |
| `-n`, `--top N` | usize / 15 | last; zero retained (no selected files) |
| `-m`, `--max-lines N` | usize / 10 | last; zero retained (no pinpointed lines) |
| `--max-regions N` | usize / 5 | last; zero retained (no regions) |
| `-C`, `--context N` | usize / 0 | last; zero retained (no context) |
| `-g`, `--glob GLOB` | string list / empty | append |
| `-x`, `--exclude GLOB` | string list / empty | append |
| `-j`, `--jobs N` | usize / 32 | last; strictly positive |
| `--json` | bool / false | set; wins over flat formatting |
| `--no-heading` | bool / false | set; flat output, no weaker tier; no conflicts |
| `--triage` | bool / false | set; also automatic above max-files |
| `--max-files N` | usize / 1500 | last; zero retained (triage then truncate to zero) |
| `--max-filesize BYTES` | u64 / 512000 | last; zero retained (skip nonempty files) |
| `--chunk-lines N` | usize / 150 | last; strictly positive |
| `--hidden` | bool / false | set |
| `--no-ignore` | bool / false | set |
| `-q`, `--quiet` | bool / false | set; does not suppress genuine errors |
| `--model MODEL` | string / `jev-latest` | last; JG_MODEL fallback; explicit empty preserved |
| `--base-url URL` | string / `https://api.typesafe.ai/v1/systemone` | last; JG_BASE_URL fallback; no new URL validation |
| `--color WHEN` (new) | ColorMode / auto | last; clap ValueEnum `auto`, `always`, `never` (case-sensitive) |
| `-V`, `--version` | generated action | Print `jg <Cargo package version>\n`; QUERY unnecessary |
| `-h`, `--help` | generated action | Print full generated help; QUERY unnecessary |

### Validation and intentional compatibility changes

- Reject probabilities outside `[0, 1]`, NaN and infinities before discovery/API access.
- Reject zero jobs and chunk-lines instead of clamping later. Retain zero for all other numeric options.
- Help and usage-error wording/layout are generated by clap, rather than byte-compatible with handwritten help. The runner must not add another usage/error wrapper.
- Add public `ColorMode::{Auto, Always, Never}` with default Auto and `Args.color`.
- Raise the toolchain/MSRV to the verified Rust 1.98.1; keep edition 2021. All three project declarations use that exact version.
- No other intentional grammar, query, environment or output-mode changes.

## Alternatives

Keeping lexopt would minimize dependency and binary-size growth, but retain duplicated documentation, manual validation and a growing handwritten grammar. A custom clap builder would work but lose the compact typed derive definition. Clap's implicit environment integration is not used: explicit normalization preserves empty-value semantics and makes environment-sensitive unit tests hermetic. A TUI is not selected because this is a composable grep-style command with strict stdout/stderr and machine-output contracts, not an interactive application.

## Consequences

Clap derive adds dependencies, build time and binary size; the release size delta and lockfile must be reviewed. In return, the command definition supplies option metadata, help, diagnostics and validators. Public normalized argument fields remain compatible except for the additive color field (downstream struct literals must supply it). Execution remains responsible for API-key resolution, empty-query diagnostics, output precedence and exit statuses. No new global mutable state is introduced.

## Tests and verification

- Parser unit tests: [`src/cli/args.rs`](../../src/cli/args.rs), under `cli::args::tests`: complete option metadata/defaults, aliases, collections, repeated scalar/boolean flags, grammar, numeric boundaries/errors/zero semantics, normalization, explicit environment precedence, generated help/version/errors, and `CommandFactory::command().debug_assert()`.
- End-to-end stream/exit/no-side-effect coverage: [`tests/cli.rs`](../../tests/cli.rs).
- Existing downstream zero/selection behavior: [`src/results.rs`](../../src/results.rs), [`src/files.rs`](../../src/files.rs), [`src/search.rs`](../../src/search.rs).
- Plan: [`docs/plans/cli-modernization.md`](../plans/cli-modernization.md).
- Clap derive and command API: <https://docs.rs/clap/latest/clap/_derive/index.html>, <https://docs.rs/clap/latest/clap/struct.Command.html>.

### Verification (2026-09-18)

The parser has 21 focused tests, all passing under Rust 1.98.1. The full isolated
`scripts/check.sh` also passes: 94 Rust tests, 8 live tests ignored, 24 helper tests,
formatting, Clippy, doctests, workflow/shell lint, release build and real PTY checks.
`tests/fixtures/README.md` documents baseline output capture and the intentionally
changed help/error fixtures. The final implementation report records remote evidence.

The host release binary grew from 2,782,128 to 3,074,552 bytes (+292,424; 10.51%).
This buys typed derive parsing/help, Unicode-aware styles, and tested progress cleanup.
The lockfile adds 42 packages (including platform-only and dev-only dependencies),
removes lexopt, and changes no preexisting dependency versions. The compiler updates
the lockfile format from 3 to 4. No unrelated dependency refresh was performed.
