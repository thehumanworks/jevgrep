# Architecture decisions

Decisions describe the implemented contract, not a release announcement.

| ADR | Status | Decision |
| --- | --- | --- |
| [0001](0001-declarative-cli-parsing.md) | Accepted | Declarative clap parsing, option inventory, validation and normalization |
| [0002](0002-terminal-presentation-and-progress.md) | Accepted | Console presentation and indicatif progress, per-stream policy and cleanup |
| [0003](0003-rust-toolchain-and-quality-gates.md) | Accepted | Exact Rust/MSRV pins, narrow lints and shared local/CI/release gates |
| [0004](0004-pluggable-decision-backends.md) | Accepted | Reusable `DecisionBackend` with Jev default and ChatGPT subscription transport |
| [0005](0005-openai-compatible-backend.md) | Accepted | One `openai` backend for any OpenAI-compatible service: `OPENAI_API_KEY` / `OPENAI_BASE_URL`, Responses API with structured outputs, adaptive fallbacks |
| [0006](0006-clippy-correctness.md) | Accepted | Correctness-first Clippy: deny correctness/suspicious, cherry-pick bug-catching pedantic/restriction lints, keep `-D warnings` in every gate |
| [0007](0007-tests-as-specification.md) | Accepted | Requirements are table-driven and property-style tests on real invariants, not comments-only |
| [0008](0008-pre-commit-hook.md) | Accepted | Installable pre-commit hook runs fmt, Clippy, and tests; CI still runs isolated `scripts/check.sh` |

ADRs 0001–0003 are dated September 18, 2026; 0004 and 0005 are dated September 19, 2026;
0006–0008 are dated September 20, 2026. The implementation plan is
[CLI modernization](../plans/cli-modernization.md). Test and validation commands
are in the [development guide](../../README.md#development).

The new flag, validation and help presentation warrant a future **minor release
(0.3.0)** under this pre-1.0 project's release policy. This work does not bump the
package version, create a tag, publish a release, or change branch protection.
