//! Turns (queries, files) into Jev requests, fans them out over threads, aggregates answers.
//!
//! One request per file chunk. Each request carries, per query:
//!   - one Score question: how relevant is this file section to the query (ranks files)
//!   - one Noul question per logical block: do these lines contain what the query seeks (regions)
//!   - one Noul question per line: does this line directly answer the query (anchors)
//!
//! Answers are often a whole function or class rather than a line. Then no single line scores high,
//! but its block does, which is why regions and not lines are the primary unit of output.
//! Jev evaluates every question in a request in parallel, so extra queries and extra
//! lines add tokens but almost no latency. Extra queries reuse the same state.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;

use serde_json::{json, Map, Value};

use crate::client::{JevClient, JevError};
use crate::files::{chunk_lines, display_path, indent, is_definition, read_lines, split_chunk, Chunk, Chunking};
use crate::filters::{Rules, FILTER_GATE};

pub const RELEVANCE_LEVELS: [&str; 4] = [
    "Irrelevant: nothing here relates to the query",
    "Tangential: touches the same area but would not help answer the query",
    "Relevant: contains code or text that helps answer the query",
    "Direct hit: this is the code or text the query is looking for",
];

// Question wording was chosen by benchmark. "Directly answer" keeps the target lines above 0.5
// while flagging 10-30x fewer lines elsewhere than the looser "relevant to" phrasing.
// Filter terms are asked as positive category questions; polarity is applied in code (filters.rs).
// The category wording must cope with raw phrases like "source code files".
fn line_instruction(n: usize, which: &str, broad: bool) -> String {
    if broad {
        format!("Is line {n} of the code relevant to {which}?")
    } else {
        format!("Does line {n} of the code directly answer {which}?")
    }
}

fn block_instruction(a: usize, b: usize, which: &str) -> String {
    format!("Do lines {a}-{b} contain the code that {which} is looking for?")
}

fn filter_section_instruction(term: &str) -> String {
    format!("Does this file fall under the category: {term}?")
}

fn filter_block_instruction(a: usize, b: usize, term: &str) -> String {
    format!("Do lines {a}-{b} fall under the category: {term}?")
}

#[derive(Debug, Clone)]
pub struct Options {
    pub files_only: bool,
    pub broad: bool,
    pub jobs: usize,
    pub chunk_lines: usize,
    pub chunk_tokens: usize,
    pub max_files: usize,
    pub triage: bool,
    pub triage_threshold: f64,
    /// Natural-language include/exclude filter.
    pub rules: Option<Rules>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            files_only: false,
            broad: false,
            jobs: 32,
            chunk_lines: 150,
            chunk_tokens: 14_000,
            max_files: 1500,
            triage: false,
            triage_threshold: 0.2,
            rules: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LineHit {
    pub line: usize,
    pub p: f64,
    pub text: String,
    /// Relevance (0..1) of the chunk this line was scored in.
    pub section: f64,
    /// P(passes the --filter rules), inherited from the line's block.
    pub keep: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlockHit {
    pub start: usize,
    pub end: usize,
    /// P(these lines contain the code the query is looking for).
    pub p: f64,
    /// First meaningful line of the block, usually a signature.
    pub label: String,
    /// Line number the label text comes from.
    pub label_line: usize,
    /// P(passes the --filter rules).
    pub keep: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileResult {
    pub path: String,
    /// 0..1, best chunk relevance.
    pub score: f64,
    /// Jev's confidence in that chunk's relevance score.
    pub confidence: f64,
    /// P(file passes the --filter rules): mean over its sections.
    pub keep: f64,
    pub blocks: Vec<BlockHit>,
    pub lines: Vec<LineHit>,
}

impl FileResult {
    pub fn new(path: &str) -> Self {
        FileResult { path: path.to_owned(), score: 0.0, confidence: 0.0, keep: 1.0, blocks: vec![], lines: vec![] }
    }

    pub fn best_evidence(&self) -> f64 {
        self.blocks.iter().map(|b| b.p).chain(self.lines.iter().map(|h| h.p)).fold(0.0, f64::max)
    }

    pub fn rank(&self) -> f64 {
        let (score, evidence) = (self.score, self.best_evidence());
        score.max(evidence) + 0.25 * score.min(evidence)
    }
}

/// The line that best names a block: its definition if one opens it, else its first real line.
///
/// Skips leading decorators, attributes and comments, so `@property` / `#[derive]` / a doc
/// comment do not hide the `def` or `struct` right below them.
pub fn block_label<'a>(lines: &[&'a str]) -> &'a str {
    let mut real = lines.iter().copied().filter(|l| l.chars().any(char::is_alphanumeric));
    let first = real.next().unwrap_or("");
    std::iter::once(first).chain(real).take(6).find(|l| is_definition(l)).unwrap_or(first)
}

