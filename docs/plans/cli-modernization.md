# CLI modernization: implementation handoff

Status: planned; implementation has not started.
Prepared: September 18, 2026.
Repository: `/home/tomas/Projects/jevgrep` (`thehumanworks/jevgrep`).
Reviewed baseline: commit `72d526d`, package `jevgrep` 0.2.0, executable `jg`.

## 1. Objective and scope

Integrate `clap`, `console`, and `indicatif`; upgrade the Rust toolchain and declared minimum Rust version; create ADRs; expand CLI contract tests; and require lint, test, and release-target build checks in GitHub Actions.

This is an implementation plan, not evidence that the migration or CI verification has happened. In a fresh session, inspect the actual checkout and adapt paths or ADR numbering if the project has changed.

In scope:

- `clap` derive-based argument definitions, generated help, and explicit value validation.
- `console` styling and display-width-aware truncation for human output.
- `indicatif` progress reporting on stderr, with predictable cleanup and log handling.
- A documented, tested `--color auto|always|never` policy.
- Consistent, pinned Rust versions across development, Cargo, and CI.
- ADRs, maintainable lint rules, hermetic tests, and shared CI/release validation.
- Actual GitHub Actions verification of the final implementation commit, when remote access and authorization permit.

Out of scope:

- Changes to Jev prompts, ranking, filters, selection rules, request scheduling, or retry policy.
- A full-screen TUI, new output formats, or a redesign of the JSON schema.
- A Rust edition migration: keep edition 2021 unless a concrete requirement emerges.
- Live/paid API tests, benchmark API calls, publishing, tagging, merging, or pushing directly to `main`.
- Repository branch-protection/settings changes without separate authorization.

Do not revert existing user work. Do not run `scripts/release.sh`, even as a shortcut for QA.

## 2. Known starting point

| Area | Current implementation |
| --- | --- |
| Parser | `lexopt = "0.3"`; handwritten parsing, help, and defaults in `src/cli.rs` |
| Output | Custom `Write`-based text/JSON renderers in `src/cli.rs`; `serde_json` already handles JSON |
| Selection | Domain rules live in `src/results.rs`; keep these separate from presentation |
| Terminal | `IsTerminal`, manual progress writes, and raw erase-line sequences |
| Rust | Cargo minimum 1.82; `rust-toolchain.toml` uses floating `stable`; `mise.toml` pins 1.98.1 |
| Local override | At review, `RUSTUP_TOOLCHAIN` selected 1.98.1; do not assume the toolchain file controls every local command |
| CI | Only `.github/workflows/release.yml`; tag/manual builds, no PR quality workflow |
| Release targets | `aarch64-apple-darwin`, `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl` |
| Tests | Existing fake-HTTP integration tests; prior review: 46 passed, 8 live tests ignored |

The prior test result is historical context. Rerun the baseline before implementation.

Confirmed parser gaps to address: thresholds accept `NaN`, infinity, and values outside `[0, 1]`; `--jobs 0` is silently clamped later rather than rejected. See `src/cli.rs:136-217` and `src/cli.rs:444-460` at the reviewed commit.

## 3. Rust version decision

The official stable distribution manifest reported **Rust 1.98.1**, with manifest date **September 3, 2026**, when checked on **September 18, 2026**. Source: [S1].

At implementation start, fetch that manifest again. If a newer stable release exists, use that release instead and record the exact version and verification date. Do not infer "latest" from the installed compiler or a floating CI image.

- Pin `rust-toolchain.toml` to the verified exact version, with `rustfmt` and `clippy` components.
- Set `Cargo.toml` `rust-version` and the `mise.toml` Rust version to that same version. For the currently verified release, all three values become `1.98.1`.
- Treat `rust-toolchain.toml` as the canonical toolchain pin; add a checked consistency test for the other two declarations.
- Explicitly install and invoke the pinned toolchain in CI. Log `rustc --version --verbose` and `cargo --version`.
- Guard against `RUSTUP_TOOLCHAIN`/mise overrides by invoking `cargo +<verified-version>` in the QA entrypoint.
- Commit the updated `Cargo.lock`. Select supported stable crate releases and review the dependency diff; avoid an unrelated full dependency refresh.
- Record that this deliberately raises the supported minimum Rust version. Do not claim compatibility with Rust 1.82 afterward.
- Future compiler upgrades should be explicit, reviewed pin updates, not silent movement of `stable`.

