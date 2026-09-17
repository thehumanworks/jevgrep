//! A fake Jev, as a function and as a local HTTP server for end-to-end runs of the binary.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;

use serde_json::{json, Map, Value};

/// The numbered listing in `state.code`, as line number -> text.
pub fn listing(body: &Value) -> BTreeMap<usize, String> {
    let code = body["state"]["code"].as_str().unwrap_or("");
    code.lines().filter_map(|row| row.split_once("| ")).map(|(n, text)| (n.trim().parse().unwrap(), text.to_owned())).collect()
}

/// `q0.B3-9` -> Some((3, 9))
pub fn block_range(qid: &str) -> Option<(usize, usize)> {
    let (a, b) = qid.split_once(".B")?.1.split_once('-')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// Fake Jev: a line is a hit when its text contains one of `hot`; a block when any of its lines does.
pub fn answer_all(body: &Value, hot: &[&str]) -> String {
    let code = listing(body);
    let is_hot = |n: usize| code.get(&n).is_some_and(|text| hot.iter().any(|h| text.contains(h)));
    let mut answers = Map::new();
    for (qid, q) in body["questions"].as_object().unwrap() {
        let answer = if q["type"] == "score" {
            json!({"type": "score", "score": if code.keys().any(|&n| is_hot(n)) { 3.0 } else { 0.0 }, "confidence": 0.9, "probabilities": {}, "legend": {}})
        } else if let Some((lo, hi)) = block_range(qid) {
            json!({"type": "noul", "noul": if (lo..=hi).any(is_hot) { 0.9 } else { 0.03 }})
        } else {
            let n: usize = qid.split_once(".L").unwrap().1.parse().unwrap();
            json!({"type": "noul", "noul": if is_hot(n) { 0.95 } else { 0.02 }})
        };
        answers.insert(qid.clone(), answer);
    }
    json!({"model": "jev-test", "answers": answers, "usage": {"input_tokens": 100, "output_tokens": 10}}).to_string()
}

pub struct Request {
    pub authorization: String,
    pub body: Value,
}

/// Serves `handler` over HTTP/1.1 with keep-alive on a random local port. Returns the base URL.
pub fn serve(handler: impl Fn(&Request) -> (u16, String) + Send + Sync + 'static) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1/systemone", listener.local_addr().unwrap());
    let handler = Arc::new(handler);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let handler = handler.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut stream = stream;
                loop {
                    let (mut length, mut authorization, mut line) = (0usize, String::new(), String::new());
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return; // client closed the connection
                    }
                    loop {
                        line.clear();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                            break;
                        }
                        if let Some((name, value)) = line.split_once(':') {
                            match name.to_ascii_lowercase().as_str() {
                                "content-length" => length = value.trim().parse().unwrap(),
                                "authorization" => authorization = value.trim().to_owned(),
                                _ => {}
                            }
                        }
                    }
                    let mut raw = vec![0; length];
                    if reader.read_exact(&mut raw).is_err() {
                        return;
                    }
                    let request = Request { authorization, body: serde_json::from_slice(&raw).unwrap_or(Value::Null) };
                    let (status, text) = handler(&request);
                    let head = format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n", text.len());
                    if stream.write_all(head.as_bytes()).and_then(|()| stream.write_all(text.as_bytes())).is_err() {
                        return;
                    }
                }
            });
        }
    });
    url
}

/// A fresh scratch directory holding `files`.
pub fn repo(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("jg-e2e-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (rel, text) in files {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

pub struct Ran {
    pub code: i32,
    pub out: String,
    pub err: String,
}

/// Runs the real `jg` binary in `dir` against `base_url` with a test key.
pub fn jg(dir: &Path, base_url: &str, args: &[&str]) -> Ran {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jg"));
    cmd.current_dir(dir).args(args).env("TYPESAFE_API_KEY", "test-key").env("JG_BASE_URL", base_url).env_remove("JG_MODEL");
    ran(cmd.output().unwrap())
}

pub fn ran(output: Output) -> Ran {
    Ran {
        code: output.status.code().unwrap_or(-1),
        out: String::from_utf8_lossy(&output.stdout).into_owned(),
        err: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}
