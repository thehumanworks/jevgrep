//! End-to-end: the real `jg` binary against a local fake Jev served over HTTP.
mod common;

use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::{json, Map, Value};

use common::{answer_all, jg, repo, serve};

fn needle_server() -> String {
    serve(|req| {
        assert_eq!(req.authorization, "Bearer test-key");
        assert_eq!(req.body["model"], "jev-latest");
        (200, answer_all(&req.body, &["needle"]))
    })
}

#[test]
fn text_json_flat_and_files_output_and_exit_codes() {
    let url = needle_server();
    let dir = repo("formats", &[("t.py", "a = 1\nneedle = 2\nb = 3\n")]);

    let run = jg(&dir, &url, &["find needle", "-C", "1"]);
    assert_eq!(run.code, 0, "{}", run.err);
    assert_eq!(
        run.out.lines().collect::<Vec<_>>(),
        [
            "t.py  relevance=1.00",
            "        1-3  0.90  a = 1", // line 1 is the region label, so -C does not print it again
            "          2  0.95  needle = 2",
            "          3        b = 3",
            "",
        ]
    );
    assert!(run.err.contains("jg: 1 files, 1 requests, 100 tokens (~$0.0000), "), "{}", run.err);

    let run = jg(&dir, &url, &["find needle", "--json", "-q"]);
    assert_eq!((run.code, run.err.as_str()), (0, ""));
    let row: Value = serde_json::from_str(&run.out).unwrap();
    assert_eq!(
        row,
        json!({
            "query": "find needle", "path": "t.py", "relevance": 1.0, "section_relevance": 1.0, "confidence": 0.9, "match": "strong",
            "regions": [{"start": 1, "end": 3, "p": 0.9, "label": "a = 1", "label_line": 1, "lines": [{"line": 2, "p": 0.95, "text": "needle = 2"}]}],
            "lines": [], "more_regions": 0, "more_lines": 0,
        })
    );
    assert!(run.out.starts_with(r#"{"query":"find needle","path":"t.py","relevance":1.0,"#)); // stable key order

    let run = jg(&dir, &url, &["find needle", "--no-heading", "-q"]);
    assert_eq!((run.code, run.out.as_str()), (0, "t.py:1-3:0.90:a = 1\nt.py:2:0.95:needle = 2\n"));

    let run = jg(&dir, &url, &["-lq", "find needle"]);
    assert_eq!((run.code, run.out.as_str()), (0, "1.00  t.py\n"));
    let run = jg(&dir, &url, &["find needle", "-l", "--json", "-q"]);
    assert_eq!(
        serde_json::from_str::<Value>(&run.out).unwrap(),
        json!({"query": "find needle", "path": "t.py", "relevance": 1.0, "confidence": 0.9})
    );

    // Options may come in any position and any spelling argparse accepted.
    let run = jg(&dir, &url, &["-t0.99", "--top=3", "find needle", ".", "-q", "--no-heading"]);
    assert_eq!((run.code, run.out.as_str()), (1, "")); // nothing reaches 0.99
    let run = jg(&dir, &url, &["-t0.99", "find needle", "-q"]);
    assert!(run.code == 0 && run.out.starts_with("-- weaker:") && run.out.contains("(>= 0.69)"), "{}", run.out); // 0.90 is a near miss
    let run = jg(&dir, &url, &["find needle", "t.py", "-q", "-g", "*.py", "-x", "zzz*"]);
    assert_eq!(run.code, 0);

    std::fs::write(dir.join("t.py"), "nothing here\n").unwrap();
    assert_eq!(jg(&dir, &url, &["find needle", "-q"]).code, 1);
    let run = jg(&dir, &url, &["find needle", "missing-dir", "-q"]);
    assert_eq!((run.code, run.err.as_str()), (2, "jg: missing-dir: no such file or directory\n"));
}

#[test]
fn several_queries_get_a_heading_each() {
    let url = needle_server();
    let dir = repo("multi", &[("t.py", "a = 1\nneedle = 2\n")]);
    let run = jg(&dir, &url, &["find needle", "-e", "another", "-q", "-l"]);
    assert_eq!((run.code, run.out.as_str()), (0, "== find needle\n1.00  t.py\n\n== another\n1.00  t.py\n\n"));
    let run = jg(&dir, &url, &["find needle", "-e", "another", "-q", "--json"]);
    let asked: Vec<Value> = run.out.lines().map(|l| serde_json::from_str::<Value>(l).unwrap()["query"].clone()).collect();
    assert_eq!(asked, ["find needle", "another"]);
}

/// Weak rows need the "weaker" separator to be read correctly; flat output has none.
#[test]
fn flat_output_never_carries_weak_tier_rows() {
    let url = serve(|req| {
        let answers: Map<String, Value> = req.body["questions"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, q)| {
                let answer = if q["type"] == "score" {
                    json!({"type": "score", "score": 2.7, "confidence": 0.9})
                } else {
                    json!({"type": "noul", "noul": if k.contains(".B") { 0.41 } else { 0.1 }})
                };
                (k.clone(), answer)
            })
            .collect();
        (200, json!({"answers": answers, "usage": {}}).to_string())
    });
    let dir = repo("weak", &[("t.py", "a = 1\nb = 2\n")]);
    let run = jg(&dir, &url, &["q", "-q"]);
    assert_eq!(run.code, 0);
    assert_eq!(
        run.out,
        "-- weaker: these files look related overall, but nothing in them reached 0.50; near misses (>= 0.35) shown\n\n\
                         t.py  relevance=0.90\n        1-2  0.41  a = 1\n\n"
    );
    let run = jg(&dir, &url, &["q", "-q", "--no-heading"]);
    assert_eq!((run.code, run.out.as_str()), (1, "")); // nothing cleared -t, so flat mode has no rows
    let run = jg(&dir, &url, &["q", "-q", "--json"]);
    assert_eq!(run.code, 0);
    assert_eq!(serde_json::from_str::<Value>(&run.out).unwrap()["match"], "weak");
}

/// Everything matches the query. `documentation` applies to .md files, `imports` to import lines.
fn filter_server() -> String {
    serve(|req| {
        let (path, code) = (req.body["state"]["file"].as_str().unwrap(), common::listing(&req.body));
        let answers: Map<String, Value> = req.body["questions"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(qid, q)| {
                let text = q["instructions"].as_str().unwrap();
                let answer = if q["type"] == "score" {
                    json!({"type": "score", "score": 3.0, "confidence": 0.9})
                } else if qid.starts_with('F') && (text.contains("documentation") || text.contains("Source code")) {
                    let docs = path.ends_with(".md");
                    json!({"type": "noul", "noul": if docs == text.contains("documentation") { 0.97 } else { 0.03 }})
                } else if qid.starts_with('F') {
                    let hit = common::block_range(qid).is_some_and(|(lo, hi)| (lo..=hi).any(|n| code[&n].starts_with("import")));
                    json!({"type": "noul", "noul": if hit { 0.95 } else { 0.04 }})
                } else {
                    json!({"type": "noul", "noul": 0.9})
                };
                (qid.clone(), answer)
            })
            .collect();
        (200, json!({"answers": answers, "usage": {"input_tokens": 50}}).to_string())
    })
}

fn shown_paths(out: &str) -> Vec<&str> {
    out.lines().filter(|l| l.contains("relevance=")).map(|l| l.split_whitespace().next().unwrap()).collect()
}

#[test]
fn natural_language_filters_end_to_end() {
    let url = filter_server();
    let dir = repo(
        "filters",
        &[("app.py", "import os\nimport sys\n\n\ndef run():\n    return os.getcwd()\n"), ("README.md", "# App\n\nRun it.\n")],
    );

    let run = jg(&dir, &url, &["anything"]);
    assert_eq!((run.code, shown_paths(&run.out)), (0, vec!["README.md", "app.py"]));

    let run = jg(&dir, &url, &["anything", "--filter", "Source code only. No documentation"]);
    assert_eq!((run.code, shown_paths(&run.out)), (0, vec!["app.py"]));
    assert!(run.err.contains("jg: filter: only Source code; not documentation\n"), "{}", run.err); // shows how the filter was read
    assert!(run.err.contains("jg: filter removed 1 matching region and 2 lines\n"), "{}", run.err);

    let run = jg(&dir, &url, &["anything", "--not", "imports", "--not=documentation", "-q", "--no-heading"]);
    let rows: Vec<&str> = run.out.lines().collect();
    assert!(run.code == 0 && !rows.is_empty() && rows.iter().all(|r| r.starts_with("app.py:")), "{}", run.out);
    // The import block is gone: region-level filter.
    assert!(!rows.iter().any(|r| ["1", "2", "1-2"].contains(&r.split(':').nth(1).unwrap())), "{}", run.out);

    let run = jg(&dir, &url, &["anything", "-l", "--only", "documentation", "-q"]);
    assert_eq!((run.code, run.out.as_str()), (0, "1.00  README.md\n"));

    let run = jg(&dir, &url, &["anything", "-f", " . ", "-q"]); // a filter that names nothing is an error
    assert_eq!((run.code, run.err.as_str()), (2, "jg: the filter names no category; try e.g. --filter \"no tests\"\n"));
}

#[test]
fn rate_limits_are_retried_over_real_http_and_bad_keys_are_not() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let url = serve(move |req| {
        if seen.fetch_add(1, Ordering::SeqCst) == 0 {
            (429, r#"{"detail": "Rate limit exceeded"}"#.into())
        } else {
            (200, answer_all(&req.body, &["needle"]))
        }
    });
    let dir = repo("retry", &[("t.py", "needle = 2\n")]);
    let run = jg(&dir, &url, &["find needle", "--no-heading"]);
    assert_eq!((run.code, run.out.as_str()), (0, "t.py:1-1:0.90:needle = 2\n")); // the line is its region's label, so it is printed once
    assert!(run.err.contains("1 requests, 1 retries"), "{}", run.err);

    let denied = Arc::new(AtomicUsize::new(0));
    let seen = denied.clone();
    let url = serve(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
        (401, r#"{"detail": "bad key"}"#.into())
    });
    let run = jg(&dir, &url, &["find needle"]);
    assert_eq!(run.code, 2);
    assert!(run.err.contains("jg: TypeSafe rejected the API key (HTTP 401)"), "{}", run.err);
    assert_eq!(denied.load(Ordering::SeqCst), 1);
}

#[test]
fn usage_errors_help_and_missing_key() {
    let dir = repo("usage", &[("t.py", "x = 1\n")]);
    let bare = |args: &[&str]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_jg"));
        cmd.current_dir(&dir).args(args).env_remove("TYPESAFE_API_KEY").env("JG_NO_FNOX", "1");
        common::ran(cmd.output().unwrap())
    };
    let run = bare(&["q"]);
    assert!(run.code == 2 && run.err.contains("TYPESAFE_API_KEY is not set"), "{}", run.err);
    let run = bare(&[]);
    assert!(run.code == 2 && run.err.contains("QUERY"), "{}", run.err);
    let run = bare(&["q", "--bogus"]);
    assert!(run.code == 2 && run.err.contains("--bogus"), "{}", run.err);
    let run = bare(&["q", "-t", "high"]);
    assert!(run.code == 2 && run.err.contains("high"), "{}", run.err);
    assert_eq!(bare(&["  "]).err, "jg: empty query\n");
    let run = bare(&["--version"]);
    assert_eq!((run.code, run.out), (0, format!("jg {}\n", env!("CARGO_PKG_VERSION"))));
    let run = bare(&["-h"]);
    assert!(run.code == 0 && run.out.starts_with("usage: jg") && run.out.contains("--filter TEXT"));
}