Verification example; Python is only a planning/development tool here:

```bash
curl -fsSL https://static.rust-lang.org/dist/channel-rust-stable.toml \
  | python3 -c 'import sys,tomllib; m=tomllib.loads(sys.stdin.read()); print(m["date"], m["pkg"]["rust"]["version"])'
```

## 4. ADRs to create before changing behavior

Create `docs/adr/README.md` with an index and three ADRs, using the next available numbers:

| Proposed file | Required decisions |
| --- | --- |
| `0001-declarative-cli-parsing.md` | Why `clap` replaces `lexopt`; typed arguments and normalization; validators; repeated-option behavior; environment precedence; intentional compatibility changes; dependency/build-size tradeoff |
| `0002-terminal-presentation-and-progress.md` | Responsibilities of `console` and `indicatif`; stdout/stderr separation; color policy; machine-output guarantees; Unicode width; progress lifecycle; deterministic testing |
| `0003-rust-toolchain-and-quality-gates.md` | Exact toolchain/MSRV pin and update policy; lint rules; shared local/CI checks; supported release targets; publication gates; required CI evidence |

Each ADR must contain status, date, context, decision, alternatives, consequences, compatibility impact, and links/references to its tests. Explain why keeping `lexopt` or adopting a TUI was not selected. Keep ADRs synchronized with the final implementation; do not leave rejected or unimplemented decisions marked accepted.

## 5. Migration sequence

### Phase A — Establish and freeze the contract

1. Read applicable project instructions, `README.md`, `.claude/skills/release/SKILL.md`, manifests, source, tests, and workflows.
2. Inspect the working tree and branch. Record the baseline commit and existing modifications.
3. Run existing tests, Clippy, formatting, and a release build. Record the host release binary size for later comparison.
4. Inventory every existing option: name, alias, type, default, repeat behavior, validation, environment source, and interactions. Store this in the parser ADR or a referenced test matrix.
5. Add characterization tests before replacing the parser. Capture help/version/error behavior and representative output fixtures. Do not merely regenerate snapshots after a regression.
6. Write the initial ADRs, including the deliberate changes listed below.

### Phase B — Upgrade Rust and establish lint policy

1. Reverify stable Rust and synchronize all three version declarations.
2. Keep edition 2021 and the existing release optimization settings.
3. Add narrowly scoped lint policy to `Cargo.toml`, using Cargo's supported lint tables [S2]:

```toml
[lints.rust]
unsafe_code = "forbid"

[lints.clippy]
all = { level = "warn", priority = -1 }
dbg_macro = "deny"
todo = "deny"
unimplemented = "deny"
```

4. Run Clippy with `-D warnings`; fix warnings rather than adding crate-wide suppression. Any necessary exception must be local and explained. Do not enable all pedantic/restriction lints or ban all test `unwrap()` calls as part of this migration.
5. Keep `rustfmt.toml`; require the pinned formatter. Add workflow linting with a verified, pinned `actionlint` release. Lint changed shell scripts as well if shell tooling is introduced or modified.
6. Establish one local QA entrypoint, preferably `scripts/check.sh`, that CI also uses. It must fail on the first failed required check and must not call release/publishing commands.

### Phase C — Integrate clap

