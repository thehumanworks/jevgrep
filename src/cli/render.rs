//! Writer-based rendering. Selection and machine payloads stay independent of styling.

use std::collections::{BTreeSet, HashMap};
use std::fmt::Display;
use std::io::{self, Write};

use console::{Style, StyledObject};
use serde_json::json;

use super::args::{Args, ColorMode};
use crate::results::{FileView, Row, WEAK_RATIO};
use crate::search::FileResult;

/// Environment values captured by execution, never read or mutated by renderers.
#[derive(Clone, Copy, Debug, Default)]
pub struct ColorEnv<'a> {
    pub no_color: Option<&'a str>,
    pub term: Option<&'a str>,
    pub clicolor_force: Option<&'a str>,
    pub clicolor: Option<&'a str>,
}

/// Per-stream policy. Use a plain palette for machine formats at the runtime boundary.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Palette {
    enabled: bool,
}

impl Palette {
    pub const fn plain() -> Self {
        Self { enabled: false }
    }

    /// Pure precedence: explicit option, NO_COLOR/dumb, FORCE, CLICOLOR, then TTY.
    pub fn new(mode: ColorMode, is_terminal: bool, env: ColorEnv<'_>) -> Self {
        let enabled = match mode {
            ColorMode::Never => false,
            ColorMode::Always => true,
            ColorMode::Auto => {
                if env.no_color.is_some_and(|v| !v.is_empty()) || env.term == Some("dumb") {
                    false
                } else if env.clicolor_force.is_some_and(|v| !v.is_empty() && v != "0") {
                    true
                } else {
                    env.clicolor != Some("0") && is_terminal
                }
            }
        };
        Self { enabled }
    }

    pub fn heading<D: Display>(self, value: D) -> StyledObject<D> {
        Style::new().cyan().bold().force_styling(self.enabled).apply_to(value)
    }

    pub fn metadata<D: Display>(self, value: D) -> StyledObject<D> {
        Style::new().green().force_styling(self.enabled).apply_to(value)
    }

    pub fn notice<D: Display>(self, value: D) -> StyledObject<D> {
        Style::new().yellow().force_styling(self.enabled).apply_to(value)
    }
}

fn file_lines(path: &str) -> Vec<String> {
    std::fs::read(path).map(|d| String::from_utf8_lossy(&d).lines().map(str::to_owned).collect()).unwrap_or_default()
}

fn cut(text: &str, width: usize) -> String {
    console::truncate_str(text, width, &"..."[..width.min(3)]).into_owned()
}

fn r4(x: f64) -> f64 {
    (x * 1e4).round() / 1e4
}

pub(super) fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

pub fn render_files(query: &str, files: &[&FileResult], as_json: bool, out: &mut dyn Write) -> io::Result<()> {
    render_files_styled(query, files, as_json, Palette::plain(), out)
}

/// JSON always bypasses the palette, even when color is forced.
pub fn render_files_styled(query: &str, files: &[&FileResult], as_json: bool, palette: Palette, out: &mut dyn Write) -> io::Result<()> {
    for fr in files {
        if as_json {
            let row = json!({"query": query, "path": fr.path, "relevance": r4(fr.score), "confidence": r4(fr.confidence)});
            writeln!(out, "{row}")?;
        } else {
            writeln!(out, "{}  {}", palette.metadata(format!("{:.2}", fr.score)), palette.heading(&fr.path))?;
        }
    }
    Ok(())
}

/// One row shape throughout: location, probability, text. Regions are `a-b`, lines are `n`.
///
/// Every printed probability clears its bar and no source line is printed twice; see results.rs.
pub fn render_text(views: &[FileView], args: &Args, out: &mut dyn Write) -> io::Result<()> {
    render_text_styled(views, args, Palette::plain(), out)
}

