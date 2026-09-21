use std::fmt;
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::limiter::{jitter, AdaptiveLimiter};
use crate::transport::{Http, Transport};
use crate::{Error, Usage, API_KEY_VAR, DEFAULT_BASE_URL, DEFAULT_MODEL};

/// Statuses worth another attempt: timeouts, conflicts, throttling and server-side trouble.
const RETRYABLE: &[u16] = &[408, 409, 425, 429, 500, 502, 503, 504, 529];

/// How a [`Client`] reaches the API and how hard it tries.
///
/// Build one with `Config { field: value, ..Config::default() }`. Every field has a default that
/// suits TypeSafe's hosted endpoint.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// The endpoint every request is POSTed to. [`DEFAULT_BASE_URL`], or a gateway in front of
    /// it. The URL is used as given: a token is sent to whatever host it names.
    pub base_url: String,
    /// The model name sent in every request. [`DEFAULT_MODEL`] follows TypeSafe's latest Jev.
    pub model: String,
    /// Who serves the endpoint, for error messages only: `TypeSafe`, or the name of a gateway.
    pub provider: String,
    /// The `User-Agent` header. Defaults to this crate's name and version.
    pub user_agent: String,
    /// Time allowed to open a connection.
    pub connect_timeout: Duration,
    /// Time allowed for each of sending the request, waiting for the response and reading it.
    pub timeout: Duration,
    /// Attempts after the first for transient failures; see [`Error`] for what is transient.
    pub max_retries: u32,
    /// The longest a retry's exponential backoff grows to. The backoff starts at half a second
    /// and doubles per attempt, up to this cap, times a random factor in [0.5, 1.5).
    pub max_backoff: Duration,
    /// Connections kept open, and the ceiling of the concurrency [`AdaptiveLimiter`]. Set it to
    /// the number of threads that will call [`Client::ask`] at once.
    pub pool_size: usize,
    /// How long every thread waits after a throttled reply (HTTP 429 or 529) before the next
    /// attempt, times a random factor in [1, 2).
    pub throttle_pause: Duration,
    /// Multiplies the retry backoff and any `Retry-After` wait. Zero makes retries immediate,
    /// which tests want; [`Config::throttle_pause`] is separate. A negative or non-finite scale
    /// also means no wait.
    pub backoff_scale: f64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            base_url: DEFAULT_BASE_URL.into(),
            model: DEFAULT_MODEL.into(),
            provider: "TypeSafe".into(),
            user_agent: concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION")).into(),
            connect_timeout: Duration::from_secs(15),
            timeout: Duration::from_secs(60),
            max_retries: 8,
            max_backoff: Duration::from_secs(30),
            pool_size: 64,
            throttle_pause: Duration::from_secs(1),
            backoff_scale: 1.0,
        }
    }
}

type DebugReporter = dyn Fn(&str) + Send + Sync;

/// A connection to the System One API. One shared pool; call [`ask`](Client::ask) from as many
/// threads as [`Config::pool_size`] allows.
pub struct Client {
    model: String,
    provider: String,
    max_retries: u32,
    max_backoff: Duration,
    backoff_scale: f64,
    usage: Usage,
    limiter: AdaptiveLimiter,
    transport: Box<dyn Transport>,
    debug_reporter: Option<Box<DebugReporter>>,
}

impl Client {
    /// A client that sends `api_key` as a bearer token to [`Config::base_url`] over HTTPS.
    /// Whitespace around the key, such as the newline at the end of a key file, is dropped.
    ///
    /// Nothing is sent until the first [`ask`](Client::ask): a key the API rejects is reported
    /// then, as [`Error::Auth`].
    ///
    /// # Errors
    ///
    /// What no request could be sent with is reported here rather than retried later:
    /// [`Error::Auth`] for a blank key or one that cannot go in a header, and
    /// [`Error::InvalidConfig`] for a [`Config::base_url`] that is not an absolute `http` or
    /// `https` URL or a [`Config::user_agent`] that is not a valid header value.
    pub fn new(api_key: &str, cfg: Config) -> Result<Self, Error> {
        let http = Http::new(api_key, &cfg)?;
        Ok(Self::with_transport(http, cfg))
    }

