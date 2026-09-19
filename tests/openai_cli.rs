//! The openai backend through the real `jg` binary against a local OpenAI-compatible server
//! that speaks both the Responses API and chat completions.
//!
//! Keys are fake ASCII fixtures. No test reaches a real service, and none may launch `fnox`.

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use common::{listing, ran, repo, serve, Ran, Request};

const ENV_KEY: &str = "sk-env-fixture";
const FLAG_KEY: &str = "sk-flag-fixture";

fn fixture_dir() -> PathBuf {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    repo(&format!("openai-cli-{}", SEQ.fetch_add(1, Ordering::Relaxed)), &[("t.py", "a = 1\nneedle = 2\nb = 3\n")])
}

/// The Jev-shaped request inside a Responses `input` or a chat completion's user message.
fn asked(body: &Value) -> Value {
    let text = body["input"].as_str().or_else(|| body["messages"][1]["content"].as_str()).expect("the user's message");
    serde_json::from_str(text).expect("state and questions")
}

fn numbers_after(instructions: &str, word: &str) -> Option<Vec<usize>> {
    let rest = instructions.split_once(word)?.1;
    let span: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect();
    span.split('-').map(|n| n.parse().ok()).collect()
}

/// A fake model answering in the terse wire shape: lines containing `needle` are hits.
fn answers_text(body: &Value) -> String {
    let asked = asked(body);
    let code = listing(&asked);
    let is_hot = |n: usize| code.get(&n).is_some_and(|text| text.contains("needle"));
    let mut answers = serde_json::Map::new();
    for (wire_id, question) in asked["questions"].as_object().unwrap() {
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

/// A completed Responses API body, as an aggregator that states a cost sends it.
fn response(text: &str) -> String {
    json!({
        "object": "response",
        "status": "completed",
        "error": null,
        "output": [
            {"type": "reasoning", "summary": []},
            {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": text}]},
        ],
        "usage": {"input_tokens": 1200, "output_tokens": 300, "cost": 0.0125},
    })
    .to_string()
}

/// A plain chat completion: other names for the usage counts, and no cost.
fn completion(content: &str) -> String {
    json!({
        "choices": [{"index": 0, "finish_reason": "stop", "message": {"role": "assistant", "content": content}}],
        "usage": {"prompt_tokens": 800, "completion_tokens": 40, "total_tokens": 840},
    })
    .to_string()
}

/// (path, authorization, body) of every request, in order.
type Seen = Arc<Mutex<Vec<(String, String, Value)>>>;

/// A server that records every request and answers it with `respond`. Returns its API root.
fn recording(respond: impl Fn(&Request, usize) -> (u16, String) + Send + Sync + 'static) -> (String, Seen) {
    let seen: Seen = Arc::default();
    let sink = seen.clone();
    let url = serve(move |request| {
        let mut seen = sink.lock().unwrap();
        seen.push((request.path.clone(), request.authorization.clone(), request.body.clone()));
        respond(request, seen.len())
    });
    (url.replace("/v1/systemone", "/api/v1"), seen)
}

/// A service with both APIs and structured outputs.
fn full_service() -> (String, Seen) {
    recording(|request, _| match request.path.ends_with("/responses") {
        true => (200, response(&answers_text(&request.body))),
        false => (200, completion(&answers_text(&request.body))),
    })
}

/// `jg --backend openai` configured the way OpenAI's SDKs are: by environment alone.
fn jg(dir: &Path, root: &str) -> std::process::Command {
    let mut cmd = common::command(dir);
    cmd.args(["--backend", "openai", "--model", "vendor/some-model"]).env("OPENAI_BASE_URL", root).env("OPENAI_API_KEY", ENV_KEY);
    cmd
}

fn run(dir: &Path, root: &str, args: &[&str]) -> Ran {
    ran(jg(dir, root).args(args).output().unwrap())
}

fn assert_no_keys(run: &Ran) {
    for key in [ENV_KEY, FLAG_KEY] {
        assert!(!run.out.contains(key) && !run.err.contains(key), "output leaked a key: {}{}", run.out, run.err);
    }
}

#[test]
fn search_prefers_the_responses_api_with_a_strict_schema_and_matches_the_jev_output_shapes() {
    let (root, seen) = full_service();
    let dir = fixture_dir();
    let text = run(&dir, &root, &["-q", "find needle"]);
    assert_eq!(
        (text.code, text.out.as_str(), text.err.as_str()),
        (0, "t.py  relevance=1.00\n        1-3  0.90  a = 1\n          2  0.95  needle = 2\n\n", "")
    );
    let flat = run(&dir, &root, &["-q", "--no-heading", "find needle"]);
    assert_eq!((flat.code, flat.out.as_str()), (0, "t.py:1-3:0.90:a = 1\nt.py:2:0.95:needle = 2\n"));
    let parsed: Value = serde_json::from_str(run(&dir, &root, &["-q", "--json", "find needle"]).out.trim()).unwrap();
    assert_eq!(
        (&parsed["path"], &parsed["relevance"], &parsed["regions"][0]["lines"][0]["line"]),
        (&json!("t.py"), &json!(1.0), &json!(2))
    );

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 3);
    for (path, authorization, body) in seen.iter() {
        assert_eq!((path.as_str(), authorization), ("/api/v1/responses", &format!("Bearer {ENV_KEY}")));
        // Only the API's common core: nothing a strict service would refuse, nothing of one vendor's.
        let fields: Vec<&str> = body.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(fields, ["model", "instructions", "input", "store", "temperature", "text"], "{body}");
        assert_eq!((&body["model"], &body["store"]), (&json!("vendor/some-model"), &json!(false)));
        let format = &body["text"]["format"];
        assert_eq!((&format["type"], &format["strict"]), (&json!("json_schema"), &json!(true)));
        let asked = asked(body);
        assert_eq!(asked["state"]["query"], "find needle");
        let ids: Vec<&String> = asked["questions"].as_object().unwrap().keys().collect();
        assert!(ids.iter().all(|id| id.parse::<usize>().is_ok()));
        assert_eq!(format["schema"]["properties"]["answers"]["required"].as_array().unwrap().len(), ids.len());
    }
}

#[test]
fn a_service_without_responses_or_structured_outputs_costs_refusals_not_the_search() {
    // No /responses route; the model has no structured outputs and fixed sampling.
    let (root, seen) = recording(|request, _| {
        if !request.path.ends_with("/chat/completions") {
            return (404, json!({"error": {"message": "Not Found", "code": 404}}).to_string());
        }
        if request.body.get("response_format").is_some() {
            let raw = "{\"code\":400, \"reason\":\"INVALID_REQUEST_BODY\", \"message\":\"model features structured outputs not support\"}";
            let error = json!({"message": "Provider returned error", "code": 400, "metadata": {"raw": raw, "provider_name": "Novita"}});
            return (400, json!({"error": error}).to_string());
        }
        if request.body.get("temperature").is_some() {
            let message = "Unsupported value: 'temperature' does not support 0 with this model. Only the default (1) value is supported.";
            return (400, json!({"error": {"message": message, "type": "invalid_request_error", "param": "temperature"}}).to_string());
        }
        (200, completion(&answers_text(&request.body)))
    });
    let done = run(&fixture_dir(), &root, &["-j", "1", "find needle"]);
    assert_eq!(done.code, 0, "{}", done.err);
    assert!(done.out.contains("needle = 2") && done.err.contains("1 requests, 3 retries"), "{}", done.err);
    let shapes: Vec<(String, bool, bool)> = seen
        .lock()
        .unwrap()
        .iter()
        .map(|(path, _, body)| {
            (path.clone(), body.get("text").or(body.get("response_format")).is_some(), body.get("temperature").is_some())
        })
        .collect();
    let chat = || "/api/v1/chat/completions".to_owned();
    assert_eq!(shapes, [("/api/v1/responses".to_owned(), true, true), (chat(), true, true), (chat(), false, true), (chat(), false, false)]);
}

#[test]
fn a_full_endpoint_url_settles_the_api_and_no_schema_skips_the_first_refusal() {
    let (root, seen) = full_service();
    let dir = fixture_dir();
    let chat = format!("{root}/chat/completions");
    assert_eq!(ran(jg(&dir, &root).args(["-q", "--base-url", &chat, "--no-schema", "find needle"]).output().unwrap()).code, 0);
    assert_eq!(ran(jg(&dir, &root).args(["-q", "find needle"]).env("JG_BASE_URL", format!("{root}/responses/")).output().unwrap()).code, 0);
    let seen = seen.lock().unwrap();
    let fields = |body: &Value| body.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
    assert_eq!(
        (seen[0].0.as_str(), fields(&seen[0].2)),
        ("/api/v1/chat/completions", vec!["model".into(), "messages".into(), "temperature".into()])
    );
    assert_eq!((seen[1].0.as_str(), seen[1].2.get("text").is_some()), ("/api/v1/responses", true));
}

#[test]
fn the_api_key_flag_wins_over_the_environment_and_is_never_printed() {
    let (root, seen) = full_service();
    let both = run(&fixture_dir(), &root, &["--api-key", FLAG_KEY, "find needle"]);
    assert_eq!(both.code, 0, "{}", both.err);
    assert_no_keys(&both);
    let seen = seen.lock().unwrap();
    assert!(seen.len() == 1 && seen[0].1 == format!("Bearer {FLAG_KEY}"));
}

#[cfg(unix)]
#[test]
fn openai_itself_needs_a_key_which_is_never_fetched_from_fnox_or_borrowed() {
    use std::os::unix::fs::PermissionsExt;
    let dir = fixture_dir();
    // A `fnox` that would hand over a key, and leave a mark, if jg ever asked it. The mark is made
    // with a shell builtin: PATH holds nothing but this script.
    let bin = dir.join("fake-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let marker = dir.join("fnox-was-run");
    std::fs::write(bin.join("fnox"), format!("#!/bin/sh\n: > '{}'\necho sk-from-fnox\n", marker.display())).unwrap();
    std::fs::set_permissions(bin.join("fnox"), std::fs::Permissions::from_mode(0o755)).unwrap();

    // The default base URL is api.openai.com, so each of these is refused before any request.
    for key in [None, Some(""), Some("  ")] {
        let mut cmd = common::command(&dir);
        cmd.args(["--backend", "openai", "--model", "m", "find needle"]).env("PATH", &bin).env_remove("JG_NO_FNOX");
        // Other services' keys are different credentials and must not be borrowed.
        cmd.env("OPENROUTER_API_KEY", "sk-or-other").env("TYPESAFE_API_KEY", "jev-key");
        if let Some(key) = key {
            cmd.env("OPENAI_API_KEY", key);
        }
        let missing = ran(cmd.output().unwrap());
        assert_eq!((missing.code, missing.out.as_str()), (2, ""));
        assert_eq!(missing.err, "jg: OPENAI_API_KEY is not set. Export it, or pass --api-key <KEY>.\n");
    }
    assert!(!marker.exists(), "jg launched fnox for an OpenAI key");
}

#[test]
fn a_local_server_needs_no_key_and_one_that_wants_a_key_says_so() {
    let (root, seen) = full_service();
    let dir = fixture_dir();
    let mut keyless = common::command(&dir);
    keyless.args(["--backend", "openai", "--base-url", &root, "--model", "m", "-q", "find needle"]);
    let done = ran(keyless.output().unwrap());
    assert_eq!((done.code, done.err.as_str()), (0, ""));
    assert_eq!(seen.lock().unwrap()[0].1, "", "no Authorization header without a key");

    let no_model = ran(common::command(&dir).args(["--backend", "openai", "--base-url", &root, "find needle"]).output().unwrap());
    assert_eq!((no_model.code, no_model.err.as_str()), (2, "error: --backend openai has no default model; pass --model or set JG_MODEL\n"));
    assert_eq!(seen.lock().unwrap().len(), 1);

    let (root, _) = recording(|_, _| (401, json!({"error": {"message": "Invalid API key", "type": "invalid_request_error"}}).to_string()));
    let wanted =
        ran(common::command(&dir).args(["--backend", "openai", "--base-url", &root, "--model", "m", "find needle"]).output().unwrap());
    assert_eq!(wanted.code, 2);
    assert_eq!(
        wanted.err,
        "jg: 127.0.0.1 wants an API key (HTTP 401: Invalid API key) and none was sent. Set OPENAI_API_KEY or pass --api-key <KEY>.\n"
    );
}

#[test]
fn extra_body_reaches_the_wire_from_the_flag_or_the_environment() {
    let (root, seen) = full_service();
    let dir = fixture_dir();
    let extra = r#"{"reasoning":{"effort":"low"},"temperature":null,"provider":{"require_parameters":true}}"#;
    assert_eq!(run(&dir, &root, &["-q", "--extra-body", extra, "find needle"]).code, 0);
    assert_eq!(ran(jg(&dir, &root).args(["-q", "find needle"]).env("JG_EXTRA_BODY", r#"{"seed":7}"#).output().unwrap()).code, 0);
    let seen = seen.lock().unwrap();
    let body = &seen[0].2;
    assert_eq!((&body["reasoning"]["effort"], &body["provider"]["require_parameters"]), (&json!("low"), &json!(true)));
    assert!(body.get("temperature").is_none() && body.get("text").is_some(), "{body}");
    assert_eq!(seen[1].2["seed"], 7);

    let reserved = run(&dir, &root, &["--extra-body", r#"{"stream":true}"#, "find needle"]);
    assert_eq!((reserved.code, reserved.err.as_str()), (2, "error: --extra-body cannot set `stream`; jg owns that request field\n"));
}

#[test]
fn stats_name_the_service_and_model_and_show_a_cost_only_where_one_is_reported() {
    let (root, _) = full_service();
    let dir = fixture_dir();
    let stats = |run: &Ran| run.err.lines().last().unwrap().to_owned();
    let responses = run(&dir, &root, &["find needle"]);
    assert_eq!(responses.code, 0);
    assert!(
        stats(&responses)
            .starts_with("jg: 1 files, 1 requests, 1,200 input / 300 output tokens (127.0.0.1 vendor/some-model; reported cost $0.0125), "),
        "{}",
        responses.err
    );
    let chat = run(&dir, &root, &["--base-url", &format!("{root}/chat/completions"), "find needle"]);
    assert!(
        stats(&chat).starts_with("jg: 1 files, 1 requests, 800 input / 40 output tokens (127.0.0.1 vendor/some-model), "),
        "{}",
        chat.err
    );
    assert!(!chat.err.contains("ChatGPT") && !chat.err.contains("(~$"), "{}", chat.err);
}

#[test]
fn an_unusable_reply_is_resampled_warmer_and_a_persistent_one_is_an_error() {
    let (root, seen) = recording(|request, count| {
        let content =
            if count == 1 { "Line 2 looks relevant to me.".to_owned() } else { format!("```json\n{}\n```", answers_text(&request.body)) };
        (200, response(&content))
    });
    let recovered = run(&fixture_dir(), &root, &["find needle"]);
    assert_eq!(recovered.code, 0, "{}", recovered.err);
    assert!(recovered.out.contains("needle = 2") && recovered.err.contains("2 requests, 1 retries"), "{}", recovered.err);
    let temperatures: Vec<f64> = seen.lock().unwrap().iter().map(|(_, _, body)| body["temperature"].as_f64().unwrap()).collect();
    assert_eq!(temperatures, [0.0, 0.7]);

    let (root, seen) = recording(|_, _| (200, response("{\"answers\": {\"0\": 50}}")));
    let failed = run(&fixture_dir(), &root, &["find needle"]);
    assert_eq!((failed.code, failed.out.as_str()), (2, ""));
    assert!(failed.err.contains("127.0.0.1 API error: model gave an unusable reply 3 times"), "{}", failed.err);
    assert_eq!(seen.lock().unwrap().len(), 3);
}

#[test]
fn a_rejected_key_is_actionable_and_echoes_no_key() {
    let (root, seen) = recording(|_, _| (401, json!({"error": {"code": 401, "message": "User not found."}}).to_string()));
    let rejected = run(&fixture_dir(), &root, &["--api-key", FLAG_KEY, "find needle"]);
    assert_eq!((rejected.code, rejected.out.as_str()), (2, ""));
    assert_eq!(rejected.err, "jg: 127.0.0.1 rejected the API key (HTTP 401: User not found.). Check OPENAI_API_KEY or --api-key.\n");
    assert_eq!(seen.lock().unwrap().len(), 1, "an authentication failure is not retried");

    // A service that echoes the request back must not get the key onto the terminal.
    let (root, _) = recording(|request, _| {
        let raw = format!("upstream exploded\nheaders: {}", request.authorization);
        let error = json!({"code": 403, "message": "Provider returned error", "metadata": {"raw": raw, "provider_name": "Fixture"}});
        (403, json!({"error": error}).to_string())
    });
    let echoed = run(&fixture_dir(), &root, &["find needle"]);
    assert_eq!(echoed.code, 2);
    assert!(
        echoed.err.contains("HTTP 403: Provider returned error (Fixture: upstream exploded headers: Bearer <redacted>)"),
        "{}",
        echoed.err
    );
    assert_no_keys(&echoed);
}

#[test]
fn credentials_are_not_sent_over_plaintext_to_another_host() {
    let refused = run(&fixture_dir(), "http://llm.example/api/v1", &["find needle"]);
    assert_eq!((refused.code, refused.out.as_str()), (2, ""));
    assert!(refused.err.contains("refusing to send llm.example credentials"), "{}", refused.err);
    assert_no_keys(&refused);
}

#[test]
fn flags_are_scoped_to_the_backend_and_help_documents_it() {
    let dir = fixture_dir();
    for args in [&["--api-key", FLAG_KEY, "q"][..], &["--backend", "jev", "--api-key", FLAG_KEY, "q"][..]] {
        let scoped = ran(common::command(&dir).args(args).output().unwrap());
        assert_eq!((scoped.code, scoped.out.as_str(), scoped.err.as_str()), (2, "", "error: --api-key requires --backend openai\n"));
    }
    let gone = ran(common::command(&dir).args(["--backend", "openrouter", "q"]).output().unwrap());
    assert!(gone.code == 2 && gone.err.contains("openai"), "{}", gone.err);
    let help = ran(common::command(&dir).arg("--help").output().unwrap());
    assert_eq!(help.code, 0);
    for text in [
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "--api-key <KEY>",
        "--no-schema",
        "--extra-body <JSON>",
        "Responses API",
        "fnox is never consulted",
    ] {
        assert!(help.out.contains(text), "help is missing {text}");
    }
}
