//! jg command line: parsing, presentation, and execution have separate boundaries.

mod args;
mod progress;
mod render;

pub use args::{parse_args, ApiKey, Args, BackendKind, ColorMode, Parsed};
pub use render::{render_files, render_json, render_text};

use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::backend::{DecisionBackend, DecisionError};
use crate::chatgpt::{ChatGptClient, ChatGptConfig};
use crate::chatgpt_auth::{device_login, resolve_credentials};
use crate::files::{discover, Discover};
use crate::filters::{parse_filter, Rules};
use crate::jev::resolve_api_key;
use crate::openai::{evaluation_url, label, resolve_key, OpenAiClient, OpenAiConfig, OPENAI_KEY_VAR};
use crate::results::{filtered_out, select, select_files, Limits};
use crate::search::{search, Options};
use progress::Progress;
use render::{plural, render_files_styled, render_text_styled, ColorEnv, Palette};
use typesafe_jev::{Client as JevClient, Config};

fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Flush failures are just as significant as write failures. A downstream closed
/// pipe is the only output failure treated as success (grep-style pipelines).
fn output_status(result: io::Result<i32>) -> i32 {
    match result {
        Ok(code) => code,
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => 0,
        Err(e) => {
            eprintln!("jg: {e}");
            2
        }
    }
}

fn print_and_flush(text: &str, out: &mut dyn Write) -> io::Result<i32> {
    out.write_all(text.as_bytes())?;
    out.flush()?;
    Ok(0)
}

/// Runs jg and returns the exit status: 0 matches found, 1 none, 2 error.
pub fn run(argv: Vec<OsString>) -> i32 {
    let args = match parse_args(argv) {
        Ok(Parsed::Run(args)) => args,
        Ok(Parsed::Print(text)) => {
            return output_status(print_and_flush(&text, &mut io::stdout().lock()));
        }
        Err(e) => {
            eprint!("{e}");
            return 2;
        }
    };
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    output_status(execute(&args, &mut out).and_then(|code| out.flush().map(|()| code)))
}