/// Flat rows always bypass the palette, even when color is forced.
pub fn render_text_styled(views: &[FileView], args: &Args, palette: Palette, out: &mut dyn Write) -> io::Result<()> {
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
                "{}\n",
                palette.notice(format!(
                    "-- weaker: these files look related overall, but nothing in them reached {:.2}; near misses (>= {:.2}) shown",
                    args.threshold,
                    WEAK_RATIO * args.threshold
                ))
            )?;
        }
        writeln!(out, "{}  {}", palette.heading(&v.path), palette.metadata(format!("relevance={:.2}", v.relevance)))?;
        let source = if args.context > 0 { file_lines(&v.path) } else { vec![] };
        let mut printed: BTreeSet<usize> = BTreeSet::new();
        for row in v.rows() {
            let hits: HashMap<usize, &crate::search::LineHit> = match row {
                Row::Region(r) => {
                    writeln!(
                        out,
                        "{}  {}",
                        palette.metadata(format!("{:>11}  {:.2}", format!("{}-{}", r.start, r.end), r.p)),
                        cut(&r.label, 110)
                    )?;
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
                    writeln!(out, "{}  {}", palette.metadata(format!("{n:>11}  {:.2}", h.p)), cut(&h.text, 200))?;
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
            writeln!(out, "{:>11}  {}", "", palette.notice(format!("not shown: {}", more.join(", "))))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{parse_args, Parsed};
    use crate::results::Region;
    use crate::search::LineHit;

    fn args(flags: &[&str]) -> Box<Args> {
        let argv = std::iter::once("query").chain(flags.iter().copied()).map(Into::into).collect();
        match parse_args(argv).unwrap() {
            Parsed::Run(args) => args,
            Parsed::Print(_) => panic!("expected search arguments"),
        }
    }

    fn view() -> FileView {
        FileView {
            path: "src/example.rs".into(),
            relevance: 0.987654,
            section_relevance: 0.876543,
            confidence: 0.765432,
            regions: vec![Region {
                start: 1,
                end: 3,
                p: 0.654321,
                label: "fn example() {".into(),
                label_line: 1,
                lines: vec![LineHit { line: 2, p: 0.912345, text: "    payload();".into(), section: 1.0, keep: 1.0 }],
            }],
            lines: vec![LineHit { line: 7, p: 0.812345, text: "outside();".into(), section: 1.0, keep: 1.0 }],
            more_regions: 2,
            more_lines: 1,
            weak: false,
        }
    }

    fn forced() -> Palette {
        Palette::new(ColorMode::Always, false, ColorEnv::default())
    }

    fn text(views: &[FileView], args: &Args, palette: Palette) -> String {
        let mut out = Vec::new();
        render_text_styled(views, args, palette, &mut out).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn auto_policy_precedence_and_empty_values() {
        let cases = [
            (ColorEnv::default(), false, false),
            (ColorEnv::default(), true, true),
            (ColorEnv { no_color: Some("1"), ..ColorEnv::default() }, true, false),
            (ColorEnv { no_color: Some("0"), clicolor_force: Some("1"), ..ColorEnv::default() }, true, false),
            (ColorEnv { no_color: Some(""), ..ColorEnv::default() }, true, true),
            (ColorEnv { term: Some("dumb"), clicolor_force: Some("1"), ..ColorEnv::default() }, true, false),
            (ColorEnv { clicolor_force: Some("1"), clicolor: Some("0"), ..ColorEnv::default() }, false, true),
            (ColorEnv { clicolor_force: Some("yes"), ..ColorEnv::default() }, false, true),
            (ColorEnv { clicolor_force: Some("0"), ..ColorEnv::default() }, false, false),
            (ColorEnv { clicolor_force: Some(""), ..ColorEnv::default() }, false, false),
            (ColorEnv { clicolor: Some("0"), ..ColorEnv::default() }, true, false),
            (ColorEnv { clicolor: Some("1"), ..ColorEnv::default() }, false, false),
            (ColorEnv { clicolor: Some(""), ..ColorEnv::default() }, true, true),
        ];
        for (env, tty, expected) in cases {
            assert_eq!(Palette::new(ColorMode::Auto, tty, env).enabled, expected, "{env:?}, tty={tty}");
            assert!(Palette::new(ColorMode::Always, tty, env).enabled);
            assert!(!Palette::new(ColorMode::Never, tty, env).enabled);
        }
    }

    #[test]
    fn stream_palettes_are_independent_and_styles_force_both_states() {
        for (stdout, stderr) in [(false, true), (true, false), (true, true), (false, false)] {
            let out = Palette::new(ColorMode::Auto, stdout, ColorEnv::default());
            let err = Palette::new(ColorMode::Auto, stderr, ColorEnv::default());
            for (palette, enabled) in [(out, stdout), (err, stderr)] {
                for styled in
                    [palette.heading("heading").to_string(), palette.metadata("metadata").to_string(), palette.notice("notice").to_string()]
                {
                    assert_eq!(styled.contains('\x1b'), enabled);
                    if enabled {
                        assert!(styled.ends_with("\x1b[0m"));
                    }
                }
            }
        }
        assert_eq!(Palette::plain().heading("plain").to_string(), "plain");
        assert!(forced().heading("color").to_string().contains('\x1b'));
    }

    #[test]
    fn plain_wrapper_preserves_ascii_layout_and_styling_changes_only_presentation() {
        let args = args(&[]);
        let views = [view()];
        let expected = concat!(
            "src/example.rs  relevance=0.99\n",
            "        1-3  0.65  fn example() {\n",
            "          2  0.91      payload();\n",
            "          7  0.81  outside();\n",
            "             not shown: 2 more regions (raise --max-regions), 1 more line (raise -m)\n\n",
        );
        let mut plain = Vec::new();
        render_text(&views, &args, &mut plain).unwrap();
        assert_eq!(String::from_utf8(plain).unwrap(), expected);
        let styled = text(&views, &args, forced());
        assert!(styled.contains('\x1b'));
        assert_eq!(console::strip_ansi_codes(&styled), expected);
    }

    #[test]
    fn weak_notice_retains_label_and_is_announced_once() {
        let args = args(&[]);
        let mut weak = view();
        weak.weak = true;
        let styled = text(&[weak.clone(), weak], &args, forced());
        let plain = console::strip_ansi_codes(&styled);
        assert_eq!(plain.matches("-- weaker:").count(), 1);
        assert!(plain
            .starts_with("-- weaker: these files look related overall, but nothing in them reached 0.50; near misses (>= 0.35) shown\n\n"));
    }

    #[test]
    fn flat_bypasses_styles_truncation_and_weak_rows() {
        let args = args(&["--no-heading"]);
        let mut strong = view();
        strong.regions[0].label = "界".repeat(120);
        let mut weak = view();
        weak.weak = true;
        let expected = format!(
            "src/example.rs:1-3:0.65:{}\nsrc/example.rs:2:0.91:    payload();\nsrc/example.rs:7:0.81:outside();\n",
            strong.regions[0].label
        );
        let views = [strong, weak];
        assert_eq!(text(&views, &args, forced()), expected);
        assert_eq!(text(&views, &args, Palette::plain()), expected);
    }

    #[test]
    fn json_preserves_exact_order_precision_and_payloads() {
        let mut out = Vec::new();
        render_json("query", &[view()], &mut out).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), concat!(
            "{\"query\":\"query\",\"path\":\"src/example.rs\",\"relevance\":0.9877,\"section_relevance\":0.8765,\"confidence\":0.7654,\"match\":\"strong\",",
            "\"regions\":[{\"start\":1,\"end\":3,\"p\":0.6543,\"label\":\"fn example() {\",\"label_line\":1,\"lines\":[{\"line\":2,\"p\":0.9123,\"text\":\"    payload();\"}]}],",
            "\"lines\":[{\"line\":7,\"p\":0.8123,\"text\":\"outside();\"}],\"more_regions\":2,\"more_lines\":1}\n"
        ));
        let mut v = view();
        v.regions[0].label = "界\t\"\n\x1b[31m".repeat(120);
        v.lines[0].text = "e\u{301}🙂\t\"\\\n".repeat(200);
        let mut out = Vec::new();
        render_json("q\n\"", &[v.clone()], &mut out).unwrap();
        assert!(!out.contains(&b'\x1b'));
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(parsed["regions"][0]["label"], v.regions[0].label);
        assert_eq!(parsed["lines"][0]["text"], v.lines[0].text);
        assert_eq!(parsed["query"], "q\n\"");
    }

    fn file() -> FileResult {
        FileResult { path: "src/example.rs".into(), score: 0.987654, confidence: 0.765432, keep: 1.0, blocks: vec![], lines: vec![] }
    }

    #[test]
    fn files_plain_wrapper_and_json_ignore_color() {
        let f = file();
        let mut out = Vec::new();
        render_files("q", &[&f], false, &mut out).unwrap();
        assert_eq!(out, b"0.99  src/example.rs\n");
        let mut styled = Vec::new();
        render_files_styled("q", &[&f], false, forced(), &mut styled).unwrap();
        assert!(styled.contains(&b'\x1b'));
        assert_eq!(console::strip_ansi_codes(std::str::from_utf8(&styled).unwrap()).as_bytes(), out);
        for palette in [Palette::plain(), forced()] {
            let mut json = Vec::new();
            render_files_styled("q", &[&f], true, palette, &mut json).unwrap();
            assert_eq!(json, b"{\"query\":\"q\",\"path\":\"src/example.rs\",\"relevance\":0.9877,\"confidence\":0.7654}\n");
        }
    }

    #[test]
    fn unicode_display_width_truncation_and_tiny_budgets_are_safe() {
        for sample in ["ASCII", "café", "界漢", "e\u{301}", "🙂"] {
            let long = sample.repeat(250);
            for width in [0, 1, 2, 3, 4, 7, 110, 200] {
                let truncated = cut(&long, width);
                assert!(console::measure_text_width(&truncated) <= width, "{sample:?} width={width}: {truncated:?}");
                assert!(truncated.ends_with(&"..."[..width.min(3)]));
            }
        }
        assert_eq!(cut("界界界", 5), "界...");
        assert_eq!(cut("café", 4), "café");
        assert_eq!(cut("e\u{301}", 1), "e\u{301}");
        assert_eq!(cut("🙂", 2), "🙂");
        assert_eq!(cut("", 0), "");
        assert_eq!(cut("abcdef", 0), "");
        assert_eq!(cut("abcdef", 1), ".");
        assert_eq!(cut("abcdef", 2), "..");
        assert_eq!(cut(&"a".repeat(111), 110), format!("{}...", "a".repeat(107)));
        assert_eq!(cut(&"a".repeat(201), 200), format!("{}...", "a".repeat(197)));
    }

    #[test]
    fn ansi_sequences_do_not_consume_width_and_resets_survive_truncation() {
        let raw = "界".repeat(100);
        let styled = forced().heading(&raw).to_string();
        let truncated = cut(&styled, 110);
        assert_eq!(console::measure_text_width(&truncated), 109);
        assert_eq!(console::strip_ansi_codes(&truncated), cut(&raw, 110));
        assert!(truncated.ends_with("\x1b[0m"));
        let mut v = view();
        v.regions[0].label = raw.repeat(2);
        v.lines[0].text = raw.repeat(2);
        let rendered = text(&[v], &args(&[]), forced());
        let plain = console::strip_ansi_codes(&rendered);
        assert_eq!(console::measure_text_width(plain.lines().nth(1).unwrap()), 19 + 109);
        assert_eq!(console::measure_text_width(plain.lines().nth(3).unwrap()), 19 + 199);
    }

    struct FailingWriter(io::ErrorKind);

    impl Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(self.0, "injected output failure"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn every_renderer_propagates_broken_pipe_and_other_write_errors() {
        for kind in [io::ErrorKind::BrokenPipe, io::ErrorKind::PermissionDenied] {
            let mut out = FailingWriter(kind);
            let f = file();
            let views = [view()];
            for json in [false, true] {
                assert_eq!(render_files("q", &[&f], json, &mut out).unwrap_err().kind(), kind);
                assert_eq!(render_files_styled("q", &[&f], json, forced(), &mut out).unwrap_err().kind(), kind);
            }
            for flags in [vec![], vec!["--no-heading"]] {
                let args = args(&flags);
                assert_eq!(render_text(&views, &args, &mut out).unwrap_err().kind(), kind);
                assert_eq!(render_text_styled(&views, &args, forced(), &mut out).unwrap_err().kind(), kind);
            }
            assert_eq!(render_json("q", &views, &mut out).unwrap_err().kind(), kind);
        }
    }

    #[test]
    fn empty_results_do_not_write() {
        let mut out = FailingWriter(io::ErrorKind::Other);
        render_files("q", &[], false, &mut out).unwrap();
        render_files("q", &[], true, &mut out).unwrap();
        render_text(&[], &args(&[]), &mut out).unwrap();
        render_json("q", &[], &mut out).unwrap();
    }
}
