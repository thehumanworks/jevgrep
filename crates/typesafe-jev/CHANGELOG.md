# Changelog

All notable changes to `typesafe-jev` are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the crate follows
[semantic versioning](https://semver.org/): before 1.0, a breaking change bumps the minor version.

## [0.2.0] - 2026-09-21

Questions and answers are Rust types that follow TypeSafe's
[API reference](https://docs.typesafe.ai/api) field for field, in place of `serde_json::Value`
and `Map`.

### Added

- `Noul`, `Choice` and `Score`, the three question types, with `NoulCriteria` for what a yes and
  a no mean; `Question`, the enum of them; and `Questions`, the ordered set of one request.
  `Choice` is new to the crate: 0.1 documented `noul` and `score` only.
- `NoulAnswer`, `ChoiceAnswer` and `ScoreAnswer`; `Answer`, the enum of them; and `Response`, with
  the `model` that answered, the ordered `answers`, the request's `TokenUsage`, and lookups by id
  and type (`Response::noul`, `choice`, `score`).
- `Content`, the `string | object | array` that instructions and descriptions take.
- `Error::InvalidRequest`, for a state that does not serialize and for a body the API refuses as
  malformed (HTTP 422, which was `Error::Api`).
- Every wire type implements `Serialize` and `Deserialize`, and is `#[non_exhaustive]`.

### Changed

- **Breaking:** `Client::ask` takes `&Questions` and returns `Response`, and its `state` is any
  `Serialize` value (a `&str`, a `serde_json::Value`, your own type) instead of `&Value`.
- **Breaking:** a reply is read strictly. An answer without a field the reference requires, or of
  an unknown `type`, and a reply without `model` or `answers`, is `Error::Api` naming what is
  missing; 0.1 passed whatever came back through. Unknown fields are ignored, and absent token
  counts are `None`.
- Questions and a `Choice`'s options are sent in the order they were added, whether or not some
  crate in the build turns on `serde_json`'s `preserve_order`.
- New dependencies: `serde` (with `derive`) and `indexmap` (with `serde`), both part of the public
  API. The request body for the same questions is byte for byte what 0.1 sent.

### Migrating from 0.1

```rust
// 0.1
let mut questions = serde_json::Map::new();
questions.insert("hit".into(), json!({"type": "noul", "instructions": "Does it read a file?"}));
let answers = client.ask(&json!({"code": code}), &questions)?;
let p = answers["hit"]["noul"].as_f64().unwrap_or(0.0);

// 0.2
let questions = Questions::new().with("hit", Noul::new("Does it read a file?"));
let response = client.ask(&json!({"code": code}), &questions)?;
let p = response.noul("hit").map_or(0.0, |answer| answer.noul);
```

Questions that exist as JSON already read into the types with
`serde_json::from_value::<Questions>(json)`, and `serde_json::to_value(&response.answers)` gives
the answers back as JSON.

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

[0.2.0]: https://crates.io/crates/typesafe-jev/0.2.0
[0.1.0]: https://crates.io/crates/typesafe-jev/0.1.0
