# ADR 0003: Pinned Rust and shared quality gates

- Status: Accepted
- Date: 2026-09-18

## Context

The baseline (72d526d) has a tag-only build workflow, floating Rust selection,
and no required PR quality gates. Local environment overrides and user credentials
can make tests non-reproducible. Packaging itself needs validation, not only compilation.
The modernization deliberately raises the supported minimum compiler to Rust 1.98.1
(official stable manifest dated 2026-09-03, verified 2026-09-18 from the official distribution manifest).

## Decision

Use `rust-toolchain.toml` as the exact canonical compiler pin. Require Cargo's
`rust-version` and mise's Rust pin to match it, retaining edition 2021. Invoke
`cargo +<pin>` explicitly. Future upgrades are reviewed changes to all three pins.
Keep the narrowly scoped Rust/Clippy lint policy in Cargo; deny Clippy warnings,
without blanket pedantic rules or bans on test unwraps. Build documentation with
`RUSTDOCFLAGS=-D warnings` as well as running doctests.

Provide one fail-fast `scripts/check.sh` for required local and CI checks. Tool
installation is separate. Isolate HOME, XDG directories, global Git configuration,
and application credentials; intentionally retain compiler/tool caches. Never run
ignored live tests. Require formatting, Clippy, tests, doctests, release compilation,
workflow and shell lint, helper tests, and pin consistency. Exercise the
fake-API/PTY harness against the host release binary.

Share quality and native release-target jobs through `checks.yml`, called by thin
CI and release workflows. Build Apple Silicon Darwin, x86_64 musl, and aarch64 musl
with `--locked`; assert each runner's actual architecture. Use native `macos-26`
(ARM64), `ubuntu-24.04` (x86_64), and `ubuntu-24.04-arm` (aarch64) runners.
GitHub's current runner documentation and ARM64 image manifest confirm the Darwin
mapping. The original `macos-15` label stayed queued without an assigned runner for
ten minutes during verification, so it was replaced by the supported, explicit
`macos-26` label rather than a floating `macos-latest`. Targets and archive names
are unchanged. Keep existing tarball
names, README layout, and SHA256 sidecars. Download artifacts before checking
checksums, extraction, help/version, fake-API smoke, and Linux static linkage.
Use reviewed immutable external action commits with Node 24 or newer, bounded
job timeouts, and concurrency only in callers. Only a push of a version tag may
publish, after every shared check succeeds and the tag matches Cargo's version.
Only that publication job receives contents-write permission.

## Alternatives

- Floating stable or independent pins: rejected because results silently change.
- Separate release and PR commands: rejected because checks drift.
- Cross-compilation alone: rejected because native execution and packaging matter.
- Installing tools on every local check: rejected; explicit installation is easier
  to audit and does not make a normal check mutate the developer's tool selection.
- Keeping lexopt or adopting a TUI does not solve these quality-gate concerns;
  parser and presentation choices belong to ADRs 0001 and 0002.

## Consequences and compatibility

The minimum supported Rust version rises to 1.98.1. No Rust 1.82 compatibility is
claimed. The compiler, lint tools, Python 3.11+, and native C toolchains must be
installed before checks. CI takes longer because all three release targets run on
PRs. The Linux static contract and existing downloadable layout remain unchanged.
CI-only changes do not require a release; the combined user-visible modernization
warrants a future minor release, not a release during this implementation.

## Verification and evidence

Implementation references: `scripts/check.sh`, `scripts/check-toolchain.py`,
`scripts/install-qa-tools.sh`, `scripts/package.sh`, `scripts/verify-package.py`,
`scripts/test_quality.py`, and `.github/workflows/{checks,ci,release}.yml`.
`scripts/terminal-smoke.py --binary PATH [--pty]` supplies fake-API and real-terminal checks.
The quality gate runs the full PTY harness against the host release binary; each
native job invokes ordinary fake-API smoke against the extracted downloaded binary.
The helper suite covers pin drift, checksum/layout/linkage failures, package-version
and extracted-binary smoke wiring, installer checksum failure, action pins, publication
gates, fail-fast behavior, and actual isolated child-process HOME read paths.

The stable aggregate check is **`checks / required checks`**. It depends on
`checks / quality` and all three `checks / native (<target>)` matrix jobs. Require
the aggregate in branch protection after verifying the displayed check context in
an actual run. No repository protection settings are changed by this work.

