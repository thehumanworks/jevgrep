//! Deterministic protocol/coverage tests. These do NOT measure semantic accuracy.
mod common;
use jevgrep::backend::{DecisionBackend, DecisionError, Usage};
use jevgrep::rules::{parse, prepare};
use serde_json::{json, Map, Value};

struct Fake {
    usage: Usage,
    value: Value,
}
impl DecisionBackend for Fake {
    fn ask(&self, _: &Value, questions: &Map<String, Value>) -> Result<Map<String, Value>, DecisionError> {
        if self.value == "error" {
            return Err(DecisionError::Api("fixture failure".into()));
        }
        if self.value == "missing" {
            return Ok(Map::new());
        }
        Ok(questions.keys().map(|k| (k.clone(), self.value.clone())).collect())
    }
    fn usage(&self) -> &Usage {
        &self.usage
    }
}
fn fake(value: Value) -> Fake {
    Fake { usage: Usage::default(), value }
}
fn fixture(name: &str, code: &str) -> (String, Vec<String>) {
    let dir = common::repo(name, &[("RULES.md", "# Rules\n- [PURE] Query functions must not mutate state.\n"), ("code.py", code)]);
    (dir.join("RULES.md").to_str().unwrap().into(), vec![dir.join("code.py").to_str().unwrap().into()])
}

#[test]
fn labels_are_unique_and_prose_is_never_silently_discarded() {
    assert_eq!(parse("# Rules\n\n- [NAME_1] Describe the action.\n- [B-2] Preserve errors.").unwrap().len(), 2);
    for bad in [
        "",
        "# No rules",
        "- [X] ",
        "- [] body",
        "- [two words] body",
        "- [X] a\n- [X] b",
        "unlabelled requirement",
        "- [X] a\n  continuation",
    ] {
        assert!(parse(bad).is_err(), "{bad}");
    }
}

#[test]
fn findings_are_exact_snapshot_ranges_and_do_not_invent_an_explanation() {
    let code = format!("def fetch():\n    return '{}尾'\n", "x".repeat(400));
    let (rules, sources) = fixture("rule-snapshot", &code);
    let mut plan = prepare(&rules, &sources, 128).unwrap();
    assert!(plan.requests[0].state["code"].as_str().unwrap().contains("尾"));
    std::fs::write(&sources[0], "changed after snapshot\n").unwrap();
    let (findings, completed) = plan.evaluate(&fake(json!({"type":"noul","noul":0.9})));
    assert_eq!(completed, 1);
    assert_eq!(findings[0]["evidence"], code.trim_end());
    assert_eq!((findings[0]["start"].as_u64(), findings[0]["end"].as_u64()), (Some(1), Some(2)));
    assert_eq!(findings[0]["reason_kind"], "scored_proposition");
}

#[test]
fn malformed_missing_or_failed_answers_never_become_clean() {
    let (rules, sources) = fixture("rule-answers", "def f():\n    return 1\n");
    for value in [
        json!("missing"),
        json!("error"),
        json!({"type":"noul","noul":1.1}),
        json!({"type":"score","noul":0.99}),
        json!({"type":"noul","noul":"0.99"}),
    ] {
        let mut plan = prepare(&rules, &sources, 128).unwrap();
        let (findings, completed) = plan.evaluate(&fake(value));
        assert!(findings.is_empty());
        assert_eq!(completed, 0);
        assert_eq!(plan.gaps.len(), 1);
    }
}

#[test]
fn skips_are_explicit_and_budget_never_truncates_source_or_answers() {
    let (rules, sources) = fixture("rule-budget", "def first():\n    return 1\n\ndef second():\n    return 2\n");
    let plan = prepare(&rules, &sources, 1).unwrap();
    assert!(plan.requests.is_empty());
    assert_eq!(plan.gaps.len(), 1);
    let report = plan.report("planned", &[], 0, None);
    assert_eq!(report["coverage"]["whole_repository"], false);
    assert_eq!(report["coverage"]["completed"], 0);
    assert!(prepare(&rules, &[sources[0].clone(), sources[0].clone()], 128).is_err());
}

