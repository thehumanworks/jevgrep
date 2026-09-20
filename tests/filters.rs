//! Natural-language filters: parsing into polar rules and applying polarity in code.
use std::collections::HashMap;

use jevgrep::filters::{parse_filter, Rules};

#[test]
fn parse_filter_reads_plain_language() {
    type Case = (&'static str, &'static [&'static [&'static str]], &'static [&'static str]);
    let cases: &[Case] = &[
        ("Source code files only. No documentation", &[&["Source code files"]], &["documentation"]),
        ("No tests", &[], &["tests"]),
        ("Exclude documentation, examples and config files", &[], &["documentation", "examples", "config files"]),
        ("Only documentation", &[&["documentation"]], &[]),
        ("source code only, no docs or tests", &[&["source code"]], &["docs", "tests"]),
        ("async code", &[&["async code"]], &[]), // a bare phrase is an inclusion
        ("Only Rust or Python files; skip generated code", &[&["Rust", "Python files"]], &["generated code"]),
        ("Don't include vendored code. Just the public API", &[&["public API"]], &["vendored code"]),
        ("python files but not tests", &[&["python files"]], &["tests"]),
        ("Ignore comments and docstrings", &[], &["comments", "docstrings"]),
        ("definitions only, not call sites or imports", &[&["definitions"]], &["call sites", "imports"]),
        ("without any tests, excluding the benchmarks", &[], &["tests", "benchmarks"]),
        ("no tests. no tests.", &[], &["tests"]), // duplicates collapse
        ("  .  ", &[], &[]),
        ("except for the tests", &[], &["tests"]),
        ("Don't include any tests", &[], &["tests"]),
        ("do not show documentation", &[], &["documentation"]),
        ("filter out all generated code", &[], &["generated code"]),
        ("leave out comments", &[], &["comments"]),
        ("omit tests; hide docs", &[], &["tests", "docs"]),
        ("just the public API", &[&["public API"]], &[]),
        ("limited to rust", &[&["rust"]], &[]),
        ("restricted to rust or python", &[&["rust", "python"]], &[]),
        ("must be rust", &[&["rust"]], &[]),
        ("keep only rust", &[&["rust"]], &[]),
        ("include rust", &[&["rust"]], &[]),
        ("tests only", &[&["tests"]], &[]),
        ("definitions / call sites", &[&["definitions", "call sites"]], &[]),
        ("docs & examples", &[&["docs", "examples"]], &[]),
        ("No tests nor benchmarks", &[], &["tests", "benchmarks"]),
        ("none of this is a filter word", &[&["none of this is a filter word"]], &[]),
        ("notes", &[&["notes"]], &[]),
        ("non-tests", &[], &["tests"]),
        ("No.", &[], &[]),
    ];
    for (text, only, exclude) in cases {
        let rules = parse_filter(text);
        let want: Vec<Vec<String>> = only.iter().map(|c| c.iter().map(|t| t.to_string()).collect()).collect();
        assert_eq!((rules.only, rules.exclude), (want, exclude.iter().map(|t| t.to_string()).collect()), "{text}");
    }
}

#[test]
fn rules_apply_polarity_in_code() {
    let rules = parse_filter("Only Rust or Python. No tests");
    assert_eq!(rules.terms(), ["Rust", "Python", "tests"]);
    let p = |rust: f64, python: f64, tests: f64| -> HashMap<String, f64> {
        HashMap::from([("Rust".into(), rust), ("Python".into(), python), ("tests".into(), tests)])
    };
    let close = |got: f64, want: f64| assert!((got - want).abs() < 1e-9, "{got} != {want}");
    close(rules.passes(&p(0.1, 0.9, 0.2)), 0.8); // any alternative, and not a test
    close(rules.passes(&p(0.1, 0.9, 0.95)), 0.05); // excluded
    close(rules.passes(&p(0.1, 0.2, 0.0)), 0.2); // no inclusion clause holds
    close(rules.passes(&HashMap::new()), 0.5); // unknown terms count as a coin flip
    assert!(Rules::default().is_empty() && Rules::default().passes(&HashMap::new()) == 1.0);
    assert_eq!(rules.describe(), "only Rust or Python; not tests");
    let merged = parse_filter("no tests").merge(parse_filter("no tests, no docs. only rust"));
    assert_eq!((merged.only, merged.exclude), (vec![vec!["rust".to_string()]], vec!["tests".to_string(), "docs".to_string()]));
}

