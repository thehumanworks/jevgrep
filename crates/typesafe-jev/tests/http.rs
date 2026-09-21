//! The real HTTPS transport against a local HTTP/1.1 listener: headers, body, status and
//! `Retry-After` all reach the client as sent.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Map, Value};
use typesafe_jev::{Client, Config, Error};

#[derive(Debug, Clone)]
struct Request {
    path: String,
    headers: Vec<(String, String)>,
    body: Value,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// Serves `handler` on a random loopback port. Returns the base URL and the requests seen.
fn serve(
    handler: impl Fn(&Request) -> (u16, Vec<(&'static str, String)>, String) + Send + Sync + 'static,
) -> (String, Arc<Mutex<Vec<Request>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1/systemone", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let handler = Arc::new(handler);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (handler, log) = (Arc::clone(&handler), Arc::clone(&log));
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let path = line.split_whitespace().nth(1).unwrap_or("").to_owned();
                    let mut headers = Vec::new();
                    let mut length = 0;
                    loop {
                        let mut line = String::new();
                        reader.read_line(&mut line).unwrap();
                        let line = line.trim_end();
                        if line.is_empty() {
                            break;
                        }
                        let (k, v) = line.split_once(':').unwrap();
                        if k.eq_ignore_ascii_case("content-length") {
                            length = v.trim().parse().unwrap();
                        }
                        headers.push((k.to_owned(), v.trim().to_owned()));
                    }
                    let mut raw = vec![0; length];
                    reader.read_exact(&mut raw).unwrap();
                    let request = Request { path, headers, body: serde_json::from_slice(&raw).unwrap_or(Value::Null) };
                    let (status, extra, body) = handler(&request);
                    log.lock().unwrap().push(request);
                    let mut response =
                        format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n", body.len());
                    for (k, v) in extra {
                        response.push_str(&format!("{k}: {v}\r\n"));
                    }
                    response.push_str("\r\n");
                    response.push_str(&body);
                    let mut stream = reader.get_ref();
                    if stream.write_all(response.as_bytes()).is_err() {
                        return;
                    }
                }
            });
        }
    });
    (url, seen)
}

fn questions() -> Map<String, Value> {
    let mut qs = Map::new();
    qs.insert("q".into(), json!({"type": "noul", "instructions": "Is it?"}));
    qs
}

#[test]
fn posts_json_with_a_bearer_token_and_the_configured_user_agent() {
    let (url, seen) =
        serve(|_| (200, vec![], json!({"answers": {"q": {"type": "noul", "noul": 0.7}}, "usage": {"input_tokens": 9}}).to_string()));
    let c = Client::new("sk-test", Config { base_url: url, user_agent: "probe/1.0".into(), ..Config::default() }).unwrap();
    let answers = c.ask(&json!({"n": 1}), &questions()).unwrap();
    assert_eq!(answers["q"]["noul"], 0.7);
    assert_eq!((c.usage().requests(), c.usage().input_tokens(), c.usage().output_tokens()), (1, 9, 0));
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    let request = &seen[0];
    assert_eq!(request.path, "/v1/systemone");
    assert_eq!(request.header("authorization"), Some("Bearer sk-test"));
    assert_eq!(request.header("content-type"), Some("application/json"));
    assert_eq!(request.header("user-agent"), Some("probe/1.0"));
    assert_eq!(
        request.body,
        json!({"model": "jev-latest", "state": {"n": 1}, "questions": {"q": {"type": "noul", "instructions": "Is it?"}}})
    );
}

#[test]
fn the_default_user_agent_names_this_crate() {
    let (url, seen) = serve(|_| (200, vec![], json!({"answers": {}}).to_string()));
    let c = Client::new("sk-test", Config { base_url: url, ..Config::default() }).unwrap();
    assert!(c.ask(&json!({}), &Map::new()).unwrap().is_empty());
    let seen = seen.lock().unwrap();
    let agent = seen[0].header("user-agent").unwrap();
    assert_eq!(agent, format!("typesafe-jev/{}", env!("CARGO_PKG_VERSION")));
}