/// A block's label and the line it came from. Mid-function blocks get the enclosing definition as a prefix.
pub fn region_label(chunk: &Chunk, a: usize, b: usize) -> (String, usize) {
    let lines: Vec<&str> = (a..=b).map(|n| chunk.text(n)).collect();
    let label = block_label(&lines);
    let at = (a..=b).find(|&n| chunk.text(n) == label).unwrap_or(a);
    (with_owner(chunk, a, label), at)
}

fn with_owner(chunk: &Chunk, a: usize, label: &str) -> String {
    if label.is_empty() || is_definition(label) {
        return label.trim().to_owned();
    }
    let mut depth = indent(label);
    for n in (chunk.ctx_start..a).rev() {
        let line = chunk.text(n);
        if !line.trim().is_empty() && indent(line) < depth {
            if is_definition(line) {
                let owner = line.trim();
                let owner = if owner.chars().count() <= 60 {
                    owner.to_owned()
                } else {
                    format!("{}...", owner.chars().take(57).collect::<String>())
                };
                return format!("{owner} > {}", label.trim());
            }
            depth = indent(line);
        }
    }
    label.trim().to_owned()
}

fn qid(i: usize) -> String {
    format!("q{i}")
}

/// Builds the `state` and `questions` of one request.
pub fn build_request(
    queries: &[String],
    chunk: &Chunk,
    files_only: bool,
    broad: bool,
    rules: Option<&Rules>,
) -> (Value, Map<String, Value>) {
    let width = chunk.end.to_string().len();
    let listing = (chunk.ctx_start..=chunk.end).map(|n| format!("{n:>width$}| {}", chunk.text(n))).collect::<Vec<_>>().join("\n");
    let mut state = Map::new();
    state.insert("task".into(), json!("code search: find where a codebase answers a natural-language query"));
    if let [only] = queries {
        state.insert("query".into(), json!(only));
    } else {
        let named: Map<String, Value> = queries.iter().enumerate().map(|(i, q)| (qid(i), json!(q))).collect();
        state.insert("queries".into(), Value::Object(named));
    }
    state.insert("file".into(), json!(chunk.path));
    state.insert("shown_lines".into(), json!(format!("{}-{}", chunk.ctx_start, chunk.end)));
    state.insert("code".into(), json!(listing));

    let mut questions = Map::new();
    let noul = |instructions: String| json!({"type": "noul", "instructions": instructions});
    // Filter questions are independent of the query, so they are asked once per chunk. The filter
    // text stays out of `state`: state is shared, and there it skews the relevance answers.
    for (t, term) in rules.map(Rules::terms).unwrap_or_default().iter().enumerate() {
        questions.insert(format!("F{t}.S"), noul(filter_section_instruction(term)));
        if !files_only {
            for &(a, b) in &chunk.blocks {
                questions.insert(format!("F{t}.B{a}-{b}"), noul(filter_block_instruction(a, b, term)));
            }
        }
    }
    let askable = if files_only { vec![] } else { chunk.askable() };
    for (i, q) in queries.iter().enumerate() {
        questions.insert(
            format!("q{i}.rel"),
            json!({
                "type": "score",
                "instructions": format!("How relevant is this section of {} to the query: {q}", chunk.path),
                "criteria": RELEVANCE_LEVELS,
            }),
        );
        let which = if queries.len() == 1 { "the query".to_owned() } else { format!("query q{i} ({q})") };
        if !files_only {
            for &(a, b) in &chunk.blocks {
                questions.insert(format!("q{i}.B{a}-{b}"), noul(block_instruction(a, b, &which)));
            }
        }
        for &n in &askable {
            questions.insert(format!("q{i}.L{n}"), noul(line_instruction(n, &which, broad)));
        }
    }
    (Value::Object(state), questions)
}

type Answers = Map<String, Value>;

fn ask_chunk(client: &JevClient, queries: &[String], chunk: Chunk, opts: &Options) -> Result<Vec<(Chunk, Answers)>, JevError> {
    let (state, questions) = build_request(queries, &chunk, opts.files_only, opts.broad, opts.rules.as_ref());
    match client.ask(&state, &questions) {
        Ok(answers) => Ok(vec![(chunk, answers)]),
        Err(JevError::TokenLimit(_)) => {
            let mut out = Vec::new();
            for part in split_chunk(&chunk) {
                out.extend(ask_chunk(client, queries, part, opts)?);
            }
            Ok(out)
        }
        Err(e) => Err(e),
    }
}

