//! Declarative command-line grammar and normalization, independent of execution.

use std::ffi::OsString;

use clap::{error::ErrorKind, ColorChoice, Parser, ValueEnum};

use crate::chatgpt::{CHATGPT_MODEL, DEFAULT_CHATGPT_URL};
use crate::client::{DEFAULT_BASE_URL, DEFAULT_MODEL};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum BackendKind {
    #[default]
    Jev,
    Chatgpt,
}

/// Application presentation policy. Clap's own help and errors always remain plain.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum ColorMode {
    #[default]
    Auto,
    Always,
    Never,
}

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
    pub backend: BackendKind,
    pub chatgpt_login: bool,
    pub model: String,
    pub base_url: String,
    pub color: ColorMode,
}

#[derive(Debug)]
pub enum Parsed {
    Run(Box<Args>),
    /// Print this to stdout and exit 0 (help, version).
    Print(String),
}

const EXTENDED_HELP: &str = r#"environment:
  TYPESAFE_API_KEY        API key. When unset, jg tries `fnox get TYPESAFE_API_KEY`.
  JG_NO_FNOX             When set, disable the fnox key lookup.
  JG_BACKEND             Backend when --backend is absent: jev (default) or chatgpt.
  CHATGPT_ACCOUNT_ID     ChatGPT account; set together with CHATGPT_ACCESS_TOKEN.
  CHATGPT_ACCESS_TOKEN   ChatGPT subscription access token (not an OpenAI API key).
  CODEX_HOME             Codex cache directory (default: ~/.codex); reads auth.json.
                         ChatGPT also checks $XDG_CONFIG_HOME/auth.toml (~/.config).
  JG_MODEL               Model when --model is absent; empty means the default.
  JG_BASE_URL            Endpoint when --base-url is absent; empty means the default.
  JG_DEBUG               When set, log retry reasons.
  NO_COLOR               Nonempty disables application styling in auto mode.
  TERM=dumb              Disables application styling in auto mode and progress.
  CLICOLOR_FORCE         Nonempty and not 0 forces auto styling unless disabled above.
  CLICOLOR=0             Disables auto styling unless CLICOLOR_FORCE enables it.
  --color always/never   Overrides color environment settings for human output only.
                         JSON, flat output, help and usage errors remain plain.

examples:
  jg "where are retries and backoff handled for HTTP requests"
  jg "how is the session cookie validated" src/ -g "*.ts"
  jg -l "database migration logic"                 # rank files only (cheapest)
  jg "auth token refresh" -e "rate limiting" -e "where config is loaded"
  jg --json "where is the retry budget set" | jq .
  jg "where are requests retried" --filter "Source code only. No tests or documentation"
  jg "where is the timeout set" --not tests --not "command line interface"
  jg --backend chatgpt "where are retries handled" src/
  jg --backend chatgpt --chatgpt-login "where is configuration loaded"

chatgpt:
  Sends Responses requests directly using your ChatGPT subscription. Uses gpt-5.6-luna,
  requests priority (fast) service, and returns the same output format as Jev.
  Credential order: environment pair, auth.toml, then ~/.codex/auth.json (or CODEX_HOME).
  --chatgpt-login explicitly runs Codex device login; inference never launches Codex.
  Scores are model estimates; Jev's calibration and per-token pricing do not apply.

jev:
  Default endpoint: https://api.typesafe.ai/v1/systemone

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
only with near misses (at least 0.7 x -t). Jev probabilities are calibrated; ChatGPT scores are
estimates. Raise -t for precision, lower it for recall. Exit status: 0 matches found, 1 none, 2 error.
"#;

