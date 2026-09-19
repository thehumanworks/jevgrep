//! ChatGPT subscription credentials, independent of the search backend.
//!
//! Resolution uses a complete environment pair first, then `auth.toml` under
//! `XDG_CONFIG_HOME` (or `~/.config`), then `auth.json` under `CODEX_HOME` (or
//! `~/.codex`). TOML supports `CHATGPT_ACCOUNT_ID` / `CHATGPT_ACCESS_TOKEN` at
//! the top level, `account_id` / `access_token` at the top level, or those
//! lowercase keys together in `[chatgpt]` or `[tokens]`, in that order.
//! Codex's JSON cache uses the same lowercase keys in its `tokens` object.
//! A partial or invalid pair is an error; credentials are never mixed between
//! sources. API keys and refresh tokens are not used. The server validates token
//! expiry, including opaque tokens, and normal resolution never starts a login.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::client::JevError;

const ACCOUNT_ENV: &str = "CHATGPT_ACCOUNT_ID";
const TOKEN_ENV: &str = "CHATGPT_ACCESS_TOKEN";
const LOGIN_HINT: &str =
    "Run `jg --chatgpt-login --backend chatgpt '<query>'`, or export both CHATGPT_ACCOUNT_ID and CHATGPT_ACCESS_TOKEN.";

#[derive(Clone, PartialEq, Eq)]
pub struct ChatGptCredentials {
    pub account_id: String,
    pub access_token: String,
}

impl fmt::Debug for ChatGptCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatGptCredentials").field("account_id", &"[redacted]").field("access_token", &"[redacted]").finish()
    }
}

/// Read subscription credentials without starting an interactive login.
pub fn resolve_credentials() -> Result<ChatGptCredentials, JevError> {
    resolve_with(&|key| std::env::var_os(key), &|path| fs::read_to_string(path))
}

/// Explicitly authenticate using Codex's device flow and return its new cache.
///
/// Codex is used only for login. File-based credential storage is selected for
/// this invocation, and all login output goes to stderr to preserve JSON stdout.
/// The resulting cache is read directly: stale environment or TOML credentials
/// cannot hide a successful login. Existing overrides still apply to subsequent
/// calls to [`resolve_credentials`].
pub fn device_login() -> Result<ChatGptCredentials, JevError> {
    device_login_with(&|key| std::env::var_os(key), &|path| fs::read_to_string(path), |codex_home| {
        login_command(codex_home).status().map(|status| status.success())
    })
}

fn login_command(codex_home: &Path) -> Command {
    let mut command = Command::new("codex");
    command
        .args(["login", "--device-auth", "-c", "cli_auth_credentials_store=\"file\""])
        .env("CODEX_HOME", codex_home)
        .stdin(Stdio::null())
        .stdout(Stdio::from(io::stderr()))
        .stderr(Stdio::inherit());
    command
}

fn resolve_with(
    env: &impl Fn(&str) -> Option<OsString>,
    read: &impl Fn(&Path) -> io::Result<String>,
) -> Result<ChatGptCredentials, JevError> {
    let account = env(ACCOUNT_ENV);
    let token = env(TOKEN_ENV);
    if account.is_some() || token.is_some() {
        let as_text = |value: Option<OsString>| {
            value
                .map(|value| value.into_string().map_err(|_| auth_error("ChatGPT environment credentials must contain valid UTF-8.")))
                .transpose()
        };
        let account = as_text(account)?;
        let token = as_text(token)?;
        return checked_pair(account.as_deref(), token.as_deref(), "ChatGPT environment variables");
    }

    let paths = credential_paths(env);
    if let Some(path) = paths.auth_toml {
        if let Some(credentials) = read_credentials(&path, Format::Toml, read)? {
            return Ok(credentials);
        }
    }
    if let Some(path) = paths.codex_json {
        if let Some(credentials) = read_credentials(&path, Format::Json, read)? {
            return Ok(credentials);
        }
    }
    Err(auth_error("No ChatGPT subscription credentials were found."))
}

