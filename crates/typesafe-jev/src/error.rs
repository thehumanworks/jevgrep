use std::fmt;

/// Why a request could not be answered.
///
/// Transient failures (connection errors, HTTP 429, 5xx) are retried inside
/// [`Client::ask`](crate::Client::ask) and only surface as [`Error::Api`] once the retries are
/// spent. The other two variants are returned at once, because retrying cannot help.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Missing or rejected credentials (HTTP 401 or 403).
    Auth(String),
    /// The request exceeded the model's context (`max_tokens_exceeded`, or HTTP 413). Ask fewer
    /// questions, or about less state, and retry.
    TokenLimit(String),
    /// Anything else: an unreadable response, an unexpected status, or retries exhausted.
    Api(String),
}

impl Error {
    /// The message, without the variant.
    pub fn message(&self) -> &str {
        let (Error::Auth(m) | Error::TokenLimit(m) | Error::Api(m)) = self;
        m
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for Error {}
