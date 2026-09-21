mod common;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Map, Value};

use common::answer_all;
use jevgrep::backend::DecisionError;
use jevgrep::files::{chunk_lines, Chunking};
use jevgrep::filters::parse_filter;
use jevgrep::search::{build_request, search, Options};
use typesafe_jev::{Client as JevClient, Config, Reply};

fn ok(body: String) -> Result<Reply, String> {
    Ok(Reply { status: 200, retry_after: None, body })
}

fn status(status: u16, body: &str) -> Result<Reply, String> {
    Ok(Reply { status, retry_after: None, body: body.to_owned() })
}

fn client(cfg: Config, handler: impl Fn(&Value) -> Result<Reply, String> + Send + Sync + 'static) -> JevClient {
    let transport = move |raw: &[u8]| handler(&serde_json::from_slice(raw).unwrap());
    JevClient::with_transport(transport, Config { backoff_scale: 0.0, throttle_pause: Duration::ZERO, ..cfg })
}

fn queries(qs: &[&str]) -> Vec<String> {
    qs.iter().map(|q| q.to_string()).collect()
}

fn keys(questions: &Map<String, Value>) -> HashSet<&str> {
    questions.keys().map(String::as_str).collect()
}

fn scratch(name: &str, files: &[(&str, String)]) -> Vec<std::path::PathBuf> {
    let dir = common::repo(name, &[]);
    files
        .iter()
        .map(|(rel, text)| {
            let path = dir.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            path
        })
        .collect()
}

#[test]
fn build_request_single_and_multi_query() {
    let chunk = &chunk_lines("a.py", &["import os", "", "def f():", "    return os.getcwd()"], Chunking::default())[0];
    let (state, qs) = build_request(&queries(&["where is cwd read"]), chunk, false, false, None);
    assert_eq!(state["query"], "where is cwd read");
    assert_eq!(state["code"], "1| import os\n2| \n3| def f():\n4|     return os.getcwd()");
    // State keys keep a fixed order: it is the prompt Jev reads.
    assert_eq!(state.as_object().unwrap().keys().collect::<Vec<_>>(), ["task", "query", "file", "shown_lines", "code"]);
    // Blank line 2 is never asked about.
    assert_eq!(keys(&qs), HashSet::from(["q0.rel", "q0.B1-1", "q0.B3-4", "q0.L1", "q0.L3", "q0.L4"]));
    assert_eq!(qs["q0.B3-4"]["instructions"], "Do lines 3-4 contain the code that the query is looking for?");
    assert_eq!(qs["q0.L4"], json!({"type": "noul", "instructions": "Does line 4 of the code directly answer the query?"}));
    assert!(qs["q0.rel"]["type"] == "score" && qs["q0.rel"]["criteria"].as_array().unwrap().len() == 4);
    assert_eq!(qs["q0.rel"]["instructions"], "How relevant is this section of a.py to the query: where is cwd read");
    let (_, broad) = build_request(&queries(&["q"]), chunk, false, true, None);
    assert_eq!(broad["q0.L4"]["instructions"], "Is line 4 of the code relevant to the query?");

    let (state, qs) = build_request(&queries(&["first", "second"]), chunk, false, false, None);
    assert_eq!(state["queries"], json!({"q0": "first", "q1": "second"}));
    assert!(keys(&qs).is_superset(&HashSet::from(["q0.rel", "q1.rel", "q0.B3-4", "q1.B3-4", "q0.L4", "q1.L4"])));
    assert_eq!(qs["q1.L4"]["instructions"], "Does line 4 of the code directly answer query q1 (second)?");

    let (_, qs) = build_request(&queries(&["q"]), chunk, true, false, None);
    assert_eq!(keys(&qs), HashSet::from(["q0.rel"]));
}

#[test]
fn listing_pads_line_numbers_to_a_common_width() {
    let lines: Vec<String> = (1..=12).map(|i| format!("v{i}")).collect();
    let chunk = &chunk_lines("a.py", &lines, Chunking::default())[0];
    let (state, _) = build_request(&queries(&["q"]), chunk, false, false, None);
    let code = state["code"].as_str().unwrap();
    assert!(code.starts_with(" 1| v1\n 2| v2") && code.ends_with("\n12| v12"));
    assert_eq!(state["shown_lines"], "1-12");
}