fn device_login_with(
    env: &impl Fn(&str) -> Option<OsString>,
    read: &impl Fn(&Path) -> io::Result<String>,
    run: impl FnOnce(&Path) -> io::Result<bool>,
) -> Result<ChatGptCredentials, JevError> {
    let path = credential_paths(env)
        .codex_json
        .ok_or_else(|| auth_error("Cannot locate the Codex cache. Set CODEX_HOME or HOME before device login."))?;
    // `codex_json` is always a directory joined with `auth.json`.
    let codex_home = path.parent().unwrap();
    let success = run(codex_home).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => auth_error("The `codex` command was not found. Install the Codex CLI to use device login."),
        _ => auth_error("Could not start or wait for Codex device login."),
    })?;
    if !success {
        return Err(auth_error("Codex device login did not complete successfully."));
    }
    read_credentials(&path, Format::Json, read)?
        .ok_or_else(|| auth_error("Codex device login completed but its auth.json cache has no ChatGPT subscription credentials."))
}

struct CredentialPaths {
    auth_toml: Option<PathBuf>,
    codex_json: Option<PathBuf>,
}

fn credential_paths(env: &impl Fn(&str) -> Option<OsString>) -> CredentialPaths {
    let path = |key| env(key).filter(|value| !value.is_empty()).map(PathBuf::from);
    let home = path("HOME").or_else(|| path("USERPROFILE"));
    // XDG requires absolute paths; an invalid relative value should not select a
    // credentials file in a potentially untrusted working directory.
    let config = path("XDG_CONFIG_HOME").filter(|path| path.is_absolute()).or_else(|| home.as_ref().map(|home| home.join(".config")));
    let codex = path("CODEX_HOME").or_else(|| home.map(|home| home.join(".codex")));
    CredentialPaths { auth_toml: config.map(|path| path.join("auth.toml")), codex_json: codex.map(|path| path.join("auth.json")) }
}

#[derive(Clone, Copy)]
enum Format {
    Toml,
    Json,
}

impl Format {
    fn source(self) -> &'static str {
        match self {
            Self::Toml => "ChatGPT auth.toml",
            Self::Json => "Codex auth.json",
        }
    }
}

fn read_credentials(
    path: &Path,
    format: Format,
    read: &impl Fn(&Path) -> io::Result<String>,
) -> Result<Option<ChatGptCredentials>, JevError> {
    let contents = match read(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(auth_error(&format!("Cannot read {}. Check the file permissions and encoding.", format.source()))),
    };
    // Parser errors can include source lines containing tokens, so do not print
    // their Display/Debug representations or the credential document itself.
    let malformed = || auth_error(&format!("Invalid {} syntax. Repair or remove that credential file.", format.source()));
    let value = match format {
        Format::Toml => {
            let document = toml::from_str::<toml::Value>(&contents).map_err(|_| malformed())?;
            serde_json::to_value(document).map_err(|_| malformed())?
        }
        Format::Json => serde_json::from_str::<Value>(&contents).map_err(|_| malformed())?,
    };
    parse_credentials(&value, format.source())
}

fn parse_credentials(value: &Value, source: &str) -> Result<Option<ChatGptCredentials>, JevError> {
    if !value.is_object() {
        return Err(auth_error(&format!("{source} must contain an object or table.")));
    }
    if let Some(pair) = json_pair(value, ACCOUNT_ENV, TOKEN_ENV, source)? {
        return Ok(Some(pair));
    }
    if let Some(pair) = json_pair(value, "account_id", "access_token", source)? {
        return Ok(Some(pair));
    }
    for table in ["chatgpt", "tokens"] {
        if let Some(value) = value.get(table).filter(|value| !value.is_null()) {
            let source = format!("the {table} table in {source}");
            if !value.is_object() {
                return Err(auth_error(&format!("Invalid {source}; expected a credential object or table.")));
            }
            if let Some(pair) = json_pair(value, "account_id", "access_token", &source)? {
                return Ok(Some(pair));
            }
        }
    }
    Ok(None)
}

