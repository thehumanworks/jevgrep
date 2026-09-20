//! File discovery (gitignore-aware) and line chunking.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use regex_lite::Regex;

pub const SKIP_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
    "dist",
    "build",
    "target",
    ".next",
    ".nuxt",
    ".cache",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".tox",
    "vendor",
    ".idea",
    ".vscode",
    "coverage",
    ".terraform",
];
pub const SKIP_NAMES: &[&str] = &[
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lock",
    "bun.lockb",
    "Cargo.lock",
    "poetry.lock",
    "uv.lock",
    "Pipfile.lock",
    "composer.lock",
    "Gemfile.lock",
    "go.sum",
    "flake.lock",
];
pub const SKIP_SUFFIXES: &[&str] = &[
    ".min.js",
    ".min.css",
    ".map",
    ".png",
    ".jpg",
    ".jpeg",
    ".gif",
    ".webp",
    ".ico",
    ".svg",
    ".pdf",
    ".zip",
    ".gz",
    ".tar",
    ".tgz",
    ".bz2",
    ".xz",
    ".7z",
    ".jar",
    ".war",
    ".class",
    ".so",
    ".dylib",
    ".dll",
    ".exe",
    ".o",
    ".a",
    ".wasm",
    ".pyc",
    ".woff",
    ".woff2",
    ".ttf",
    ".eot",
    ".otf",
    ".mp3",
    ".mp4",
    ".mov",
    ".avi",
    ".webm",
    ".bin",
    ".dat",
    ".db",
    ".sqlite",
    ".parquet",
    ".npy",
    ".npz",
    ".pkl",
    ".onnx",
    ".pt",
    ".safetensors",
    ".snap",
];
pub const MAX_LINE_CHARS: usize = 300;
/// Rough cost of one per-line question, measured against live usage.
pub const QUESTION_TOKENS: f64 = 30.0;

/// A window of a file sent to Jev in one request.
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    /// Display path.
    pub path: String,
    /// 1-based first line included as leading context.
    pub ctx_start: usize,
    /// 1-based first line we ask questions about.
    pub start: usize,
    /// 1-based last line, inclusive.
    pub end: usize,
    /// Text for `ctx_start..=end`.
    pub lines: Vec<String>,
    /// Logical blocks inside `start..=end`.
    pub blocks: Vec<(usize, usize)>,
}

impl Chunk {
    pub fn text(&self, n: usize) -> &str {
        &self.lines[n - self.ctx_start]
    }

    /// Line numbers worth a question: skips blanks and pure punctuation.
    pub fn askable(&self) -> Vec<usize> {
        (self.start..=self.end).filter(|&n| self.text(n).bytes().any(|b| b.is_ascii_alphanumeric())).collect()
    }
}