1. Add `clap` with derive support [S6]; enable `env` only where it preserves the established environment semantics. Remove the direct `lexopt` dependency after migration.
2. Separate argument definitions/normalization from execution and rendering. A reasonable layout is `src/cli/{mod,args,render,progress}.rs`; preserve existing `jevgrep::cli` entrypoints/re-exports where practical.
3. Use fallible parsing (`try_parse_from` or the equivalent command API), not process-exiting parsing inside testable library code. Existing callers pass argv without the executable name; adapt this explicitly.
4. Define the command name as `jg`, keep version output tied to Cargo package metadata, and generate option help/defaults from the parser definition. Retain the useful examples, filter explanation, output guide, and environment documentation as extended help.
5. Preserve short aliases, combined flags such as `-lq`, attached values such as `-t0.99`, `--top=3`, options after positionals, and the `--` terminator.
6. Preserve required positional `QUERY`, optional paths/default current directory, repeatable `-e/-f/--only/--not/-g/-x`, query ordering, and query trimming/empty-query behavior. `-e` alone must not silently replace the required positional query.
7. Preserve last-value-wins behavior for repeated scalar options and acceptance of repeated boolean flags, unless a documented intentional change is necessary. Do not inherit different clap defaults accidentally.
8. Preserve precedence: explicit flags override environment; unset or empty `JG_MODEL`/`JG_BASE_URL` fall back to existing defaults. Keep API-key/fnox resolution outside the parser.
9. Validate both probability thresholds as finite values in `[0, 1]`; validate `jobs` and `chunk-lines` as positive integers. Reject malformed/overflowing numbers with exit 2 before file discovery or API access.
10. Do not indiscriminately reject zero for every numeric option: characterize existing zero behavior for context, top, caps, and size limits, then retain it unless explicitly documented otherwise.
11. Preserve current mode precedence when `--json`, `--no-heading`, and `--files` are combined. Do not add new clap conflicts merely because flags seem redundant.
12. Preserve stdout/help/version versus stderr/errors and exit codes 0/1/2. Help wording/layout may change intentionally, but required content and semantics must remain tested.
13. Add a clap command-definition consistency test using `CommandFactory::command().debug_assert()`.

Intentional changes to document: generated help/error presentation, invalid probability rejection, rejection of zero jobs/chunk size, `--color`, and the higher minimum Rust version. Record any further change rather than silently broadening the scope.

### Phase D — Integrate console

1. Keep selection and JSON construction unchanged. Continue accepting injected `Write` destinations rather than hardwiring terminal writes into renderers.
2. Add a small presentation/palette abstraction. Style file headings, match metadata, and weaker-tier notices; keep text labels so color is not the only way to distinguish meaning.
3. Use `console` width/truncation utilities [S3]. Preserve existing plain ASCII layouts and existing truncation budgets; do not introduce terminal-size-dependent wrapping in this iteration.
4. Resolve color as pure policy inputs, independently for stdout and stderr. Apply per-style forced enable/disable rather than relying on mutable process-global color state in tests.
5. Implement and document this precedence:
   - JSON and flat result output never receive application-added styling, even with `--color always` or forcing environment variables.
   - Explicit `--color never` disables styling; explicit `--color always` enables eligible human-output styling.
   - In `auto`, nonempty `NO_COLOR` or `TERM=dumb` disables styling.
   - Otherwise, in `auto`, nonzero/nonempty `CLICOLOR_FORCE` enables eligible styling, `CLICOLOR=0` disables it, and terminal detection supplies the default.
6. Keep clap-generated help and usage errors plain in this iteration. Document that `--color` controls application presentation; do not add another handwritten argv parser merely to recolor clap's early help/error exits.
7. Never route JSON/flat output through a terminal-rendering API. Preserve numeric precision, stable JSON key order, schemas, and payload text.
8. Preserve broken-pipe success behavior and propagation of other output errors. Add tests using failing writers, not only string snapshots.

### Phase E — Integrate indicatif