fn json_pair(value: &Value, account_key: &str, token_key: &str, source: &str) -> Result<Option<ChatGptCredentials>, JevError> {
    let account = value.get(account_key);
    let token = value.get(token_key);
    if account.is_none() && token.is_none() {
        return Ok(None);
    }
    if account.is_some_and(|value| !value.is_string()) || token.is_some_and(|value| !value.is_string()) {
        return Err(auth_error(&format!("Credentials in {source} must be strings.")));
    }
    checked_pair(account.and_then(Value::as_str), token.and_then(Value::as_str), source).map(Some)
}

fn checked_pair(account: Option<&str>, token: Option<&str>, source: &str) -> Result<ChatGptCredentials, JevError> {
    let (Some(account_id), Some(access_token)) = (account, token) else {
        return Err(auth_error(&format!(
            "Incomplete credentials in {source}; provide both account ID and access token in the same source."
        )));
    };
    let account_id = account_id.trim();
    let access_token = access_token.trim();
    // Both values become HTTP header fields. Reject whitespace/control bytes
    // without echoing the offending input, while allowing opaque access tokens.
    if [account_id, access_token].iter().any(|value| value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_graphic())) {
        return Err(auth_error(&format!(
            "Invalid credentials in {source}; account ID and access token must be nonempty ASCII values without whitespace."
        )));
    }
    Ok(ChatGptCredentials { account_id: account_id.to_owned(), access_token: access_token.to_owned() })
}

