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