1. Replace manual progress and erase-line sequences with a small progress/reporting adapter around `indicatif` [S4]. Use a simple chunks-completed template; do not add progress flags, ETA, or a full UI unnecessarily.
2. Draw only on stderr when stderr is an eligible terminal, `TERM` is not `dumb`, and `--quiet` is false. Stdout being redirected must not by itself disable an interactive stderr progress display.
3. Keep progress hidden for redirected stderr and quiet mode. Quiet suppresses progress/stats, not genuine errors.
4. Route ordinary notes through a suspend/println mechanism that does not corrupt the active progress line. Preserve existing plain notes/stats for noninteractive stderr.
5. Explicitly finish and clear progress before final diagnostics/stats and on success, API/auth errors, no-result paths, and early output failure. Test cleanup; do not assume dropping the bar is sufficient.
6. Keep the initial progress template uncolored so it does not introduce a competing color policy. Do not change search scheduling or callback semantics to implement progress.
7. Inject progress state/draw targets for deterministic tests. Avoid sleep-based snapshots and dependence on wall-clock animation frames.

## 6. Required test matrix

Prefer existing fake-server helpers and ordinary assertions/checked-in fixtures. Add a snapshot or PTY test dependency only when it materially simplifies coverage; keep such dependencies dev-only.

| Surface | Required coverage |
| --- | --- |
| Option inventory | Every existing option, alias, default, repeatable collection, and scalar override behavior |
| Argument grammar | Required/missing/empty query; extra queries; multiple paths; clustered shorts; attached values; equals syntax; options before/after positionals; `--`; leading-hyphen values; unknown flags; missing values |
| Numeric validation | Threshold boundaries 0 and 1; negative/>1 values; NaN and infinities; malformed and overflowing integers; positive jobs/chunk size; retained zero semantics for other options |
| Environment | Explicit flag > environment > default; empty environment fallback; query-only invocations; help/version without API keys, file access, or fnox invocation |
| Generated help/errors | `-h/--help`, `-V/--version`, flag/default coverage, preserved explanatory sections; stream and exit-code checks; fixed-width, color-free fixtures |
| Output modes | Grouped text, JSONL, flat, files-only text/JSON, context, multi-query combinations, no matches, weaker tier, caps, and combined-mode precedence |
| Existing selection rules | Probability bars, relevance ordering, no duplicated source lines, cap summaries, region labels; retain existing randomized invariant tests |
| JSON/flat contracts | Exact existing fixtures plus semantic JSON assertions; stable key order/precision; no application-added ANSI/progress under any color policy |
| Color policy | auto/always/never; NO_COLOR; TERM=dumb; CLICOLOR/CLICOLOR_FORCE; conflicting settings; stdout/stderr terminal states independently |
| Width and styles | Long ASCII, accented text, CJK, combining characters, emoji, narrow truncation budgets, style reset; no invalid UTF-8 or width accounting based on ANSI bytes |
| Progress | Hidden/visible policy, count updates, notes during progress, quiet, success/error cleanup, redirected output; deterministic in-memory checks |
| Real terminal wiring | Bounded PTY smoke cases with local fake API: stdout pipe + stderr TTY; stdout TTY + stderr pipe; both pipes; TERM=dumb; quiet; timeout and process cleanup |
| IO/exit behavior | 0/1/2 statuses; help/version stdout; diagnostics stderr; broken pipe exits successfully; other writer/flush errors fail |
| Build/toolchain | Pin consistency, clap command consistency, debug/release compilation, all three packaged release binaries |

Important test rules:

- Rerun and extend `tests/cli.rs`, `tests/results.rs`, and `tests/client_search.rs`; do not replace existing assertions with weaker substring checks.
- Put pure parser/color/progress policy tests near the implementation or in focused test modules. Test all option metadata even where an end-to-end test would be redundant.
- Use child processes for environment-sensitive integration tests. Do not mutate shared environment/color globals in parallel tests.
- Isolate HOME, XDG config/cache/data paths, application environment, and global git config. Preserve access to the compiler/cache intentionally, not to user application configuration.
- Remove real API credentials, set `JG_NO_FNOX=1`, and use local fake endpoints. Normal CI must never run ignored live tests or require secrets.
- Add a sentinel test proving help/version and invalid-argument paths do not invoke fnox or the API.
- Fix locale, width, and relevant environment for fixtures. Normalize only inherently variable fields; retain meaningful formatting checks.
- PTY tests must use timeouts and clean up child processes/servers. Test terminal control behavior without snapshotting animation timing.
- Keep timing/cost fields out of brittle exact snapshots. Retain appropriate format assertions.

