//! ChatGPT backend through the real `jg` binary against a local HTTP/SSE server.
//!
//! Homes, XDG paths and `CODEX_HOME` are unique per test. Credentials are fake
//! ASCII fixtures only; the developer's Codex cache is never read.

mod common;

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use common::{listing, ran, repo, Ran};
use jevgrep::chatgpt::{CHATGPT_MODEL, DEFAULT_CHATGPT_URL};

const ACCOUNT: &str = "acct-test";
const TOKEN: &str = "tok-test";
const LOGIN_ACCOUNT: &str = "login-account";
const LOGIN_TOKEN: &str = "login-token";
const STALE_ACCOUNT: &str = "stale-account";
const STALE_TOKEN: &str = "stale-token";
const LEAK: &str = "secret-do-not-print";
const LOGIN_MARKER: &str = "FAKE-CODEX-STDOUT-MARKER";

fn unique_home() -> PathBuf {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("jg-chatgpt-home-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&dir);
    for name in ["config", "codex", "bin", "cache", "data"] {
        std::fs::create_dir_all(dir.join(name)).unwrap();
    }
    dir
}

fn chatgpt_command(dir: &Path, home: &Path) -> std::process::Command {
    let mut cmd = common::command(dir);
    cmd.env("HOME", home)
        .env("USERPROFILE", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("CODEX_HOME", home.join("codex"))
        .env("TYPESAFE_API_KEY", "jev-secret-unused");
    cmd
}

fn run_args(dir: &Path, home: &Path, args: &[&str]) -> Ran {
    ran(chatgpt_command(dir, home).args(args).output().unwrap())
}

struct Captured {
    authorization: String,
    account_id: String,
    originator: String,
    accept: String,
    body: Value,
}

struct HttpReply {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
    close: bool,
}

fn object_keys(value: &Value) -> Vec<String> {
    value.as_object().map(|object| object.keys().cloned().collect()).unwrap_or_default()
}

fn find_object_field(value: &Value, name: &str) -> Option<Value> {
    if let Some(found) = value.get(name) {
        if found.is_object() {
            return Some(found.clone());
        }
    }
    match value {
        Value::Object(map) => map.values().find_map(|item| find_object_field(item, name)),
        Value::Array(items) => items.iter().find_map(|item| find_object_field(item, name)),
        Value::String(text) => serde_json::from_str::<Value>(text).ok().and_then(|parsed| find_object_field(&parsed, name)),
        _ => None,
    }
}

fn reconstruct_jev_request(body: &Value) -> Value {
    if let Some(text) = body.pointer("/input/0/content/0/text").and_then(Value::as_str) {
        if let Ok(parsed) = serde_json::from_str::<Value>(text) {
            if parsed.get("questions").is_some_and(Value::is_object) {
                return parsed;
            }
        }
    }
    let questions = find_object_field(body, "questions")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_else(|| panic!("ChatGPT request missing a questions object; top-level keys={:?}", object_keys(body)));
    let state = find_object_field(body, "state").unwrap_or_else(|| json!({}));
    json!({"state": state, "questions": Value::Object(questions)})
}

fn read_http_body(reader: &mut impl BufRead, headers: &BTreeMap<String, String>) -> Option<Vec<u8>> {
    if headers.get("transfer-encoding").is_some_and(|value| value.to_ascii_lowercase().contains("chunked")) {
        let mut raw = Vec::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).ok()? == 0 {
                return None;
            }
            let size = usize::from_str_radix(line.trim().split(';').next().unwrap_or(""), 16).ok()?;
            if size == 0 {
                loop {
                    line.clear();
                    if reader.read_line(&mut line).ok()? == 0 || line.trim().is_empty() {
                        break;
                    }
                }
                return Some(raw);
            }
            let mut chunk = vec![0; size];
            reader.read_exact(&mut chunk).ok()?;
            raw.extend_from_slice(&chunk);
            let mut crlf = [0u8; 2];
            reader.read_exact(&mut crlf).ok()?;
        }
    }
    let length = headers.get("content-length").and_then(|value| value.parse().ok()).unwrap_or(0);
    let mut raw = vec![0; length];
    reader.read_exact(&mut raw).ok()?;
    Some(raw)
}

