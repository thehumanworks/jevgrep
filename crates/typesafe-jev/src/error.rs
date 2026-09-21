use std::fmt;

/// Why a client could not be built, or a request could not be answered.
///
/// Transient failures (connection errors, HTTP 429, 5xx) are retried inside
/// [`Client::ask`](crate::Client::ask) and only surface as [`Error::Api`] once the retries are
/// spent. The other variants are returned at once, because retrying cannot help.
///
/// The enum is `#[non_exhaustive]`: a later version may tell more failures apart, so a `match`
/// needs a wildcard arm. [`Error::message`] and `Display` give the text of any variant.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// Missing, blank, unsendable or rejected credentials (HTTP 401 or 403).
    Auth(String),
    /// The request exceeded the model's context (`max_tokens_exceeded`, or HTTP 413). Ask fewer
    /// questions, or about less state, and retry.
    TokenLimit(String),
    /// A [`Config`](crate::Config) no request could be sent with: a base URL that is not an
    /// absolute `http` or `https` URL, or a user agent that is not a valid header value.
    /// Reported by [`Client::new`](crate::Client::new), before anything is sent.
    InvalidConfig(String),
    /// Anything else: an unreadable response, an unexpected status, or retries exhausted.
    Api(String),
}

impl Error {
    /// The message, without the variant.
    pub fn message(&self) -> &str {
        let (Error::Auth(m) | Error::TokenLimit(m) | Error::InvalidConfig(m) | Error::Api(m)) = self;
        m
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for Error {}
