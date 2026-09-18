//! Live tests against the real TypeSafe API. They send only the tiny fixture files below.
//! Run: fnox exec -- cargo test --test live -- --ignored
mod common;

use std::path::Path;

use serde_json::Value;

const POOL: &str = "import asyncpg\n\nPOOL_MIN = 2\nPOOL_MAX = 20\n\n\nasync def create_pool(dsn: str):\n    return await asyncpg.create_pool(dsn, min_size=POOL_MIN, max_size=POOL_MAX)\n";
const STRINGS: &str = "def slugify(title: str) -> str:\n    return \"-\".join(title.lower().split())\n\n\ndef truncate(text: str, width: int) -> str:\n    return text if len(text) <= width else text[: width - 1] + \"...\"\n";
const BILLING: &str = "TAX_RATE = 0.21\n\n\ndef invoice_total(items):\n    subtotal = sum(i.price * i.quantity for i in items)\n    return round(subtotal * (1 + TAX_RATE), 2)\n";

fn repo(name: &str) -> std::path::PathBuf {
    common::repo(name, &[("db/pool.py", POOL), ("util/strings.py", STRINGS), ("billing/invoice.py", BILLING)])
}

/// Live tests explicitly forward only the caller's API key, not their home/config.
fn jg(dir: &Path, args: &[&str]) -> common::Ran {
    let key = jevgrep::client::resolve_api_key().expect("live tests require TYPESAFE_API_KEY (or fnox)");
    common::ran(common::command(dir).args(args).env("TYPESAFE_API_KEY", key).output().unwrap())
}