#[test]
fn filter_questions_are_positive_atomic_and_outside_state() {
    let chunk = &chunk_lines("a.py", &["import os", "", "def f():", "    return os.getcwd()"], Chunking::default())[0];
    let rules = parse_filter("source code only. no tests");
    let (state, qs) = build_request(&queries(&["q1", "q2"]), chunk, false, false, Some(&rules));
    assert!(!state.to_string().contains("filter") && !state.to_string().contains("tests")); // shared state stays clean
    assert_eq!(qs["F0.S"]["instructions"], "Does this file fall under the category: source code?");
    assert_eq!(qs["F1.B3-4"]["instructions"], "Do lines 3-4 fall under the category: tests?");
    let filters: Vec<_> = qs.iter().filter(|(k, _)| k.starts_with('F')).collect();
    assert!(filters.iter().all(|(_, q)| {
        let text = q["instructions"].as_str().unwrap().to_lowercase();
        !text.contains(" no ") && !text.contains("not") // never a negated question
    }));
    assert_eq!(filters.len(), 2 * (1 + chunk.blocks.len())); // asked once, not per query
    let (_, qs) = build_request(&queries(&["q"]), chunk, true, false, Some(&parse_filter("no tests")));
    assert_eq!(keys(&qs), HashSet::from(["F0.S", "q0.rel"])); // files-only: section level only
}

/// The Jev client is typed and `jg`'s backends share a JSON contract, so every request is read
/// into the client's types and written out again. What is written must be, byte for byte, the
/// document `jg` built: the wording and the order of the questions are behaviour.
#[test]
fn the_typed_client_sends_exactly_the_json_that_jg_built() {
    use jevgrep::backend::DecisionBackend;
    let chunk =
        &chunk_lines("src/a.py", &["import os", "", "def f():", "    return os.getcwd()", "", "print(f())"], Chunking::default())[0];
    let rules = parse_filter("source code only. no tests");
    let triage = (
        json!({"task": "pick files", "query": "where is cwd read", "paths": {"p0": "src/a.py", "p1": "docs/b.md"}}),
        json!({"p0": {"type": "noul", "instructions": "Is `p0` likely to hold it?"}, "p1": {"type": "noul", "instructions": "Is `p1` likely to hold it?"}})
            .as_object()
            .unwrap()
            .clone(),
    );
    let requests = [
        build_request(&queries(&["where is cwd read"]), chunk, false, false, None),
        build_request(&queries(&["where is cwd read", "what is printed"]), chunk, true, true, Some(&rules)),
        triage,
    ];
    let sent = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&sent);
    let transport = move |raw: &[u8]| {
        log.lock().unwrap().push(String::from_utf8(raw.to_vec()).unwrap());
        let body: Value = serde_json::from_slice(raw).unwrap();
        let answers: Map<String, Value> = body["questions"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(id, q)| match q["type"].as_str() {
                Some("score") => (id.clone(), json!({"type": "score", "score": 1.0, "confidence": 0.5, "probabilities": {}, "legend": {}})),
                _ => (id.clone(), json!({"type": "noul", "noul": 0.5})),
            })
            .collect();
        ok(json!({"model": "jev-test", "answers": answers}).to_string())
    };
    let c = JevClient::with_transport(transport, Config::default());
    for (state, questions) in &requests {
        assert!(questions.len() > 1);
        let answers = DecisionBackend::ask(&c, state, questions).unwrap();
        assert_eq!(answers.keys().collect::<Vec<_>>(), questions.keys().collect::<Vec<_>>(), "one answer per question, in order");
        assert!(answers.values().all(|a| a["type"] == "noul" || a["type"] == "score"));
    }
    let built: Vec<String> = requests
        .iter()
        .map(|(state, questions)| serde_json::to_string(&json!({"model": "jev-latest", "state": state, "questions": questions})).unwrap())
        .collect();
    assert_eq!(*sent.lock().unwrap(), built);
}

#[test]
fn request_question_ids_cover_every_askable_line_block_and_filter_term() {
    let lines = ["import os", "", "def f():", "    return os.getcwd()", "x = 1"];
    let chunk = &chunk_lines("a.py", &lines, Chunking::default())[0];
    let cases: &[(&[&str], bool, bool, Option<&str>)] = &[
        (&["q"], false, false, None),
        (&["q"], false, true, None),
        (&["q"], true, false, None),
        (&["one", "two"], false, false, Some("source code only. no tests")),
        (&["q"], true, false, Some("no tests")),
    ];
    for &(qs, files_only, broad, filter) in cases {
        let rules = filter.map(parse_filter);
        let (_, questions) = build_request(&queries(qs), chunk, files_only, broad, rules.as_ref());
        for (i, _) in qs.iter().enumerate() {
            assert!(questions.contains_key(&format!("q{i}.rel")), "{qs:?} missing relevance");
            if files_only {
                assert!(questions.keys().filter(|k| k.starts_with(&format!("q{i}.B")) || k.starts_with(&format!("q{i}.L"))).count() == 0);
            } else {
                for &(a, b) in &chunk.blocks {
                    assert!(questions.contains_key(&format!("q{i}.B{a}-{b}")));
                }
                for n in chunk.askable() {
                    assert!(questions.contains_key(&format!("q{i}.L{n}")));
                    let text = questions[&format!("q{i}.L{n}")]["instructions"].as_str().unwrap();
                    if broad {
                        assert!(text.contains("relevant to"), "{text}");
                    } else {
                        assert!(text.contains("directly answer"), "{text}");
                    }
                }
            }
        }
        if let Some(rules) = &rules {
            for (t, _) in rules.terms().iter().enumerate() {
                assert!(questions.contains_key(&format!("F{t}.S")));
                if files_only {
                    assert!(questions.keys().all(|k| !k.starts_with(&format!("F{t}.B"))));
                } else {
                    for &(a, b) in &chunk.blocks {
                        assert!(questions.contains_key(&format!("F{t}.B{a}-{b}")));
                    }
                }
            }
        }
        assert!(questions.values().all(|q| q.get("type").and_then(Value::as_str).is_some()));
    }
}