/// `passes` is the spec: AND of clauses, OR inside a clause, polarity applied in code.
#[test]
fn passes_is_min_of_clause_maxes_and_inverted_exclusions() {
    let rules = Rules { only: vec![vec!["Rust".into(), "Python".into()], vec!["source".into()]], exclude: vec!["tests".into()] };
    let p = |pairs: &[(&str, f64)]| -> HashMap<String, f64> { pairs.iter().map(|(k, v)| ((*k).to_string(), *v)).collect() };
    let close = |got: f64, want: f64| assert!((got - want).abs() < 1e-12, "{got} != {want}");
    close(rules.passes(&p(&[("Rust", 0.2), ("Python", 0.9), ("source", 0.8), ("tests", 0.1)])), 0.8);
    close(rules.passes(&p(&[("Rust", 0.9), ("Python", 0.1), ("source", 0.4), ("tests", 0.0)])), 0.4);
    close(rules.passes(&p(&[("Rust", 1.0), ("Python", 1.0), ("source", 1.0), ("tests", 1.0)])), 0.0);
    close(rules.passes(&HashMap::new()), 0.5);
    close(Rules::default().passes(&HashMap::new()), 1.0);
    assert_eq!(rules.terms(), ["Rust", "Python", "source", "tests"]);
}

#[test]
fn passes_stays_in_unit_interval_and_is_monotonic_in_term_probabilities() {
    let mut rng = 11u64;
    let next = |rng: &mut u64, n: usize| {
        *rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((*rng >> 33) as usize) % n
    };
    let unit = |rng: &mut u64| next(rng, 101) as f64 / 100.0;
    for _ in 0..400 {
        let terms = ["a", "b", "c", "d"];
        let mut only = Vec::new();
        for _ in 0..next(&mut rng, 3) {
            let mut clause = Vec::new();
            for _ in 0..=next(&mut rng, 3) {
                clause.push(terms[next(&mut rng, terms.len())].to_string());
            }
            only.push(clause);
        }
        let exclude: Vec<String> = (0..next(&mut rng, 3)).map(|_| terms[next(&mut rng, terms.len())].to_string()).collect();
        let rules = Rules { only, exclude };
        let probs: HashMap<String, f64> = terms.iter().map(|t| ((*t).to_string(), unit(&mut rng))).collect();
        let baseline = rules.passes(&probs);
        assert!((0.0..=1.0).contains(&baseline), "{baseline}");
        if rules.is_empty() {
            assert_eq!(baseline, 1.0);
        }
        for term in terms {
            let mut up = probs.clone();
            up.insert(term.into(), (probs[term] + 0.2).min(1.0));
            let mut down = probs.clone();
            down.insert(term.into(), (probs[term] - 0.2).max(0.0));
            let raised = rules.passes(&up);
            let lowered = rules.passes(&down);
            if rules.exclude.iter().any(|t| t == term) && rules.only.iter().flatten().all(|t| t != term) {
                assert!(raised <= baseline + 1e-12, "raising an exclude-only term must not increase P(pass)");
                assert!(lowered >= baseline - 1e-12, "lowering an exclude-only term must not decrease P(pass)");
            }
            if rules.only.iter().flatten().any(|t| t == term) && !rules.exclude.iter().any(|t| t == term) {
                assert!(raised >= baseline - 1e-12, "raising an include-only term must not decrease P(pass)");
                assert!(lowered <= baseline + 1e-12, "lowering an include-only term must not increase P(pass)");
            }
        }
        // FILTER_GATE is the documented keep/drop cut.
        assert_eq!(jevgrep::filters::FILTER_GATE, 0.5);
    }
}
