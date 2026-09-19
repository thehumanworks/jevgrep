//! Reusable typed decision backends. Search is one consumer of this contract.
//!
//! A backend evaluates named questions against arbitrary JSON state and returns
//! Jev-compatible `noul` and `score` answers. It does not discover files, choose
//! line numbers, or render grep results.

use serde_json::{Map, Value};

use crate::chatgpt::ChatGptClient;
use crate::client::{JevClient, Usage};
use crate::openai::OpenAiClient;

/// Provider-independent error name; the original `JevError` remains compatible.
pub use crate::client::JevError as DecisionError;

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

impl DecisionBackend for JevClient {
    fn ask(&self, state: &Value, questions: &Map<String, Value>) -> Result<Map<String, Value>, DecisionError> {
        JevClient::ask(self, state, questions)
    }

    fn usage(&self) -> &Usage {
        &self.usage
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