    /// A client that reads its API key from the [`API_KEY_VAR`] environment variable.
    ///
    /// # Errors
    ///
    /// [`Error::Auth`] if the variable is unset or blank; otherwise as [`Client::new`].
    pub fn from_env(cfg: Config) -> Result<Self, Error> {
        let key = api_key_from(std::env::var(API_KEY_VAR).ok().as_deref())?;
        Self::new(&key, cfg)
    }

    /// A client over any [`Transport`], for tests and local fakes. [`Config::base_url`],
    /// [`Config::user_agent`] and the timeouts are the transport's business and are ignored.
    pub fn with_transport(transport: impl Transport + 'static, cfg: Config) -> Self {
        Client {
            model: cfg.model,
            provider: cfg.provider,
            max_retries: cfg.max_retries,
            max_backoff: cfg.max_backoff,
            backoff_scale: cfg.backoff_scale,
            usage: Usage::default(),
            limiter: AdaptiveLimiter::new(cfg.pool_size, cfg.throttle_pause),
            transport: Box::new(transport),
            debug_reporter: None,
        }
    }

    /// Receives one line per failed attempt (the status or connection error, and any
    /// `Retry-After`) before the client retries. Nothing is reported without a reporter, and
    /// the reporter changes neither the retry policy nor the request.
    pub fn set_debug_reporter(&mut self, reporter: impl Fn(&str) + Send + Sync + 'static) {
        self.debug_reporter = Some(Box::new(reporter));
    }

    /// The model name sent in every request.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Requests, retries and tokens so far.
    pub fn usage(&self) -> &Usage {
        &self.usage
    }

    /// The concurrency gate, for inspection.
    pub fn limiter(&self) -> &AdaptiveLimiter {
        &self.limiter
    }

    fn debug(&self, msg: &str) {
        if let Some(reporter) = &self.debug_reporter {
            reporter(msg);
        }
    }

    /// A product that is negative, NaN or too large for a `Duration` is no wait, not a panic.
    fn sleep(&self, seconds: f64) {
        match Duration::try_from_secs_f64(seconds * self.backoff_scale) {
            Ok(wait) if !wait.is_zero() => std::thread::sleep(wait),
            _ => {}
        }
    }

