//! Thread-safe client for TypeSafe's System One API (the Jev model).
//!
//! Jev does not generate text. You send a `state` plus a map of typed questions and get back
//! calibrated probabilities. All questions in one request are evaluated in parallel against the
//! same state, so a request with hundreds of questions costs about the same latency as one.

use std::fmt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

pub const DEFAULT_BASE_URL: &str = "https://api.typesafe.ai/v1/systemone";
pub const DEFAULT_MODEL: &str = "jev-latest";
/// Output tokens are free.
pub const USD_PER_INPUT_MTOK: f64 = 0.042;
const RETRYABLE: &[u16] = &[408, 409, 425, 429, 500, 502, 503, 504, 529];

#[derive(Debug, Clone, PartialEq)]
pub enum JevError {
    /// Missing or rejected credentials.
    Auth(String),
    /// The request exceeded the model's context. Callers should split and retry.
    TokenLimit(String),
    /// Unrecoverable API error.
    Api(String),
}

impl fmt::Display for JevError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (JevError::Auth(m) | JevError::TokenLimit(m) | JevError::Api(m)) = self;
        f.write_str(m)
    }
}

impl std::error::Error for JevError {}

#[derive(Debug, Default)]
pub struct Usage {
    requests: AtomicU64,
    retries: AtomicU64,
    input_tokens: AtomicU64,
    output_tokens: AtomicU64,
}

impl Usage {
    pub(crate) fn add(&self, usage: Option<&Value>) {
        let field = |k: &str| usage.and_then(|u| u.get(k)).and_then(Value::as_u64).unwrap_or(0);
        self.add_counts(field("input_tokens"), field("output_tokens"));
    }

    /// One request's tokens, for providers that name the fields differently.
    pub(crate) fn add_counts(&self, input_tokens: u64, output_tokens: u64) {
        self.requests.fetch_add(1, Ordering::Relaxed);
        self.input_tokens.fetch_add(input_tokens, Ordering::Relaxed);
        self.output_tokens.fetch_add(output_tokens, Ordering::Relaxed);
    }

    pub(crate) fn add_retry(&self) {
        self.retries.fetch_add(1, Ordering::Relaxed);
    }

    pub fn requests(&self) -> u64 {
        self.requests.load(Ordering::Relaxed)
    }

    pub fn retries(&self) -> u64 {
        self.retries.load(Ordering::Relaxed)
    }

    pub fn input_tokens(&self) -> u64 {
        self.input_tokens.load(Ordering::Relaxed)
    }

    pub fn output_tokens(&self) -> u64 {
        self.output_tokens.load(Ordering::Relaxed)
    }

    pub fn cost_usd(&self) -> f64 {
        self.input_tokens() as f64 / 1_000_000.0 * USD_PER_INPUT_MTOK
    }
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

struct Gate {
    limit: f64,
    in_flight: usize,
    paused_until: Option<Instant>,
}

/// Shared concurrency gate with AIMD backoff.
///
/// TypeSafe's rate limit is a sustained token budget with no quota headers, and several jg
/// processes (one per agent) may share a key. So on a 429 every thread pauses briefly and the
/// allowed concurrency halves; each success grows it back by roughly one slot per round.
pub struct AdaptiveLimiter {
    max: f64,
    pub pause: Duration,
    gate: Mutex<Gate>,
    cv: Condvar,
}

impl AdaptiveLimiter {
    pub fn new(max_concurrency: usize, pause: Duration) -> Self {
        let max = max_concurrency.max(1) as f64;
        AdaptiveLimiter { max, pause, gate: Mutex::new(Gate { limit: max, in_flight: 0, paused_until: None }), cv: Condvar::new() }
    }

    pub fn limit(&self) -> f64 {
        self.gate.lock().unwrap().limit
    }

    pub fn in_flight(&self) -> usize {
        self.gate.lock().unwrap().in_flight
    }

