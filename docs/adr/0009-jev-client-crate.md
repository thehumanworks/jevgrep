# ADR 0009: The Jev client is its own crate

- Status: Accepted
- Date: 2026-09-21
- Baseline: jevgrep 0.3.0 plus ADRs 0001–0008
- Scope: the HTTP client for TypeSafe's System One API (the Jev model): request and reply
  shape, retries, the concurrency gate, usage accounting. Question wording, search, the other
  backends and the CLI stay in `jevgrep`.

## Context

`src/client.rs` held everything `jg` needs to talk to Jev: the bearer-token HTTP transport,
retry policy, the adaptive concurrency limiter, token accounting, the error type, and also the
`fnox` key lookup and the `JG_DEBUG` fallback. None of the first group is about code search.
The ChatGPT and OpenAI-compatible backends (ADRs 0004 and 0005) reused the limiter, the usage
counters, the `Reply` type and the URL vetting from the same module, so "the Jev client" and
"what every backend shares" had grown into one file.

Other programs want the first group without `jg`. On crates.io the name `jev` is taken by an
unrelated client, so this one is `typesafe-jev`.

## Decision

### The crate

`crates/typesafe-jev` is a workspace member and a library with two dependencies, `serde_json`
and `ureq` (rustls, gzip). Its public API is what a caller needs and no more:

- `Client::new(api_key, Config)`, `Client::from_env(Config)` (reads `TYPESAFE_API_KEY`) and
  `Client::with_transport(impl Transport, Config)` for fakes; `ask(&state, &questions)`;
  `usage()`, `limiter()`, `model()`, `set_debug_reporter()`.
- `Config`: base URL, model, provider name (diagnostics only), user agent, connect and request
  timeouts, retry budget and backoff cap, pool size, throttle pause, and `backoff_scale`, which
  tests set to zero. The test-only public fields `JevClient::backoff` and `limiter.pause` became
  those two config fields.
- `Error { Auth, TokenLimit, Api }`, `Usage`, `AdaptiveLimiter`, `Reply`, `Transport`, and the
  constants `DEFAULT_BASE_URL`, `DEFAULT_MODEL`, `API_KEY_VAR`, `USD_PER_INPUT_MTOK`.

The wire behaviour is unchanged: the same JSON body, headers, status handling, retry statuses,
`Retry-After` cap, 400-character error excerpts, body limit and limiter policy, verified by the
crate's own tests against a scripted transport and a local HTTP listener. Two things were
deliberately left as they were rather than improved in the move, because `jg`'s behaviour was
not to change: the client still follows redirects, and it still does not vet the base URL the way
the other backends do (ADR 0001's "no URL validation" rule for Jev).

What the crate does not do, because it is `jg` policy:

- No `fnox`, no `JG_NO_FNOX`, no error text that mentions `jg`. `jevgrep::jev::resolve_api_key`
  keeps that lookup, with the crate's `API_KEY_VAR` as the variable name.
- No environment-driven logging. Without a debug reporter the client is silent; `jg` installs
  one when `JG_DEBUG` is set, as it already did.
- No `jg/<version>` user agent by default. The crate announces itself; `jg` sets
  `Config::user_agent` to what it sent before.

`Usage::record` and `Usage::record_retry` are public so that the other backends, which live in
`jevgrep`, keep counting the same way (they were `pub(crate)` `add_counts` and `add_retry`).

### What stays in `jevgrep`

`backend.rs` re-exports `typesafe_jev::Error` as `DecisionError` and `typesafe_jev::Usage`, and
now owns the helpers the non-Jev backends share: `jitter`, `validate_bearer_url` and
`validate_keyless_url`. Every module that named `JevError` now names `DecisionError`, which was
already the trait's error type. `DecisionBackend` is implemented for `typesafe_jev::Client` as it
was for `JevClient`.

### Workspace

The root `Cargo.toml` is still the `jevgrep` package and now also the workspace root, with
`default-members = [".", "crates/typesafe-jev"]` so that `cargo test`, `cargo clippy` and
`cargo doc` at the root cover both crates and `scripts/check.sh` and the pre-commit hook need no
change. The Clippy policy of ADR 0006 moved to `[workspace.lints]`; both packages opt in. The
crate declares `rust-version = "1.85"` and is checked on that toolchain; `jg` keeps its exact pin.

`scripts/release.sh` releases `jg` only. Publishing the crate is `cargo publish -p typesafe-jev`,
a separate act with its own version in `crates/typesafe-jev/Cargo.toml`.

## Alternatives

Leaving the shared helpers in the crate as public API would have committed `typesafe-jev` to an
interface (URL vetting for other providers' tokens, a jitter source) that has nothing to do with
Jev; twelve lines of xorshift are cheaper to duplicate. A virtual workspace with `jevgrep` under
`crates/` would have broken every script that reads the root `Cargo.toml` for the version and pins.
An async client was not considered: `jg` fans out over threads, and the API is a single POST.

## Consequences

`jg`'s behaviour, flags, help text and output are unchanged; the release binary carries the same
code. `jevgrep::client` no longer exists; the test suites import the client from `typesafe_jev`
and the key lookup from `jevgrep::jev`. The four client-only tests of `tests/client_search.rs`
moved to the crate, where they no longer need `jg`'s question builder. The crate has no license
of its own to inherit: the manifest says `MIT OR Apache-2.0`, which the maintainer confirms or
changes before the first `cargo publish`.
