//! Jev as `jg` finds it: the API key lookup that sits in front of the `typesafe-jev` client.
//!
//! The client itself (retries, the concurrency gate, usage accounting) is the `typesafe-jev`
//! crate in `crates/typesafe-jev`; `jg` adds only where the key comes from.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use typesafe_jev::API_KEY_VAR;

use crate::backend::DecisionError;

/// `TYPESAFE_API_KEY` from the environment, falling back to `fnox get` unless `JG_NO_FNOX` is set.
pub fn resolve_api_key() -> Result<String, DecisionError> {
    if let Ok(key) = std::env::var(API_KEY_VAR) {
        if !key.trim().is_empty() {
            return Ok(key.trim().to_owned());
        }
    }
    if std::env::var_os("JG_NO_FNOX").is_none() {
        if let Some(key) = fnox_key() {
            return Ok(key);
        }
    }
    Err(DecisionError::Auth(format!(
        "{API_KEY_VAR} is not set. Export it, run via `fnox exec -- jg ...`, \
         or add it to a fnox config visible from this directory."
    )))
}

fn fnox_key() -> Option<String> {
    let mut child =
        Command::new("fnox").args(["get", API_KEY_VAR]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
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
