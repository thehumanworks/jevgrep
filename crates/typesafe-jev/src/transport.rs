use ureq::http::{HeaderValue, Uri};

use crate::{Config, Error};

/// What came back from one HTTP POST, before the client interprets it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    /// The HTTP status code.
    pub status: u16,
    /// The `Retry-After` header, verbatim, when the server sent one.
    pub retry_after: Option<String>,
    /// The response body, decoded as text.
    pub body: String,
}

/// The wire: one POST of a JSON request body to the API endpoint.
///
/// [`Client::new`](crate::Client::new) builds an HTTPS transport; [`Client::with_transport`](crate::Client::with_transport)
/// takes any implementation, so tests and local fakes need no network. `Err` is a connection-level
/// failure, which the client retries; an HTTP error status is an `Ok` reply that the client
/// interprets. The trait is implemented for any `Fn(&[u8]) -> Result<Reply, String>` closure.
pub trait Transport: Send + Sync {
    /// Sends `body` (a JSON document) and returns the reply, or a connection-level failure.
    fn post(&self, body: &[u8]) -> Result<Reply, String>;
}

impl<F: Fn(&[u8]) -> Result<Reply, String> + Send + Sync> Transport for F {
    fn post(&self, body: &[u8]) -> Result<Reply, String> {
        self(body)
    }
}

impl Transport for Box<dyn Transport> {
    fn post(&self, body: &[u8]) -> Result<Reply, String> {
        (**self).post(body)
    }
}

/// The real transport: a pooled HTTPS agent that sends a bearer token.
pub(crate) struct Http {
    agent: ureq::Agent,
    url: String,
    auth: String,
}

impl Http {
    /// Vets everything that would otherwise fail identically on every attempt, so that a typo in
    /// the configuration is one error at construction instead of a full round of retries.
    ///
    /// The messages never quote the key, nor the URL, which may carry credentials of its own.
    pub(crate) fn new(api_key: &str, cfg: &Config) -> Result<Self, Error> {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return Err(Error::Auth("the API key is empty".into()));
        }
        let auth = format!("Bearer {api_key}");
        if HeaderValue::from_str(&auth).is_err() {
            return Err(Error::Auth("the API key has characters that cannot be sent in an HTTP header".into()));
        }
        let uri: Uri = cfg.base_url.parse().map_err(|e| Error::InvalidConfig(format!("`Config::base_url` is not a URL: {e}")))?;
        if !matches!(uri.scheme_str(), Some("http" | "https")) || uri.host().is_none_or(str::is_empty) {
            return Err(Error::InvalidConfig("`Config::base_url` must be an absolute `https://` (or `http://`) URL".into()));
        }
        if HeaderValue::from_str(&cfg.user_agent).is_err() {
            return Err(Error::InvalidConfig("`Config::user_agent` has characters that cannot be sent in an HTTP header".into()));
        }
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(cfg.connect_timeout))
            .timeout_send_body(Some(cfg.timeout))
            .timeout_recv_response(Some(cfg.timeout))
            .timeout_recv_body(Some(cfg.timeout))
            .max_idle_connections(cfg.pool_size)
            .max_idle_connections_per_host(cfg.pool_size)
            .user_agent(&cfg.user_agent)
            .build()
            .into();
        Ok(Http { agent, url: cfg.base_url.clone(), auth })
    }
}

/// Bodies larger than this are not read; no answer set is that big.
const BODY_LIMIT: u64 = 256 << 20;

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
        let body = resp.body_mut().with_config().limit(BODY_LIMIT).read_to_string().map_err(|e| e.to_string())?;
        Ok(Reply { status, retry_after, body })
    }
}