## 7. GitHub Actions design

Create a shared validation/build workflow and thin callers, so PR and release builds use the same logic. Reusable workflows use `workflow_call` [S5]. Suggested files:

- `.github/workflows/checks.yml`: reusable quality gates, tests, release-target builds, smoke tests, and artifact creation.
- `.github/workflows/ci.yml`: calls checks on pull requests, pushes to `main`, and manual dispatch.
- `.github/workflows/release.yml`: calls the same checks for tag/manual runs; only the existing tag-gated publication job has write permissions.

Required behavior:

1. Pin/install the toolchain from the project declaration in every Rust job. Verify synchronized version declarations. Do not rely on the runner's preinstalled stable version.
2. Require formatting, Clippy with warnings denied, tests, doctests, toolchain consistency, and workflow linting. Run `actionlint` over all workflows and test any helper scripts it cannot check.
3. Build all currently supported targets on every PR, not just after tagging:

   | Target | Existing runner configuration to verify |
   | --- | --- |
   | `aarch64-apple-darwin` | `macos-15` |
   | `x86_64-unknown-linux-musl` | `ubuntu-24.04` |
   | `aarch64-unknown-linux-musl` | `ubuntu-24.04-arm` |

4. Verify actual runner CPU architecture and supported runner labels, especially macOS, before retaining these mappings. Use native runners for executable tests; do not assume a cross-compiled binary can run on the host.
5. Keep musl C-toolchain setup needed by `ring`, and the existing static Linux packaging contract. Run the fake-API integration suite on the native target builds where supported; at minimum all three release binaries must run help/version and a local fake-API smoke case.
6. Run release builds with `--locked`; ensure all library features needed by the production binary are present. CI must not silently update `Cargo.lock`.
7. Preserve archive names, directory layout, README inclusion, and SHA256 sidecars. Download/extract artifacts and verify checksums plus packaged binary help/version, not only the prepackaging binary.
8. Make publishing depend on every quality/build job. A failed lint, test, or target must prevent publication. Preserve the tag-versus-Cargo-version check.
9. PRs and manual builds must never publish releases. Default to `contents: read`; restrict `contents: write` to the tag-gated publish job. Do not use `pull_request_target` to execute PR code or expose credentials to tests.
10. If using caches, key them by toolchain, target, and lockfile and avoid treating cached success as verification. Keep caches optional for correctness.
11. Verify chosen action versions and pin external actions/tool installers to reviewed revisions. Do not downgrade the existing Node-24-capable actions accidentally.
12. Use stable check names, bounded job timeouts, and appropriate concurrency. Do not let a reusable workflow's concurrency group cancel its own caller.
13. Report which check names should be required by branch protection. Do not silently change repository settings.

## 8. Required local checks and remote evidence

Implement the QA entrypoint around these commands, substituting the freshly verified pinned version. Run tests with the isolated environment described above.

```bash
# 1.98.1 is the verified planning value; read the actual synchronized pin in the script.
RUST_VERSION=1.98.1
rustup toolchain install "$RUST_VERSION" --profile minimal --component rustfmt --component clippy
rustc +"$RUST_VERSION" --version --verbose
cargo +"$RUST_VERSION" --version
cargo +"$RUST_VERSION" fmt --all -- --check
cargo +"$RUST_VERSION" clippy --locked --all-targets --all-features -- -D warnings
cargo +"$RUST_VERSION" test --locked --all-targets --all-features
cargo +"$RUST_VERSION" test --locked --doc --all-features
cargo +"$RUST_VERSION" build --locked --release
./target/release/jg --help
./target/release/jg --version
actionlint
# Also run the new pin-consistency check and any helper-script checks.
git diff --check
```