Local evidence on 2026-09-18: actionlint 1.7.12 and ShellCheck 0.11.0 passed;
23 non-network Python helper tests passed; Rust pin/lock consistency passed.
The complete isolated entrypoint passed on the shared working tree with 94 Rust
tests passed and 8 ignored live tests, zero doctests, warnings-free docs,
formatting, Clippy, a locked host release build, and all 11 PTY scenarios. The Linux
x64 QA-tool installer was exercised against the upstream assets and verified both
checksums before installation. Ruff 0.16.8 formatting/checks also passed for the
three owned Python helpers; Ruff is not a required installed runtime QA tool.

Actual hosted CI is required before completion: record the exact final commit, run
ID, every job conclusion, native architecture assertions, and downloaded-artifact
checks. The final task report supplies that evidence rather than inferring it from
local builds. Darwin/ARM execution and GitHub artifact round-trips cannot be
verified on the Linux GNU development host. Normal CI uses no live API secrets;
live fixture tests are separate, explicitly authorized opt-in verification.

References: `docs/plans/cli-modernization.md`; official Rust stable distribution
manifest; GitHub reusable workflow and hosted runner documentation; upstream
release metadata and action manifests for every pinned external action/tool.


## Interfaces and reviewed dependencies

- `scripts/install-qa-tools.sh [DESTINATION]` installs only actionlint 1.7.12 and
  ShellCheck 0.11.0. The default is ignored `target/qa-tools/bin`; `check.sh` finds
  it automatically. Every supported Linux/Darwin x64/arm64 asset has its own
  embedded SHA256. Installer downloads are separate from routine checks.
- `scripts/check.sh` (or `quality`) runs all local quality gates, fail-fast.
- `scripts/check.sh target TARGET` asserts native OS/CPU, tests and builds the
  pinned target with `--locked`, and creates `target/dist/` packages.
- `scripts/check.sh verify ARCHIVE TARGET` asserts native OS/CPU and verifies a
  downloaded package. `verify-package.py --archive PATH --target TARGET` is the
  standalone verifier. It refuses links, extra/duplicate/path-traversal members,
  wrong checksums/versions, non-executable binaries, ELF interpreters, and dynamic
  `NEEDED` libraries. It extracts only the two expected regular files.
- Python must be 3.11+ for `tomllib` and `hashlib.file_digest`; every workflow
  explicitly installs Python 3.12 through setup-python, including macOS.
- Preserve intentional `CARGO_HOME`, `RUSTUP_HOME`, `CARGO_TARGET_DIR`,
  `RUSTC_WRAPPER`, `SCCACHE_DIR`, `CC`, and `AR` locations; drop ambient credentials,
  application settings, Git overrides, and `RUSTUP_TOOLCHAIN`. Resolve mise shims
  before isolating HOME. This is configuration isolation, not a network sandbox;
  cargo may download public dependencies, but smoke/API tests use only localhost.

External action releases, tag references, resolved commits, input contracts and
`runs.using: node24` manifests were reviewed through read-only GitHub API requests
on 2026-09-18. No floating external action references remain:

| Action | Release | Reviewed commit |
| --- | --- | --- |
| actions/checkout | v7.0.1 | `3d3c42e5aac5ba805825da76410c181273ba90b1` |
| actions/setup-python | v7.0.0 | `5fda3b95a4ea91299a34e894583c3862153e4b97` |
| actions/upload-artifact | v7.0.1 | `043fb46d1a93c77aae656e7c1c64a875d1fc6a0a` |
| actions/download-artifact | v8.0.1 | `3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c` |

Reverification sources (substitute each reviewed action name, release and commit):

- `https://api.github.com/repos/actions/ACTION/releases/tags/RELEASE`
- `https://api.github.com/repos/actions/ACTION/git/ref/tags/RELEASE`
- `https://raw.githubusercontent.com/actions/ACTION/COMMIT/action.yml`
- [actionlint 1.7.12 asset metadata](https://api.github.com/repos/rhysd/actionlint/releases/tags/v1.7.12)
- [ShellCheck 0.11.0 asset metadata](https://api.github.com/repos/koalaman/shellcheck/releases/tags/v0.11.0)
- [Official hosted-runner labels and architectures](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)
- [Reusable workflow semantics](https://docs.github.com/en/actions/how-tos/reuse-automations/reuse-workflows)

Asset digests are copied from the upstream release metadata into
`install-qa-tools.sh`, not trusted from a checksum downloaded alongside an unpinned
binary. The actionlint checksum-list asset itself was reported as
`433028cf0ba3c42163ea1a668dedce30fcdbe84fe912b1a5e288c006eab8a4f5`.
For the locally exercised Linux x64 tarballs the exact pinned digests are:

- actionlint: `8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8`
- ShellCheck: `b7af85e41cc99489dcc21d66c6d5f3685138f06d34651e6d34b42ec6d54fe6f6`
