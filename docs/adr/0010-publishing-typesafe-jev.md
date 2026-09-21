# ADR 0010: What `typesafe-jev` fixed before its first publication

- Status: Accepted
- Date: 2026-09-21
- Baseline: ADR 0009, which extracted the crate without changing `jg`'s behaviour
- Scope: the public API and packaging of `crates/typesafe-jev` as version 0.1.0 on crates.io.

## Context

ADR 0009 moved the Jev client into a crate and deliberately improved nothing on the way. A
published version is a promise: whatever 0.1.0 exposes can only change with a breaking release.
Reviewing the crate as a stranger's dependency, rather than as `jg`'s module, found these:

- A configuration that can never work was retried like a network failure. A base URL without a
  scheme, or a key with the newline of the file it was read from, failed identically on every
  attempt and surfaced after the whole backoff schedule, about a minute and a half by default,
  as "gave up after 8 retries".
- `Error` was an exhaustive enum, so telling one more failure apart would break every `match`.
- `Config::max_backoff` was `f64` seconds beside three `Duration` fields, and a non-finite
  `backoff_scale` or an enormous `throttle_pause` panicked inside `Duration` arithmetic, in one
  case with the limiter's lock held.
- A `Transport` that panicked leaked its limiter slot: with `pool_size` slots gone, the remaining
  threads wait forever. `release` without `acquire` underflowed the in-flight count.
- `Client` and `AdaptiveLimiter` had no `Debug`, so neither could sit in a caller's
  `#[derive(Debug)]` struct.
- The manifest promised a licence the package did not carry, the declared MSRV was checked by
  hand only, and the dependency floors (`serde_json = "1"`) were lower than what compiles.

## Decision

- `Client::new` returns `Result<Client, Error>` and refuses what no request could be sent with:
  a blank key or one that is not a valid header value (`Error::Auth`), a base URL that is not an
  absolute `http` or `https` URL, or a user agent that is not a valid header value (the new
  `Error::InvalidConfig`). It trims whitespace around the key, as `from_env` already did. This is
  syntax only: what `ureq` would reject on every attempt is rejected once. It is not the URL
  vetting of the other backends (ADR 0005), and `http://` is still accepted for local fakes. The
  messages quote neither the key nor the URL, which may carry credentials.
- `Error` is `#[non_exhaustive]`.
- `Config::max_backoff` is a `Duration`. Backoff arithmetic that is negative, NaN or too large
  for a `Duration` means no wait, never a panic.
- The client holds its limiter slot in a drop guard. The limiter tolerates a poisoned lock (its
  state is three numbers, consistent after every critical section) and an unmatched `release`.
- `Client` and `AdaptiveLimiter` implement `Debug`; the client's omits the transport, which holds
  the key. The crate warns on `missing_docs` and `missing_debug_implementations`, which the gates'
  `-D warnings` turns into errors.
- The maintainer confirmed `MIT OR Apache-2.0`, the question ADR 0009 left open. The package
  carries both licence texts, a changelog and a runnable example. The dependency
  floors are the lowest versions on which the suite passes: `serde_json` 1.0.45, `ureq` 3.0.0.
- `scripts/check.sh crate`, a required CI job, runs the crate's tests and doctests on the
  `rust-version` its manifest declares and builds it from the packaged archive alone.

`Config` stays a struct of public fields built with `..Config::default()`, as ADR 0009 documents
and as `jg`'s other backends do; a new field is therefore a breaking (minor, before 1.0) release
of the crate. `Reply` and `Transport` are unchanged: `jg`'s other backends implement them.

## Consequences

For `jg`, two call sites gained a `?` and one a `Duration`. The visible difference is that
`jg --base-url 'not a url'` now fails at once with the reason instead of after eight retries; what
is sent and accepted on the wire is unchanged, so no benchmark run is needed. It ships with the
next `jg` release as a fix.