/// The numbers a question's wording names: `Do lines 3-9 ...` -> [3, 9], `Does line 4 ...` -> [4].
/// Questions travel under positional wire ids, so the wording is all the mock has to go on.
fn numbers_after(instructions: &str, word: &str) -> Option<Vec<usize>> {
    let rest = instructions.split_once(word)?.1;
    let span: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect();
    span.split('-').map(|n| n.parse().ok()).collect()
}

/// Fake Luna, answering in the terse wire shape: a bare percentage per noul, and
/// `{confidence, probabilities[]}` per score, keyed by the ids the request used.
fn answers_json(body: &Value) -> String {
    let reconstructed = reconstruct_jev_request(body);
    let code = listing(&reconstructed);
    let is_hot = |n: usize| code.get(&n).is_some_and(|text| text.contains("needle"));
    let mut answers = serde_json::Map::new();
    for (wire_id, question) in reconstructed["questions"].as_object().unwrap() {
        let instructions = question["instructions"].as_str().unwrap();
        let answer = if question["type"] == "score" {
            let last = question["criteria"].as_array().unwrap().len() - 1;
            let hit = code.keys().any(|&n| is_hot(n));
            let probabilities: Vec<u32> =
                (0..=last).map(|index| if hit && index == last || !hit && index == 0 { 100 } else { 0 }).collect();
            json!({"confidence": 90, "probabilities": probabilities})
        } else if let Some(&[lo, hi]) = numbers_after(instructions, "Do lines ").as_deref() {
            json!(if (lo..=hi).any(is_hot) { 90 } else { 3 })
        } else if let Some(&[n]) = numbers_after(instructions, "Does line ").as_deref() {
            json!(if is_hot(n) { 95 } else { 2 })
        } else {
            json!(if code.keys().any(|&n| is_hot(n)) { 90 } else { 3 })
        };
        answers.insert(wire_id.clone(), answer);
    }
    json!({"answers": answers}).to_string()
}

fn sse_events(events: &[(&str, Value)]) -> Vec<u8> {
    let mut out = String::new();
    for (name, data) in events {
        out.push_str("event: ");
        out.push_str(name);
        out.push('\n');
        out.push_str("data: ");
        out.push_str(&data.to_string());
        out.push_str("\n\n");
    }
    out.into_bytes()
}

fn completed_event(payload: Option<&str>) -> Value {
    let output = match payload {
        Some(text) => json!([{"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": text}]}]),
        None => json!([]),
    };
    json!({
        "type": "response.completed",
        "response": {
            "status": "completed",
            "model": CHATGPT_MODEL,
            "output": output,
            "usage": {"input_tokens": 100, "output_tokens": 10},
            "service_tier": "default"
        }
    })
}

fn success_sse(body: &Value, empty_completed_output: bool) -> HttpReply {
    let payload = answers_json(body);
    let mid = payload.len() / 2;
    let completed = if empty_completed_output { completed_event(None) } else { completed_event(Some(&payload)) };
    HttpReply {
        status: 200,
        content_type: "text/event-stream",
        body: {
            let head = payload.get(..mid).unwrap_or(payload.as_str());
            let tail = payload.get(mid..).unwrap_or("");
            let mut bytes = sse_events(&[
                ("response.created", json!({"type": "response.created"})),
                ("response.output_text.delta", json!({"type": "response.output_text.delta", "delta": head})),
                ("response.output_text.delta", json!({"type": "response.output_text.delta", "delta": tail})),
                ("response.output_text.done", json!({"type": "response.output_text.done", "text": payload})),
                (
                    "response.output_item.done",
                    json!({"type": "response.output_item.done", "item": {"type": "message", "content": [{"type": "output_text", "text": payload}]}}),
                ),
                ("response.completed", completed),
            ]);
            bytes.extend_from_slice(b"data: [DONE]\n\n");
            bytes
        },
        close: true,
    }
}