    pub fn acquire(&self) {
        let mut gate = self.gate.lock().unwrap();
        loop {
            let wait = gate.paused_until.and_then(|t| t.checked_duration_since(Instant::now())).filter(|d| !d.is_zero());
            match wait {
                None if gate.in_flight < (gate.limit as usize).max(1) => {
                    gate.in_flight += 1;
                    return;
                }
                None => gate = self.cv.wait(gate).unwrap(),
                Some(d) => gate = self.cv.wait_timeout(gate, d.max(Duration::from_millis(50))).unwrap().0,
            }
        }
    }

    pub fn release(&self, throttled: bool) {
        let mut gate = self.gate.lock().unwrap();
        gate.in_flight -= 1;
        let now = Instant::now();
        if throttled {
            // One cut per throttling episode, not per 429.
            if gate.paused_until.is_none_or(|t| now >= t) {
                gate.limit = (gate.limit / 2.0).max(1.0);
                gate.paused_until = Some(now + self.pause.mul_f64(1.0 + jitter()));
            }
        } else {
            gate.limit = self.max.min(gate.limit + 1.0 / gate.limit.max(1.0));
        }
        self.cv.notify_all();
    }
}

/// TYPESAFE_API_KEY from the environment, falling back to `fnox get`.
pub fn resolve_api_key() -> Result<String, JevError> {
    if let Ok(key) = std::env::var("TYPESAFE_API_KEY") {
        if !key.trim().is_empty() {
            return Ok(key.trim().to_owned());
        }
    }
    if std::env::var_os("JG_NO_FNOX").is_none() {
        if let Some(key) = fnox_key() {
            return Ok(key);
        }
    }
    Err(JevError::Auth(
        "TYPESAFE_API_KEY is not set. Export it, run via `fnox exec -- jg ...`, \
         or add it to a fnox config visible from this directory."
            .into(),
    ))
}

fn fnox_key() -> Option<String> {
    let mut child = Command::new("fnox")
        .args(["get", "TYPESAFE_API_KEY"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let out = child.wait_with_output().ok()?;
    let key = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (out.status.success() && !key.is_empty()).then_some(key)
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

/// What came back from one HTTP POST.
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub retry_after: Option<String>,
    pub body: String,
}

/// The wire. Swapped for a fake in tests. `Err` is a connection-level failure, which is retried.
pub trait Transport: Send + Sync {
    fn post(&self, body: &[u8]) -> Result<Reply, String>;
}

impl<F: Fn(&[u8]) -> Result<Reply, String> + Send + Sync> Transport for F {
    fn post(&self, body: &[u8]) -> Result<Reply, String> {
        self(body)
    }
}

struct Http {
    agent: ureq::Agent,
    url: String,
    auth: String,
}

impl Transport for Http {
    fn post(&self, body: &[u8]) -> Result<Reply, String> {
        let mut resp = self
            .agent
            .post(&self.url)
            .header("Authorization", &self.auth)
            .header("Content-Type", "application/json")
            .send(body)
            .map_err(|e| e.to_string())?;
        let retry_after = resp.headers().get("retry-after").and_then(|v| v.to_str().ok()).map(str::to_owned);
        let status = resp.status().as_u16();
        let body = resp.body_mut().with_config().limit(256 << 20).read_to_string().map_err(|e| e.to_string())?;
        Ok(Reply { status, retry_after, body })
    }
}

pub struct Config {
    pub base_url: String,
    pub model: String,
    pub timeout: Duration,
    pub max_retries: u32,
    pub pool_size: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            base_url: DEFAULT_BASE_URL.into(),
            model: DEFAULT_MODEL.into(),
            timeout: Duration::from_secs(60),
            max_retries: 8,
            pool_size: 64,
        }
    }
}

type DebugReporter = dyn Fn(&str) + Send + Sync;

/// One shared connection pool; safe to call `ask` from many threads.
pub struct JevClient {
    model: String,
    max_retries: u32,
    /// Scales every backoff sleep. Tests set it to zero.
    pub backoff: f64,
    pub usage: Usage,
    pub limiter: AdaptiveLimiter,
    transport: Box<dyn Transport>,
    debug_reporter: Option<Box<DebugReporter>>,
}

impl JevClient {
    pub fn new(api_key: &str, cfg: Config) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_send_body(Some(cfg.timeout))
            .timeout_recv_response(Some(cfg.timeout))
            .timeout_recv_body(Some(cfg.timeout))
            .max_idle_connections(cfg.pool_size)
            .max_idle_connections_per_host(cfg.pool_size)
            .user_agent(concat!("jg/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        let http = Http { agent, url: cfg.base_url.clone(), auth: format!("Bearer {api_key}") };
        Self::with_transport(Box::new(http), cfg)
    }

    pub fn with_transport(transport: Box<dyn Transport>, cfg: Config) -> Self {
        JevClient {
            model: cfg.model,
            max_retries: cfg.max_retries,
            backoff: 1.0,
            usage: Usage::default(),
            limiter: AdaptiveLimiter::new(cfg.pool_size, Duration::from_secs(1)),
            transport,
            debug_reporter: None,
        }
    }

    /// Route diagnostic messages through the caller's presentation layer. This
    /// changes neither retry policy nor request contents; the default retains JG_DEBUG.
    pub fn set_debug_reporter(&mut self, reporter: impl Fn(&str) + Send + Sync + 'static) {
        self.debug_reporter = Some(Box::new(reporter));
    }

    fn debug(&self, msg: &str) {
        if let Some(reporter) = &self.debug_reporter {
            reporter(msg);
        } else if std::env::var_os("JG_DEBUG").is_some() {
            eprintln!("jg[debug]: {msg}");
        }
    }

    fn sleep(&self, seconds: f64) {
        let seconds = seconds * self.backoff;
        if seconds > 0.0 {
            std::thread::sleep(Duration::from_secs_f64(seconds));
        }
    }

    /// Returns the `answers` map. Retries transient failures with backoff.
    pub fn ask(&self, state: &Value, questions: &Map<String, Value>) -> Result<Map<String, Value>, JevError> {
        let body = serde_json::to_vec(&json!({"model": self.model, "state": state, "questions": questions}))
            .map_err(|e| JevError::Api(e.to_string()))?;
        let mut last = String::from("unknown error");
        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                self.usage.add_retry();
                self.sleep((0.5 * 2f64.powi(attempt as i32 - 1)).min(30.0) * (0.5 + jitter()));
            }
            self.limiter.acquire();
            let sent = self.transport.post(&body);
            self.limiter.release(matches!(&sent, Ok(r) if r.status == 429 || r.status == 529));
            let reply = match sent {
                Ok(reply) => reply,
                Err(e) => {
                    self.debug(&format!("attempt {}: {e}", attempt + 1));
                    last = e;
                    continue;
                }
            };
            if reply.status == 200 {
                let mut data: Value = serde_json::from_str(&reply.body).map_err(|e| JevError::Api(format!("unreadable response: {e}")))?;
                self.usage.add(data.get("usage"));
                return match data.get_mut("answers").map(Value::take) {
                    Some(Value::Object(answers)) => Ok(answers),
                    _ => Err(JevError::Api("response has no `answers` object".into())),
                };
            }
            let text: String = reply.body.chars().take(400).collect();
            if reply.status == 401 || reply.status == 403 {
                return Err(JevError::Auth(format!("TypeSafe rejected the API key (HTTP {}): {text}", reply.status)));
            }
            if text.contains("max_tokens_exceeded") || reply.status == 413 {
                return Err(JevError::TokenLimit(text));
            }
            if RETRYABLE.contains(&reply.status) {
                last = format!("HTTP {}: {text}", reply.status);
                self.debug(&format!("attempt {}: {last} retry-after={:?}", attempt + 1, reply.retry_after));
                if let Some(seconds) = reply.retry_after.and_then(|v| v.trim().parse::<f64>().ok()) {
                    self.sleep(seconds.clamp(0.0, 30.0));
                }
                continue;
            }
            return Err(JevError::Api(format!("HTTP {}: {text}", reply.status)));
        }
        Err(JevError::Api(format!("gave up after {} retries: {last}", self.max_retries)))
    }
}
