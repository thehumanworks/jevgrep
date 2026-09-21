//! Reusable typed decision backends. Search is one consumer of this contract.
//!
//! A backend evaluates named questions against arbitrary JSON state and returns
//! Jev-compatible `noul` and `score` answers. It does not discover files, choose
//! line numbers, or render grep results.
//!
//! Jev itself is the `typesafe-jev` crate. The error, usage and transport types every backend
//! shares are that crate's, re-exported here under provider-independent names.

use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Map, Value};

use crate::chatgpt::ChatGptClient;
use crate::openai::OpenAiClient;

/// Provider-independent error name: the Jev client's error, which every backend reuses.
pub use typesafe_jev::Error as DecisionError;
/// Requests, retries and tokens, counted the same way by every backend.
pub use typesafe_jev::Usage;

/// Shared by all workers, with provider-specific authentication and transport.
pub trait DecisionBackend: Send + Sync {
    fn ask(&self, state: &Value, questions: &Map<String, Value>) -> Result<Map<String, Value>, DecisionError>;

    fn usage(&self) -> &Usage;

    /// Effective service tier reported by the provider, when available.
    fn service_tier(&self) -> Option<String> {
        None
    }

    /// What the provider itself says the requests cost, in dollars, when it reports one.
    fn reported_cost_usd(&self) -> Option<f64> {
        None
    }

    /// True when the provider writes its answers one after another, so a request takes longer
    /// the more it asks. Jev answers every question of a request at once and leaves this false.
    fn answers_sequentially(&self) -> bool {
        false
    }
}

impl DecisionBackend for typesafe_jev::Client {
    fn ask(&self, state: &Value, questions: &Map<String, Value>) -> Result<Map<String, Value>, DecisionError> {
        typesafe_jev::Client::ask(self, state, questions)
    }

    fn usage(&self) -> &Usage {
        typesafe_jev::Client::usage(self)
    }
}

impl DecisionBackend for ChatGptClient {
    fn ask(&self, state: &Value, questions: &Map<String, Value>) -> Result<Map<String, Value>, DecisionError> {
        ChatGptClient::ask(self, state, questions)
    }

    fn usage(&self) -> &Usage {
        &self.usage
    }

    fn service_tier(&self) -> Option<String> {
        ChatGptClient::service_tier(self)
    }

    fn answers_sequentially(&self) -> bool {
        true
    }
}

impl DecisionBackend for OpenAiClient {
    fn ask(&self, state: &Value, questions: &Map<String, Value>) -> Result<Map<String, Value>, DecisionError> {
        OpenAiClient::ask(self, state, questions)
    }

    fn usage(&self) -> &Usage {
        &self.usage
    }

    fn reported_cost_usd(&self) -> Option<f64> {
        self.cost_usd()
    }

    // `answers_sequentially` stays false although these models do write token by token: spreading
    // a search over more, smaller requests buys latency with requests, and requests are what
    // hosted services ration (free models on OpenRouter: 20 a minute, and 50 or 1000 a day) and what
    // a local server queues. Measured live, spread turned a two-file search from 5 requests into 22.
}

/// Uniform in [0, 1). Only used for retry jitter, so a tiny xorshift is plenty.
pub(crate) fn jitter() -> f64 {
    static STATE: AtomicU64 = AtomicU64::new(0);
    let mut x = STATE.load(Ordering::Relaxed);
    if x == 0 {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
        x = nanos ^ (u64::from(std::process::id()) << 32) | 1;
    }
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    STATE.store(x, Ordering::Relaxed);
    (x >> 11) as f64 / (1u64 << 53) as f64
}

/// Bearer credentials must not travel over plaintext. Loopback is allowed so that tests and local
/// mocks can point the client at an ordinary HTTP server.
///
/// The URL is parsed rather than scanned, because `http://localhost@evil.example/` reads as
/// loopback to anything that splits on punctuation while ureq would dial `evil.example`. The
/// offending URL is never quoted back: a mistyped base URL can carry a token in its query string.
pub(crate) fn validate_bearer_url(provider: &str, url: &str) -> Result<(), String> {
    validate_url(url, Some(provider))
}

/// The same parse for a request that carries no credentials, such as one to a keyless server on
/// the local network. With nothing to protect, plaintext http to any host is the caller's choice.
pub(crate) fn validate_keyless_url(url: &str) -> Result<(), String> {
    validate_url(url, None)
}

fn validate_url(url: &str, credentials_of: Option<&str>) -> Result<(), String> {
    let rejected = |why: &str| match credentials_of {
        Some(provider) => Err(format!("refusing to send {provider} credentials to the configured base URL: {why}")),
        None => Err(format!("cannot use the configured base URL: {why}")),
    };
    let Ok(uri) = url.parse::<ureq::http::Uri>() else {
        return rejected("it is not a valid URL");
    };
    let Some(authority) = uri.authority() else {
        return rejected("it has no host");
    };
    // `user:pass@host` makes the host we vet differ from the host that is dialled.
    if authority.as_str().contains('@') {
        return rejected("it carries userinfo before the host");
    }
    let host = authority.host().trim_start_matches('[').trim_end_matches(']');
    match uri.scheme_str() {
        Some("https") if !host.is_empty() => Ok(()),
        Some("http") if matches!(host, "localhost" | "127.0.0.1" | "::1") => Ok(()),
        Some("http") if credentials_of.is_none() && !host.is_empty() => Ok(()),
        Some("http") => rejected("plaintext http is only allowed on localhost"),
        _ if credentials_of.is_none() => rejected("only http and https are supported"),
        _ => rejected("only https, or http on localhost, is allowed"),
    }
}
