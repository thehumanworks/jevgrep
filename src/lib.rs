//! jevgrep: natural-language code search using pluggable typed decision backends.

mod answers;
pub mod backend;
pub mod chatgpt;
pub mod chatgpt_auth;
pub mod cli;
pub mod client;
pub mod files;
pub mod filters;
pub mod openai_compat;
pub mod results;
pub mod search;
