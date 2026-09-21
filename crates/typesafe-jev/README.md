# typesafe-jev

A Rust client for [TypeSafe](https://typesafe.ai)'s System One API and its Jev model.

Jev does not generate text. You send a JSON `state` and a map of typed questions about it, and
get back one calibrated answer per question. Every question in a request is evaluated against
the same state in parallel, so a request with hundreds of questions takes about as long as one
with a single question.

This crate is the client that [jevgrep](https://github.com/thehumanworks/jevgrep) (`jg`,
natural-language code search) uses for its default backend, extracted so that it can be used on
its own. It has no async runtime and two dependencies: `serde_json` for the JSON, and `ureq` with
rustls for the HTTPS.

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
front of TypeSafe works too), the model, the timeouts, the retry budget and the connection pool.

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
  model's context as `Error::TokenLimit`, which the caller should split and retry.
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
  request, so the state you sent stays out of logs.

## License

MIT OR Apache-2.0.