fn error_json(status: u16, body: &str) -> HttpReply {
    HttpReply { status, content_type: "application/json", body: body.as_bytes().to_vec(), close: true }
}

fn serve(handler: impl Fn(&Captured) -> HttpReply + Send + Sync + 'static) -> (String, Arc<Mutex<Vec<Captured>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/backend-api/codex/responses", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let handler = Arc::new(handler);
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let handler = Arc::clone(&handler);
            let log = Arc::clone(&log);
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut stream = stream;
                loop {
                    let mut first = String::new();
                    if reader.read_line(&mut first).unwrap_or(0) == 0 {
                        return;
                    }
                    let (mut headers, mut line) = (BTreeMap::new(), String::new());
                    loop {
                        line.clear();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                            break;
                        }
                        if let Some((name, value)) = line.split_once(':') {
                            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
                        }
                    }
                    let Some(raw) = read_http_body(&mut reader, &headers) else {
                        return;
                    };
                    let close_requested = headers.get("connection").is_some_and(|value| value.eq_ignore_ascii_case("close"));
                    let captured = Captured {
                        authorization: headers.get("authorization").cloned().unwrap_or_default(),
                        account_id: headers.get("chatgpt-account-id").cloned().unwrap_or_default(),
                        originator: headers.get("originator").cloned().unwrap_or_default(),
                        accept: headers.get("accept").cloned().unwrap_or_default(),
                        body: serde_json::from_slice(&raw).unwrap_or(Value::Null),
                    };
                    let reply = catch_unwind(AssertUnwindSafe(|| handler(&captured)))
                        .unwrap_or_else(|_| error_json(500, r#"{"error":{"code":"fixture_panic"}}"#));
                    log.lock().unwrap().push(captured);
                    let connection = if reply.close || close_requested { "close" } else { "keep-alive" };
                    let head = format!(
                        "HTTP/1.1 {} X\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: {connection}\r\n\r\n",
                        reply.status,
                        reply.content_type,
                        reply.body.len()
                    );
                    if stream.write_all(head.as_bytes()).and_then(|()| stream.write_all(&reply.body)).is_err() {
                        return;
                    }
                    if reply.close || close_requested {
                        return;
                    }
                }
            });
        }
    });
    (url, seen)
}

fn needle_server() -> (String, Arc<Mutex<Vec<Captured>>>) {
    serve(|req| success_sse(&req.body, false))
}

fn empty_output_server() -> (String, Arc<Mutex<Vec<Captured>>>) {
    serve(|req| success_sse(&req.body, true))
}

fn fixture_dir() -> PathBuf {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    repo(&format!("chatgpt-cli-{}", SEQ.fetch_add(1, Ordering::Relaxed)), &[("t.py", "a = 1\nneedle = 2\nb = 3\n")])
}

fn assert_no_secrets(run: &Ran) {
    for secret in [TOKEN, LOGIN_TOKEN, STALE_TOKEN, ACCOUNT, LOGIN_ACCOUNT, STALE_ACCOUNT, LEAK, "jev-secret-unused"] {
        assert!(!run.out.contains(secret), "stdout leaked credential material");
        assert!(!run.err.contains(secret), "stderr leaked credential material");
    }
}