#[test]
fn search_aggregates_ranks_and_handles_multiple_queries() {
    let hay: Vec<String> = (0..400).map(|i| format!("hay_{i} = {i}")).collect();
    let mut target = vec!["a = 1"; 200];
    target.push("needle = find()");
    target.extend(vec!["b = 2"; 50]);
    let files = scratch("aggregate", &[("hay.py", hay.join("\n")), ("target.py", target.join("\n"))]);
    let threads = Arc::new(Mutex::new(HashSet::new()));
    let seen = threads.clone();
    let c = client(Config::default(), move |body| {
        seen.lock().unwrap().insert(std::thread::current().id());
        std::thread::sleep(Duration::from_millis(20));
        ok(answer_all(body, &["needle"]))
    });
    let mut ticks = Vec::new();
    let res = search(
        &c,
        &queries(&["find the needle", "unrelated"]),
        &files,
        &Options { jobs: 8, ..Options::default() },
        |d, t| ticks.push((d, t)),
        |_| {},
    )
    .unwrap();
    assert_eq!(res.len(), 2);
    let top = &res[0][0];
    assert!(top.path.ends_with("target.py") && top.score == 1.0 && top.confidence == 0.9);
    assert_eq!(top.lines.iter().filter(|h| h.p >= 0.5).map(|h| h.line).collect::<Vec<_>>(), [201]);
    let hot: Vec<_> = top.blocks.iter().filter(|b| b.p >= 0.5).collect();
    assert!(hot.len() == 1 && hot[0].start <= 201 && 201 <= hot[0].end);
    assert!(res[0][1..].iter().flat_map(|fr| &fr.lines).all(|h| h.p < 0.5));
    assert_eq!(top.lines.len(), 251); // every non-blank line scored exactly once
    assert!(top.lines.windows(2).all(|w| w[0].line < w[1].line));
    let requests = c.usage().requests() as usize;
    assert!(requests >= 4 && c.usage().input_tokens() == 100 * requests as u64); // both queries share each request
    assert_eq!(ticks.last(), Some(&(requests, requests)));
    assert!(threads.lock().unwrap().len() > 1); // requests really ran on multiple threads
}