fn auth_error(message: &str) -> JevError {
    JevError::Auth(format!("{message} {LOGIN_HINT}"))
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;

    use super::*;

    fn environment(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let values: HashMap<String, OsString> = pairs.iter().map(|(key, value)| ((*key).into(), (*value).into())).collect();
        move |key| values.get(key).cloned()
    }

    fn files(pairs: &[(&str, &str)]) -> impl Fn(&Path) -> io::Result<String> {
        let values: HashMap<PathBuf, String> = pairs.iter().map(|(path, value)| (PathBuf::from(path), (*value).into())).collect();
        move |path| values.get(path).cloned().ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }

    fn assert_pair(actual: &ChatGptCredentials, account: &str, token: &str) {
        assert_eq!(actual.account_id, account);
        assert_eq!(actual.access_token, token);
    }

    #[test]
    fn environment_wins_without_reading_files_or_requiring_home() {
        let env = environment(&[(ACCOUNT_ENV, " env-account "), (TOKEN_ENV, " opaque-token ")]);
        let credentials = resolve_with(&env, &|_| panic!("environment credentials must not read a cache")).unwrap();
        assert_pair(&credentials, "env-account", "opaque-token");
    }

    #[test]
    fn partial_environment_never_borrows_from_cache() {
        for pairs in [[(ACCOUNT_ENV, "account")], [(TOKEN_ENV, "token")]] {
            let error = resolve_with(&environment(&pairs), &|_| panic!("partial environment must not use cache")).unwrap_err();
            assert!(error.to_string().contains("Incomplete credentials"));
        }
    }

    #[test]
    fn empty_or_unsafe_credentials_are_rejected_without_echoing_secrets() {
        for (account, token) in
            [("", "secret-token"), ("secret-account", " "), ("account\rinjected", "secret-token"), ("secret-account", "token\nheader")]
        {
            let error = resolve_with(&environment(&[(ACCOUNT_ENV, account), (TOKEN_ENV, token)]), &files(&[])).unwrap_err();
            assert!(error.to_string().contains("Invalid credentials"));
            assert!(!error.to_string().contains("secret-"));
            assert!(!error.to_string().contains("injected"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_unicode_environment_is_rejected_without_echoing() {
        use std::os::unix::ffi::OsStringExt;
        let env = |key: &str| match key {
            ACCOUNT_ENV => Some(OsString::from_vec(vec![b's', 0xff])),
            TOKEN_ENV => Some(OsString::from("secret-token")),
            _ => None,
        };
        let error = resolve_with(&env, &files(&[])).unwrap_err().to_string();
        assert!(error.contains("UTF-8"));
        assert!(!error.contains("secret-token"));
    }

    #[test]
    fn supported_toml_shapes_override_codex_cache() {
        for document in [
            "CHATGPT_ACCOUNT_ID = 'toml-account'\nCHATGPT_ACCESS_TOKEN = 'toml-token'",
            "account_id = 'toml-account'\naccess_token = 'toml-token'",
            "[chatgpt]\naccount_id = 'toml-account'\naccess_token = 'toml-token'",
            "[tokens]\naccount_id = 'toml-account'\naccess_token = 'toml-token'\nrefresh_token = 'unused'",
        ] {
            let env = environment(&[("HOME", "/home/test")]);
            let read =
                files(&[("/home/test/.config/auth.toml", document), ("/home/test/.codex/auth.json", "malformed-lower-priority-cache")]);
            assert_pair(&resolve_with(&env, &read).unwrap(), "toml-account", "toml-token");
        }
    }

    #[test]
    fn xdg_and_codex_home_select_their_configured_paths() {
        let env = environment(&[("HOME", "/home/test"), ("XDG_CONFIG_HOME", "/custom/config"), ("CODEX_HOME", "/custom/codex")]);
        let visited = RefCell::new(Vec::new());
        let read = |path: &Path| {
            visited.borrow_mut().push(path.to_path_buf());
            if path == Path::new("/custom/codex/auth.json") {
                Ok(r#"{"tokens":{"account_id":"custom-account","access_token":"custom-token"}}"#.into())
            } else {
                Err(io::ErrorKind::NotFound.into())
            }
        };
        assert_pair(&resolve_with(&env, &read).unwrap(), "custom-account", "custom-token");
        assert_eq!(*visited.borrow(), [PathBuf::from("/custom/config/auth.toml"), PathBuf::from("/custom/codex/auth.json")]);
    }

    #[test]
    fn empty_and_relative_xdg_paths_use_home_instead_of_working_directory() {
        for config in ["", "relative-config"] {
            let env = environment(&[("HOME", "/home/test"), ("XDG_CONFIG_HOME", config), ("CODEX_HOME", "")]);
            let paths = credential_paths(&env);
            assert_eq!(paths.auth_toml.unwrap(), PathBuf::from("/home/test/.config/auth.toml"));
            assert_eq!(paths.codex_json.unwrap(), PathBuf::from("/home/test/.codex/auth.json"));
        }
    }

    #[test]
    fn unrelated_toml_allows_codex_cache_fallback() {
        let env = environment(&[("HOME", "/home/test")]);
        let read = files(&[
            ("/home/test/.config/auth.toml", "[other_service]\naccess_token = 'unrelated'"),
            (
                "/home/test/.codex/auth.json",
                r#"{"OPENAI_API_KEY":null,"tokens":{"id_token":"unused","access_token":"codex-token","refresh_token":"unused","account_id":"codex-account"}}"#,
            ),
        ]);
        assert_pair(&resolve_with(&env, &read).unwrap(), "codex-account", "codex-token");
    }

    #[test]
    fn partial_cache_never_mixes_pairs_or_falls_back() {
        for document in [
            "CHATGPT_ACCOUNT_ID = 'secret-account'\n[chatgpt]\naccess_token = 'secret-token'",
            "[chatgpt]\naccount_id = 'secret-account'\n[tokens]\naccount_id = 'other-account'\naccess_token = 'other-token'",
        ] {
            let error = resolve_with(&environment(&[("HOME", "/home/test")]), &files(&[("/home/test/.config/auth.toml", document)]))
                .unwrap_err()
                .to_string();
            assert!(error.contains("Incomplete credentials"));
            assert!(!error.contains("secret-account"));
            assert!(!error.contains("secret-token"));
        }
    }

    #[test]
    fn malformed_documents_and_types_never_leak_source() {
        for (document, format) in [
            ("CHATGPT_ACCESS_TOKEN = 'secret-token", Format::Toml),
            (r#"{"access_token":"secret-token""#, Format::Json),
            (r#"{"tokens":{"account_id":123,"access_token":"secret-token"}}"#, Format::Json),
            (r#"{"tokens":"secret-token"}"#, Format::Json),
            (r#"["secret-token"]"#, Format::Json),
        ] {
            let error = read_credentials(Path::new("cache"), format, &|_| Ok(document.to_string())).unwrap_err().to_string();
            assert!(!error.contains("secret-token"));
            assert!(error.contains("jg --chatgpt-login"));
        }
    }

    #[test]
    fn unreadable_file_is_an_error_without_echoing_io_details() {
        let read = |_: &Path| Err(io::Error::new(io::ErrorKind::PermissionDenied, "secret-token"));
        let error = resolve_with(&environment(&[("HOME", "/home/test")]), &read).unwrap_err().to_string();
        assert!(error.contains("Cannot read ChatGPT auth.toml"));
        assert!(!error.contains("secret-token"));
    }

    #[test]
    fn api_keys_and_refresh_tokens_are_not_subscription_credentials() {
        let env = environment(&[("HOME", "/home/test"), ("OPENAI_API_KEY", "secret-key")]);
        for document in
            [r#"{"OPENAI_API_KEY":"secret-key","tokens":null}"#, r#"{"tokens":{"refresh_token":"secret-refresh","id_token":"secret-id"}}"#]
        {
            let error = resolve_with(&env, &files(&[("/home/test/.codex/auth.json", document)])).unwrap_err().to_string();
            assert!(error.contains("No ChatGPT subscription credentials"));
            assert!(!error.contains("secret-"));
        }
    }

    #[test]
    fn credentials_debug_redacts_both_fields() {
        let credentials = ChatGptCredentials { account_id: "secret-account".into(), access_token: "secret-token".into() };
        let debug = format!("{credentials:?}");
        assert!(debug.contains("[redacted]"));
        assert!(!debug.contains("secret-"));
    }

    #[test]
    fn explicit_login_reads_new_cache_even_with_stale_overrides() {
        let env = environment(&[("CODEX_HOME", "/custom/codex"), ("HOME", "/home/test"), (ACCOUNT_ENV, "stale-account")]);
        let ran = Cell::new(false);
        let read = |path: &Path| {
            assert!(ran.get(), "cache must be read after login");
            assert_eq!(path, Path::new("/custom/codex/auth.json"));
            Ok(r#"{"tokens":{"account_id":"fresh-account","access_token":"fresh-token"}}"#.into())
        };
        let credentials = device_login_with(&env, &read, |home| {
            assert_eq!(home, Path::new("/custom/codex"));
            ran.set(true);
            Ok(true)
        })
        .unwrap();
        assert_pair(&credentials, "fresh-account", "fresh-token");
    }

    #[test]
    fn failed_login_never_returns_old_cache() {
        let env = environment(&[("HOME", "/home/test")]);
        let error = device_login_with(&env, &|_| panic!("failed login must not return stale cache"), |_| Ok(false)).unwrap_err();
        assert!(error.to_string().contains("did not complete successfully"));
    }

    #[test]
    fn login_without_readable_subscription_cache_is_actionable() {
        let error = device_login_with(&environment(&[("HOME", "/home/test")]), &files(&[]), |_| Ok(true)).unwrap_err();
        assert!(error.to_string().contains("auth.json cache has no ChatGPT subscription credentials"));
        let error = device_login_with(&environment(&[]), &files(&[]), |_| panic!("cannot login without a cache location")).unwrap_err();
        assert!(error.to_string().contains("Set CODEX_HOME or HOME"));
    }

    #[test]
    fn unavailable_login_command_is_actionable_and_redacted() {
        let error = device_login_with(&environment(&[("HOME", "/home/test")]), &files(&[]), |_| {
            Err(io::Error::new(io::ErrorKind::NotFound, "secret-token"))
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("Install the Codex CLI"));
        assert!(!error.contains("secret-token"));
    }

    #[test]
    fn login_uses_device_auth_and_explicit_file_storage() {
        let command = login_command(Path::new("/custom/codex"));
        assert_eq!(command.get_program(), "codex");
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["login", "--device-auth", "-c", "cli_auth_credentials_store=\"file\""]);
        assert!(command.get_envs().any(|(key, value)| key == "CODEX_HOME" && value == Some(Path::new("/custom/codex").as_os_str())));
    }
}