/// Shell-style match where `*` also crosses `/` (the semantics of Python's fnmatch).
pub fn fnmatch(text: &str, pattern: &str) -> bool {
    let (t, p): (Vec<char>, Vec<char>) = (text.chars().collect(), pattern.chars().collect());
    let (mut ti, mut pi) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        let mut step = None;
        if pi < p.len() {
            match p[pi] {
                '*' => {
                    star = Some((pi, ti));
                    pi += 1;
                    continue;
                }
                '?' => step = Some(pi + 1),
                '[' => match class(&p, pi, t[ti]) {
                    Some((true, next)) => step = Some(next),
                    None if t[ti] == '[' => step = Some(pi + 1),
                    _ => {}
                },
                c if c == t[ti] => step = Some(pi + 1),
                _ => {}
            }
        }
        match (step, star) {
            (Some(next), _) => {
                pi = next;
                ti += 1;
            }
            (None, Some((sp, st))) => {
                pi = sp + 1;
                ti = st + 1;
                star = Some((sp, st + 1));
            }
            (None, None) => return false,
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// Matches a `[...]` class at `p[at]`. Returns (matched, index after the class), or None if unterminated.
fn class(p: &[char], at: usize, c: char) -> Option<(bool, usize)> {
    let mut i = at + 1;
    let negate = i < p.len() && (p[i] == '!' || p[i] == '^');
    if negate {
        i += 1;
    }
    let first = i;
    let mut hit = false;
    while i < p.len() && (p[i] != ']' || i == first) {
        if i + 2 < p.len() && p[i + 1] == '-' && p[i + 2] != ']' {
            hit |= p[i] <= c && c <= p[i + 2];
            i += 3;
        } else {
            hit |= p[i] == c;
            i += 1;
        }
    }
    (i < p.len()).then_some((hit != negate, i + 1))
}

fn matches(rel: &str, patterns: &[String]) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    patterns.iter().any(|p| fnmatch(rel, p) || fnmatch(name, p) || fnmatch(rel, &format!("*/{p}")))
}

fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// Relative to the cwd when the file is inside it, otherwise as the user spelled it.
pub fn display_path(p: &Path) -> String {
    let shown = match std::env::current_dir() {
        Ok(cwd) => match normalize(&cwd.join(p)).strip_prefix(normalize(&cwd)) {
            Ok(rel) if !rel.as_os_str().is_empty() => rel.to_path_buf(),
            _ => p.to_path_buf(),
        },
        Err(_) => p.to_path_buf(),
    };
    shown.to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/")
}

#[derive(Debug, Clone)]
pub struct Discover {
    pub globs: Vec<String>,
    pub excludes: Vec<String>,
    pub max_bytes: u64,
    pub hidden: bool,
    pub no_ignore: bool,
}

impl Default for Discover {
    fn default() -> Self {
        Discover { globs: vec![], excludes: vec![], max_bytes: 512_000, hidden: false, no_ignore: false }
    }
}

fn walk(root: &Path, hidden: bool, honor_ignore: bool) -> Vec<PathBuf> {
    let mut builder = ignore::WalkBuilder::new(root);
    builder
        .hidden(!hidden)
        .ignore(honor_ignore)
        .git_ignore(honor_ignore)
        .git_global(honor_ignore)
        .git_exclude(honor_ignore)
        .parents(honor_ignore)
        .sort_by_file_name(|a, b| a.cmp(b))
        .filter_entry(|e| {
            e.depth() == 0 || !e.file_type().is_some_and(|t| t.is_dir()) || !SKIP_DIRS.contains(&e.file_name().to_string_lossy().as_ref())
        });
    builder
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()) || e.path_is_symlink())
        .map(|e| e.into_path())
        .collect()
}

/// Expands paths into searchable text files, honoring .gitignore inside git repos.
///
/// Returns `Err(path)` for a path that does not exist.
pub fn discover(paths: &[String], opts: &Discover) -> Result<Vec<PathBuf>, String> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut result = Vec::new();
    let default = [".".to_string()];
    for raw in if paths.is_empty() { &default[..] } else { paths } {
        let root = Path::new(raw);
        let (candidates, explicit) = if root.is_file() {
            (vec![root.to_path_buf()], true)
        } else if root.is_dir() {
            // An explicitly named directory is searched even if git ignores all of it.
            let mut found = if opts.no_ignore { vec![] } else { walk(root, opts.hidden, true) };
            if found.is_empty() {
                found = walk(root, opts.hidden, false);
            }
            (found, false)
        } else {
            return Err(raw.clone());
        };
        for p in candidates {
            if !p.is_file() {
                continue;
            }
            let resolved = p.canonicalize().unwrap_or_else(|_| p.clone());
            if seen.contains(&resolved) {
                continue;
            }
            let rel = display_path(&p);
            if !explicit {
                // Skip rules apply below the root the user named, not to the root itself.
                let inner = p.strip_prefix(root).unwrap_or(&p);
                let parts: Vec<String> = inner
                    .components()
                    .filter_map(|c| match c {
                        Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                        _ => None,
                    })
                    .collect();
                let name = parts.last().cloned().unwrap_or_default();
                let lower = name.to_lowercase();
                if parts[..parts.len().saturating_sub(1)].iter().any(|d| SKIP_DIRS.contains(&d.as_str()))
                    || (!opts.hidden && parts.iter().any(|part| part.starts_with('.')))
                    || SKIP_NAMES.contains(&name.as_str())
                    || SKIP_SUFFIXES.iter().any(|s| lower.ends_with(s))
                    || (!opts.globs.is_empty() && !matches(&rel, &opts.globs))
                {
                    continue;
                }
                match p.metadata() {
                    Ok(m) if m.len() > 0 && m.len() <= opts.max_bytes => {}
                    _ => continue,
                }
            }
            if !opts.excludes.is_empty() && matches(&rel, &opts.excludes) {
                continue;
            }
            seen.insert(resolved);
            result.push(p);
        }
    }
    Ok(result)
}

