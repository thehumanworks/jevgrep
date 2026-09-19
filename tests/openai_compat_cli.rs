//! The OpenAI-compatible backends through the real `jg` binary against a local chat-completions
//! server: the `openrouter` preset, and `openai` pointed at "some other service".
//!
//! Keys are fake ASCII fixtures. No test reaches a real service, and none may launch `fnox`.

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use common::{listing, ran, repo, serve, Ran, Request};
use jevgrep::openai_compat::{OPENAI, OPENROUTER};

const ENV_KEY: &str = "sk-or-env-fixture";
const FLAG_KEY: &str = "sk-or-flag-fixture";
const OTHER_KEY: &str = "gsk-other-service-fixture";

fn fixture_dir() -> PathBuf {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    repo(&format!("openrouter-cli-{}", SEQ.fetch_add(1, Ordering::Relaxed)), &[("t.py", "a = 1\nneedle = 2\nb = 3\n")])
}

/// The Jev-shaped request inside the user message.
fn asked(body: &Value) -> Value {
    serde_json::from_str(body["messages"][1]["content"].as_str().expect("user message")).expect("state and questions")
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

fn completion(content: &str) -> String {
    json!({
        "provider": "Fixture",
        "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": content}}],
        "usage": {"prompt_tokens": 1200, "completion_tokens": 300, "cost": 0.0125},
    })
    .to_string()
}

/// A plain OpenAI-compatible completion: no `cost`, no aggregator fields.
fn plain_completion(content: &str) -> String {
    json!({
        "choices": [{"index": 0, "finish_reason": "stop", "message": {"role": "assistant", "content": content}}],
        "usage": {"prompt_tokens": 800, "completion_tokens": 40, "total_tokens": 840},
    })
    .to_string()
}

/// (authorization, body) of every request, in order.
type Seen = Arc<Mutex<Vec<(String, Value)>>>;

/// A server that records every request and answers it with `respond`.
fn recording(respond: impl Fn(&Request, usize) -> (u16, String) + Send + Sync + 'static) -> (String, Seen) {
    let seen: Seen = Arc::default();
    let sink = seen.clone();
    let url = serve(move |request| {
        let mut seen = sink.lock().unwrap();
        seen.push((request.authorization.clone(), request.body.clone()));
        respond(request, seen.len())
    });
    (url, seen)
}

fn needle_server() -> (String, Seen) {
    recording(|request, _| (200, completion(&answers_text(&request.body))))
}

/// `jg --backend openrouter` with the key in the environment, as `fnox run -- jg ...` provides it.
fn run(dir: &Path, url: &str, args: &[&str]) -> Ran {
    let mut cmd = common::command(dir);
    cmd.args(["--backend", "openrouter", "--base-url", url]).args(args).env("OPENROUTER_API_KEY", ENV_KEY);
    ran(cmd.output().unwrap())
}

fn assert_no_keys(run: &Ran) {
    for key in [ENV_KEY, FLAG_KEY, OTHER_KEY] {
        assert!(!run.out.contains(key) && !run.err.contains(key), "output leaked a key: {}{}", run.out, run.err);
    }
}

#[test]
fn search_matches_the_jev_output_shapes_and_sends_a_plain_chat_completion() {
    let (url, seen) = needle_server();
    let dir = fixture_dir();
    let text = run(&dir, &url, &["-q", "find needle"]);
    assert_eq!(
        (text.code, text.out.as_str(), text.err.as_str()),
        (0, "t.py  relevance=1.00\n        1-3  0.90  a = 1\n          2  0.95  needle = 2\n\n", "")
    );
    let flat = run(&dir, &url, &["-q", "--no-heading", "find needle"]);
    assert_eq!((flat.code, flat.out.as_str()), (0, "t.py:1-3:0.90:a = 1\nt.py:2:0.95:needle = 2\n"));
    let parsed: Value = serde_json::from_str(run(&dir, &url, &["-q", "--json", "find needle"]).out.trim()).unwrap();
    assert_eq!(
        (&parsed["path"], &parsed["relevance"], &parsed["regions"][0]["lines"][0]["line"]),
        (&json!("t.py"), &json!(1.0), &json!(2))
    );

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 3);
    for (authorization, body) in seen.iter() {
        assert_eq!(authorization, &format!("Bearer {ENV_KEY}"));
        assert_eq!(body["model"], "inclusionai/ling-3.0-flash-fin:free");
        assert_eq!(body["messages"][0]["role"], "system");
        // The default model's provider rejects a schema outright, and nothing streams.
        assert!(body.get("response_format").is_none() && body.get("stream").is_none(), "{body}");
        let asked = asked(body);
        assert_eq!(asked["state"]["query"], "find needle");
        assert!(asked["questions"].as_object().unwrap().keys().all(|id| id.parse::<usize>().is_ok()));
    }
}

