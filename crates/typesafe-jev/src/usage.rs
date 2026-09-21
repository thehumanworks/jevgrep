use std::sync::atomic::{AtomicU64, Ordering};

use crate::{TokenUsage, USD_PER_INPUT_MTOK};

/// Thread-safe request and token counters.
///
/// A [`Client`](crate::Client) fills its own `Usage` from the `usage` object of every successful
/// reply and counts each retry. The counters are atomic, so the totals can be read while requests
/// are in flight; they are then a snapshot, not a transaction.
///
/// [`Usage::record`] and [`Usage::record_retry`] are public so that a caller who talks to another
/// provider through the same code path can keep the same accounting.
#[derive(Debug, Default)]
pub struct Usage {
    requests: AtomicU64,
    retries: AtomicU64,
    input_tokens: AtomicU64,
    output_tokens: AtomicU64,
}

impl Usage {
    /// One completed request and the tokens it used.
    pub fn record(&self, input_tokens: u64, output_tokens: u64) {
        self.requests.fetch_add(1, Ordering::Relaxed);
        self.input_tokens.fetch_add(input_tokens, Ordering::Relaxed);
        self.output_tokens.fetch_add(output_tokens, Ordering::Relaxed);
    }

    /// One attempt that had to be repeated. A request that is retried twice before it succeeds
    /// counts as one request and two retries.
    pub fn record_retry(&self) {
        self.retries.fetch_add(1, Ordering::Relaxed);
    }

    /// One completed request, from the reply's `usage`. A count the API left out is zero.
    pub(crate) fn record_reported(&self, usage: TokenUsage) {
        self.record(usage.input_tokens.unwrap_or(0), usage.output_tokens.unwrap_or(0));
    }

    /// Requests that were answered.
    pub fn requests(&self) -> u64 {
        self.requests.load(Ordering::Relaxed)
    }

    /// Attempts that were repeated after a transient failure.
    pub fn retries(&self) -> u64 {
        self.retries.load(Ordering::Relaxed)
    }

    /// Input tokens, as reported by the API.
    pub fn input_tokens(&self) -> u64 {
        self.input_tokens.load(Ordering::Relaxed)
    }

    /// Output tokens, as reported by the API. Jev reports them, but does not charge for them.
    pub fn output_tokens(&self) -> u64 {
        self.output_tokens.load(Ordering::Relaxed)
    }

    /// The input tokens at Jev's list price, [`USD_PER_INPUT_MTOK`]. An estimate: the price is a
    /// constant in this crate, not something the API reports.
    pub fn cost_usd(&self) -> f64 {
        self.input_tokens() as f64 / 1_000_000.0 * USD_PER_INPUT_MTOK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_requests_retries_and_tokens() {
        let usage = Usage::default();
        assert_eq!((usage.requests(), usage.retries(), usage.input_tokens(), usage.output_tokens()), (0, 0, 0, 0));
        usage.record(100, 10);
        usage.record_retry();
        let reported = |text: &str| serde_json::from_str::<TokenUsage>(text).unwrap();
        usage.record_reported(reported(r#"{"input_tokens": 50, "output_tokens": 5}"#));
        usage.record_reported(reported(r#"{"output_tokens": null}"#));
        usage.record_reported(TokenUsage::default());
        assert_eq!((usage.requests(), usage.retries(), usage.input_tokens(), usage.output_tokens()), (4, 1, 150, 15));
        assert!((usage.cost_usd() - 150.0 / 1_000_000.0 * USD_PER_INPUT_MTOK).abs() < 1e-15);
    }
}