/// Returns the file's lines, or None for binary/unreadable files.
pub fn read_lines(path: &Path) -> Option<Vec<String>> {
    let data = std::fs::read(path).ok()?;
    if data[..data.len().min(8192)].contains(&0) {
        return None;
    }
    Some(String::from_utf8_lossy(&data).lines().map(str::to_owned).collect())
}

pub fn clip(line: &str) -> String {
    let line = line.trim_end();
    match line.char_indices().nth(MAX_LINE_CHARS) {
        Some((at, _)) => format!("{} ...", line.get(..at).unwrap_or(line)),
        None => line.to_owned(),
    }
}

pub fn estimate_tokens(line: &str, questions_per_line: f64) -> usize {
    6 + line.chars().count() / 3 + (QUESTION_TOKENS * questions_per_line).round() as usize
}

/// True when the line opens a definition (function, class, struct, ...) in most languages.
pub fn is_definition(line: &str) -> bool {
    static DEFINITION: OnceLock<Regex> = OnceLock::new();
    DEFINITION
        .get_or_init(|| {
            Regex::new(concat!(
                r"^\s*(?:(?:export|default|pub(?:\([a-z]+\))?|public|private|protected|internal|static|final|abstract|",
                r"async|unsafe|extern|inline|virtual|override|const)\s+)*",
                r"(?:macro_rules!|(?:def|class|fn|func|function|impl|struct|enum|trait|interface|type|module|mod|",
                r"namespace|object|record|sub|proc|procedure|package)\b)",
            ))
            .expect("valid regex")
        })
        .is_match(line)
}

pub fn indent(line: &str) -> usize {
    line.chars().take_while(|c| c.is_whitespace()).count()
}

fn blank(line: &str) -> bool {
    line.trim().is_empty()
}

/// Splits a file into logical blocks, returned as 1-based inclusive (start, end) ranges.
///
/// Language-agnostic, driven by blank lines and indentation. A new block starts at a paragraph
/// (a non-blank line after a blank one) when that line is:
///   - no deeper than the block's first line (next function, class, top-level statement), or
///   - at the block's member level (next method of a class) once the block has `MEMBER_LEN` lines, or
///   - anything at all once the block has `MAX_LEN` lines.
pub fn split_blocks<S: AsRef<str>>(lines: &[S]) -> Vec<(usize, usize)> {
    const MIN_LEN: usize = 6;
    const MEMBER_LEN: usize = 12;
    const MAX_LEN: usize = 40;
    const DEEP: usize = 1_000_000;
    let mut blocks = Vec::new();
    let mut start: Option<usize> = None; // 0-based start of the current block
    let (mut base, mut inner) = (0usize, 0usize); // indent of the block's first line / shallowest nested indent
    for (i, line) in lines.iter().enumerate() {
        let line = line.as_ref();
        if blank(line) {
            continue;
        }
        let ind = indent(line);
        let Some(s) = start else {
            (start, base, inner) = (Some(i), ind, DEEP);
            continue;
        };
        let length = i - s;
        let para = i > 0 && blank(lines[i - 1].as_ref());
        let breaks = para
            && (ind < base
                || (ind <= inner.min(base + 8) && is_definition(line) && (ind <= base || length >= MIN_LEN))
                || (length >= MIN_LEN && (ind <= base || (ind <= inner && length >= MEMBER_LEN) || length >= MAX_LEN)));
        if breaks || length >= MAX_LEN + 10 {
            blocks.push((s + 1, i));
            (start, base, inner) = (Some(i), ind, DEEP);
        } else if ind > base {
            inner = inner.min(ind);
        }
    }
    if let Some(s) = start {
        blocks.push((s + 1, lines.len()));
    }
    for (a, b) in blocks.iter_mut() {
        while *b > *a && blank(lines[*b - 1].as_ref()) {
            *b -= 1;
        }
    }
    blocks
}