fn execute(args: &Args, out: &mut dyn Write) -> io::Result<i32> {
    if args.queries.is_empty() {
        eprintln!("jg: empty query");
        return Ok(2);
    }
    let started = Instant::now();
    let (no_color, term, force, clicolor) =
        (std::env::var("NO_COLOR").ok(), std::env::var("TERM").ok(), std::env::var("CLICOLOR_FORCE").ok(), std::env::var("CLICOLOR").ok());
    let env =
        ColorEnv { no_color: no_color.as_deref(), term: term.as_deref(), clicolor_force: force.as_deref(), clicolor: clicolor.as_deref() };
    let palette = if args.json || args.no_heading { Palette::plain() } else { Palette::new(args.color, io::stdout().is_terminal(), env) };
    let diagnostics = Palette::new(args.color, io::stderr().is_terminal(), env);
    let progress = Arc::new(Progress::new(io::stderr().is_terminal(), args.quiet, term.as_deref()));
    let note = |msg: &str| {
        // Diagnostic writes historically are best-effort. All stdout errors propagate.
        let _ = progress.note(&diagnostics.notice(msg).to_string(), &mut io::stderr());
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
            progress.finish_and_clear();
            eprintln!("jg: {}", diagnostics.notice(format!("{path}: no such file or directory")));
            return Ok(2);
        }
    };
    if files.is_empty() {
        progress.finish_and_clear();
        note("no searchable files");
        return Ok(1);
    }

    let mut rules = Rules { only: args.only.iter().map(|t| vec![t.clone()]).collect(), exclude: args.exclude_terms.clone() };
    for text in &args.filter {
        rules = rules.merge(parse_filter(text));
    }
    let asked = !(args.filter.is_empty() && args.only.is_empty() && args.exclude_terms.is_empty());
    if asked && rules.is_empty() {
        progress.finish_and_clear();
        eprintln!("jg: the filter names no category; try e.g. --filter \"no tests\"");
        return Ok(2);
    }
    if !rules.is_empty() {
        note(&format!("filter: {}", rules.describe()));
    }

    let opts = Options {
        files_only: args.files,
        broad: args.broad,
        jobs: args.jobs,
        chunk_lines: args.chunk_lines,
        max_files: args.max_files,
        triage: args.triage,
        rules: (!rules.is_empty()).then(|| rules.clone()),
        ..Options::default()
    };
    let searched = build_backend(args, &progress).and_then(|client| {
        search(client.as_ref(), &args.queries, &files, &opts, |done, total| progress.update(done, total), note)
            .map(|ranked| (ranked, client))
    });
    // Finish before *all* final diagnostics and before any fallible result write.
    progress.finish_and_clear();
    let (ranked, client) = match searched {
        Ok(done) => done,
        Err(e @ DecisionError::Auth(_)) => {
            eprintln!("jg: {}", diagnostics.notice(e));
            return Ok(2);
        }
        Err(e) => {
            let provider = match args.backend {
                BackendKind::Jev => "TypeSafe".to_owned(),
                BackendKind::Chatgpt => "ChatGPT".to_owned(),
                BackendKind::Openai => label(&args.base_url),
            };
            eprintln!("jg: {}", diagnostics.notice(format!("{provider} API error: {e}")));
            return Ok(2);
        }
    };

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
            writeln!(out, "{}", palette.heading(format!("== {query}")))?;
        }
        let shown = if args.files {
            let picked = select_files(results, args.file_threshold, args.top);
            render_files_styled(query, &picked, args.json, palette, out)?;
            picked.len()
        } else {
            let picked = select(results, &limits);
            if args.json {
                render_json(query, &picked, out)?;
                picked.len()
            } else {
                render_text_styled(&picked, args, palette, out)?;
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
        let usage = client.usage();
        let retries = if usage.retries() > 0 { format!(", {} retries", usage.retries()) } else { String::new() };
        match args.backend {
            BackendKind::Jev => eprintln!(
                "jg: {} files, {} requests{retries}, {} tokens (~${:.4}), {:.1}s",
                files.len(),
                usage.requests(),
                thousands(usage.input_tokens()),
                usage.cost_usd(),
                started.elapsed().as_secs_f64()
            ),
            BackendKind::Chatgpt => eprintln!(
                "jg: {} files, {} requests{retries}, {} input / {} output tokens (ChatGPT subscription; requested priority, served {}), {:.1}s",
                files.len(),
                usage.requests(),
                thousands(usage.input_tokens()),
                thousands(usage.output_tokens()),
                client.service_tier().as_deref().unwrap_or("unreported"),
                started.elapsed().as_secs_f64()
            ),
            BackendKind::Openai => {
                // A cost is shown only where the service states one; jg knows no price list.
                let cost = client.reported_cost_usd().map(|usd| format!("; reported cost ${usd:.4}")).unwrap_or_default();
                eprintln!(
                    "jg: {} files, {} requests{retries}, {} input / {} output tokens ({} {}{cost}), {:.1}s",
                    files.len(),
                    usage.requests(),
                    thousands(usage.input_tokens()),
                    thousands(usage.output_tokens()),
                    label(&args.base_url),
                    args.model,
                    started.elapsed().as_secs_f64()
                )
            }
        }
    }
    Ok(if any { 0 } else { 1 })
}

/// The `User-Agent` the Jev client keeps sending; the other backends build the same string themselves.
const USER_AGENT: &str = concat!("jg/", env!("CARGO_PKG_VERSION"));

