//! The client against a scripted transport: request shape, answer decoding, retries, errors.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Map, Value};
use typesafe_jev::{Client, Config, Error, Reply, Transport};

fn ok(body: impl Into<String>) -> Result<Reply, String> {
    Ok(Reply { status: 200, retry_after: None, body: body.into() })
}

fn status(status: u16, body: &str) -> Result<Reply, String> {
    Ok(Reply { status, retry_after: None, body: body.to_owned() })
}

fn instant(cfg: Config) -> Config {
    Config { backoff_scale: 0.0, throttle_pause: Duration::ZERO, ..cfg }
}

fn client(cfg: Config, handler: impl Fn(&Value) -> Result<Reply, String> + Send + Sync + 'static) -> Client {
    let transport = move |raw: &[u8]| handler(&serde_json::from_slice(raw).unwrap());
    Client::with_transport(transport, instant(cfg))
}

fn questions() -> Map<String, Value> {
    let mut qs = Map::new();
    qs.insert("hit".into(), json!({"type": "noul", "instructions": "Does the code read a file?"}));
    qs.insert("rel".into(), json!({"type": "score", "instructions": "How relevant?", "criteria": ["no", "somewhat", "yes"]}));
    qs
}

fn answers() -> String {
    json!({
        "model": "jev-test",
        "answers": {
            "hit": {"type": "noul", "noul": 0.95},
            "rel": {"type": "score", "score": 1.5, "confidence": 0.8, "probabilities": {}, "legend": {}},
        },
        "usage": {"input_tokens": 120, "output_tokens": 7},
    })
    .to_string()
}

#[test]
fn sends_model_state_and_questions_and_returns_the_answers() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let c = client(Config { model: "jev-2".into(), ..Config::default() }, move |body| {
        log.lock().unwrap().push(body.clone());
        ok(answers())
    });
    let got = c.ask(&json!({"code": "open('x')"}), &questions()).unwrap();
    assert_eq!(got["hit"]["noul"], 0.95);
    assert_eq!(got["rel"]["score"], 1.5);
    let sent = seen.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["model"], "jev-2");
    assert_eq!(sent[0]["state"], json!({"code": "open('x')"}));
    assert_eq!(sent[0]["questions"], Value::Object(questions()));
    assert_eq!(sent[0].as_object().unwrap().len(), 3, "nothing but model, state and questions is sent");
    assert_eq!(c.model(), "jev-2");
    assert_eq!((c.usage().requests(), c.usage().retries(), c.usage().input_tokens(), c.usage().output_tokens()), (1, 0, 120, 7));
}

#[test]
fn a_reply_without_answers_or_without_json_is_an_api_error() {
    let c = client(Config::default(), |_| ok("{\"model\": \"jev\"}"));
    assert_eq!(c.ask(&json!({}), &questions()), Err(Error::Api("response has no `answers` object".into())));
    let c = client(Config::default(), |_| ok("{\"answers\": []}"));
    assert!(matches!(c.ask(&json!({}), &questions()), Err(Error::Api(m)) if m.contains("no `answers` object")));
    let c = client(Config::default(), |_| ok("<html>maintenance</html>"));
    assert!(matches!(c.ask(&json!({}), &questions()), Err(Error::Api(m)) if m.starts_with("unreadable response")));
    assert_eq!(c.usage().requests(), 0, "a failed request is not counted as answered");
}

#[test]
fn retries_transient_statuses_and_connection_failures_then_succeeds() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&calls);
    let c = client(Config::default(), move |_| match seen.fetch_add(1, Ordering::SeqCst) {
        0 => Err("connection reset".into()),
        1 => status(503, "try again shortly"),
        2 => Ok(Reply { status: 429, retry_after: Some("0".into()), body: "slow down".into() }),
        _ => ok(answers()),
    });
    assert_eq!(c.ask(&json!({}), &questions()).unwrap()["hit"]["noul"], 0.95);
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    assert_eq!((c.usage().requests(), c.usage().retries()), (1, 3));
}

#[test]
fn every_retryable_status_is_retried_and_others_fail_at_once() {
    for code in [408u16, 409, 425, 429, 500, 502, 503, 504, 529] {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&calls);
        let c = client(Config { max_retries: 1, ..Config::default() }, move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            status(code, "later")
        });
        let err = c.ask(&json!({}), &questions()).unwrap_err();
        assert_eq!(calls.load(Ordering::SeqCst), 2, "HTTP {code}");
        assert!(
            matches!(&err, Error::Api(m) if m.contains("gave up after 1 retries") && m.contains(&format!("HTTP {code}: later"))),
            "{err:?}"
        );
    }
    for code in [400u16, 404, 418, 422, 451] {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&calls);
        let c = client(Config::default(), move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            status(code, "nope")
        });
        assert_eq!(c.ask(&json!({}), &questions()), Err(Error::Api(format!("HTTP {code}: nope"))));
        assert_eq!(calls.load(Ordering::SeqCst), 1, "HTTP {code}");
        assert_eq!(c.usage().retries(), 0);
    }
}

