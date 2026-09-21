//! Client for [TypeSafe](https://typesafe.ai)'s System One API and its Jev model.
//!
//! Jev does not generate text. A request carries a JSON `state` and a map of typed questions
//! about it; the reply carries one calibrated answer per question. Every question in a request is
//! evaluated against the same state in parallel, so a request with hundreds of questions takes
//! about as long as one with a single question.
//!
//! ```no_run
//! use serde_json::{json, Map};
//! use typesafe_jev::{Client, Config};
//!
//! # fn main() -> Result<(), typesafe_jev::Error> {
//! let client = Client::from_env(Config::default())?; // reads TYPESAFE_API_KEY
//!
//! let state = json!({
//!     "code": "1| import os\n2| def cwd():\n3|     return os.getcwd()",
//!     "query": "where is the working directory read",
//! });
//! let mut questions = Map::new();
//! questions.insert("line3".into(), json!({
//!     "type": "noul",
//!     "instructions": "Does line 3 of the code directly answer the query?",
//! }));
//! questions.insert("relevance".into(), json!({
//!     "type": "score",
//!     "instructions": "How relevant is the code to the query?",
//!     "criteria": ["unrelated", "tangential", "relevant", "exactly what was asked"],
//! }));
//!
//! let answers = client.ask(&state, &questions)?;
//! let p = answers["line3"]["noul"].as_f64().unwrap_or(0.0); // probability in [0, 1]
//! let score = answers["relevance"]["score"].as_f64().unwrap_or(0.0); // position among the criteria
//! println!("line 3 answers the query with p={p:.2}; relevance {score:.1}");
//! println!("{} input tokens, ~${:.4}", client.usage().input_tokens(), client.usage().cost_usd());
//! # Ok(())
//! # }
//! ```
//!
//! # Questions and answers
//!
//! Two question types are supported:
//!
//! - `noul` asks a yes/no question. The answer is
//!   `{"type": "noul", "noul": p}`, with `p` the probability in [0, 1] that the answer is yes.
//! - `score` asks for a judgment on an ordered scale. The question lists its `criteria`, one
//!   per level, in order; the answer is `{"type": "score", "score": s, "confidence": c,
//!   "probabilities": {...}, "legend": {...}}`, with `s` a position on that scale counted from
//!   the first criterion.
//!
//! The probabilities are calibrated: a `noul` of 0.9 is right about nine times in ten. The
//! `questions` map is passed through as given, so any field the API accepts can be sent.
//!
//! # Retries, concurrency and cost
//!
//! [`Client::ask`] retries connection failures and transient HTTP statuses with exponential
//! backoff and jitter, honouring `Retry-After`. Rejected credentials come back at once as
//! [`Error::Auth`]; a request too large for the model's context as [`Error::TokenLimit`], which
//! the caller should split and retry. A key or a [`Config`] that no request could be sent with is
//! reported by [`Client::new`], as [`Error::Auth`] or [`Error::InvalidConfig`], not retried.
//!
//! Calls are blocking. The client is `Send + Sync` and shares one connection pool, so call it from
//! as many threads as [`Config::pool_size`]. An [`AdaptiveLimiter`] caps the concurrency and halves
//! it whenever the API throttles, growing it back as requests succeed.
//!
//! [`Usage`] counts requests, retries and tokens across threads; [`Usage::cost_usd`] prices the
//! input tokens at Jev's list price, [`USD_PER_INPUT_MTOK`].
//!
//! # Testing without the network
//!
//! [`Client::with_transport`] takes any [`Transport`], including a closure from the request body
//! to a [`Reply`]:
//!
//! ```
//! use serde_json::{json, Map};
//! use typesafe_jev::{Client, Config, Reply};
//!
//! let client = Client::with_transport(
//!     |_body: &[u8]| Ok(Reply { status: 200, retry_after: None, body: r#"{"answers": {"q": {"type": "noul", "noul": 0.5}}}"#.into() }),
//!     Config::default(),
//! );
//! let mut questions = Map::new();
//! questions.insert("q".into(), json!({"type": "noul", "instructions": "Is the state empty?"}));
//! let answers = client.ask(&json!({}), &questions).unwrap();
//! assert_eq!(answers["q"]["noul"], 0.5);
//! ```

#![warn(missing_docs, missing_debug_implementations)]

mod client;
mod error;
mod limiter;
mod transport;
mod usage;

pub use client::{Client, Config};
pub use error::Error;
pub use limiter::AdaptiveLimiter;
pub use transport::{Reply, Transport};
pub use usage::Usage;

/// TypeSafe's hosted System One endpoint.
pub const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai/v1/systemone";
/// The model name that follows TypeSafe's latest Jev release.
pub const DEFAULT_MODEL: &str = "jev-latest";
/// The environment variable [`Client::from_env`] reads.
pub const API_KEY_VAR: &str = "TYPESAFE_API_KEY";
/// Jev's list price per million input tokens, in dollars. Output tokens are free.
pub const USD_PER_INPUT_MTOK: f64 = 0.042;

/// The README's examples compile and run with the crate's own tests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