#[test]
fn status_and_retry_after_reach_the_client() {
    let (url, seen) = serve(|request| {
        if request.header("authorization") != Some("Bearer good") {
            return (401, vec![], r#"{"detail": "bad key"}"#.into());
        }
        (429, vec![("Retry-After", "0".into())], r#"{"detail": "slow down"}"#.into())
    });
    let cfg = Config { base_url: url, max_retries: 1, backoff_scale: 0.0, throttle_pause: Duration::ZERO, ..Config::default() };
    let bad = Client::new("wrong", cfg.clone()).unwrap();
    assert!(matches!(bad.ask(&json!({}), &questions()), Err(Error::Auth(m)) if m.contains("HTTP 401") && m.contains("bad key")));
    let mut good = Client::new("good", cfg).unwrap();
    let messages = Arc::new(Mutex::new(Vec::new()));
    let reported = Arc::clone(&messages);
    good.set_debug_reporter(move |message| reported.lock().unwrap().push(message.to_owned()));
    let err = good.ask(&json!({}), &questions()).unwrap_err();
    assert!(matches!(&err, Error::Api(m) if m.contains("gave up after 1 retries") && m.contains("slow down")), "{err:?}");
    assert!(messages.lock().unwrap().iter().all(|m| m.contains("retry-after=Some(\"0\")")), "{:?}", messages.lock().unwrap());
    assert_eq!(seen.lock().unwrap().len(), 3);
    assert!(good.limiter().limit() < 64.0);
}

#[test]
fn an_unreachable_endpoint_is_retried_then_reported() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    drop(listener);
    let c = Client::new("sk", Config { base_url: url, max_retries: 1, backoff_scale: 0.0, ..Config::default() }).unwrap();
    let err = c.ask(&json!({}), &questions()).unwrap_err();
    assert!(matches!(&err, Error::Api(m) if m.starts_with("gave up after 1 retries")), "{err:?}");
    assert_eq!(c.usage().retries(), 1);
}

#[test]
fn a_key_or_a_config_that_cannot_be_sent_is_refused_before_any_request() {
    let (url, seen) = serve(|_| (200, vec![], json!({"answers": {}}).to_string()));
    let at = |base_url: &str| Config { base_url: base_url.into(), ..Config::default() };
    for key in ["", "  \n", "sk\nsecret-half", "sk-\u{7f}"] {
        let err = Client::new(key, at(&url)).unwrap_err();
        assert!(matches!(&err, Error::Auth(m) if m.starts_with("the API key") && !m.contains("secret")), "{key:?} -> {err:?}");
    }
    for base_url in ["", "not a url", "api.typesafe.ai/v1/systemone", "ftp://example.com/x", "/v1/systemone", "https://"] {
        let err = Client::new("sk", at(base_url)).unwrap_err();
        assert!(matches!(&err, Error::InvalidConfig(m) if m.contains("`Config::base_url`")), "{base_url:?} -> {err:?}");
    }
    let err = Client::new("sk", at("https://user:hunter2@exa mple.com/")).unwrap_err();
    assert!(!err.message().contains("hunter2"), "a URL's credentials stay out of the message: {err:?}");
    let err = Client::new("sk", Config { user_agent: "line\nbreak".into(), ..at(&url) }).unwrap_err();
    assert!(matches!(&err, Error::InvalidConfig(m) if m.contains("`Config::user_agent`")), "{err:?}");
    assert!(seen.lock().unwrap().is_empty(), "nothing was sent");
}

#[test]
fn whitespace_around_the_key_is_dropped_and_debug_output_omits_the_key() {
    let (url, seen) = serve(|_| (200, vec![], json!({"answers": {}}).to_string()));
    let c = Client::new(" sk-from-a-file\n", Config { base_url: url, ..Config::default() }).unwrap();
    c.ask(&json!({}), &Map::new()).unwrap();
    assert_eq!(seen.lock().unwrap()[0].header("authorization"), Some("Bearer sk-from-a-file"));
    let shown = format!("{c:?}");
    assert!(shown.starts_with("Client {") && shown.contains("jev-latest") && shown.contains("requests: 1"), "{shown}");
    assert!(!shown.contains("sk-from-a-file"), "{shown}");
}