fn assert_wire_contract(req: &Captured, account: &str, token: &str) {
    assert_eq!(req.authorization, format!("Bearer {token}"));
    assert_eq!(req.account_id, account);
    assert_eq!(req.originator, "codex_cli_rs");
    assert!(req.accept.contains("text/event-stream"), "{}", req.accept);
    assert_eq!(req.body["model"], CHATGPT_MODEL);
    assert_eq!(req.body["service_tier"], "priority");
    assert_ne!(req.body["service_tier"], "fast");
    assert_eq!(req.body["stream"], true);
    assert_eq!(req.body["store"], false);
    let format = &req.body["text"]["format"];
    assert_eq!(format["type"], "json_schema");
    assert_eq!(format["name"], "jev_answers");
    assert_eq!(format["strict"], true);
    assert!(format["schema"].to_string().contains("answers"), "schema must describe answers");
    // Luna thinks at `medium` by default; the measured setting is sent on every request.
    assert_eq!(req.body["reasoning"]["effort"], "low");
}

fn expected_json() -> Value {
    json!({
        "query": "find needle", "path": "t.py", "relevance": 1.0, "section_relevance": 1.0, "confidence": 0.9, "match": "strong",
        "regions": [{"start": 1, "end": 3, "p": 0.9, "label": "a = 1", "label_line": 1, "lines": [{"line": 2, "p": 0.95, "text": "needle = 2"}]}],
        "lines": [], "more_regions": 0, "more_lines": 0,
    })
}

fn expected_text() -> &'static str {
    "t.py  relevance=1.00\n        1-3  0.90  a = 1\n          2  0.95  needle = 2\n\n"
}

fn expected_flat() -> &'static str {
    "t.py:1-3:0.90:a = 1\nt.py:2:0.95:needle = 2\n"
}