#[derive(Debug, Parser)]
#[command(
    name = "jg",
    version,
    about = "jevgrep: search code with a natural-language query. Score files, regions and lines with Jev or ChatGPT, in parallel.",
    color = ColorChoice::Never,
    term_width = 100,
    args_override_self = true,
    after_help = EXTENDED_HELP
)]
struct CommandLine {
    /// What you are looking for, in plain language
    #[arg(value_name = "QUERY")]
    query: String,
    /// Files or directories (default: .)
    #[arg(value_name = "PATH")]
    paths: Vec<String>,
    /// Additional query answered in the same pass (repeatable)
    #[arg(short = 'e', long = "query", value_name = "QUERY", allow_hyphen_values = true)]
    extra: Vec<String>,
    /// Rank relevant files only, no region or line scoring (about 3x cheaper)
    #[arg(short = 'l', long)]
    files: bool,
    /// Plain-language include/exclude rules, e.g. "Source code only. No tests" (repeatable)
    #[arg(short = 'f', long, value_name = "TEXT", allow_hyphen_values = true)]
    filter: Vec<String>,
    /// Keep only code in this category (repeatable: all must hold)
    #[arg(long, value_name = "CATEGORY", allow_hyphen_values = true)]
    only: Vec<String>,
    /// Drop code in this category, e.g. --not tests (repeatable)
    #[arg(long = "not", value_name = "CATEGORY", allow_hyphen_values = true)]
    exclude_terms: Vec<String>,
    /// Flag every line related to the query, not just answers (more recall, more noise)
    #[arg(short = 'b', long)]
    broad: bool,
    /// Min probability for a region or line to count as a match
    #[arg(short = 't', long, value_name = "P", default_value_t = 0.5, value_parser = probability, allow_hyphen_values = true)]
    threshold: f64,
    /// File relevance needed for -l or the weaker tier when nothing clears -t
    #[arg(short = 'T', long, value_name = "P", default_value_t = 0.6, value_parser = probability, allow_hyphen_values = true)]
    file_threshold: f64,
    /// Max files per query
    #[arg(short = 'n', long, value_name = "N", default_value_t = 15, allow_hyphen_values = true)]
    top: usize,
    /// Max pinpointed lines per file
    #[arg(short = 'm', long, value_name = "N", default_value_t = 10, allow_hyphen_values = true)]
    max_lines: usize,
    /// Max regions per file
    #[arg(long, value_name = "N", default_value_t = 5, allow_hyphen_values = true)]
    max_regions: usize,
    /// Lines of context around each pinpointed line
    #[arg(short = 'C', long, value_name = "N", default_value_t = 0, allow_hyphen_values = true)]
    context: usize,
    /// Only search files matching GLOB (repeatable)
    #[arg(short = 'g', long, value_name = "GLOB", allow_hyphen_values = true)]
    glob: Vec<String>,
    /// Skip files matching GLOB (repeatable)
    #[arg(short = 'x', long, value_name = "GLOB", allow_hyphen_values = true)]
    exclude: Vec<String>,
    /// Concurrent requests
    #[arg(short = 'j', long, value_name = "N", default_value_t = 32, value_parser = positive_usize, allow_hyphen_values = true)]
    jobs: usize,
    /// JSON lines, one object per matching file
    #[arg(long)]
    json: bool,
    /// Flat output: path:START-END:prob:label or path:LINE:prob:text; no weaker tier
    #[arg(long)]
    no_heading: bool,
    /// Pre-filter files by path first (automatic above --max-files)
    #[arg(long)]
    triage: bool,
    /// File cap before path triage kicks in
    #[arg(long, value_name = "N", default_value_t = 1500, allow_hyphen_values = true)]
    max_files: usize,
    /// Skip files larger than BYTES
    #[arg(long, value_name = "BYTES", default_value_t = 512_000, allow_hyphen_values = true)]
    max_filesize: u64,
    /// Most lines per request (ChatGPT may use fewer)
    #[arg(long, value_name = "N", default_value_t = 150, value_parser = positive_usize, allow_hyphen_values = true)]
    chunk_lines: usize,
    /// Include dotfiles
    #[arg(long)]
    hidden: bool,
    /// Do not honor .gitignore
    #[arg(long)]
    no_ignore: bool,
    /// No progress or stats on stderr
    #[arg(short = 'q', long)]
    quiet: bool,
    /// Decision backend (default: jev, or $JG_BACKEND)
    #[arg(long, value_name = "BACKEND", value_enum)]
    backend: Option<BackendKind>,
    /// Run Codex device login for ChatGPT credentials before searching
    #[arg(long)]
    chatgpt_login: bool,
    /// Model (Jev: jev-latest or $JG_MODEL; ChatGPT: gpt-5.6-luna only)
    #[arg(long, value_name = "MODEL", allow_hyphen_values = true)]
    model: Option<String>,
    /// Override the selected backend's endpoint (or $JG_BASE_URL)
    #[arg(long, value_name = "URL", allow_hyphen_values = true)]
    base_url: Option<String>,
    /// Color eligible human output; JSON, flat output, help and usage errors remain plain
    #[arg(long, value_name = "WHEN", value_enum, default_value = "auto", allow_hyphen_values = true)]
    color: ColorMode,
}

