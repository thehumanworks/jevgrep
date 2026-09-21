//! Typed client for [TypeSafe](https://typesafe.ai)'s System One API and its Jev model.
//!
//! Jev does not generate text. A request carries a `state` and a set of typed questions about it;
//! the reply carries one calibrated answer per question. Every question in a request is evaluated
//! against the same state in parallel, so a request with hundreds of questions takes about as
//! long as one with a single question.
//!
//! ```no_run
//! use typesafe_jev::{Choice, Client, Config, Noul, Questions, Score};
//!
//! # fn main() -> Result<(), typesafe_jev::Error> {
//! let client = Client::from_env(Config::default())?; // reads TYPESAFE_API_KEY
//!
//! let ticket = "Hi, I've been trying to connect my Stripe account for 3 days and the \
//!               integration keeps failing. I'm losing sales. Please help ASAP.";
//! let questions = Questions::new()
//!     .with("department", Choice::new("Which team should handle this?", [
//!         ("billing", "Payment or subscription issues"),
//!         ("technical", "Bugs or integration problems"),
//!         ("sales", "Pricing or account questions"),
//!     ]))
//!     .with("frustration", Score::new("How frustrated the customer appears", [
//!         "Calm, just stating facts",
//!         "Frustrated but civil",
//!         "Very angry, strong language",
//!     ]))
//!     .with("is_urgent", Noul::new("The message conveys urgency or time-sensitivity"));
//!
//! let response = client.ask(ticket, &questions)?;
//!
//! let department = response.choice("department").expect("asked as a choice");
//! println!("{} (confidence {:.2})", department.choice, department.confidence); // technical (0.78)
//! let frustration = response.score("frustration").expect("asked as a score");
//! println!("level {:.1} of 2", frustration.score); // 1.0
//! let urgent = response.noul("is_urgent").expect("asked as a noul");
//! println!("urgent with p={:.2}", urgent.noul); // 1.00
//! println!("{} input tokens, ~${:.4}", client.usage().input_tokens(), client.usage().cost_usd());
//! # Ok(())
//! # }
//! ```
//!
//! # Questions and answers
//!
//! The types follow TypeSafe's [API reference](https://docs.typesafe.ai/api) field for field, and
//! serialize to exactly the documents it shows.
//!
//! | Question | Asks | Answer |
//! | --- | --- | --- |
//! | [`Noul`] | a yes/no question, with optional descriptions of a yes and a no | [`NoulAnswer`]: the probability of a yes |
//! | [`Choice`] | for one option out of a set, each with an optional description | [`ChoiceAnswer`]: the option, the probability of each, a confidence |
//! | [`Score`] | for a rating against ordered, described levels | [`ScoreAnswer`]: a position on the scale, the probability of each level, a confidence |
//!
//! [`Questions`] holds the questions of one request under ids you choose, in the order you add
//! them, and a [`Response`] holds an [`Answer`] under each of those ids. The probabilities are
//! calibrated: of the `noul` answers given as 0.9, about nine in ten are a yes.
//!
//! Instructions and descriptions are [`Content`]: text, or a JSON object or array when a question
//! refers to data of its own. The `state` is any value that serializes to a JSON string, object
//! or array: a `&str`, a `serde_json::Value`, or your own `#[derive(Serialize)]` type.
//!
//! A reply is read strictly: an answer without a field the reference requires is
//! [`Error::Api`], not a default. Fields this version does not know are ignored, and the
//! structs and enums are `#[non_exhaustive]`, so the API can grow without breaking callers.
//!
//! # Retries, concurrency and cost
//!
//! [`Client::ask`] retries connection failures and transient HTTP statuses with exponential
//! backoff and jitter, honouring `Retry-After`. Rejected credentials come back at once as
//! [`Error::Auth`]; a request too large for the model's context as [`Error::TokenLimit`], which
//! the caller should split and retry; a body the API refuses as malformed (HTTP 422) as
//! [`Error::InvalidRequest`]. A key or a [`Config`] that no request could be sent with is
//! reported by [`Client::new`], as [`Error::Auth`] or [`Error::InvalidConfig`], not retried.
//!
//! Calls are blocking. The client is `Send + Sync` and shares one connection pool, so call it from
//! as many threads as [`Config::pool_size`]. An [`AdaptiveLimiter`] caps the concurrency and halves
//! it whenever the API throttles, growing it back as requests succeed.
//!
//! [`Usage`] counts requests, retries and tokens across threads; [`Usage::cost_usd`] prices the
//! input tokens at Jev's list price, [`USD_PER_INPUT_MTOK`]. Each [`Response`] also carries the
//! [`TokenUsage`] of its own request.
//!
//! # Testing without the network
//!
//! [`Client::with_transport`] takes any [`Transport`], including a closure from the request body
//! to a [`Reply`]:
//!
//! ```
//! use typesafe_jev::{Client, Config, Noul, Questions, Reply};
//!
//! let client = Client::with_transport(
//!     |_body: &[u8]| Ok(Reply {
//!         status: 200,
//!         retry_after: None,
//!         body: r#"{"model": "fake", "answers": {"q": {"type": "noul", "noul": 0.5}}}"#.into(),
//!     }),
//!     Config::default(),
//! );
//! let response = client.ask("any state", &Questions::new().with("q", Noul::new("Is the state empty?"))).unwrap();
//! assert_eq!(response.noul("q").unwrap().noul, 0.5);
//! ```

#![warn(missing_docs, missing_debug_implementations)]

mod answer;
mod client;
mod content;
mod error;
mod limiter;
mod question;
mod transport;
mod usage;

pub use answer::{Answer, ChoiceAnswer, NoulAnswer, Response, ScoreAnswer, TokenUsage};
pub use client::{Client, Config};
pub use content::Content;
pub use error::Error;
pub use limiter::AdaptiveLimiter;
pub use question::{Choice, Noul, NoulCriteria, Question, Questions, Score};
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