    /// Evaluates `questions` against `state` and returns the `answers` map, keyed like the
    /// questions.
    ///
    /// `state` is any JSON the questions refer to. Each question is an object with a `type`
    /// (`noul` or `score`) and `instructions`; a `score` question also lists its `criteria`. Each
    /// answer is an object with the same `type` and the model's judgment: a `noul` probability in
    /// [0, 1], or a `score` with its `confidence` and `probabilities`. The crate documentation
    /// shows both shapes.
    ///
    /// Transient failures are retried with exponential backoff up to [`Config::max_retries`]
    /// times, honouring `Retry-After` (capped at 30 seconds) and the shared [`AdaptiveLimiter`].
    /// A `Retry-After` given as a date rather than as seconds is ignored in favour of the backoff.
    /// The call blocks the calling thread; run it from a thread pool for concurrency.
    ///
    /// # Errors
    ///
    /// [`Error::Auth`] for HTTP 401 and 403, and [`Error::TokenLimit`] for a request larger than
    /// the model's context, both without a retry. [`Error::Api`] for any other status that is not
    /// worth retrying, for a reply that is not the documented JSON, and once the retries are
    /// spent; the message then ends with the last failure.
    pub fn ask(&self, state: &Value, questions: &Map<String, Value>) -> Result<Map<String, Value>, Error> {
        let body = serde_json::to_vec(&json!({"model": self.model, "state": state, "questions": questions}))
            .map_err(|e| Error::Api(e.to_string()))?;
        let mut last = String::from("unknown error");
        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                self.usage.record_retry();
                let doublings = (attempt - 1).min(32) as i32;
                self.sleep((0.5 * 2f64.powi(doublings)).min(self.max_backoff.as_secs_f64()) * (0.5 + jitter()));
            }
            let sent = {
                let mut slot = Slot::acquire(&self.limiter);
                let sent = self.transport.post(&body);
                slot.throttled = matches!(&sent, Ok(r) if r.status == 429 || r.status == 529);
                sent
            };
            let reply = match sent {
                Ok(reply) => reply,
                Err(e) => {
                    self.debug(&format!("attempt {}: {e}", attempt + 1));
                    last = e;
                    continue;
                }
            };
            if reply.status == 200 {
                let mut data: Value = serde_json::from_str(&reply.body).map_err(|e| Error::Api(format!("unreadable response: {e}")))?;
                self.usage.record_reported(data.get("usage"));
                return match data.get_mut("answers").map(Value::take) {
                    Some(Value::Object(answers)) => Ok(answers),
                    _ => Err(Error::Api("response has no `answers` object".into())),
                };
            }
            let text: String = reply.body.chars().take(400).collect();
            if reply.status == 401 || reply.status == 403 {
                return Err(Error::Auth(format!("{} rejected the API key (HTTP {}): {text}", self.provider, reply.status)));
            }
            if text.contains("max_tokens_exceeded") || reply.status == 413 {
                return Err(Error::TokenLimit(text));
            }
            if RETRYABLE.contains(&reply.status) {
                last = format!("HTTP {}: {text}", reply.status);
                self.debug(&format!("attempt {}: {last} retry-after={:?}", attempt + 1, reply.retry_after));
                if let Some(seconds) = reply.retry_after.and_then(|v| v.trim().parse::<f64>().ok()) {
                    self.sleep(seconds.clamp(0.0, 30.0));
                }
                continue;
            }
            return Err(Error::Api(format!("HTTP {}: {text}", reply.status)));
        }
        Err(Error::Api(format!("gave up after {} retries: {last}", self.max_retries)))
    }
}

/// The model, the retry policy and the counters. The transport is left out: it holds the key.
impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("model", &self.model)
            .field("provider", &self.provider)
            .field("max_retries", &self.max_retries)
            .field("max_backoff", &self.max_backoff)
            .field("backoff_scale", &self.backoff_scale)
            .field("usage", &self.usage)
            .field("limiter", &self.limiter)
            .finish_non_exhaustive()
    }
}

/// One slot of the limiter, given back when dropped: a transport that panics must not leak it,
/// or the threads that are left would wait for a slot that never frees.
struct Slot<'a> {
    limiter: &'a AdaptiveLimiter,
    throttled: bool,
}

impl<'a> Slot<'a> {
    fn acquire(limiter: &'a AdaptiveLimiter) -> Self {
        limiter.acquire();
        Slot { limiter, throttled: false }
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.limiter.release(self.throttled);
    }
}

/// The key as the environment supplies it: trimmed, and required to be non-blank.
fn api_key_from(value: Option<&str>) -> Result<String, Error> {
    match value.map(str::trim) {
        Some(key) if !key.is_empty() => Ok(key.to_owned()),
        _ => Err(Error::Auth(format!("{API_KEY_VAR} is not set"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_key_is_trimmed_and_must_not_be_blank() {
        assert_eq!(api_key_from(Some("  sk-1 \n")).unwrap(), "sk-1");
        for missing in [None, Some(""), Some("   ")] {
            let error = api_key_from(missing).unwrap_err();
            assert!(matches!(&error, Error::Auth(m) if m.contains(API_KEY_VAR)), "{missing:?} -> {error:?}");
        }
    }

    #[test]
    fn defaults_point_at_typesafe() {
        let cfg = Config::default();
        assert_eq!((cfg.base_url.as_str(), cfg.model.as_str(), cfg.provider.as_str()), (DEFAULT_BASE_URL, DEFAULT_MODEL, "TypeSafe"));
        assert!(cfg.user_agent.starts_with("typesafe-jev/"), "{}", cfg.user_agent);
        assert_eq!((cfg.max_retries, cfg.max_backoff, cfg.pool_size, cfg.backoff_scale), (8, Duration::from_secs(30), 64, 1.0));
    }
}