/// Runs `job` over `items` on up to `jobs` threads, handing each result to `sink` on the calling
/// thread as it completes. `sink` returns false to stop early.
fn fan_out<T: Send, R: Send>(items: Vec<T>, jobs: usize, job: impl Fn(T) -> R + Sync, mut sink: impl FnMut(R) -> bool) {
    let slots: Vec<std::sync::Mutex<Option<T>>> = items.into_iter().map(|t| std::sync::Mutex::new(Some(t))).collect();
    let (next, stop) = (AtomicUsize::new(0), AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    std::thread::scope(|scope| {
        for _ in 0..jobs.clamp(1, slots.len().max(1)) {
            let (tx, slots, next, stop, job) = (tx.clone(), &slots, &next, &stop, &job);
            scope.spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let Some(slot) = slots.get(next.fetch_add(1, Ordering::Relaxed)) else { break };
                    let Some(item) = slot.lock().unwrap().take() else { break };
                    if tx.send(job(item)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        for result in rx {
            if !sink(result) {
                stop.store(true, Ordering::Relaxed);
                break;
            }
        }
    });
}

fn noul(answers: &Answers, key: &str) -> Option<f64> {
    Some(answers.get(key)?.get("noul").and_then(Value::as_f64).unwrap_or(0.0))
}

/// Cheap pre-filter on file paths alone. Returns path -> P(worth opening).
pub fn triage_paths(client: &JevClient, queries: &[String], paths: &[String], jobs: usize) -> Result<HashMap<String, f64>, JevError> {
    let wanted = queries.join(" | ");
    let batches: Vec<&[String]> = paths.chunks(250).collect();
    let run = |batch: &[String]| -> Result<Vec<(String, f64)>, JevError> {
        let named: Map<String, Value> = batch.iter().enumerate().map(|(i, p)| (format!("p{i}"), json!(p))).collect();
        let state = json!({
            "task": "decide which files are worth opening for a code search",
            "query": wanted,
            "paths": named,
        });
        let questions: Map<String, Value> = batch
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let text = format!("Judging by its path, could the file paths.p{i} ({p}) plausibly contain code relevant to the query?");
                (format!("p{i}"), json!({"type": "noul", "instructions": text}))
            })
            .collect();
        let answers = client.ask(&state, &questions)?;
        Ok(batch
            .iter()
            .enumerate()
            .map(|(i, p)| (p.clone(), answers.get(&format!("p{i}")).and_then(|a| a.get("noul")).and_then(Value::as_f64).unwrap_or(1.0)))
            .collect())
    };
    let mut scores = HashMap::new();
    let mut failure = None;
    fan_out(batches, jobs, run, |part| match part {
        Ok(part) => {
            scores.extend(part);
            true
        }
        Err(e) => {
            failure = Some(e);
            false
        }
    });
    failure.map_or(Ok(scores), Err)
}

/// Returns one ranked FileResult list per query (unfiltered; callers apply thresholds).
pub fn search(
    client: &JevClient,
    queries: &[String],
    files: &[PathBuf],
    opts: &Options,
    mut progress: impl FnMut(usize, usize),
    mut note: impl FnMut(&str),
) -> Result<Vec<Vec<FileResult>>, JevError> {
    let mut named: Vec<(String, &PathBuf)> = files.iter().map(|p| (display_path(p), p)).collect();
    if opts.triage || named.len() > opts.max_files {
        let shown: Vec<String> = named.iter().map(|(d, _)| d.clone()).collect();
        let scores = triage_paths(client, queries, &shown, opts.jobs)?;
        let score = |d: &str| scores.get(d).copied().unwrap_or(1.0);
        let total = named.len();
        named.retain(|(d, _)| score(d) >= opts.triage_threshold);
        named.sort_by(|a, b| score(&b.0).total_cmp(&score(&a.0)));
        named.truncate(opts.max_files);
        note(&format!("path triage kept {} of {total} files", named.len()));
    }

    let rules = opts.rules.as_ref();
    let terms = rules.map(Rules::terms).unwrap_or_default();
    // Roughly one block question per 8 lines.
    let per_line = if opts.files_only { 0.0 } else { queries.len() as f64 + 0.12 * terms.len() as f64 };
    let cfg = Chunking { max_lines: opts.chunk_lines, max_tokens: opts.chunk_tokens, questions_per_line: per_line, ..Chunking::default() };
    let chunks: Vec<Chunk> = named.iter().filter_map(|(shown, path)| Some(chunk_lines(shown, &read_lines(path)?, cfg))).flatten().collect();

    let mut per_query: Vec<HashMap<String, FileResult>> = vec![HashMap::new(); queries.len()];
    let mut section_keeps: HashMap<String, Vec<f64>> = HashMap::new();
    let mut absorb = |chunk: &Chunk, answers: &Answers| {
        // A term applies to a block if either the section or the block itself falls under it:
        // sections decide file kinds (tests, docs), blocks decide constructs (imports, comments).
        let section_p: HashMap<String, f64> =
            terms.iter().enumerate().map(|(k, t)| (t.clone(), noul(answers, &format!("F{k}.S")).unwrap_or(0.5))).collect();
        let section_keep = rules.map_or(1.0, |r| r.passes(&section_p));
        let block_keep: Vec<f64> = chunk
            .blocks
            .iter()
            .map(|&(a, b)| {
                rules.map_or(1.0, |r| {
                    let merged = terms
                        .iter()
                        .enumerate()
                        .map(|(k, t)| (t.clone(), section_p[t].max(noul(answers, &format!("F{k}.B{a}-{b}")).unwrap_or(0.0))))
                        .collect();
                    r.passes(&merged)
                })
            })
            .collect();
        let keep_of = |n: usize| chunk.blocks.iter().position(|&(a, b)| a <= n && n <= b).map_or(section_keep, |at| block_keep[at]);
        section_keeps.entry(chunk.path.clone()).or_default().push(section_keep);
        for (i, table) in per_query.iter_mut().enumerate() {
            let rel = answers.get(&format!("q{i}.rel"));
            let field = |k: &str| rel.and_then(|r| r.get(k)).and_then(Value::as_f64).unwrap_or(0.0);
            let score = field("score") / (RELEVANCE_LEVELS.len() - 1) as f64;
            let fr = table.entry(chunk.path.clone()).or_insert_with(|| FileResult::new(&chunk.path));
            // A filtered-out section cannot carry the file.
            if score >= fr.score && section_keep >= FILTER_GATE {
                fr.score = score;
                fr.confidence = field("confidence");
            }
            for n in chunk.start..=chunk.end {
                if let Some(p) = noul(answers, &format!("q{i}.L{n}")) {
                    fr.lines.push(LineHit { line: n, p, text: chunk.text(n).to_owned(), section: score, keep: keep_of(n) });
                }
            }
            for (at, &(a, b)) in chunk.blocks.iter().enumerate() {
                if let Some(p) = noul(answers, &format!("q{i}.B{a}-{b}")) {
                    let (label, label_line) = region_label(chunk, a, b);
                    fr.blocks.push(BlockHit { start: a, end: b, p, label, label_line, keep: block_keep[at] });
                }
            }
        }
    };

    let total = chunks.len();
    let (mut done, mut errors, mut fatal) = (0, Vec::new(), None);
    fan_out(
        chunks,
        opts.jobs,
        |chunk| ask_chunk(client, queries, chunk, opts),
        |outcome| {
            match outcome {
                Ok(parts) => parts.iter().for_each(|(chunk, answers)| absorb(chunk, answers)),
                Err(e @ JevError::Auth(_)) => {
                    fatal = Some(e);
                    return false;
                }
                Err(e) => errors.push(e),
            }
            done += 1;
            progress(done, total);
            true
        },
    );
    if let Some(e) = fatal {
        return Err(e);
    }
    if let Some(first) = errors.first() {
        note(&format!("{} request(s) failed, results may be incomplete; first error: {first}", errors.len()));
        if errors.len() == total {
            return Err(errors.swap_remove(0));
        }
    }

    Ok(per_query
        .into_iter()
        .map(|table| {
            let mut ranked: Vec<FileResult> = table.into_values().collect();
            for fr in &mut ranked {
                if let Some(keeps) = section_keeps.get(&fr.path).filter(|k| !k.is_empty()) {
                    fr.keep = keeps.iter().sum::<f64>() / keeps.len() as f64;
                }
                fr.lines.sort_by_key(|h| h.line);
                fr.blocks.sort_by_key(|b| b.start);
            }
            ranked.sort_by(|a, b| b.rank().total_cmp(&a.rank()).then_with(|| a.path.cmp(&b.path)));
            ranked
        })
        .collect())
}