#[test]
fn rejected_credentials_are_not_retried_and_name_the_provider() {
    for code in [401u16, 403] {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&calls);
        let c = client(Config { provider: "Gateway".into(), ..Config::default() }, move |_| {
            seen.fetch_add(1, Ordering::SeqCst);
            status(code, r#"{"detail": "bad key"}"#)
        });
        let err = c.ask(&json!({}), &questions()).unwrap_err();
        assert!(
            matches!(&err, Error::Auth(m) if m.starts_with("Gateway rejected the API key") && m.contains(&format!("HTTP {code}")) && m.contains("bad key")),
            "{err:?}"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn context_overflow_is_a_token_limit_error() {
    let c = client(Config::default(), |_| status(400, r#"{"detail": {"error_type": "max_tokens_exceeded"}}"#));
    assert!(matches!(c.ask(&json!({}), &questions()), Err(Error::TokenLimit(m)) if m.contains("max_tokens_exceeded")));
    let c = client(Config::default(), |_| status(413, "too large"));
    assert_eq!(c.ask(&json!({}), &questions()), Err(Error::TokenLimit("too large".into())));
    assert_eq!(c.usage().retries(), 0);
}

#[test]
fn error_bodies_are_truncated_to_400_characters() {
    let long = "x".repeat(1000);
    let c = client(Config::default(), move |_| status(422, &long));
    let err = c.ask(&json!({}), &questions()).unwrap_err();
    assert_eq!(err.message().len(), "HTTP 422: ".len() + 400);
    assert_eq!(err.to_string(), err.message());
}

#[test]
fn gives_up_after_max_retries_with_the_last_reason() {
    let c = client(Config { max_retries: 2, ..Config::default() }, |_| status(529, "overloaded"));
    let err = c.ask(&json!({}), &questions()).unwrap_err();
    assert!(matches!(&err, Error::Api(m) if m == "gave up after 2 retries: HTTP 529: overloaded"), "{err:?}");
    assert_eq!(c.usage().retries(), 2);
    let c = client(Config { max_retries: 0, ..Config::default() }, |_| Err("connection reset".into()));
    assert_eq!(c.ask(&json!({}), &questions()), Err(Error::Api("gave up after 0 retries: connection reset".into())));
}

#[test]
fn the_debug_reporter_hears_every_failed_attempt() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&calls);
    let mut c = client(Config { pool_size: 16, ..Config::default() }, move |_| match seen.fetch_add(1, Ordering::SeqCst) {
        0 => Ok(Reply { status: 429, retry_after: Some("0".into()), body: "rate limit".into() }),
        1 => Err("connection reset".into()),
        _ => ok(answers()),
    });
    let messages = Arc::new(Mutex::new(Vec::new()));
    let reported = Arc::clone(&messages);
    c.set_debug_reporter(move |message| reported.lock().unwrap().push(message.to_owned()));
    c.ask(&json!({}), &questions()).unwrap();
    let messages = messages.lock().unwrap();
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert!(
        messages[0].starts_with("attempt 1: HTTP 429: rate limit") && messages[0].contains("retry-after=Some(\"0\")"),
        "{}",
        messages[0]
    );
    assert_eq!(messages[1], "attempt 2: connection reset");
    assert!(c.limiter().limit() < 16.0, "a throttled reply narrows the gate");
    assert_eq!(c.limiter().in_flight(), 0);
}

#[test]
fn concurrent_asks_share_the_pool_and_the_usage() {
    let c = Arc::new(client(Config { pool_size: 4, ..Config::default() }, |_| {
        std::thread::sleep(Duration::from_millis(5));
        ok(answers())
    }));
    let workers: Vec<_> = (0..12)
        .map(|_| {
            let c = Arc::clone(&c);
            std::thread::spawn(move || c.ask(&json!({}), &questions()).unwrap()["hit"]["noul"].as_f64().unwrap())
        })
        .collect();
    for worker in workers {
        assert_eq!(worker.join().unwrap(), 0.95);
    }
    assert_eq!((c.usage().requests(), c.usage().input_tokens(), c.limiter().in_flight()), (12, 12 * 120, 0));
    assert_eq!(c.limiter().limit(), 4.0);
}

#[test]
fn a_boxed_transport_is_a_transport() {
    let boxed: Box<dyn Transport> = Box::new(|_: &[u8]| ok(answers()));
    let c = Client::with_transport(boxed, Config::default());
    assert_eq!(c.ask(&json!({}), &questions()).unwrap()["hit"]["noul"], 0.95);
}

#[test]
fn errors_display_their_message_and_compare_by_value() {
    let err = Error::TokenLimit("too big".into());
    assert_eq!(err.to_string(), "too big");
    assert_eq!(err.clone(), err);
    assert_ne!(err, Error::Api("too big".into()));
    let boxed: Box<dyn std::error::Error> = Box::new(err);
    assert_eq!(boxed.to_string(), "too big");
}
