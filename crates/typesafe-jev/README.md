# typesafe-jev

[![crates.io](https://img.shields.io/crates/v/typesafe-jev.svg)](https://crates.io/crates/typesafe-jev)
[![docs.rs](https://img.shields.io/docsrs/typesafe-jev)](https://docs.rs/typesafe-jev)
[![MSRV 1.85](https://img.shields.io/badge/rustc-1.85+-blue.svg)](#compatibility)
[![license](https://img.shields.io/crates/l/typesafe-jev.svg)](#license)

A Rust client for [TypeSafe](https://typesafe.ai)'s System One API and its Jev model.

Jev does not generate text. You send a JSON `state` and a map of typed questions about it, and
get back one calibrated answer per question. Every question in a request is evaluated against
the same state in parallel, so a request with hundreds of questions takes about as long as one
with a single question.

This crate is the client that [jevgrep](https://github.com/thehumanworks/jevgrep) (`jg`,
natural-language code search) uses for its default backend, extracted so that it can be used on
its own. It has no async runtime and two dependencies: `serde_json` for the JSON, and `ureq` with
rustls for the HTTPS. It is an independent client, not an official TypeSafe SDK.

## Usage

```toml
[dependencies]
typesafe-jev = "0.1"
serde_json = "1"
```

```rust,no_run
use serde_json::{json, Map};
use typesafe_jev::{Client, Config};

fn main() -> Result<(), typesafe_jev::Error> {
    let client = Client::from_env(Config::default())?; // reads TYPESAFE_API_KEY

    let state = json!({
        "code": "1| import os\n2| def cwd():\n3|     return os.getcwd()",
        "query": "where is the working directory read",
    });
    let mut questions = Map::new();
    questions.insert("line3".into(), json!({
        "type": "noul",
        "instructions": "Does line 3 of the code directly answer the query?",
    }));
    questions.insert("relevance".into(), json!({
        "type": "score",
        "instructions": "How relevant is the code to the query?",
        "criteria": ["unrelated", "tangential", "relevant", "exactly what was asked"],
    }));

    let answers = client.ask(&state, &questions)?;
    let p = answers["line3"]["noul"].as_f64().unwrap_or(0.0);
    let score = answers["relevance"]["score"].as_f64().unwrap_or(0.0);
    println!("line 3 answers the query with p={p:.2}; relevance {score:.1}");
    println!("{} input tokens, ~${:.4}", client.usage().input_tokens(), client.usage().cost_usd());
    Ok(())
}
```

`Client::new(api_key, config)` takes the key directly. `Config` sets the endpoint (a gateway in
front of TypeSafe works too), the model, the timeouts, the retry budget and the connection pool:

```rust
use std::time::Duration;
use typesafe_jev::{Client, Config, Error};

fn main() -> Result<(), Error> {
    let config = Config { timeout: Duration::from_secs(20), max_retries: 3, pool_size: 8, ..Config::default() };
    let client = Client::new("sk-...", config)?;
    assert_eq!(client.model(), "jev-latest");

    // A key or a config that no request could be sent with is refused here, not retried later.
    let typo = Config { base_url: "api.typesafe.ai/v1/systemone".into(), ..Config::default() };
    assert!(matches!(Client::new("sk-...", typo), Err(Error::InvalidConfig(_))));
    Ok(())
}
```

[`examples/ask.rs`](examples/ask.rs) is a complete program: `TYPESAFE_API_KEY=... cargo run --example ask`.

## Questions and answers

- `noul` asks a yes/no question. The answer is
  `{"type": "noul", "noul": p}`, with `p` the probability in [0, 1] that the answer is yes.
- `score` asks for a judgment on an ordered scale. The question lists its `criteria`, one per
  level, in order; the answer is `{"type": "score", "score": s, "confidence": c,
  "probabilities": {...}, "legend": {...}}`, with `s` a position on that scale counted from the
  first criterion.

The probabilities are calibrated: a `noul` of 0.9 is right about nine times in ten. The
`questions` map is sent as given, so any field the API accepts can be used.

## Retries, concurrency and cost

- Connection failures and transient HTTP statuses (408, 409, 425, 429, 5xx, 529) are retried
  with exponential backoff and jitter, honouring `Retry-After`, up to `Config::max_retries`.
- Rejected credentials are returned at once as `Error::Auth`; a request too large for the
  model's context as `Error::TokenLimit`, which the caller should split and retry. Everything
  else, including retries exhausted, is `Error::Api`. The enum is `#[non_exhaustive]`.
- Calls block. The client is `Send + Sync` and shares one connection pool: call `ask` from as many
  threads as `Config::pool_size`. An `AdaptiveLimiter` caps the concurrency and halves it
  whenever the API throttles, growing it back as requests succeed. TypeSafe's rate limit is a
  sustained token budget with no quota headers, and the limiter is what keeps several processes on
  one key from tripping over each other.
- `Usage` counts requests, retries and tokens across threads. `Usage::cost_usd` prices the input
  tokens at Jev's list price (`USD_PER_INPUT_MTOK`); output tokens are free.
- `Client::set_debug_reporter` receives one line per failed attempt. Nothing is logged otherwise.

## Testing without the network

`Client::with_transport` takes any `Transport`, including a closure from the request body to a
`Reply`, so tests and local fakes need no server:

```rust
use serde_json::{json, Map};
use typesafe_jev::{Client, Config, Reply};

let client = Client::with_transport(
    |_body: &[u8]| Ok(Reply { status: 200, retry_after: None, body: r#"{"answers": {"q": {"type": "noul", "noul": 0.5}}}"#.into() }),
    Config { backoff_scale: 0.0, ..Config::default() },
);
let mut questions = Map::new();
questions.insert("q".into(), json!({"type": "noul", "instructions": "Is the state empty?"}));
assert_eq!(client.ask(&json!({}), &questions).unwrap()["q"]["noul"], 0.5);
```

## Security notes

- The key is sent as a bearer token to whatever `Config::base_url` names, over HTTPS by default.
  Plain `http://` is accepted, for local fakes; do not point a real key at one.
- Error messages quote at most 400 characters of a failed response body. They never quote the
  request, the key or the base URL, so the state you sent and your credentials stay out of logs.
  `Client`'s `Debug` output leaves the key out too.
- Redirects are followed, but the `Authorization` header is not sent to the redirect's target.
- `#![forbid(unsafe_code)]`; TLS is rustls with the bundled web PKI roots.

## Compatibility

- The minimum supported Rust version is 1.85, checked in CI. Raising it is a minor version bump.
- `serde_json` is part of the public API (`Value`, `Map`); `ureq` is not. The lowest versions the
  manifest allows (`serde_json` 1.0.45, `ureq` 3.0.0) pass the test suite.
- The crate follows semantic versioning. Before 1.0, a breaking change bumps the minor version;
  see the [changelog](CHANGELOG.md).

## License

Licensed under either of the [Apache License, Version 2.0](LICENSE-APACHE) or the
[MIT license](LICENSE-MIT), at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
this crate by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without
any additional terms or conditions.
