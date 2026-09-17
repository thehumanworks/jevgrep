//! jg command line.

use std::collections::{BTreeSet, HashMap};
use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};
use std::time::Instant;

use serde_json::json;

use crate::client::{resolve_api_key, Config, JevClient, JevError, DEFAULT_BASE_URL, DEFAULT_MODEL};
use crate::files::{discover, Discover};
use crate::filters::{parse_filter, Rules};
use crate::results::{filtered_out, select, select_files, FileView, Limits, Row, WEAK_RATIO};
use crate::search::{search, FileResult, Options};

const USAGE: &str = "usage: jg [options] QUERY [PATH ...]";

const HELP: &str = r#"usage: jg [options] QUERY [PATH ...]

jevgrep: search code with a natural-language query. Every file chunk is scored line by line by
TypeSafe's Jev model, in parallel.

arguments:
  QUERY                   what you are looking for, in plain language
  PATH                    files or directories (default: .)

options:
  -e, --query QUERY       additional query answered in the same pass (repeatable)
  -l, --files             rank relevant files only, no region or line scoring (about 3x cheaper)
  -f, --filter TEXT       plain-language include/exclude rules, e.g. "Source code only. No tests or
                          documentation" (repeatable)
      --only CATEGORY     keep only code in this category, e.g. --only "async code" (repeatable: all must hold)
      --not CATEGORY      drop code in this category, e.g. --not tests --not "generated code" (repeatable)
  -b, --broad             flag every line related to the query, not just the lines that answer it
                          (more recall, more noise)
  -t, --threshold P       min probability for a region or line to count as a match (default 0.5)
  -T, --file-threshold P  file relevance needed to list a file with -l, or to put a file with nothing
                          above -t into the weaker tier (default 0.6)
  -n, --top N             max files per query (default 15)
  -m, --max-lines N       max pinpointed lines per file (default 10)
      --max-regions N     max regions per file (default 5)
  -C, --context N         lines of context around each pinpointed line
  -g, --glob GLOB         only search files matching GLOB (repeatable)
  -x, --exclude GLOB      skip files matching GLOB (repeatable)
  -j, --jobs N            concurrent Jev requests (default 32)
      --json              JSON lines, one object per matching file
      --no-heading        flat output: path:START-END:prob:label for regions, path:LINE:prob:text for
                          lines; matches only, no weaker tier
      --triage            pre-filter files by path with Jev first (automatic above --max-files)
      --max-files N       file cap before path triage kicks in (default 1500)
      --max-filesize BYTES  skip files larger than BYTES (default 512000)
      --chunk-lines N     lines per Jev request (default 150)
      --hidden            include dotfiles
      --no-ignore         do not honor .gitignore
  -q, --quiet             no progress or stats on stderr
      --model MODEL       Jev model (default jev-latest, or $JG_MODEL)
      --base-url URL      API endpoint (default TypeSafe's, or $JG_BASE_URL)
  -V, --version           print the version
  -h, --help              print this help

environment:
  TYPESAFE_API_KEY        API key. When unset, jg tries `fnox get TYPESAFE_API_KEY`.

examples:
  jg "where are retries and backoff handled for HTTP requests"
  jg "how is the session cookie validated" src/ -g "*.ts"
  jg -l "database migration logic"                 # rank files only (cheapest)
  jg "auth token refresh" -e "rate limiting" -e "where config is loaded"   # several queries, one pass
  jg --json "where is the retry budget set" | jq .
  jg "where are requests retried" --filter "Source code only. No tests or documentation"
  jg "where is the timeout set" --not tests --not "command line interface"

filters:
  --filter takes plain language. jg splits it into categories and asks Jev one positive question
  per category ("does this fall under the category: tests?"), then applies only/not in code,
  because Jev answers atomic yes/no questions far more reliably than negated or compound rules.
  Name kinds of thing: "tests", "documentation", "imports", "generated code", "async code".
  Categories can describe files (tests, docs) or code inside them (imports, comments).
  --only / --not skip the parsing and take one category each. jg prints how it read the filter.
  For rules a glob can express (file extensions, directories) use -g / -x: exact, free, faster.

output, one row shape throughout (location, probability, text):
  src/http/client.py  relevance=0.96        file; files are sorted by this number
      494-508  0.96  def _redirect_method(  region: P(these lines contain what you are looking for)
          502  0.93      method = "GET"     line:   P(this line directly answers the query)

Only rows that clear -t are printed, and no source line is printed twice. A line can clear -t
when its surrounding block does not; it is then listed on its own, without a region row. A broad
query ("how does auth work") has no single answering line, so it returns regions only. Files that
look related overall but hold nothing above -t are listed last, under a "weaker" separator, and
only with near misses (at least 0.7 x -t). Probabilities are calibrated: 0.9 means right about 9
times in 10. Raise -t for precision, lower it for recall. Exit status: 0 matches found, 1 none, 2 error.
"#;

#[derive(Debug)]
pub struct Args {
    pub queries: Vec<String>,
    pub paths: Vec<String>,
    pub files: bool,
    pub filter: Vec<String>,
    pub only: Vec<String>,
    pub exclude_terms: Vec<String>,
    pub broad: bool,
    pub threshold: f64,
    pub file_threshold: f64,
    pub top: usize,
    pub max_lines: usize,
    pub max_regions: usize,
    pub context: usize,
    pub glob: Vec<String>,
    pub exclude: Vec<String>,
    pub jobs: usize,
    pub json: bool,
    pub no_heading: bool,
    pub triage: bool,
    pub max_files: usize,
    pub max_filesize: u64,
    pub chunk_lines: usize,
    pub hidden: bool,
    pub no_ignore: bool,
    pub quiet: bool,
    pub model: String,
    pub base_url: String,
}

pub enum Parsed {
    Run(Box<Args>),
    /// Print this to stdout and exit 0 (help, version).
    Print(String),
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_owned())
}

pub fn parse_args(argv: Vec<OsString>) -> Result<Parsed, String> {
    use lexopt::prelude::*;
    let mut a = Args {
        queries: vec![],
        paths: vec![],
        files: false,
        filter: vec![],
        only: vec![],
        exclude_terms: vec![],
        broad: false,
        threshold: 0.5,
        file_threshold: 0.6,
        top: 15,
        max_lines: 10,
        max_regions: 5,
        context: 0,
        glob: vec![],
        exclude: vec![],
        jobs: 32,
        json: false,
        no_heading: false,
        triage: false,
        max_files: 1500,
        max_filesize: 512_000,
        chunk_lines: 150,
        hidden: false,
        no_ignore: false,
        quiet: false,
        model: env_or("JG_MODEL", DEFAULT_MODEL),
        base_url: env_or("JG_BASE_URL", DEFAULT_BASE_URL),
    };
    let (mut query, mut extra): (Option<String>, Vec<String>) = (None, vec![]);
    let mut parser = lexopt::Parser::from_args(argv);
    let run = |parser: &mut lexopt::Parser,
               a: &mut Args,
               query: &mut Option<String>,
               extra: &mut Vec<String>|
     -> Result<Option<String>, lexopt::Error> {
        while let Some(arg) = parser.next()? {
            match arg {
                Short('e') | Long("query") => extra.push(parser.value()?.string()?),
                Short('l') | Long("files") => a.files = true,
                Short('f') | Long("filter") => a.filter.push(parser.value()?.string()?),
                Long("only") => a.only.push(parser.value()?.string()?),
                Long("not") => a.exclude_terms.push(parser.value()?.string()?),
                Short('b') | Long("broad") => a.broad = true,
                Short('t') | Long("threshold") => a.threshold = parser.value()?.parse()?,
                Short('T') | Long("file-threshold") => a.file_threshold = parser.value()?.parse()?,
                Short('n') | Long("top") => a.top = parser.value()?.parse()?,
                Short('m') | Long("max-lines") => a.max_lines = parser.value()?.parse()?,
                Long("max-regions") => a.max_regions = parser.value()?.parse()?,
                Short('C') | Long("context") => a.context = parser.value()?.parse()?,
                Short('g') | Long("glob") => a.glob.push(parser.value()?.string()?),
                Short('x') | Long("exclude") => a.exclude.push(parser.value()?.string()?),
                Short('j') | Long("jobs") => a.jobs = parser.value()?.parse()?,
                Long("json") => a.json = true,
                Long("no-heading") => a.no_heading = true,
                Long("triage") => a.triage = true,
                Long("max-files") => a.max_files = parser.value()?.parse()?,
                Long("max-filesize") => a.max_filesize = parser.value()?.parse()?,
                Long("chunk-lines") => a.chunk_lines = parser.value()?.parse()?,
                Long("hidden") => a.hidden = true,
                Long("no-ignore") => a.no_ignore = true,
                Short('q') | Long("quiet") => a.quiet = true,
                Long("model") => a.model = parser.value()?.string()?,
                Long("base-url") => a.base_url = parser.value()?.string()?,
                Short('V') | Long("version") => return Ok(Some(format!("jg {}\n", env!("CARGO_PKG_VERSION")))),
                Short('h') | Long("help") => return Ok(Some(HELP.to_owned())),
                Value(v) if query.is_none() => *query = Some(v.string()?),
                Value(v) => a.paths.push(v.string()?),
                other => return Err(other.unexpected()),
            }
        }
        Ok(None)
    };
    if let Some(text) = run(&mut parser, &mut a, &mut query, &mut extra).map_err(|e| e.to_string())? {
        return Ok(Parsed::Print(text));
    }
    let query = query.ok_or("the following arguments are required: QUERY")?;
    a.queries = std::iter::once(query).chain(extra).map(|q| q.trim().to_owned()).filter(|q| !q.is_empty()).collect();
    Ok(Parsed::Run(Box::new(a)))
}

fn file_lines(path: &str) -> Vec<String> {
    std::fs::read(path).map(|d| String::from_utf8_lossy(&d).lines().map(str::to_owned).collect()).unwrap_or_default()
}

fn cut(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        text.to_owned()
    } else {
        format!("{}...", text.chars().take(width - 3).collect::<String>())
    }
}

