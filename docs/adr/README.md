# Architecture decisions

Decisions describe the implemented contract, not a release announcement.

| ADR | Status | Decision |
| --- | --- | --- |
| [0001](0001-declarative-cli-parsing.md) | Accepted | Declarative clap parsing, option inventory, validation and normalization |
| [0002](0002-terminal-presentation-and-progress.md) | Accepted | Console presentation and indicatif progress, per-stream policy and cleanup |
| [0003](0003-rust-toolchain-and-quality-gates.md) | Accepted | Exact Rust/MSRV pins, narrow lints and shared local/CI/release gates |

All decisions are dated September 18, 2026. The implementation plan is
[CLI modernization](../plans/cli-modernization.md). Test and validation commands
are in the [development guide](../../README.md#development).

The new flag, validation and help presentation warrant a future **minor release
(0.3.0)** under this pre-1.0 project's release policy. This work does not bump the
package version, create a tag, publish a release, or change branch protection.
