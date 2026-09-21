# Changelog

All notable changes to `typesafe-jev` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate follows
[semantic versioning](https://semver.org/): before 1.0, a breaking change bumps the minor version.

## [0.1.0] - 2026-09-21

The first release: the Jev client of [jevgrep](https://github.com/thehumanworks/jevgrep) 0.3.0,
extracted into a crate of its own.

### Added

- `Client`, over HTTPS (`Client::new`, `Client::from_env`) or over any `Transport`
  (`Client::with_transport`), with `ask` for one request of many questions about one state.
- `Config` for the endpoint, model, user agent, timeouts, retry budget, backoff and pool size.
- Retries with exponential backoff, jitter and `Retry-After` for connection failures and HTTP 408,
  409, 425, 429, 500, 502, 503, 504 and 529.
- `AdaptiveLimiter`, the shared concurrency gate that halves on throttling and grows back.
- `Usage`, thread-safe request, retry and token counters with a cost estimate.
- `Error`, a `#[non_exhaustive]` enum: `Auth`, `TokenLimit`, `InvalidConfig` and `Api`.

[0.1.0]: https://github.com/thehumanworks/jevgrep/tree/main/crates/typesafe-jev