fn r4(x: f64) -> f64 {
    (x * 1e4).round() / 1e4
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn render_files(query: &str, files: &[&FileResult], as_json: bool, out: &mut dyn Write) -> io::Result<()> {
    for fr in files {
        if as_json {
            let row = json!({"query": query, "path": fr.path, "relevance": r4(fr.score), "confidence": r4(fr.confidence)});
            writeln!(out, "{row}")?;
        } else {
            writeln!(out, "{:.2}  {}", fr.score, fr.path)?;
        }
    }
    Ok(())
}

/// One row shape throughout: location, probability, text. Regions are `a-b`, lines are `n`.
///
/// Every printed probability clears its bar and no source line is printed twice; see results.rs.
pub fn render_text(views: &[FileView], args: &Args, out: &mut dyn Write) -> io::Result<()> {
    let mut announced = false;
    for v in views {
        if args.no_heading {
            if v.weak {
                continue; // flat rows cannot carry the "weaker" caveat, so they carry only real matches
            }
            for row in v.rows() {
                let hits = match row {
                    Row::Region(r) => {
                        writeln!(out, "{}:{}-{}:{:.2}:{}", v.path, r.start, r.end, r.p, r.label)?;
                        r.lines.iter().collect::<Vec<_>>()
                    }
                    Row::Line(h) => vec![h],
                };
                for h in hits {
                    writeln!(out, "{}:{}:{:.2}:{}", v.path, h.line, h.p, h.text)?;
                }
            }
            continue;
        }
        if v.weak && !announced {
            announced = true;
            writeln!(
                out,
                "-- weaker: these files look related overall, but nothing in them reached {:.2}; near misses (>= {:.2}) shown\n",
                args.threshold,
                WEAK_RATIO * args.threshold
            )?;
        }
        writeln!(out, "{}  relevance={:.2}", v.path, v.relevance)?;
        let source = if args.context > 0 { file_lines(&v.path) } else { vec![] };
        let mut printed: BTreeSet<usize> = BTreeSet::new();
        for row in v.rows() {
            let hits: HashMap<usize, &crate::search::LineHit> = match row {
                Row::Region(r) => {
                    writeln!(out, "{:>11}  {:.2}  {}", format!("{}-{}", r.start, r.end), r.p, cut(&r.label, 110))?;
                    printed.insert(r.label_line);
                    r.lines.iter().map(|h| (h.line, h)).collect()
                }
                Row::Line(h) => HashMap::from([(h.line, h)]),
            };
            let around: BTreeSet<usize> = hits.keys().flat_map(|&n| n.saturating_sub(args.context).max(1)..=n + args.context).collect();
            for n in around {
                if !printed.insert(n) {
                    continue;
                }
                if let Some(h) = hits.get(&n) {
                    writeln!(out, "{n:>11}  {:.2}  {}", h.p, cut(&h.text, 200))?;
                } else if let Some(line) = source.get(n - 1).filter(|l| !l.trim().is_empty()) {
                    writeln!(out, "{n:>11}        {}", cut(line.trim_end(), 200))?;
                }
            }
        }
        let mut more = Vec::new();
        if v.more_regions > 0 {
            more.push(format!("{} more region{} (raise --max-regions)", v.more_regions, plural(v.more_regions)));
        }
        if v.more_lines > 0 {
            more.push(format!("{} more line{} (raise -m)", v.more_lines, plural(v.more_lines)));
        }
        if !more.is_empty() {
            writeln!(out, "{:>11}  not shown: {}", "", more.join(", "))?;
        }
        writeln!(out)?;
    }
    Ok(())
}

pub fn render_json(query: &str, views: &[FileView], out: &mut dyn Write) -> io::Result<()> {
    let line = |h: &crate::search::LineHit| json!({"line": h.line, "p": r4(h.p), "text": h.text});
    for v in views {
        let regions: Vec<_> = v
            .regions
            .iter()
            .map(|r| {
                json!({
                    "start": r.start, "end": r.end, "p": r4(r.p), "label": r.label, "label_line": r.label_line,
                    "lines": r.lines.iter().map(line).collect::<Vec<_>>(),
                })
            })
            .collect();
        let row = json!({
            "query": query,
            "path": v.path,
            "relevance": r4(v.relevance),
            "section_relevance": r4(v.section_relevance),
            "confidence": r4(v.confidence),
            "match": if v.weak { "weak" } else { "strong" },
            "regions": regions,
            "lines": v.lines.iter().map(line).collect::<Vec<_>>(),
            "more_regions": v.more_regions,
            "more_lines": v.more_lines,
        });
        writeln!(out, "{row}")?;
    }
    Ok(())
}

/// Runs jg and returns the exit status: 0 matches found, 1 none, 2 error.
pub fn run(argv: Vec<OsString>) -> i32 {
    let args = match parse_args(argv) {
        Ok(Parsed::Run(args)) => args,
        Ok(Parsed::Print(text)) => {
            let _ = io::stdout().write_all(text.as_bytes());
            return 0;
        }
        Err(e) => {
            eprintln!("{USAGE}\njg: error: {e} (see jg --help)");
            return 2;
        }
    };
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    match execute(&args, &mut out).and_then(|code| out.flush().map(|()| code)) {
        Ok(code) => code,
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => 0, // e.g. `jg ... | head`
        Err(e) => {
            eprintln!("jg: {e}");
            2
        }
    }
}

fn execute(args: &Args, out: &mut dyn Write) -> io::Result<i32> {
    if args.queries.is_empty() {
        eprintln!("jg: empty query");
        return Ok(2);
    }
    let tty = io::stderr().is_terminal() && !args.quiet;
    let started = Instant::now();
    let note = |msg: &str| {
        if !args.quiet {
            eprintln!("{}jg: {msg}", if tty { "\r\x1b[K" } else { "" });
        }
    };

    let found = discover(
        &args.paths,
        &Discover {
            globs: args.glob.clone(),
            excludes: args.exclude.clone(),
            max_bytes: args.max_filesize,
            hidden: args.hidden,
            no_ignore: args.no_ignore,
        },
    );
    let files = match found {
        Ok(files) => files,
        Err(path) => {
            eprintln!("jg: {path}: no such file or directory");
            return Ok(2);
        }
    };
    if files.is_empty() {
        if !args.quiet {
            eprintln!("jg: no searchable files");
        }
        return Ok(1);
    }

    let mut rules = Rules { only: args.only.iter().map(|t| vec![t.clone()]).collect(), exclude: args.exclude_terms.clone() };
    for text in &args.filter {
        rules = rules.merge(parse_filter(text));
    }
    let asked = !(args.filter.is_empty() && args.only.is_empty() && args.exclude_terms.is_empty());
    if asked && rules.is_empty() {
        eprintln!("jg: the filter names no category; try e.g. --filter \"no tests\"");
        return Ok(2);
    }
    if !rules.is_empty() {
        note(&format!("filter: {}", rules.describe()));
    }

    let opts = Options {
        files_only: args.files,
        broad: args.broad,
        jobs: args.jobs.max(1),
        chunk_lines: args.chunk_lines,
        max_files: args.max_files,
        triage: args.triage,
        rules: (!rules.is_empty()).then(|| rules.clone()),
        ..Options::default()
    };
    let progress = |done: usize, total: usize| {
        if tty {
            eprint!("\rjg: {done}/{total} chunks");
        }
    };
    let searched = resolve_api_key().and_then(|key| {
        let cfg = Config { base_url: args.base_url.clone(), model: args.model.clone(), pool_size: args.jobs.max(8), ..Config::default() };
        let client = JevClient::new(&key, cfg);
        search(&client, &args.queries, &files, &opts, progress, note).map(|ranked| (ranked, client))
    });
    let (ranked, client) = match searched {
        Ok(done) => done,
        Err(e @ JevError::Auth(_)) => {
            eprintln!("{}jg: {e}", if tty { "\r\x1b[K" } else { "" });
            return Ok(2);
        }
        Err(e) => {
            eprintln!("{}jg: TypeSafe API error: {e}", if tty { "\r\x1b[K" } else { "" });
            return Ok(2);
        }
    };

    if tty {
        eprint!("\r\x1b[K"); // wipe the progress line so results start on a clean row
    }
    let limits = Limits {
        threshold: args.threshold,
        file_threshold: args.file_threshold,
        top: args.top,
        max_regions: args.max_regions,
        max_lines: args.max_lines,
    };
    let mut any = false;
    for (query, results) in args.queries.iter().zip(&ranked) {
        let heading = args.queries.len() > 1 && !args.json;
        if heading {
            writeln!(out, "== {query}")?;
        }
        let shown = if args.files {
            let picked = select_files(results, args.file_threshold, args.top);
            render_files(query, &picked, args.json, out)?;
            picked.len()
        } else {
            let picked = select(results, &limits);
            if args.json {
                render_json(query, &picked, out)?;
                picked.len()
            } else {
                render_text(&picked, args, out)?;
                picked.iter().filter(|v| !(args.no_heading && v.weak)).count()
            }
        };
        any |= shown > 0;
        if heading && shown == 0 {
            writeln!(out, "(no matches)\n")?;
        } else if heading && args.files {
            writeln!(out)?;
        }
    }
    out.flush()?;

    if !rules.is_empty() && !args.quiet && !args.files {
        let (regions, lines) =
            ranked.iter().map(|results| filtered_out(results, args.threshold)).fold((0, 0), |acc, g| (acc.0 + g.0, acc.1 + g.1));
        eprintln!("jg: filter removed {regions} matching region{} and {lines} line{}", plural(regions), plural(lines));
    }
    if !args.quiet {
        let usage = &client.usage;
        let retries = if usage.retries() > 0 { format!(", {} retries", usage.retries()) } else { String::new() };
        eprintln!(
            "jg: {} files, {} requests{retries}, {} tokens (~${:.4}), {:.1}s",
            files.len(),
            usage.requests(),
            thousands(usage.input_tokens()),
            usage.cost_usd(),
            started.elapsed().as_secs_f64()
        );
    }
    Ok(if any { 0 } else { 1 })
}