fn rows(out: &str) -> Vec<Value> {
    out.lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

fn paths(found: &[Value]) -> Vec<&str> {
    found.iter().map(|r| r["path"].as_str().unwrap()).collect()
}

#[test]
#[ignore = "hits the real TypeSafe API"]
fn finds_the_right_file_and_line() {
    let run = jg(&repo("live-pin"), &["where is the maximum number of database connections configured", "--json", "-q"]);
    assert_eq!(run.code, 0, "{}", run.err);
    let found = rows(&run.out);
    let top = &found[0];
    assert!(top["path"] == "db/pool.py" && top["relevance"].as_f64().unwrap() > 0.8 && top["match"] == "strong", "{top}");
    let regions = top["regions"].as_array().unwrap();
    let nested = regions.iter().flat_map(|r| r["lines"].as_array().unwrap());
    let located: Vec<u64> = nested.chain(top["lines"].as_array().unwrap()).map(|l| l["line"].as_u64().unwrap()).collect();
    let on = |r: &Value, n: u64| r["start"].as_u64().unwrap() <= n && n <= r["end"].as_u64().unwrap();
    assert!(located.contains(&4) || located.contains(&8) || regions.iter().any(|r| on(r, 8)), "{top}"); // POOL_MAX, or create_pool
    assert!(regions.iter().all(|r| r["lines"].as_array().unwrap().iter().all(|l| on(r, l["line"].as_u64().unwrap()))));
    assert!(!paths(&found).contains(&"util/strings.py")); // calibrated: unrelated file stays out
}

#[test]
#[ignore = "hits the real TypeSafe API"]
fn several_queries_in_one_pass() {
    let (tax, slug) = ("how is sales tax applied to an invoice", "turning a title into a URL slug");
    let run = jg(&repo("live-multi"), &[tax, "-e", slug, "--json", "-q"]);
    assert_eq!(run.code, 0, "{}", run.err);
    let found = rows(&run.out);
    let best = |q: &str| found.iter().find(|r| r["query"] == q).map(|r| r["path"].as_str().unwrap().to_owned());
    assert_eq!((best(tax).as_deref(), best(slug).as_deref()), (Some("billing/invoice.py"), Some("util/strings.py")));
}

#[test]
#[ignore = "hits the real TypeSafe API"]
fn files_only_mode_ranks_files() {
    let run = jg(&repo("live-files"), &["database connection pooling", "-l", "-q"]);
    assert_eq!(run.code, 0, "{}", run.err);
    let first: Vec<&str> = run.out.lines().next().unwrap().split_whitespace().collect();
    assert!(first[1] == "db/pool.py" && first[0].parse::<f64>().unwrap() > 0.6, "{}", run.out);
}

#[test]
#[ignore = "hits the real TypeSafe API"]
fn broad_query_returns_a_region_even_when_no_single_line_answers_it() {
    let run = jg(&repo("live-broad"), &["how does invoicing work", "--json", "-q"]);
    assert_eq!(run.code, 0, "{}", run.err);
    let found = rows(&run.out);
    assert_eq!(found[0]["path"], "billing/invoice.py");
    for r in &found {
        let regions = r["regions"].as_array().unwrap();
        assert!(!regions.is_empty() || !r["lines"].as_array().unwrap().is_empty(), "{r}"); // every file shown points at a location
        assert!(regions.iter().all(|reg| reg["p"].as_f64().unwrap() >= 0.35), "{r}");
        // and never at a long shot
    }
}

#[test]
#[ignore = "hits the real TypeSafe API"]
fn every_shown_file_has_a_location_in_text_output() {
    let run = jg(&repo("live-text"), &["database connection settings", "-q"]);
    assert_eq!(run.code, 0, "{}", run.err);
    for block in run.out.split("\n\n").filter(|b| !b.trim().is_empty() && !b.starts_with("--")) {
        let mut body = block.lines();
        assert!(body.next().unwrap().contains("relevance="), "{block}");
        let rows: Vec<&str> = body.collect();
        assert!(!rows.is_empty(), "{block}");
        assert!(
            rows.iter().all(|row| row.split_whitespace().next().unwrap().replace('-', "").chars().all(|c| c.is_ascii_digit())),
            "{block}"
        );
    }
}

#[test]
#[ignore = "hits the real TypeSafe API"]
fn natural_language_filter_excludes_documentation() {
    let dir = repo("live-filter");
    std::fs::write(
        dir.join("README.md"),
        "# Pooling\n\nThe database connection pool holds at most 20 connections.\nSet POOL_MAX in db/pool.py to change the maximum.\n",
    )
    .unwrap();
    let query = "what is the maximum number of database connections";
    let run = jg(&dir, &[query, "--json", "-q"]);
    let found = rows(&run.out);
    assert!(paths(&found).contains(&"README.md") && paths(&found).contains(&"db/pool.py"), "{}", run.out); // unfiltered: docs and code both match

    let run = jg(&dir, &[query, "--json", "-q", "--filter", "Source code files only. No documentation"]);
    assert_eq!(run.code, 0, "{}", run.err);
    let found = rows(&run.out);
    assert!(paths(&found).contains(&"db/pool.py") && !paths(&found).contains(&"README.md"), "{}", run.out);

    let run = jg(&dir, &[query, "-l", "-q", "--only", "documentation"]);
    assert_eq!(run.out.lines().map(|l| l.split_whitespace().nth(1).unwrap()).collect::<Vec<_>>(), ["README.md"]);
}

#[test]
#[ignore = "hits the real TypeSafe API"]
fn no_match_exits_1() {
    let run = jg(&repo("live-none"), &["kubernetes pod autoscaling policy", "-q"]);
    assert_eq!((run.code, run.out.as_str()), (1, ""), "{}", run.err);
}

#[test]
#[ignore = "hits the real TypeSafe API"]
fn bad_key_exits_2() {
    let mut cmd = common::command(&repo("live-badkey"));
    cmd.args(["anything"]).env("TYPESAFE_API_KEY", "apikey_invalid");
    let run = common::ran(cmd.output().unwrap());
    assert!(run.code == 2 && run.err.contains("rejected the API key"), "{}", run.err);
}