fn probability(value: &str) -> Result<f64, String> {
    let parsed = value.parse::<f64>().map_err(|_| "expected a finite probability in [0, 1]".to_owned())?;
    if parsed.is_finite() && (0.0..=1.0).contains(&parsed) {
        Ok(parsed)
    } else {
        Err("expected a finite probability in [0, 1]".to_owned())
    }
}

fn positive_usize(value: &str) -> Result<usize, String> {
    let parsed = value.parse::<usize>().map_err(|_| "expected a positive integer".to_owned())?;
    if parsed > 0 {
        Ok(parsed)
    } else {
        Err("expected a positive integer".to_owned())
    }
}

/// Parse argv without an executable name. Errors are complete plain clap diagnostics.
pub fn parse_args(argv: Vec<OsString>) -> Result<Parsed, String> {
    parse_args_with_env(argv, |name| std::env::var(name).ok())
}

// Inject the only environment reads, rather than changing process-global state in tests.
fn parse_args_with_env(argv: Vec<OsString>, mut env: impl FnMut(&str) -> Option<String>) -> Result<Parsed, String> {
    let raw = match CommandLine::try_parse_from(std::iter::once(OsString::from("jg")).chain(argv)) {
        Ok(raw) => raw,
        Err(error) if matches!(error.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) => {
            return Ok(Parsed::Print(error.to_string()));
        }
        Err(error) => return Err(error.to_string()),
    };
    let backend = match raw.backend {
        Some(backend) => backend,
        None => match env("JG_BACKEND").filter(|v| !v.is_empty()) {
            Some(value) => BackendKind::from_str(&value, false).map_err(|_| "error: JG_BACKEND must be jev or chatgpt\n".to_owned())?,
            None => BackendKind::default(),
        },
    };
    let (default_model, default_url) = match backend {
        BackendKind::Jev => (DEFAULT_MODEL, DEFAULT_BASE_URL),
        BackendKind::Chatgpt => (CHATGPT_MODEL, DEFAULT_CHATGPT_URL),
    };
    let model = raw.model.unwrap_or_else(|| env("JG_MODEL").filter(|v| !v.is_empty()).unwrap_or_else(|| default_model.to_owned()));
    if backend == BackendKind::Chatgpt && model != CHATGPT_MODEL {
        return Err(format!("error: the ChatGPT backend requires model {CHATGPT_MODEL}; remove --model or JG_MODEL\n"));
    }
    if raw.chatgpt_login && backend != BackendKind::Chatgpt {
        return Err("error: --chatgpt-login requires --backend chatgpt\n".to_owned());
    }
    let base_url = raw.base_url.unwrap_or_else(|| env("JG_BASE_URL").filter(|v| !v.is_empty()).unwrap_or_else(|| default_url.to_owned()));
    Ok(Parsed::Run(Box::new(Args {
        queries: std::iter::once(raw.query).chain(raw.extra).map(|q| q.trim().to_owned()).filter(|q| !q.is_empty()).collect(),
        paths: raw.paths,
        files: raw.files,
        filter: raw.filter,
        only: raw.only,
        exclude_terms: raw.exclude_terms,
        broad: raw.broad,
        threshold: raw.threshold,
        file_threshold: raw.file_threshold,
        top: raw.top,
        max_lines: raw.max_lines,
        max_regions: raw.max_regions,
        context: raw.context,
        glob: raw.glob,
        exclude: raw.exclude,
        jobs: raw.jobs,
        json: raw.json,
        no_heading: raw.no_heading,
        triage: raw.triage,
        max_files: raw.max_files,
        max_filesize: raw.max_filesize,
        chunk_lines: raw.chunk_lines,
        hidden: raw.hidden,
        no_ignore: raw.no_ignore,
        quiet: raw.quiet,
        backend,
        chatgpt_login: raw.chatgpt_login,
        model,
        base_url,
        color: raw.color,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn argv(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    fn parse(args: &[&str]) -> Result<Parsed, String> {
        parse_args_with_env(argv(args), |_| None)
    }

    fn run(args: &[&str]) -> Box<Args> {
        match parse(args).unwrap() {
            Parsed::Run(args) => args,
            Parsed::Print(text) => panic!("unexpected printed output: {text}"),
        }
    }

    fn printed(args: &[&str]) -> String {
        match parse(args).unwrap() {
            Parsed::Print(text) => text,
            Parsed::Run(_) => panic!("expected printed output"),
        }
    }

    #[test]
    fn command_definition_is_consistent() {
        CommandLine::command().debug_assert();
    }

    #[test]
    fn every_default_matches_the_baseline() {
        let a = run(&["q"]);
        assert_eq!(a.queries, ["q"]);
        assert!(a.paths.is_empty()); // Discovery interprets this as the current directory.
        assert!(!a.files && !a.broad && !a.json && !a.no_heading && !a.triage && !a.hidden && !a.no_ignore && !a.quiet);
        assert!(a.filter.is_empty() && a.only.is_empty() && a.exclude_terms.is_empty() && a.glob.is_empty() && a.exclude.is_empty());
        assert_eq!((a.threshold, a.file_threshold), (0.5, 0.6));
        assert_eq!((a.top, a.max_lines, a.max_regions, a.context), (15, 10, 5, 0));
        assert_eq!((a.jobs, a.max_files, a.max_filesize, a.chunk_lines), (32, 1500, 512_000, 150));
        assert_eq!((a.model.as_str(), a.base_url.as_str()), (DEFAULT_MODEL, DEFAULT_BASE_URL));
        assert_eq!(a.color, ColorMode::Auto);
        assert_eq!(a.backend, BackendKind::Jev);
        assert!(!a.chatgpt_login);
        assert_eq!(ColorMode::default(), ColorMode::Auto);
    }

    #[test]
    fn every_option_and_alias_is_in_the_command_definition() {
        let mut command = CommandLine::command();
        command.build();
        let mut actual: Vec<_> = command.get_arguments().filter_map(|arg| arg.get_long().map(|long| (long, arg.get_short()))).collect();
        let mut expected = vec![
            ("query", Some('e')),
            ("files", Some('l')),
            ("filter", Some('f')),
            ("only", None),
            ("not", None),
            ("broad", Some('b')),
            ("threshold", Some('t')),
            ("file-threshold", Some('T')),
            ("top", Some('n')),
            ("max-lines", Some('m')),
            ("max-regions", None),
            ("context", Some('C')),
            ("glob", Some('g')),
            ("exclude", Some('x')),
            ("jobs", Some('j')),
            ("json", None),
            ("no-heading", None),
            ("triage", None),
            ("max-files", None),
            ("max-filesize", None),
            ("chunk-lines", None),
            ("hidden", None),
            ("no-ignore", None),
            ("quiet", Some('q')),
            ("backend", None),
            ("chatgpt-login", None),
            ("model", None),
            ("base-url", None),
            ("color", None),
            ("version", Some('V')),
            ("help", Some('h')),
        ];
        actual.sort_unstable();
        expected.sort_unstable();
        assert_eq!(actual, expected);
    }

    #[test]
    fn collections_append_and_queries_normalize_in_the_original_order() {
        let a = run(&[
            "-e",
            " before ",
            " main ",
            "one",
            "--query=after",
            "two",
            "-e",
            " \t ",
            "-f",
            "first",
            "--filter",
            "second",
            "--only",
            "a",
            "--only=b",
            "--not",
            "c",
            "--not=d",
            "-g",
            "*.rs",
            "--glob=*.py",
            "-x",
            "a/*",
            "--exclude=b/*",
        ]);
        assert_eq!(a.queries, ["main", "before", "after"]);
        assert_eq!(a.paths, ["one", "two"]);
        assert_eq!(a.filter, ["first", "second"]);
        assert_eq!(a.only, ["a", "b"]);
        assert_eq!(a.exclude_terms, ["c", "d"]);
        assert_eq!(a.glob, ["*.rs", "*.py"]);
        assert_eq!(a.exclude, ["a/*", "b/*"]);
    }

    #[test]
    fn query_is_required_even_with_extras_and_empty_queries_are_removed() {
        for args in [&[][..], &["-e", "extra"][..], &["--quiet"][..]] {
            assert!(parse(args).unwrap_err().contains("QUERY"));
        }
        for query in ["", " \n\t "] {
            assert!(run(&[query]).queries.is_empty());
            assert_eq!(run(&[query, "-e", " extra "]).queries, ["extra"]);
        }
        assert_eq!(run(&["\u{2003}query\u{2003}", " path "]).queries, ["query"]);
        assert_eq!(run(&["q", " path "]).paths, [" path "]);
    }

    #[test]
    fn scalar_options_use_last_value_including_mixed_aliases() {
        let a = run(&[
            "q",
            "-t0.1",
            "--threshold=0.9",
            "-T0.2",
            "--file-threshold=0.8",
            "-n1",
            "--top=2",
            "-m3",
            "--max-lines=4",
            "--max-regions=5",
            "--max-regions=6",
            "-C7",
            "--context=8",
            "-j9",
            "--jobs=10",
            "--max-files=11",
            "--max-files=12",
            "--max-filesize=13",
            "--max-filesize=14",
            "--chunk-lines=15",
            "--chunk-lines=16",
            "--model=first",
            "--model=second",
            "--base-url=first",
            "--base-url=second",
            "--color=always",
            "--color=never",
        ]);
        assert_eq!((a.threshold, a.file_threshold), (0.9, 0.8));
        assert_eq!((a.top, a.max_lines, a.max_regions, a.context), (2, 4, 6, 8));
        assert_eq!((a.jobs, a.max_files, a.max_filesize, a.chunk_lines), (10, 12, 14, 16));
        assert_eq!((a.model.as_str(), a.base_url.as_str()), ("second", "second"));
        assert_eq!(a.color, ColorMode::Never);
    }

    #[test]
    fn invalid_scalar_occurrences_are_not_hidden_by_later_overrides_or_help() {
        for (flag, invalid, valid) in [("--top", "bad", "3"), ("--threshold", "NaN", "0.5"), ("--jobs", "0", "2")] {
            assert!(parse(&["q", flag, invalid, flag, valid]).is_err());
            assert!(parse(&["q", flag, invalid, "--help"]).is_err());
        }
    }

    #[test]
    fn empty_nonquery_strings_and_single_dashes_are_not_rewritten() {
        let a = run(&["-", "", "-e", "", "--filter=", "--only=", "--not=", "--glob=", "--exclude=", "--model=", "--base-url="]);
        assert_eq!(a.queries, ["-"]);
        assert_eq!(a.paths, [""]);
        assert_eq!(a.filter, [""]);
        assert_eq!(a.only, [""]);
        assert_eq!(a.exclude_terms, [""]);
        assert_eq!(a.glob, [""]);
        assert_eq!(a.exclude, [""]);
        assert!(a.model.is_empty() && a.base_url.is_empty());
    }

    #[test]
    fn booleans_repeat_and_output_modes_do_not_conflict() {
        let a = run(&[
            "-llbbqq",
            "q",
            "--files",
            "--broad",
            "--quiet",
            "--json",
            "--json",
            "--no-heading",
            "--no-heading",
            "--triage",
            "--triage",
            "--hidden",
            "--hidden",
            "--no-ignore",
            "--no-ignore",
        ]);
        assert!(a.files && a.broad && a.quiet && a.json && a.no_heading && a.triage && a.hidden && a.no_ignore);
    }

    #[test]
    fn grammar_preserves_clusters_attached_values_and_interspersed_options() {
        let a = run(&["-lq", "-t0.99", "--top=3", "q", "one", "-C2", "two", "-j4"]);
        assert!(a.files && a.quiet);
        assert_eq!((a.threshold, a.top, a.context, a.jobs), (0.99, 3, 2, 4));
        assert_eq!(a.paths, ["one", "two"]);
        let a = run(&["--", "-query", "-path", "--json"]);
        assert_eq!(a.queries, ["-query"]);
        assert_eq!(a.paths, ["-path", "--json"]);
        assert!(!a.json);
        assert_eq!(run(&["q", "--", "-path"]).paths, ["-path"]);
    }

    #[test]
    fn option_values_may_start_with_hyphens_even_when_they_look_like_flags() {
        let a = run(&[
            "q",
            "-e",
            "--help",
            "-f",
            "-filter",
            "--only",
            "-only",
            "--not",
            "-not",
            "-g",
            "-glob",
            "-x",
            "-exclude",
            "--model",
            "--version",
            "--base-url",
            "--",
        ]);
        assert_eq!(a.queries, ["q", "--help"]);
        assert_eq!(a.filter, ["-filter"]);
        assert_eq!(a.only, ["-only"]);
        assert_eq!(a.exclude_terms, ["-not"]);
        assert_eq!(a.glob, ["-glob"]);
        assert_eq!(a.exclude, ["-exclude"]);
        assert_eq!((a.model.as_str(), a.base_url.as_str()), ("--version", "--"));
        assert!(parse(&["-query"]).is_err());
    }

    #[test]
    fn missing_values_unknown_options_and_extra_boolean_values_are_errors() {
        let command = CommandLine::command();
        for arg in command.get_arguments().filter(|a| a.get_action().takes_values()) {
            if let Some(long) = arg.get_long() {
                let flag = format!("--{long}");
                let error = parse(&["q", &flag]).unwrap_err();
                assert!(error.contains(&flag), "{error}");
            }
        }
        for args in [&["q", "--bogus"][..], &["q", "--json=true"][..], &["q", "--color=ALWAYS"][..]] {
            assert!(parse(args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn probabilities_must_be_finite_and_inclusive_unit_interval() {
        for flag in ["-t", "--threshold", "-T", "--file-threshold"] {
            for value in ["0", "1", "0.5", "-0", "+0.5", "1e-1"] {
                assert!(parse(&["q", flag, value]).is_ok(), "{flag} {value}");
            }
            for value in ["-0.1", "1.1", "NaN", "nan", "inf", "+inf", "-inf", "infinity", "-Infinity", "1e999", "high", "", " 0.5"] {
                let error = parse(&["q", flag, value]).unwrap_err();
                assert!(error.contains("finite probability"), "{flag} {value}: {error}");
                assert!(!error.contains('\u{1b}'));
            }
        }
    }

    #[test]
    fn only_jobs_and_chunk_size_reject_zero() {
        for flag in ["-j", "--jobs", "--chunk-lines"] {
            for value in ["0", "-1", "1.5", "many", "", "999999999999999999999999999999999999"] {
                assert!(parse(&["q", flag, value]).unwrap_err().contains("positive integer"));
            }
            for value in ["1", "+2", &usize::MAX.to_string()] {
                assert!(parse(&["q", flag, value]).is_ok(), "{flag} {value}");
            }
        }
        let a = run(&["q", "-n0", "-m0", "--max-regions=0", "-C0", "--max-files=0", "--max-filesize=0"]);
        assert_eq!((a.top, a.max_lines, a.max_regions, a.context, a.max_files, a.max_filesize), (0, 0, 0, 0, 0, 0));
    }

    #[test]
    fn other_integers_reject_malformed_negative_and_overflowing_values() {
        for flag in ["--top", "--max-lines", "--max-regions", "--context", "--max-files", "--max-filesize"] {
            for value in ["-1", "1.5", "NaN", "many", "", " 1", "999999999999999999999999999999999999"] {
                assert!(parse(&["q", flag, value]).is_err(), "{flag} {value}");
            }
            assert!(parse(&["q", flag, "+2"]).is_ok(), "{flag}");
        }
        assert_eq!(run(&["q", "--max-filesize", &u64::MAX.to_string()]).max_filesize, u64::MAX);
        assert_eq!(run(&["q", "--top", &usize::MAX.to_string()]).top, usize::MAX);
    }

    #[test]
    fn color_is_a_typed_case_sensitive_last_value_wins_option() {
        for (value, expected) in [("auto", ColorMode::Auto), ("always", ColorMode::Always), ("never", ColorMode::Never)] {
            assert_eq!(run(&["q", "--color", value]).color, expected);
        }
        for value in ["", "yes", "Always", "0"] {
            assert!(parse(&["q", "--color", value]).is_err());
        }
    }

    #[test]
    fn environment_precedence_and_empty_fallback_are_explicit() {
        for environment in [None, Some(""), Some(" "), Some("configured")] {
            for explicit in [None, Some(""), Some("flag")] {
                let mut words = vec!["q", "--backend", "jev"];
                if let Some(value) = explicit {
                    words.extend(["--model", value, "--base-url", value]);
                }
                let mut reads = Vec::new();
                let parsed = parse_args_with_env(argv(&words), |name| {
                    reads.push(name.to_owned());
                    environment.map(str::to_owned)
                })
                .unwrap();
                let Parsed::Run(a) = parsed else { panic!("expected Run") };
                let expected = explicit.or(environment.filter(|v| !v.is_empty()));
                assert_eq!(a.model, expected.unwrap_or(DEFAULT_MODEL));
                assert_eq!(a.base_url, expected.unwrap_or(DEFAULT_BASE_URL));
                if explicit.is_some() {
                    assert!(reads.is_empty());
                } else {
                    assert_eq!(reads, ["JG_MODEL", "JG_BASE_URL"]);
                }
            }
        }
        let Parsed::Run(a) = parse_args_with_env(argv(&["q", "--backend", "jev", "--model", "explicit"]), |name| {
            assert_eq!(name, "JG_BASE_URL");
            Some("endpoint".into())
        })
        .unwrap() else {
            panic!("expected Run")
        };
        assert_eq!((a.model.as_str(), a.base_url.as_str()), ("explicit", "endpoint"));
    }

    #[test]
    fn backend_selection_controls_defaults_and_validates_fixed_chatgpt_model() {
        let a = run(&["q", "--backend", "chatgpt"]);
        assert_eq!(a.backend, BackendKind::Chatgpt);
        assert_eq!((a.model.as_str(), a.base_url.as_str(), a.jobs), (CHATGPT_MODEL, DEFAULT_CHATGPT_URL, 32));
        let a = run(&["q", "--backend", "chatgpt", "--jobs=3", "--chatgpt-login", "--model", CHATGPT_MODEL]);
        assert!(a.chatgpt_login);
        assert_eq!(a.jobs, 3);
        assert!(parse(&["q", "--chatgpt-login"]).unwrap_err().contains("--backend chatgpt"));
        assert!(parse(&["q", "--backend", "chatgpt", "--model", "other"]).unwrap_err().contains(CHATGPT_MODEL));
        for value in ["", "other", "CHATGPT"] {
            assert!(parse(&["q", "--backend", value]).is_err());
        }
        let Parsed::Run(a) = parse_args_with_env(argv(&["q"]), |key| (key == "JG_BACKEND").then(|| "chatgpt".into())).unwrap() else {
            panic!("expected Run")
        };
        assert_eq!(a.backend, BackendKind::Chatgpt);
        assert_eq!(a.model, CHATGPT_MODEL);
        assert!(parse_args_with_env(argv(&["q"]), |key| match key {
            "JG_BACKEND" => Some("chatgpt".into()),
            "JG_MODEL" => Some("jev-latest".into()),
            _ => None,
        })
        .unwrap_err()
        .contains(CHATGPT_MODEL));
        let Parsed::Run(a) = parse_args_with_env(argv(&["q", "--backend", "jev"]), |_| None).unwrap() else { panic!("expected Run") };
        assert_eq!(a.backend, BackendKind::Jev);
        assert!(parse_args_with_env(argv(&["q"]), |key| (key == "JG_BACKEND").then(|| "unknown".into())).is_err());
    }

    #[test]
    fn help_version_and_errors_never_read_application_environment() {
        for args in [vec!["-h"], vec!["--help"], vec!["-V"], vec!["--version"], vec![], vec!["q", "--jobs=0"], vec!["q", "--bogus"]] {
            let _ = parse_args_with_env(argv(&args), |_| panic!("early exits must not resolve environment"));
        }
    }

    #[test]
    fn generated_help_has_all_flags_defaults_and_required_sections() {
        let short = printed(&["-h"]);
        let long = printed(&["--help"]);
        assert_eq!(short, long);
        assert!(short.contains("Usage: jg [OPTIONS] <QUERY> [PATH]..."), "{short}");
        for section in ["Arguments:", "Options:", "environment:", "examples:", "filters:", "output, one row shape throughout"] {
            assert!(short.contains(section), "missing {section}");
        }
        // Wrapping may split the default annotation between words. Check its semantic
        // content here; the unmodified help below still has strict width/plainness checks.
        let unwrapped = short.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut command = CommandLine::command();
        command.build();
        for arg in command.get_arguments() {
            if let Some(long) = arg.get_long() {
                assert!(short.contains(&format!("--{long}")), "missing {long}");
            }
            if arg.get_action().takes_values() {
                for default in arg.get_default_values() {
                    assert!(
                        unwrapped.contains(&format!("[default: {}]", default.to_string_lossy())),
                        "missing default for {}",
                        arg.get_id()
                    );
                }
            }
        }
        for text in
            [DEFAULT_MODEL, DEFAULT_BASE_URL, "TYPESAFE_API_KEY", "JG_MODEL", "JG_BASE_URL", "0 matches found, 1 none, 2 error", "weaker"]
        {
            assert!(unwrapped.contains(text), "missing {text}");
        }
        assert!(!short.contains('\u{1b}'));
        assert!(short.lines().all(|line| line.chars().count() <= 100), "{short}");
        assert_eq!(short, printed(&["--color=always", "--help"]));
    }

    #[test]
    fn generated_version_and_diagnostics_are_plain_complete_strings() {
        for flag in ["-V", "--version"] {
            assert_eq!(printed(&[flag]), format!("jg {}\n", env!("CARGO_PKG_VERSION")));
        }
        let error = parse(&["q", "--color=always", "--bogus"]).unwrap_err();
        assert!(error.starts_with("error:"), "{error}");
        assert!(error.contains("--bogus") && error.contains("Usage: jg") && error.ends_with('\n'), "{error}");
        assert!(!error.contains('\u{1b}'));
    }

    #[cfg(unix)]
    #[test]
    fn invalid_unicode_is_rejected_for_positionals_and_option_values() {
        use std::os::unix::ffi::OsStringExt;
        for prefix in [vec![], vec!["q"], vec!["q", "--model"], vec!["q", "-e"]] {
            let mut args = argv(&prefix);
            args.push(OsString::from_vec(vec![0xff]));
            assert!(parse_args_with_env(args, |_| None).is_err());
        }
    }
}