fn build_backend(args: &Args, progress: &Arc<Progress>) -> Result<Box<dyn DecisionBackend>, DecisionError> {
    let debug_progress = Arc::clone(progress);
    let reporter = move |msg: &str| debug_progress.suspend(|| eprintln!("jg[debug]: {msg}"));
    let debug = std::env::var_os("JG_DEBUG").is_some();
    match args.backend {
        BackendKind::Jev => {
            let key = resolve_api_key()?;
            let cfg = Config {
                base_url: args.base_url.clone(),
                model: args.model.clone(),
                pool_size: args.jobs.max(8),
                user_agent: USER_AGENT.into(),
                ..Config::default()
            };
            let mut client = JevClient::new(&key, cfg)?;
            if debug {
                client.set_debug_reporter(reporter);
            }
            Ok(Box::new(client))
        }
        BackendKind::Chatgpt => {
            let credentials = if args.chatgpt_login { device_login()? } else { resolve_credentials()? };
            let cfg = ChatGptConfig { base_url: args.base_url.clone(), pool_size: args.jobs, ..ChatGptConfig::default() };
            let mut client = ChatGptClient::new(&credentials, cfg);
            if debug {
                client.set_debug_reporter(reporter);
            }
            Ok(Box::new(client))
        }
        BackendKind::Openai => {
            let key = resolve_key(&args.base_url, args.api_key.as_ref().map(|key| key.0.as_str()), |name| std::env::var(name).ok())?;
            // Jev behind a gateway is still Jev: same questions, same answers, same client.
            if let Some(url) = evaluation_url(&args.base_url, &args.model) {
                let key =
                    key.ok_or_else(|| DecisionError::Auth(format!("{OPENAI_KEY_VAR} is not set. Export it, or pass --api-key <KEY>.")))?;
                let cfg = Config {
                    base_url: url.to_owned(),
                    model: args.model.clone(),
                    provider: label(&args.base_url),
                    // Measured live, the gateway answers 503 "try again shortly" at once, to an
                    // eighth of small requests and half of large ones whatever the concurrency, so
                    // a retry is soon and often: waiting longer only made one unlucky request the
                    // whole search's tail.
                    max_retries: 16,
                    max_backoff: Duration::from_secs(2),
                    pool_size: args.jobs.max(8),
                    user_agent: USER_AGENT.into(),
                    ..Config::default()
                };
                let mut client = JevClient::new(&key, cfg)?;
                if debug {
                    client.set_debug_reporter(reporter);
                }
                return Ok(Box::new(client));
            }
            let cfg = OpenAiConfig {
                base_url: args.base_url.clone(),
                model: args.model.clone(),
                json_schema: !args.no_schema,
                extra_body: args.extra_body.clone(),
                pool_size: args.jobs,
                ..OpenAiConfig::default()
            };
            let mut client = OpenAiClient::new(key.as_deref(), cfg);
            if debug {
                client.set_debug_reporter(reporter);
            }
            Ok(Box::new(client))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FailingWriter {
        write_error: Option<io::ErrorKind>,
        flush_error: Option<io::ErrorKind>,
    }

    impl Write for FailingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.write_error.map_or(Ok(bytes.len()), |kind| Err(io::Error::new(kind, "test write")))
        }
        fn flush(&mut self) -> io::Result<()> {
            self.flush_error.map_or(Ok(()), |kind| Err(io::Error::new(kind, "test flush")))
        }
    }

    #[test]
    fn help_and_version_output_checks_both_write_and_flush_errors() {
        for kind in [io::ErrorKind::BrokenPipe, io::ErrorKind::PermissionDenied, io::ErrorKind::Other] {
            let expected = if kind == io::ErrorKind::BrokenPipe { 0 } else { 2 };
            let mut writer = FailingWriter { write_error: Some(kind), flush_error: None };
            assert_eq!(output_status(print_and_flush("help", &mut writer)), expected);
            let mut writer = FailingWriter { write_error: None, flush_error: Some(kind) };
            assert_eq!(output_status(print_and_flush("version", &mut writer)), expected);
        }
        assert_eq!(output_status(print_and_flush("jg\n", &mut Vec::new())), 0);
        for code in [0, 1, 2] {
            assert_eq!(output_status(Ok(code)), code);
        }
    }
}
