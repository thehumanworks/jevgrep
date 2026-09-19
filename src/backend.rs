//! Reusable typed decision backends. Search is one consumer of this contract.
//!
//! A backend evaluates named questions against arbitrary JSON state and returns
//! Jev-compatible `noul` and `score` answers. It does not discover files, choose
//! line numbers, or render grep results.

use serde_json::{Map, Value};

use crate::chatgpt::ChatGptClient;
use crate::client::{JevClient, Usage};

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
