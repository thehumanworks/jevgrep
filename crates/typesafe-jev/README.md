# typesafe-jev

[![crates.io](https://img.shields.io/crates/v/typesafe-jev.svg)](https://crates.io/crates/typesafe-jev)
[![docs.rs](https://img.shields.io/docsrs/typesafe-jev)](https://docs.rs/typesafe-jev)
[![MSRV 1.85](https://img.shields.io/badge/rustc-1.85+-blue.svg)](#compatibility)
[![license](https://img.shields.io/crates/l/typesafe-jev.svg)](#license)

A typed Rust client for [TypeSafe](https://typesafe.ai)'s System One API and its Jev model.

Jev does not generate text. You send a `state` and a set of typed questions about it, and get
back one calibrated answer per question. Every question in a request is evaluated against the
same state in parallel, so a request with hundreds of questions takes about as long as one with
a single question.

The three question types of the [API reference](https://docs.typesafe.ai/api), `Noul`, `Choice`
and `Score`, and their answers are Rust types here, field for field, so a misspelt field or a
missing `criteria` is a compile error rather than an HTTP 422.

This crate is the client that [jevgrep](https://github.com/thehumanworks/jevgrep) (`jg`,
natural-language code search) uses for its default backend, extracted so that it can be used on
its own. It has no async runtime; its dependencies are `serde` and `serde_json` for the JSON,
`indexmap` to keep questions and options in the order you give them, and `ureq` with rustls for
the HTTPS. It is an independent client, not an official TypeSafe SDK.

## Usage

```toml
[dependencies]
typesafe-jev = "0.2"
```

```rust,no_run
use typesafe_jev::{Choice, Client, Config, Noul, Questions, Score};

fn main() -> Result<(), typesafe_jev::Error> {
    let client = Client::from_env(Config::default())?; // reads TYPESAFE_API_KEY

    let ticket = "Hi, I've been trying to connect my Stripe account for 3 days and the \
                  integration keeps failing. I'm losing sales. Please help ASAP.";
    let questions = Questions::new()
        .with("department", Choice::new("Which team should handle this?", [
            ("billing", "Payment or subscription issues"),
            ("technical", "Bugs or integration problems"),
            ("sales", "Pricing or account questions"),
        ]))
        .with("frustration", Score::new("How frustrated the customer appears", [
            "Calm, just stating facts",
            "Frustrated but civil",
            "Very angry, strong language",
        ]))
        .with("is_urgent", Noul::new("The message conveys urgency or time-sensitivity"));

    let response = client.ask(ticket, &questions)?;

    let department = response.choice("department").expect("asked as a choice");
    println!("{} (confidence {:.2})", department.choice, department.confidence); // technical (0.78)
    for (option, probability) in &department.probabilities {
        println!("  {option}: {probability:.2}"); // billing: 0.15, technical: 0.85, sales: 0.00
    }
    let frustration = response.score("frustration").expect("asked as a score");
    println!("level {:.1} of 2", frustration.score); // 1.0
    let urgent = response.noul("is_urgent").expect("asked as a noul");
    println!("urgent with p={:.2}", urgent.noul); // 1.00

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

| Question | Asks | Answer |
| --- | --- | --- |
| `Noul::new(instructions)`, optionally `.yes(..)` and `.no(..)` | a yes/no question | `NoulAnswer { noul }`: the probability of a yes, from 0 to 1 |
| `Choice::new(instructions, [(option, description), ..])`, or `Choice::labels(instructions, [option, ..])` | for one option out of up to 255 | `ChoiceAnswer { choice, confidence, probabilities }` |
| `Score::new(instructions, [level, ..])` | for a rating against 2 to 10 ordered levels | `ScoreAnswer { score, confidence, legend, probabilities }`, with levels numbered from 0 |

- The probabilities are calibrated: of the `noul` answers given as 0.9, about nine in ten are a
  yes. `confidence` says how far to trust a `choice` or a `score`; see TypeSafe's
  [confidence](https://docs.typesafe.ai/confidence) page.
- `Questions` keeps its questions, and a `Choice` its options, in the order you add them, and that
  is the order on the wire. `Response::noul`, `choice` and `score` look an answer up by id and
  type; `response.answers` is the whole ordered map of `Answer`s.
- Instructions and descriptions are `Content`: a `&str` or `String` converts with `into()`, and
  `Content::try_from(json!({...}))` takes the structured form, where a question names its own
  data fields in backticks:

  ```rust
  use serde_json::json;
  use typesafe_jev::{Content, Noul};

  let instructions = Content::try_from(json!({
      "potential_duplicate": {"name": "John Smith", "location": "Oakland, California"},
      "question": "Is the resume for the same person as `potential_duplicate`?",
  }))
  .expect("an object is content; a number, a boolean or null is not");
  let question = Noul::new(instructions);
  ```

- The `state` is anything that serializes to a JSON string, object or array: a `&str`, a
  `serde_json::Value`, or your own `#[derive(Serialize)]` type. Jev reads text only.
- Every type implements `Serialize` and `Deserialize`, so questions can live in a config file and
  answers in a log. A reply is read strictly: an answer that lacks a field the reference requires
  is an `Error::Api`, not a silent default. Unknown fields are ignored, and the types are
  `#[non_exhaustive]`, so the API can grow without breaking your build.

## Retries, concurrency and cost

- Connection failures and transient HTTP statuses (408, 409, 425, 429, 5xx, 529) are retried
  with exponential backoff and jitter, honouring `Retry-After`, up to `Config::max_retries`.
- Rejected credentials are returned at once as `Error::Auth`; a request too large for the
  model's context as `Error::TokenLimit`, which the caller should split and retry; a body the API
  refuses as malformed (HTTP 422) as `Error::InvalidRequest`. Everything else, including retries
  exhausted, is `Error::Api`. The enum is `#[non_exhaustive]`.
- Calls block. The client is `Send + Sync` and shares one connection pool: call `ask` from as many
  threads as `Config::pool_size`. An `AdaptiveLimiter` caps the concurrency and halves it
  whenever the API throttles, growing it back as requests succeed. TypeSafe's rate limit is a
  sustained token budget with no quota headers, and the limiter is what keeps several processes on
  one key from tripping over each other.
- `Usage` counts requests, retries and tokens across threads. `Usage::cost_usd` prices the input
  tokens at Jev's list price (`USD_PER_INPUT_MTOK`); output tokens are free. Each `Response` also
  carries the `TokenUsage` of its own request, and the `model` that answered it.
- `Client::set_debug_reporter` receives one line per failed attempt. Nothing is logged otherwise.

## Testing without the network

`Client::with_transport` takes any `Transport`, including a closure from the request body to a
`Reply`, so tests and local fakes need no server:

```rust
use typesafe_jev::{Client, Config, Noul, Questions, Reply};

let client = Client::with_transport(
    |_body: &[u8]| Ok(Reply {
        status: 200,
        retry_after: None,
        body: r#"{"model": "fake", "answers": {"q": {"type": "noul", "noul": 0.5}}}"#.into(),
    }),
    Config { backoff_scale: 0.0, ..Config::default() },
);
let response = client.ask("any state", &Questions::new().with("q", Noul::new("Is the state empty?"))).unwrap();
assert_eq!(response.noul("q").unwrap().noul, 0.5);
```

`NoulAnswer::new`, `ChoiceAnswer::new` and `ScoreAnswer::new` build answers for a fake that works
above the client.

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
- `serde`, `serde_json` (`Value` and `Map` inside `Content`) and `indexmap` (`IndexMap` in
  `Choice::criteria`, `ChoiceAnswer::probabilities` and `Response::answers`) are part of the public
  API; `ureq` is not. The lowest versions the manifest allows pass the test suite.
- The crate follows semantic versioning. Before 1.0, a breaking change bumps the minor version;
  see the [changelog](CHANGELOG.md).

## License

Licensed under either of the [Apache License, Version 2.0](LICENSE-APACHE) or the
[MIT license](LICENSE-MIT), at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
this crate by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without
any additional terms or conditions.