#[test]
fn the_api_key_flag_wins_over_the_environment_and_is_never_printed() {
    let (url, seen) = needle_server();
    let both = run(&fixture_dir(), &url, &["--api-key", FLAG_KEY, "find needle"]);
    assert_eq!(both.code, 0, "{}", both.err);
    assert_no_keys(&both);
    // With the flag alone, as on a machine where nothing exports the variable.
    let mut cmd = common::command(&fixture_dir());
    cmd.args(["--backend=openrouter", "--base-url", &url, &format!("--api-key={FLAG_KEY}"), "-q", "find needle"]);
    let flag_only = ran(cmd.output().unwrap());
    assert_eq!((flag_only.code, flag_only.err.as_str()), (0, ""));
    let seen = seen.lock().unwrap();
    assert!(seen.len() == 2 && seen.iter().all(|(authorization, _)| authorization == &format!("Bearer {FLAG_KEY}")));
}

#[cfg(unix)]
#[test]
fn a_missing_key_is_actionable_and_never_launches_fnox() {
    use std::os::unix::fs::PermissionsExt;
    let (url, seen) = needle_server();
    let dir = fixture_dir();
    // A `fnox` that would hand over a key, and leave a mark, if jg ever asked it. The mark is made
    // with a shell builtin: PATH holds nothing but this script.
    let bin = dir.join("fake-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let marker = dir.join("fnox-was-run");
    std::fs::write(bin.join("fnox"), format!("#!/bin/sh\n: > '{}'\necho sk-or-from-fnox\n", marker.display())).unwrap();
    std::fs::set_permissions(bin.join("fnox"), std::fs::Permissions::from_mode(0o755)).unwrap();

    for key in [None, Some(""), Some("  ")] {
        let mut cmd = common::command(&dir);
        cmd.args(["--backend", "openrouter", "--base-url", &url, "find needle"]).env("PATH", &bin).env_remove("JG_NO_FNOX");
        if let Some(key) = key {
            cmd.env("OPENROUTER_API_KEY", key);
        }
        let missing = ran(cmd.output().unwrap());
        assert_eq!((missing.code, missing.out.as_str()), (2, ""));
        assert_eq!(missing.err, "jg: OPENROUTER_API_KEY is not set. Export it, or pass --api-key-env <VAR> or --api-key <KEY>.\n");
    }
    // Other services' keys are different credentials and must not be borrowed.
    let mut cmd = common::command(&dir);
    cmd.args(["--backend", "openrouter", "--base-url", &url, "find needle"])
        .env("TYPESAFE_API_KEY", "jev-key")
        .env("OPENAI_API_KEY", "sk-openai");
    assert_eq!(ran(cmd.output().unwrap()).code, 2);
    assert!(!marker.exists(), "jg launched fnox for an OpenRouter key");
    assert!(seen.lock().unwrap().is_empty(), "no request may be sent without a key");
}

#[test]
fn model_and_backend_come_from_flags_or_environment() {
    let (url, seen) = needle_server();
    let dir = fixture_dir();
    assert_eq!(run(&dir, &url, &["-q", "--model", "vendor/chosen", "find needle"]).code, 0);
    let mut cmd = common::command(&dir);
    cmd.args(["-q", "find needle"])
        .env("JG_BACKEND", "openrouter")
        .env("JG_MODEL", "vendor/from-env")
        .env("JG_BASE_URL", &url)
        .env("OPENROUTER_API_KEY", ENV_KEY);
    assert_eq!(ran(cmd.output().unwrap()).code, 0);
    let models: Vec<Value> = seen.lock().unwrap().iter().map(|(_, body)| body["model"].clone()).collect();
    assert_eq!(models, [json!("vendor/chosen"), json!("vendor/from-env")]);
}

#[test]
fn stats_report_tokens_model_and_the_cost_openrouter_states() {
    let (url, _) = needle_server();
    let done = run(&fixture_dir(), &url, &["find needle"]);
    assert_eq!(done.code, 0);
    let stats = done.err.lines().last().unwrap();
    assert!(
        stats.starts_with("jg: 1 files, 1 requests, 1,200 input / 300 output tokens (OpenRouter inclusionai/ling-3.0-flash-fin:free; reported cost $0.0125), "),
        "{stats}"
    );
    assert!(!done.err.contains("ChatGPT") && !done.err.contains("(~$"), "{}", done.err);
}

#[test]
fn an_unusable_reply_is_resampled_warmer_and_a_persistent_one_is_an_error() {
    let (url, seen) = recording(|request, count| {
        let content =
            if count == 1 { "Line 2 looks relevant to me.".to_owned() } else { format!("```json\n{}\n```", answers_text(&request.body)) };
        (200, completion(&content))
    });
    let recovered = run(&fixture_dir(), &url, &["find needle"]);
    assert_eq!(recovered.code, 0, "{}", recovered.err);
    assert!(recovered.out.contains("needle = 2") && recovered.err.contains("2 requests, 1 retries"), "{}", recovered.err);
    let temperatures: Vec<f64> = seen.lock().unwrap().iter().map(|(_, body)| body["temperature"].as_f64().unwrap()).collect();
    assert_eq!(temperatures, [0.0, 0.7]);

    let (url, seen) = recording(|_, _| (200, completion("{\"answers\": {\"0\": 50}}")));
    let failed = run(&fixture_dir(), &url, &["find needle"]);
    assert_eq!((failed.code, failed.out.as_str()), (2, ""));
    assert!(failed.err.contains("OpenRouter API error: model gave an unusable reply 3 times"), "{}", failed.err);
    assert_eq!(seen.lock().unwrap().len(), 3);
}

#[test]
fn a_rejected_key_is_actionable_and_echoes_no_key() {
    let (url, seen) = recording(|_, _| (401, json!({"error": {"code": 401, "message": "User not found."}}).to_string()));
    let rejected = run(&fixture_dir(), &url, &["--api-key", FLAG_KEY, "find needle"]);
    assert_eq!((rejected.code, rejected.out.as_str()), (2, ""));
    assert_eq!(
        rejected.err,
        "jg: OpenRouter rejected the API key (HTTP 401: User not found.). Check OPENROUTER_API_KEY, or pass --api-key-env <VAR> or --api-key <KEY>.\n"
    );
    assert_eq!(seen.lock().unwrap().len(), 1, "an authentication failure is not retried");

    // A provider that echoes the request back must not get the key onto the terminal.
    let (url, _) = recording(|request, _| {
        let raw = format!("bad request\nheaders: {}", request.authorization);
        (
            400,
            json!({"error": {"code": 400, "message": "Provider returned error", "metadata": {"raw": raw, "provider_name": "Fixture"}}})
                .to_string(),
        )
    });
    let echoed = run(&fixture_dir(), &url, &["find needle"]);
    assert_eq!(echoed.code, 2);
    assert!(echoed.err.contains("HTTP 400: Provider returned error (Fixture: bad request headers: Bearer <redacted>)"), "{}", echoed.err);
    assert_no_keys(&echoed);
}

#[test]
fn credentials_are_not_sent_over_plaintext_to_another_host() {
    let refused = run(&fixture_dir(), "http://openrouter.example/api/v1/chat/completions", &["find needle"]);
    assert_eq!((refused.code, refused.out.as_str()), (2, ""));
    assert!(refused.err.contains("refusing to send OpenRouter credentials"), "{}", refused.err);
    assert_no_keys(&refused);
}

/// `jg --backend openai` pointed at the local server as "some other service", with no key in sight.
fn other_service(dir: &Path, url: &str) -> std::process::Command {
    let mut cmd = common::command(dir);
    cmd.args(["--backend", "openai", "--base-url", url, "--model", "vendor/some-model"]);
    cmd
}

#[test]
fn any_compatible_service_works_from_its_api_root_with_only_the_common_request() {
    let paths = Arc::new(Mutex::new(Vec::new()));
    let sink = paths.clone();
    let (url, seen) = recording(move |request, _| {
        sink.lock().unwrap().push(request.path.clone());
        (200, plain_completion(&answers_text(&request.body)))
    });
    // What a service documents for OpenAI SDKs is its root; the endpoint itself works as well.
    let root = url.replace("/v1/systemone", "/openai/v1");
    let dir = fixture_dir();
    let from_root = ran(other_service(&dir, &root)
        .args(["--api-key-env", "OTHER_API_KEY", "find needle"])
        .env("OTHER_API_KEY", OTHER_KEY)
        .output()
        .unwrap());
    assert_eq!(from_root.code, 0, "{}", from_root.err);
    assert!(from_root.out.contains("needle = 2"), "{}", from_root.out);
    assert_no_keys(&from_root);
    // No price list is known and none was reported, so none is shown; the service is named by its host.
    let stats = from_root.err.lines().last().unwrap();
    assert!(stats.starts_with("jg: 1 files, 1 requests, 800 input / 40 output tokens (127.0.0.1 vendor/some-model), "), "{stats}");
    let full = format!("{root}/chat/completions/");
    assert_eq!(ran(other_service(&dir, &full).args(["-q", "find needle"]).env("OPENAI_API_KEY", OTHER_KEY).output().unwrap()).code, 0);

    assert_eq!(*paths.lock().unwrap(), ["/openai/v1/chat/completions", "/openai/v1/chat/completions"]);
    for (authorization, body) in seen.lock().unwrap().iter() {
        assert_eq!(authorization, &format!("Bearer {OTHER_KEY}"));
        // Nothing of OpenRouter's, and nothing else a strict API would refuse.
        let fields: Vec<&str> = body.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(fields, ["model", "messages", "temperature"], "{body}");
        assert_eq!(body["model"], "vendor/some-model");
    }
}

#[test]
fn a_local_server_needs_no_key_but_a_hosted_default_does() {
    let (url, seen) = recording(|request, _| (200, plain_completion(&answers_text(&request.body))));
    let dir = fixture_dir();
    let keyless = ran(other_service(&dir, &url).args(["-q", "find needle"]).output().unwrap());
    assert_eq!((keyless.code, keyless.err.as_str()), (0, ""));
    assert_eq!(seen.lock().unwrap()[0].0, "", "no Authorization header without a key");
    // The root may also come from the variable OpenAI's SDKs use.
    let mut cmd = common::command(&dir);
    cmd.args(["--backend", "openai", "--model", "m", "-q", "find needle"]).env("OPENAI_BASE_URL", &url);
    assert_eq!(ran(cmd.output().unwrap()).code, 0);
    assert_eq!(seen.lock().unwrap().len(), 2);

    // api.openai.com is known to want one, so nothing is sent without it.
    let hosted = ran(common::command(&dir).args(["--backend", "openai", "--model", "m", "find needle"]).output().unwrap());
    assert_eq!((hosted.code, hosted.out.as_str()), (2, ""));
    assert_eq!(hosted.err, "jg: OPENAI_API_KEY is not set. Export it, or pass --api-key-env <VAR> or --api-key <KEY>.\n");
    let unset = ran(other_service(&dir, &url).args(["--api-key-env", "OTHER_API_KEY", "find needle"]).output().unwrap());
    assert_eq!((unset.code, unset.err.as_str()), (2, "jg: OTHER_API_KEY, named by --api-key-env, is not set\n"));
    let no_model = ran(common::command(&dir).args(["--backend", "openai", "--base-url", &url, "find needle"]).output().unwrap());
    assert_eq!((no_model.code, no_model.err.as_str()), (2, "error: --backend openai has no default model; pass --model or set JG_MODEL\n"));
    assert_eq!(seen.lock().unwrap().len(), 2, "none of the refusals reached the server");

    // A keyless server that turns out to want a key says so, under its own name.
    let (url, _) = recording(|_, _| (401, json!({"error": {"message": "Invalid API key", "type": "invalid_request_error"}}).to_string()));
    let wanted = ran(other_service(&dir, &url).arg("find needle").output().unwrap());
    assert_eq!(wanted.code, 2);
    assert_eq!(
        wanted.err,
        "jg: 127.0.0.1 wants an API key (HTTP 401: Invalid API key) and none was sent. Pass --api-key-env <VAR> or --api-key <KEY>.\n"
    );
}

#[test]
fn schema_and_extra_body_reach_the_wire_and_the_environment_can_supply_the_extras() {
    let (url, seen) = recording(|request, _| (200, plain_completion(&answers_text(&request.body))));
    let dir = fixture_dir();
    let extra = r#"{"reasoning_effort":"low","temperature":null,"provider":{"order":["Fixture"]}}"#;
    assert_eq!(
        ran(other_service(&dir, &url).args(["-q", "--json-schema", "--extra-body", extra, "find needle"]).output().unwrap()).code,
        0
    );
    assert_eq!(ran(other_service(&dir, &url).args(["-q", "find needle"]).env("JG_EXTRA_BODY", r#"{"seed":7}"#).output().unwrap()).code, 0);
    let seen = seen.lock().unwrap();
    let body = &seen[0].1;
    assert_eq!((&body["reasoning_effort"], &body["provider"]["order"][0]), (&json!("low"), &json!("Fixture")));
    assert!(body.get("temperature").is_none(), "{body}");
    let format = &body["response_format"];
    assert_eq!((&format["type"], &format["json_schema"]["strict"]), (&json!("json_schema"), &json!(true)));
    let wanted = format["json_schema"]["schema"]["properties"]["answers"]["required"].as_array().unwrap().len();
    assert_eq!(wanted, asked(body)["questions"].as_object().unwrap().len());
    assert_eq!((&seen[1].1["seed"], seen[1].1.get("response_format")), (&json!(7), None));

    let reserved = ran(other_service(&dir, &url).args(["--extra-body", r#"{"stream":true}"#, "find needle"]).output().unwrap());
    assert_eq!((reserved.code, reserved.err.as_str()), (2, "error: --extra-body cannot set `stream`; jg owns that request field\n"));
}

#[test]
fn a_model_with_fixed_sampling_costs_one_refusal_not_the_search() {
    let (url, seen) = recording(|request, _| match request.body.get("temperature") {
        Some(_) => {
            let message = "Unsupported value: 'temperature' does not support 0 with this model. Only the default (1) value is supported.";
            (400, json!({"error": {"message": message, "type": "invalid_request_error", "param": "temperature", "code": "unsupported_value"}}).to_string())
        }
        None => (200, plain_completion(&answers_text(&request.body))),
    });
    let done = ran(other_service(&fixture_dir(), &url).args(["-j", "1", "find needle"]).output().unwrap());
    assert_eq!(done.code, 0, "{}", done.err);
    assert!(done.out.contains("needle = 2") && done.err.contains("1 requests, 1 retries"), "{}", done.err);
    let sent: Vec<bool> = seen.lock().unwrap().iter().map(|(_, body)| body.get("temperature").is_some()).collect();
    assert_eq!(sent, [true, false]);
}

#[test]
fn flags_are_scoped_to_their_backends_and_help_documents_them() {
    let dir = fixture_dir();
    for args in [&["--api-key", FLAG_KEY, "q"][..], &["--backend", "jev", "--api-key", FLAG_KEY, "q"][..]] {
        let scoped = ran(common::command(&dir).args(args).output().unwrap());
        let expected = "error: --api-key requires an OpenAI-compatible backend: --backend openrouter or openai\n";
        assert_eq!((scoped.code, scoped.out.as_str(), scoped.err.as_str()), (2, "", expected));
    }
    let login = ran(common::command(&dir).args(["--backend", "openrouter", "--chatgpt-login", "q"]).output().unwrap());
    assert_eq!((login.code, login.err.as_str()), (2, "error: --chatgpt-login requires --backend chatgpt\n"));
    let help = ran(common::command(&dir).arg("--help").output().unwrap());
    assert_eq!(help.code, 0);
    let presets = [OPENROUTER.key_var, OPENROUTER.model.unwrap(), OPENAI.key_var, "OPENAI_BASE_URL"];
    let flags =
        ["openai-compatible", "--api-key <KEY>", "--api-key-env <VAR>", "--json-schema", "--extra-body <JSON>", "fnox is never consulted"];
    for text in presets.iter().chain(&flags) {
        assert!(help.out.contains(text), "help is missing {text}");
    }
}
