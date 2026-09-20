//! Natural-language include/exclude filters, turned into the question shape Jev answers best.
//!
//! Benchmarks show Jev is unreliable on negated or compound rules. Asked whether a file satisfies
//! "No tests", ordinary source files score near 0.5. Asked "Is this a test file?", the same files
//! score near 0 and test files near 1. This matches TypeSafe's guidance: ask atomic questions,
//! phrase them so that yes is the high-probability answer, and compose answers in code.
//!
//! So a filter such as "Source code files only. No documentation" is parsed into polar rules:
//!     only:  [["source code files"]]      every clause must match (alternatives inside a clause: any)
//!     not:   ["documentation"]            none may match
//! Each term becomes one positive category question. Polarity is applied here, in code.

use std::collections::HashMap;
use std::sync::OnceLock;

use regex_lite::Regex;

/// A row is kept when P(passes the rules) reaches this.
pub const FILTER_GATE: f64 = 0.5;

struct Patterns {
    clause: Regex,
    exclude: Regex,
    include: Regex,
    only_suffix: Regex,
    list: Regex,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| {
        let re = |s: &str| Regex::new(s).expect("valid regex");
        Patterns {
            clause: re(r"[.\n;]+|\s+but\s+"),
            exclude: re(concat!(
                r"(?i)^(?:no|not|non|without|except(?:\s+for)?|exclud(?:e|es|ed|ing)|ignor(?:e|es|ed|ing)|skip(?:s|ped|ping)?|",
                r"omit(?:s|ted|ting)?|hide|drop|never|minus|leave\s+out|filter\s+out|",
                r"(?:do\s+not|don'?t)\s+(?:include|show|match|return|want|search))\b[\s:-]*(?:any\s+|all\s+|the\s+)?",
            )),
            include: re(concat!(
                r"(?i)^(?:only|just|solely|include\s+only|show\s+only|match\s+only|search\s+only|limit(?:ed)?\s+to|",
                r"restrict(?:ed)?\s+to|must\s+be|keep\s+only|include)\b[\s:-]*(?:the\s+)?",
            )),
            only_suffix: re(r"(?i)[\s,]+only$"),
            list: re(r"(?i)\s*(?:,|;|/|\band\b|\bor\b|\bnor\b|&)\s*"),
        }
    })
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rules {
    /// AND of clauses, OR within a clause.
    pub only: Vec<Vec<String>>,
    /// None may match.
    pub exclude: Vec<String>,
}

fn dedup(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in items {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

impl Rules {
    pub fn is_empty(&self) -> bool {
        self.only.is_empty() && self.exclude.is_empty()
    }

    /// Distinct terms, in a stable order: one question each.
    pub fn terms(&self) -> Vec<String> {
        dedup(self.only.iter().flatten().chain(&self.exclude).cloned())
    }

    /// P(row passes), from per-term probabilities. Missing terms count as unknown (0.5).
    pub fn passes(&self, p: &HashMap<String, f64>) -> f64 {
        let get = |t: &String| p.get(t).copied().unwrap_or(0.5);
        let only = self.only.iter().map(|clause| clause.iter().map(get).fold(f64::MIN, f64::max));
        let exclude = self.exclude.iter().map(|t| 1.0 - get(t));
        only.chain(exclude).fold(1.0, f64::min)
    }

    pub fn describe(&self) -> String {
        let only = self.only.iter().map(|c| format!("only {}", c.join(" or ")));
        let exclude = self.exclude.iter().map(|t| format!("not {t}"));
        only.chain(exclude).collect::<Vec<_>>().join("; ")
    }

    pub fn merge(mut self, other: Rules) -> Rules {
        self.only.extend(other.only);
        self.exclude = dedup(self.exclude.into_iter().chain(other.exclude));
        self
    }
}

fn clean(term: &str) -> &str {
    term.trim_matches(|c: char| " \t\"'`.,:;!()[]".contains(c)).trim()
}

/// Parses free text like "Source code only. No docs, tests or examples" into polar rules.
pub fn parse_filter(text: &str) -> Rules {
    let pat = patterns();
    let mut rules = Rules::default();
    for clause in pat.clause.split(text) {
        let clause = clean(clause);
        if clause.is_empty() {
            continue;
        }
        let clause = pat.only_suffix.replace(clause, "");
        let mut exclude = false; // a bare clause such as "async code" is an inclusion
        let mut alternatives = Vec::new();
        for part in pat.list.split(&clause) {
            let mut part = clean(part);
            if part.is_empty() {
                continue;
            }
            if let Some(m) = pat.exclude.find(part) {
                (exclude, part) = (true, part.get(m.end()..).unwrap_or(""));
            } else if let Some(m) = pat.include.find(part) {
                (exclude, part) = (false, part.get(m.end()..).unwrap_or(""));
            }
            let stripped = pat.only_suffix.replace(part, "");
            if stripped.len() != part.len() {
                exclude = false;
            }
            let part = clean(&stripped);
            if part.is_empty() {
                continue;
            }
            if exclude {
                rules.exclude.push(part.to_owned());
            } else {
                alternatives.push(part.to_owned());
            }
        }
        if !alternatives.is_empty() {
            rules.only.push(alternatives);
        }
    }
    rules.exclude = dedup(rules.exclude);
    rules
}