#[test]
fn absent_invalid_and_oversized_files_are_coverage_gaps() {
    let (rules, sources) = fixture("rule-files", "valid\n");
    let missing = format!("{}.missing", sources[0]);
    assert_eq!(prepare(&rules, &[missing], 128).unwrap().gaps.len(), 1);
    for data in [vec![0], vec![255], vec![b'a'; 256 * 1024 + 1]] {
        std::fs::write(&sources[0], data).unwrap();
        let plan = prepare(&rules, &sources, 128).unwrap();
        assert!(plan.requests.is_empty());
        assert_eq!(plan.gaps.len(), 1);
    }
}

#[test]
fn context_includes_surrounding_original_code_across_chunks() {
    let code = (0..250).map(|i| format!("line_{i} = {i}\n")).collect::<String>();
    let (rules, sources) = fixture("rule-context", &code);
    let plan = prepare(&rules, &sources, 128).unwrap();
    assert!(plan.requests.len() >= 3);
    let second = &plan.requests[1];
    let code = second.state["code"].as_str().unwrap();
    assert!(code.contains(&format!("{}|", second.start - 1)));
    assert!(code.contains(&format!("{}|", second.end + 1)));
}

#[test]
fn offline_plan_does_not_resolve_auth_and_rejects_ambiguous_scope() {
    let (rules, sources) = fixture("rule-cli-plan", "value = 1\n");
    let dir = std::path::Path::new(&rules).parent().unwrap();
    let output = common::ran(common::command(dir).args(["--rules", &rules, "--source", &sources[0], "--plan"]).output().unwrap());
    assert_eq!(output.code, 0, "{}", output.err);
    let report: Value = serde_json::from_str(&output.out).unwrap();
    assert_eq!(report["status"], "planned");
    assert_eq!(report["coverage"]["completed"], 0);
    for extra in [vec!["query"], vec!["--filter", "tests"], vec!["--files"], vec!["--check-budget", "1025"]] {
        let output = common::ran(common::command(dir).args(["--rules", &rules, "--source", &sources[0]]).args(extra).output().unwrap());
        assert_eq!(output.code, 2);
    }
}

#[test]
fn real_cli_distinguishes_findings_no_findings_and_provider_failure() {
    let (rules, sources) = fixture("rule-cli-live-fake", "def f():\n    return 1\n");
    let dir = std::path::Path::new(&rules).parent().unwrap();
    for (p, status) in [(0.9, "findings"), (0.1, "no_findings")] {
        let url = common::serve(move |request| {
            let answers: Map<String, Value> =
                request.body["questions"].as_object().unwrap().keys().map(|id| (id.clone(), json!({"type":"noul","noul":p}))).collect();
            (200, json!({"model":"jev-test", "answers":answers, "usage":{"input_tokens":100,"output_tokens":10}}).to_string())
        });
        let output = common::jg(dir, &url, &["--rules", &rules, "--source", &sources[0]]);
        assert_eq!(output.code, 0, "{} {}", output.err, output.out);
        let report: Value = serde_json::from_str(&output.out).unwrap();
        assert_eq!(report["status"], status);
        assert_eq!(report["coverage"]["completed"], 1);
    }
    let url = common::serve(|_| (503, "unavailable".into()));
    let output = common::jg(dir, &url, &["--rules", &rules, "--source", &sources[0]]);
    assert_eq!(output.code, 2);
    let report: Value = serde_json::from_str(&output.out).unwrap();
    assert_eq!(report["status"], "incomplete");
}

#[test]
#[cfg(unix)]
fn source_locations_resolve_parent_symlinks_before_dot_dot() {
    let (rules, sources) = fixture("rule-parent-link", "wrong = 0\n");
    let dir = std::path::Path::new(&sources[0]).parent().unwrap();
    std::fs::create_dir_all(dir.join("actual/sub")).unwrap();
    std::fs::write(dir.join("actual/code.py"), "correct = 1\n").unwrap();
    std::os::unix::fs::symlink(dir.join("actual/sub"), dir.join("link")).unwrap();
    let source = dir.join("link/../code.py").to_str().unwrap().to_owned();
    let plan = prepare(&rules, &[source], 128).unwrap();
    assert_eq!(std::fs::read_to_string(&plan.requests[0].path).unwrap(), "correct = 1\n");
}