#[test]
fn token_limit_splits_chunk_and_retries_halves() {
    let mut lines = vec!["x = 1"; 99];
    lines.push("needle()");
    let files = scratch("split", &[("big.py", lines.join("\n"))]);
    let sizes = Arc::new(Mutex::new(Vec::new()));
    let log = sizes.clone();
    let c = client(Config::default(), move |body| {
        let n = body["questions"].as_object().unwrap().keys().filter(|k| k.contains(".L")).count();
        log.lock().unwrap().push(n);
        if n > 30 {
            status(400, r#"{"detail": {"error_type": "max_tokens_exceeded"}}"#)
        } else {
            ok(answer_all(body, &["needle"]))
        }
    });
    let res = search(&c, &queries(&["needle"]), &files, &Options { jobs: 1, ..Options::default() }, |_, _| {}, |_| {}).unwrap();
    let sizes = sizes.lock().unwrap();
    assert_eq!(sizes[0], 100);
    assert_eq!(sizes.iter().filter(|&&n| n <= 30).sum::<usize>(), 100); // halved until it fits
    assert_eq!(res[0][0].lines.iter().filter(|h| h.p >= 0.5).map(|h| h.line).collect::<Vec<_>>(), [100]);
    assert_eq!(res[0][0].lines.len(), 100); // every line scored exactly once despite splitting
}

#[test]
fn token_limit_on_single_line_is_an_error_instead_of_empty_success() {
    let files = scratch("unsplittable", &[("one.py", "needle = 1\n".into())]);
    let c = client(Config::default(), |_| status(413, "context limit"));
    let result = search(&c, &queries(&["needle"]), &files, &Options::default(), |_, _| {}, |_| {});
    assert!(matches!(result, Err(DecisionError::TokenLimit(_))));
}

#[test]
fn a_search_where_every_request_fails_is_an_error_and_a_partial_one_is_noted() {
    let files = scratch("failing", &[("a.py", "needle = 1\n".into()), ("b.py", "other = 2\n".into())]);
    let c = client(Config::default(), |_| status(422, "nope"));
    assert!(matches!(search(&c, &queries(&["q"]), &files, &Options::default(), |_, _| {}, |_| {}), Err(DecisionError::InvalidRequest(_))));
    let c = client(Config::default(), |body| {
        if body["state"]["file"].as_str().unwrap().ends_with("b.py") {
            status(422, "nope")
        } else {
            ok(answer_all(body, &["needle"]))
        }
    });
    let mut notes = Vec::new();
    let mut progress = Vec::new();
    let res =
        search(&c, &queries(&["q"]), &files, &Options::default(), |done, total| progress.push((done, total)), |n| notes.push(n.to_owned()))
            .unwrap();
    assert_eq!(progress, [(1, 2), (2, 2)]);
    assert!(res[0].len() == 1 && res[0][0].path.ends_with("a.py"));
    assert!(notes[0].starts_with("1 request(s) failed, results may be incomplete; first error: HTTP 422"), "{notes:?}");
}

#[test]
fn path_triage_keeps_likely_files() {
    let files = scratch(
        "triage",
        &[("auth.py", "token = load()\n".into()), ("billing.py", "token = load()\n".into()), ("ui.py", "token = load()\n".into())],
    );
    let c = client(Config::default(), |body| {
        let Some(paths) = body["state"]["paths"].as_object() else { return ok(answer_all(body, &["token"])) };
        let answers: Map<String, Value> = paths
            .iter()
            .map(|(k, v)| (k.clone(), json!({"type": "noul", "noul": if v.as_str().unwrap().contains("auth") { 0.9 } else { 0.01 }})))
            .collect();
        ok(json!({"model": "jev-test", "answers": answers, "usage": {}}).to_string())
    });
    let mut notes = Vec::new();
    let res = search(
        &c,
        &queries(&["auth token"]),
        &files,
        &Options { triage: true, ..Options::default() },
        |_, _| {},
        |n| notes.push(n.to_owned()),
    )
    .unwrap();
    assert_eq!(res[0].iter().map(|fr| fr.path.rsplit('/').next().unwrap()).collect::<Vec<_>>(), ["auth.py"]);
    assert!(notes[0].contains("kept 1 of 3"));
}

#[test]
fn filter_answers_gate_blocks_lines_and_whole_sections() {
    let files = scratch(
        "gating",
        &[("app.py", "import os\nimport sys\n\n\ndef run():\n    return os.getcwd()\n".into()), ("README.md", "# App\n\nRun it.\n".into())],
    );
    let c = client(Config::default(), |body| {
        let (path, code) = (body["state"]["file"].as_str().unwrap(), common::listing(body));
        let answers: Map<String, Value> = body["questions"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(qid, q)| {
                let text = q["instructions"].as_str().unwrap();
                let answer = if q["type"] == "score" {
                    json!({"type": "score", "score": 3.0, "confidence": 0.9, "probabilities": {}, "legend": {}})
                } else if qid.starts_with('F') && text.contains("documentation") {
                    json!({"type": "noul", "noul": if path.ends_with(".md") { 0.97 } else { 0.03 }})
                } else if qid.starts_with('F') {
                    let hit = common::block_range(qid).is_some_and(|(lo, hi)| (lo..=hi).any(|n| code[&n].starts_with("import")));
                    json!({"type": "noul", "noul": if hit { 0.95 } else { 0.04 }})
                } else {
                    json!({"type": "noul", "noul": 0.9})
                };
                (qid.clone(), answer)
            })
            .collect();
        ok(json!({"model": "jev-test", "answers": answers}).to_string())
    });
    let opts = Options { rules: Some(parse_filter("no imports, no documentation")), ..Options::default() };
    let res = search(&c, &queries(&["anything"]), &files, &opts, |_, _| {}, |_| {}).unwrap();
    let by_name = |name: &str| res[0].iter().find(|fr| fr.path.ends_with(name)).unwrap();
    let app = by_name("app.py");
    let block = |start: usize| app.blocks.iter().find(|b| b.start == start).unwrap();
    assert!(block(1).keep < 0.1 && block(5).keep > 0.9); // the import block is filtered, the function is not
    assert!(app.lines.iter().all(|h| (h.keep < 0.1) == (h.line <= 2))); // lines inherit their block's verdict
    assert!(app.keep > 0.9 && app.score == 1.0);
    let readme = by_name("README.md");
    assert!(readme.keep < 0.1 && readme.score == 0.0); // a filtered-out section cannot carry the file
    assert!(readme.blocks.iter().all(|b| b.keep < 0.1));
}
