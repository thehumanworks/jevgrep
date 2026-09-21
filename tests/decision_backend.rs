//! Search consumes the decision contract without depending on a provider client.
mod common;

use jevgrep::backend::{DecisionBackend, DecisionError, Usage};
use jevgrep::search::{search, Options};
use serde_json::{json, Map, Value};

#[derive(Default)]
struct Decisions {
    usage: Usage,
}

impl DecisionBackend for Decisions {
    fn ask(&self, state: &Value, questions: &Map<String, Value>) -> Result<Map<String, Value>, DecisionError> {
        assert!(state.is_object());
        Ok(questions
            .iter()
            .map(|(id, question)| {
                let answer = if question["type"] == "score" {
                    json!({"type":"score", "score":3.0, "confidence":0.9})
                } else {
                    json!({"type":"noul", "noul":0.95})
                };
                (id.clone(), answer)
            })
            .collect())
    }

    fn usage(&self) -> &Usage {
        &self.usage
    }
}

#[test]
fn independent_backend_runs_triage_search_and_multiple_queries() {
    let dir = common::repo("decision-backend", &[("one.py", "needle = 1\n")]);
    let backend: Box<dyn DecisionBackend> = Box::<Decisions>::default();
    let results = search(
        backend.as_ref(),
        &["needle".into(), "assignment".into()],
        &[dir.join("one.py")],
        &Options { triage: true, ..Options::default() },
        |_, _| {},
        |_| {},
    )
    .unwrap();
    assert_eq!(results.len(), 2);
    for query in results {
        assert_eq!(query.len(), 1);
        assert_eq!(query[0].score, 1.0);
        assert_eq!(query[0].lines[0].line, 1);
        assert_eq!(query[0].lines[0].p, 0.95);
    }
}

/// Records how many lines each request asked about; `sequential` is what the backend claims.
struct Counting {
    usage: Usage,
    sequential: bool,
    lines_per_request: std::sync::Mutex<Vec<usize>>,
}

impl Counting {
    fn new(sequential: bool) -> Self {
        Counting { usage: Usage::default(), sequential, lines_per_request: Default::default() }
    }
}

impl DecisionBackend for Counting {
    fn ask(&self, state: &Value, questions: &Map<String, Value>) -> Result<Map<String, Value>, DecisionError> {
        self.lines_per_request.lock().unwrap().push(questions.keys().filter(|id| id.contains(".L")).count());
        Decisions::default().ask(state, questions)
    }

    fn usage(&self) -> &Usage {
        &self.usage
    }

    fn answers_sequentially(&self) -> bool {
        self.sequential
    }
}

fn numbered_source(lines: usize) -> String {
    (1..=lines).map(|n| format!("value_{n} = {n}\n")).collect()
}

/// A backend that writes its answers one after another is slow in proportion to what one request
/// asks, so a search too small to fill the lanes is cut finer. Jev's requests must not change.
#[test]
fn small_searches_are_spread_over_idle_lanes_only_for_sequential_backends() {
    let dir = common::repo("decision-spread", &[("big.py", &numbered_source(300))]);
    let run = |sequential: bool, jobs: usize| {
        let backend = Counting::new(sequential);
        let ranked =
            search(&backend, &["needle".into()], &[dir.join("big.py")], &Options { jobs, ..Options::default() }, |_, _| {}, |_| {})
                .unwrap();
        // Every line is asked about exactly once however the file was cut.
        assert_eq!(ranked[0][0].lines.iter().map(|hit| hit.line).collect::<Vec<_>>(), (1..=300).collect::<Vec<_>>());
        backend.lines_per_request.into_inner().unwrap()
    };
    assert_eq!(run(false, 32), [150, 150], "Jev-style requests keep the full chunk");
    let spread = run(true, 32);
    assert!(spread.len() >= 6 && spread.len() <= 24, "{spread:?}");
    // A quarter of the usual chunk is the floor, so requests never shrink to a few lines each.
    assert!(spread.iter().all(|&lines| lines <= 38), "{spread:?}");
    // With no lanes to fill there is nothing to gain from more requests.
    assert_eq!(run(true, 2).len(), 2);
}

/// Files are read in parallel batches; none may be dropped or duplicated, and unreadable ones
/// (binary here) are skipped as before.
#[test]
fn batched_parallel_reads_cover_every_file_once() {
    let names: Vec<String> = (0..40).map(|i| format!("f{i:02}.py")).collect();
    let mut files: Vec<(&str, &str)> = names.iter().map(|name| (name.as_str(), "x = 1\ny = 2\n")).collect();
    files.push(("blob.bin", "\0\0\0"));
    let dir = common::repo("decision-batches", &files);
    let paths: Vec<_> = files.iter().map(|(name, _)| dir.join(name)).collect();
    let backend = Counting::new(false);
    let ranked = search(&backend, &["x".into()], &paths, &Options::default(), |_, _| {}, |_| {}).unwrap();
    let mut seen: Vec<&str> = ranked[0].iter().map(|file| file.path.rsplit('/').next().unwrap()).collect();
    seen.sort_unstable();
    assert_eq!(seen, names.iter().map(String::as_str).collect::<Vec<_>>());
    assert_eq!(backend.lines_per_request.into_inner().unwrap().len(), 40);
}