#[test]
fn sse_embedded_output_matches_jev_json_and_text() {
    let (url, seen) = needle_server();
    let dir = fixture_dir();
    let home = unique_home();
    let run = ran(chatgpt_command(&dir, &home)
        .args(["--backend", "chatgpt", "--base-url", &url, "find needle", "--json", "-q"])
        .env("CHATGPT_ACCOUNT_ID", ACCOUNT)
        .env("CHATGPT_ACCESS_TOKEN", TOKEN)
        .output()
        .unwrap());
    assert_eq!((run.code, run.err.as_str()), (0, ""), "{}", run.err);
    assert_eq!(serde_json::from_str::<Value>(&run.out).unwrap(), expected_json());
    assert!(run.out.starts_with(r#"{"query":"find needle","path":"t.py","relevance":1.0,"#));
    assert_no_secrets(&run);
    {
        let captured = seen.lock().unwrap();
        assert!(!captured.is_empty(), "expected at least one ChatGPT request");
        for req in captured.iter() {
            assert_wire_contract(req, ACCOUNT, TOKEN);
        }
    }

    let run = ran(chatgpt_command(&dir, &home)
        .args(["--backend", "chatgpt", "--base-url", &url, "find needle", "-q"])
        .env("CHATGPT_ACCOUNT_ID", ACCOUNT)
        .env("CHATGPT_ACCESS_TOKEN", TOKEN)
        .output()
        .unwrap());
    assert_eq!((run.code, run.out.as_str(), run.err.as_str()), (0, expected_text(), ""));
    assert_no_secrets(&run);

    let run = ran(chatgpt_command(&dir, &home)
        .args(["--backend", "chatgpt", "--base-url", &url, "find needle", "--no-heading", "-q"])
        .env("CHATGPT_ACCOUNT_ID", ACCOUNT)
        .env("CHATGPT_ACCESS_TOKEN", TOKEN)
        .output()
        .unwrap());
    assert_eq!((run.code, run.out.as_str()), (0, expected_flat()));
}

#[test]
fn sse_empty_completed_output_assembles_deltas() {
    let (url, seen) = empty_output_server();
    let dir = fixture_dir();
    let home = unique_home();
    let run = ran(chatgpt_command(&dir, &home)
        .args(["--backend", "chatgpt", "--base-url", &url, "find needle", "--json", "-q"])
        .env("CHATGPT_ACCOUNT_ID", ACCOUNT)
        .env("CHATGPT_ACCESS_TOKEN", TOKEN)
        .output()
        .unwrap());
    assert_eq!((run.code, run.err.as_str()), (0, ""), "{}", run.err);
    assert_eq!(serde_json::from_str::<Value>(&run.out).unwrap(), expected_json());
    assert!(!seen.lock().unwrap().is_empty());
    assert_no_secrets(&run);
}

#[test]
fn jg_backend_env_routes_and_keep_alive_handles_two_files() {
    let (url, seen) = needle_server();
    let dir = repo("chatgpt-two", &[("a.py", "needle = 1\n"), ("b.py", "needle = 2\n")]);
    let home = unique_home();
    let run = ran(chatgpt_command(&dir, &home)
        .args(["find needle", "--json", "-q"])
        .env("JG_BACKEND", "chatgpt")
        .env("JG_BASE_URL", &url)
        .env("CHATGPT_ACCOUNT_ID", ACCOUNT)
        .env("CHATGPT_ACCESS_TOKEN", TOKEN)
        .output()
        .unwrap());
    assert_eq!(run.code, 0, "{}", run.err);
    let paths: Vec<_> =
        run.out.lines().map(|line| serde_json::from_str::<Value>(line).unwrap()["path"].as_str().unwrap().to_owned()).collect();
    assert_eq!(paths, ["a.py", "b.py"]);
    assert!(seen.lock().unwrap().len() >= 2);
    assert_no_secrets(&run);
}

#[test]
fn stats_report_subscription_tokens_not_jev_price() {
    let (url, _) = needle_server();
    let dir = fixture_dir();
    let home = unique_home();
    let run = ran(chatgpt_command(&dir, &home)
        .args(["--backend", "chatgpt", "--base-url", &url, "find needle"])
        .env("CHATGPT_ACCOUNT_ID", ACCOUNT)
        .env("CHATGPT_ACCESS_TOKEN", TOKEN)
        .output()
        .unwrap());
    assert_eq!(run.code, 0, "{}", run.err);
    assert_eq!(run.out, expected_text());
    assert!(run.err.contains("input /"), "{}", run.err);
    assert!(run.err.contains("output tokens"), "{}", run.err);
    assert!(run.err.contains("ChatGPT subscription"), "{}", run.err);
    assert!(run.err.contains("requested priority"), "{}", run.err);
    assert!(run.err.contains("served"), "{}", run.err);
    assert!(!run.err.contains('$'), "{}", run.err);
    assert!(!run.err.contains("0.042"), "{}", run.err);
    assert!(!run.err.contains("TypeSafe"), "{}", run.err);
    assert_no_secrets(&run);
}

#[test]
fn explicit_non_luna_model_is_rejected_without_http() {
    let (url, seen) = needle_server();
    let dir = fixture_dir();
    let home = unique_home();
    for args in [
        vec!["--backend", "chatgpt", "--model", "gpt-4", "--base-url", url.as_str(), "find needle"],
        vec!["--backend", "chatgpt", "--model", "jev-latest", "find needle"],
    ] {
        let run = run_args(&dir, &home, &args);
        assert_eq!(run.code, 2, "{}", run.err);
        assert!(run.err.contains(CHATGPT_MODEL), "{}", run.err);
        assert!(run.out.is_empty());
        assert_no_secrets(&run);
    }
    assert!(seen.lock().unwrap().is_empty());
    let run = ran(chatgpt_command(&dir, &home)
        .args(["--backend", "chatgpt", "--base-url", &url, "find needle"])
        .env("JG_MODEL", "jev-latest")
        .output()
        .unwrap());
    assert_eq!(run.code, 2, "{}", run.err);
    assert!(run.err.contains(CHATGPT_MODEL), "{}", run.err);
    assert!(seen.lock().unwrap().is_empty());
    let run = ran(chatgpt_command(&dir, &home)
        .args(["--backend", "chatgpt", "--model", CHATGPT_MODEL, "--base-url", &url, "find needle", "-q"])
        .env("JG_MODEL", "jev-latest")
        .env("CHATGPT_ACCOUNT_ID", ACCOUNT)
        .env("CHATGPT_ACCESS_TOKEN", TOKEN)
        .output()
        .unwrap());
    assert_eq!(run.code, 0, "{}", run.err);
    assert!(!seen.lock().unwrap().is_empty());
}

#[test]
fn missing_and_partial_credentials_are_actionable() {
    let (url, seen) = needle_server();
    let dir = fixture_dir();
    let home = unique_home();
    let run = ran(chatgpt_command(&dir, &home).args(["--backend", "chatgpt", "--base-url", &url, "find needle"]).output().unwrap());
    assert_eq!(run.code, 2, "{}", run.err);
    assert!(run.err.contains("No ChatGPT subscription credentials"), "{}", run.err);
    assert!(run.err.contains("--chatgpt-login"), "{}", run.err);
    assert!(run.err.contains("--backend chatgpt"), "{}", run.err);
    assert_no_secrets(&run);
    assert!(seen.lock().unwrap().is_empty());

    let run = ran(chatgpt_command(&dir, &home)
        .args(["--backend", "chatgpt", "--base-url", &url, "find needle"])
        .env("CHATGPT_ACCOUNT_ID", ACCOUNT)
        .output()
        .unwrap());
    assert_eq!(run.code, 2, "{}", run.err);
    assert!(run.err.contains("Incomplete credentials"), "{}", run.err);
    assert!(!run.err.contains(ACCOUNT), "{}", run.err);
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn auth_error_is_actionable_and_does_not_echo_secrets() {
    let (url, seen) = serve(|_| error_json(401, &format!(r#"{{"error":{{"code":"invalid_token","raw":"{LEAK}"}}}}"#)));
    let dir = fixture_dir();
    let home = unique_home();
    let run = ran(chatgpt_command(&dir, &home)
        .args(["--backend", "chatgpt", "--base-url", &url, "find needle"])
        .env("CHATGPT_ACCOUNT_ID", ACCOUNT)
        .env("CHATGPT_ACCESS_TOKEN", TOKEN)
        .output()
        .unwrap());
    assert_eq!(run.code, 2, "{}", run.err);
    assert!(run.err.contains("ChatGPT rejected the subscription credentials"), "{}", run.err);
    assert!(run.err.contains("HTTP 401"), "{}", run.err);
    assert!(run.err.contains("--chatgpt-login") || run.err.contains("Codex CLI"), "{}", run.err);
    assert_no_secrets(&run);
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[cfg(unix)]
#[test]
fn normal_search_never_starts_codex_login() {
    let (url, seen) = needle_server();
    let dir = fixture_dir();
    let home = unique_home();
    install_fake_codex(&home);
    let run = ran(with_codex_path(chatgpt_command(&dir, &home), &home)
        .args(["--backend", "chatgpt", "--base-url", &url, "find needle", "--json", "-q"])
        .env("CHATGPT_ACCOUNT_ID", ACCOUNT)
        .env("CHATGPT_ACCESS_TOKEN", TOKEN)
        .output()
        .unwrap());
    assert_eq!(run.code, 0, "{}", run.err);
    assert!(!home.join("codex/codex-args").exists());
    assert!(!run.err.contains(LOGIN_MARKER) && !run.out.contains(LOGIN_MARKER));
    assert_eq!(seen.lock().unwrap()[0].authorization, format!("Bearer {TOKEN}"));
    assert_no_secrets(&run);
}

#[cfg(unix)]
#[test]
fn chatgpt_login_runs_device_auth_and_redirects_codex_stdout() {
    let (url, seen) = needle_server();
    let dir = fixture_dir();
    let home = unique_home();
    install_fake_codex(&home);
    let run = ran(with_codex_path(chatgpt_command(&dir, &home), &home)
        .args(["--backend", "chatgpt", "--chatgpt-login", "--base-url", &url, "find needle", "--json", "-q"])
        .env("CHATGPT_ACCOUNT_ID", STALE_ACCOUNT)
        .env("CHATGPT_ACCESS_TOKEN", STALE_TOKEN)
        .output()
        .unwrap());
    assert_eq!(run.code, 0, "{}", run.err);
    assert!(run.err.contains(LOGIN_MARKER), "codex stdout must be redirected to stderr: {}", run.err);
    assert!(!run.out.contains(LOGIN_MARKER), "login marker must not pollute JSON stdout: {}", run.out);
    assert_eq!(serde_json::from_str::<Value>(&run.out).unwrap(), expected_json());
    let args = std::fs::read_to_string(home.join("codex/codex-args")).unwrap();
    assert!(args.contains("login"), "{args}");
    assert!(args.contains("--device-auth"), "{args}");
    assert!(args.contains("cli_auth_credentials_store=\"file\""), "{args}");
    let captured = seen.lock().unwrap();
    assert!(!captured.is_empty());
    for req in captured.iter() {
        assert_wire_contract(req, LOGIN_ACCOUNT, LOGIN_TOKEN);
    }
    assert!(!run.out.contains(STALE_TOKEN) && !run.err.contains(STALE_TOKEN));
}

#[test]
fn non_local_http_base_url_is_rejected() {
    let dir = fixture_dir();
    let home = unique_home();
    let run = ran(chatgpt_command(&dir, &home)
        .args(["--backend", "chatgpt", "--base-url", "http://example.com/backend-api/codex/responses", "find needle"])
        .env("CHATGPT_ACCOUNT_ID", ACCOUNT)
        .env("CHATGPT_ACCESS_TOKEN", TOKEN)
        .output()
        .unwrap());
    assert_eq!(run.code, 2, "{}", run.err);
    assert!(run.err.contains("https") || run.err.contains("localhost") || run.err.contains("refusing"), "{}", run.err);
    assert_no_secrets(&run);
}

#[test]
fn chatgpt_login_requires_chatgpt_backend() {
    let dir = fixture_dir();
    let home = unique_home();
    let run = run_args(&dir, &home, &["--chatgpt-login", "find needle"]);
    assert_eq!(run.code, 2, "{}", run.err);
    assert!(run.err.contains("--backend chatgpt"), "{}", run.err);
    assert!(run.out.is_empty());
}

#[test]
fn help_documents_chatgpt_backend_without_reading_credentials() {
    let dir = fixture_dir();
    let home = unique_home();
    let run = run_args(&dir, &home, &["--help"]);
    assert_eq!(run.code, 0, "{}", run.err);
    assert!(run.err.is_empty());
    assert!(run.out.contains("--backend"));
    assert!(run.out.contains("--chatgpt-login"));
    assert!(run.out.contains("JG_BACKEND"));
    assert!(run.out.contains(CHATGPT_MODEL));
    assert!(run.out.contains("chatgpt"));
    assert_ne!(DEFAULT_CHATGPT_URL, jevgrep::client::DEFAULT_BASE_URL);
    assert_no_secrets(&run);
}

#[cfg(unix)]
fn install_fake_codex(home: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let path = home.join("bin/codex");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' '{LOGIN_MARKER}'\nprintf '%s\\n' \"$*\" > \"$CODEX_HOME/codex-args\"\ncat > \"$CODEX_HOME/auth.json\" <<'EOF'\n{{\"tokens\":{{\"account_id\":\"{LOGIN_ACCOUNT}\",\"access_token\":\"{LOGIN_TOKEN}\"}}}}\nEOF\nexit 0\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
fn with_codex_path(mut cmd: std::process::Command, home: &Path) -> std::process::Command {
    let path = format!("{}:{}", home.join("bin").display(), std::env::var("PATH").unwrap_or_default());
    cmd.env("PATH", path);
    cmd
}