#[derive(Debug, Clone, Copy)]
pub struct Chunking {
    pub max_lines: usize,
    pub max_tokens: usize,
    pub context: usize,
    pub questions_per_line: f64,
}

impl Default for Chunking {
    fn default() -> Self {
        Chunking { max_lines: 150, max_tokens: 14_000, context: 12, questions_per_line: 1.0 }
    }
}

/// Packs a file's logical blocks into windows bounded by line count and a token budget.
///
/// Windows break between blocks, so a function is not cut in half unless it alone is too big.
/// Each window carries up to `context` preceding lines so the model can see what encloses it,
/// but questions are only asked about the window itself.
pub fn chunk_lines<S: AsRef<str>>(path: &str, lines: &[S], cfg: Chunking) -> Vec<Chunk> {
    let clipped: Vec<String> = lines.iter().map(|l| clip(l.as_ref())).collect();
    let cost: Vec<usize> = clipped.iter().map(|l| estimate_tokens(l, cfg.questions_per_line)).collect();
    let max_lines = cfg.max_lines.max(1);
    let mut pieces = Vec::new();
    for (a, b) in split_blocks(&clipped) {
        // A block too large for one window is cut into window-sized pieces.
        let mut i = a;
        while i <= b {
            let (mut j, mut budget) = (i, cfg.max_tokens);
            while j <= b && j - i < max_lines && (cost[j - 1] <= budget || j == i) {
                budget = budget.saturating_sub(cost[j - 1]);
                j += 1;
            }
            pieces.push((i, j - 1));
            i = j;
        }
    }
    let mut chunks = Vec::new();
    let mut group: Vec<(usize, usize)> = Vec::new();
    let mut flush = |group: &mut Vec<(usize, usize)>| {
        if let (Some(&(first, _)), Some(&(_, last))) = (group.first(), group.last()) {
            let ctx = first.saturating_sub(cfg.context).max(1);
            chunks.push(Chunk {
                path: path.to_owned(),
                ctx_start: ctx,
                start: first,
                end: last,
                lines: clipped[ctx - 1..last].to_vec(),
                blocks: std::mem::take(group),
            });
        }
    };
    let mut used = 0;
    for (a, b) in pieces {
        let need: usize = cost[a - 1..b].iter().sum();
        if !group.is_empty() && (b - group[0].0 + 1 > max_lines || used + need > cfg.max_tokens) {
            flush(&mut group);
            used = 0;
        }
        group.push((a, b));
        used += need;
    }
    flush(&mut group);
    chunks
}

/// Halves a chunk (used when the API reports the request was too large).
pub fn split_chunk(chunk: &Chunk) -> Vec<Chunk> {
    let span = chunk.end - chunk.start + 1;
    if span < 2 {
        return vec![];
    }
    let mut mid = chunk.start + span / 2;
    if chunk.blocks.len() > 1 {
        // Prefer the block boundary nearest the middle.
        mid = chunk.blocks[1..].iter().map(|&(a, _)| a).min_by_key(|&a| a.abs_diff(mid)).unwrap_or(mid);
    }
    let off = chunk.ctx_start;
    let part = |ctx: usize, start: usize, end: usize| Chunk {
        path: chunk.path.clone(),
        ctx_start: ctx,
        start,
        end,
        lines: chunk.lines[ctx - off..=end - off].to_vec(),
        blocks: chunk.blocks.iter().filter(|&&(a, b)| a <= end && b >= start).map(|&(a, b)| (a.max(start), b.min(end))).collect(),
    };
    vec![part(chunk.ctx_start, chunk.start, mid - 1), part(chunk.start.max(mid.saturating_sub(12)), mid, chunk.end)]
}