Keep tool installation separate from routine checks if that makes repeated local runs faster. Do not run live API tests merely to validate a UI migration.

After local checks:

- Review diff, dependency tree, test count, and release binary-size delta against the baseline. Explain material growth; no arbitrary zero-growth requirement.
- Obtain remote-write authorization if not already granted. Push only a feature branch/open or update a PR; never push directly to `main` for this task.
- Observe actual GitHub Actions runs for the exact final commit. Record commit SHA, run IDs/URLs, job conclusions, target results, and artifact verification.
- Fix CI-specific failures and rerun affected checks. A local build or valid YAML is not evidence of GitHub Actions success.
- If GitHub authentication, repository permissions, runner availability, or push authorization blocks remote verification, finish the local work and report the specific blocker. Mark remote CI **unverified** rather than claiming completion.

## 9. Completion checklist

- [ ] All three libraries are integrated and direct `lexopt` usage is removed.
- [ ] Stable Rust is freshly verified, pinned consistently, and actually used by local/CI checks.
- [ ] ADR index and three accepted ADRs match the implemented decisions.
- [ ] Every existing flag/default/interaction has contract coverage.
- [ ] Intentional parser/help/color/MSRV changes are documented and tested.
- [ ] JSON/flat contracts, selection invariants, IO handling, and exit codes are preserved.
- [ ] Color and progress obey per-stream, quiet, redirected, dumb-terminal, and machine-output policies.
- [ ] Tests are deterministic, isolated, credential-free, and include real PTY wiring checks.
- [ ] Formatting, linting, tests, doctests, workflow checks, and release builds pass.
- [ ] All three packaged release targets are verified in GitHub Actions at the final commit.
- [ ] Release publishing is gated on the same checks; no release was created during implementation.
- [ ] README documents the new toolchain, color behavior, validation, and local QA command.
- [ ] Final report lists work, test counts, CI evidence, artifact checks, binary-size delta, intentional changes, and remaining blockers.

The new flag and visible behavior changes imply a **minor** release under the existing pre-1.0 policy (0.3.0 if the project is still at 0.2.0). Record that recommendation; do not bump/tag/publish as part of this task unless separately authorized.

## 10. Fresh-session starter prompt

> Implement the plan in `docs/plans/cli-modernization.md` for jevgrep. Start by checking the current checkout and project instructions. Reverify the latest stable Rust release rather than trusting the planning version. Integrate clap, console, and indicatif; create the ADRs; preserve the documented CLI contracts; add the full test coverage and lint rules; and share quality/build gates between PR and release workflows. Finish all unblocked local implementation and verification. Ask before remote writes unless I have already authorized them. Do not merge, tag, publish, run live API tests, or change branch protection. Report actual GitHub Actions results for the final commit, or explicitly state why remote verification remains blocked. No further library-selection discussion is needed; those choices are approved.

## Sources checked during planning

- [S1] Official Rust stable distribution manifest: `https://static.rust-lang.org/dist/channel-rust-stable.toml` (checked September 18, 2026; Rust 1.98.1, manifest September 3, 2026).
- [S2] Cargo lint tables: `https://doc.rust-lang.org/cargo/reference/manifest.html#the-lints-section`.
- [S3] console crate documentation and per-style controls: `https://raw.githubusercontent.com/console-rs/console/master/src/lib.rs` and `https://raw.githubusercontent.com/console-rs/console/master/src/utils.rs`.
- [S4] indicatif crate documentation: `https://raw.githubusercontent.com/console-rs/indicatif/main/src/lib.rs`.
- [S5] GitHub reusable workflows: `https://docs.github.com/en/actions/how-tos/reuse-automations/reuse-workflows`.
- [S6] clap derive validation example and feature manifest: `https://raw.githubusercontent.com/clap-rs/clap/master/examples/tutorial_derive/04_02_validate.rs` and `https://raw.githubusercontent.com/clap-rs/clap/v4.6.0/Cargo.toml`. Recheck current releases/APIs at implementation time.
