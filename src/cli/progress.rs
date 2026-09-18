//! Stderr-only progress. Execution owns TTY/environment capture and final cleanup.

use std::io::{self, Write};

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

/// Pure visibility policy; stdout's terminal state intentionally does not participate.
pub fn enabled(stderr_is_terminal: bool, quiet: bool, term: Option<&str>) -> bool {
    stderr_is_terminal && !quiet && term != Some("dumb")
}

/// One search's progress and notes. No timers, scheduling changes, or global state.
///
/// Explicitly call `finish_and_clear` before final diagnostics, stats, or result writes,
/// including API/auth failures. Drop is only a fallback for unforeseen early returns.
pub struct Progress {
    bar: ProgressBar,
    quiet: bool,
}

impl Progress {
    pub fn new(stderr_is_terminal: bool, quiet: bool, term: Option<&str>) -> Self {
        Self::with_draw_target(stderr_is_terminal, quiet, term, ProgressDrawTarget::stderr())
    }

    /// The supplied target is ignored when policy says hidden. Tests can supply a
    /// terminal-like target without depending on the process terminal or elapsed time.
    pub fn with_draw_target(stderr_is_terminal: bool, quiet: bool, term: Option<&str>, target: ProgressDrawTarget) -> Self {
        let target = if enabled(stderr_is_terminal, quiet, term) { target } else { ProgressDrawTarget::hidden() };
        let bar = ProgressBar::with_draw_target(Some(0), target);
        bar.set_style(ProgressStyle::with_template("jg: {pos}/{len} chunks").expect("static progress template is valid"));
        Self { bar, quiet }
    }

    /// Accept exactly the existing search callback's done/total values.
    pub fn update(&self, done: usize, total: usize) {
        if !self.bar.is_finished() {
            self.bar.set_length(total as u64);
            self.bar.set_position(done as u64);
        }
    }

    /// Preserve plain notes on redirected stderr; unlike `ProgressBar::println`,
    /// suspend does not discard the note when the bar is hidden. Never use this for
    /// genuine errors, which execution must print even in quiet mode.
    pub fn note(&self, message: &str, out: &mut dyn Write) -> io::Result<()> {
        if self.quiet {
            return Ok(());
        }
        self.bar.suspend(|| {
            writeln!(out, "jg: {message}")?;
            out.flush()
        })
    }

    /// Debug logging may originate on worker threads. Acquire the output lock
    /// inside this closure, never before the progress lock, to avoid lock inversion.
    pub fn suspend(&self, log: impl FnOnce()) {
        self.bar.suspend(log);
    }

    /// Idempotent, explicit finalization. Further updates cannot revive the bar.
    pub fn finish_and_clear(&self) {
        self.bar.finish_and_clear();
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        self.finish_and_clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatif::{InMemoryTerm, TermLike};

    fn visible() -> (Progress, InMemoryTerm) {
        let term = InMemoryTerm::new(8, 80);
        let progress = Progress::with_draw_target(true, false, Some("xterm"), ProgressDrawTarget::term_like(Box::new(term.clone())));
        (progress, term)
    }

    // Writes into the same screen while suspend has cleared the progress row.
    struct TerminalWriter(InMemoryTerm);

    impl Write for TerminalWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            // Model a normal terminal's ONLCR output translation. InMemoryTerm's
            // raw write_str intentionally does not turn LF into CRLF for us.
            self.0.write_str(&std::str::from_utf8(bytes).unwrap().replace('\n', "\r\n"))?;
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }

    #[test]
    fn visibility_is_stderr_tty_not_quiet_and_not_dumb() {
        for tty in [false, true] {
            for quiet in [false, true] {
                for term in [None, Some(""), Some("xterm"), Some("dumb")] {
                    assert_eq!(enabled(tty, quiet, term), tty && !quiet && term != Some("dumb"));
                    let screen = InMemoryTerm::new(8, 80);
                    let p = Progress::with_draw_target(tty, quiet, term, ProgressDrawTarget::term_like(Box::new(screen.clone())));
                    p.update(1, 4);
                    p.bar.force_draw();
                    assert_eq!(screen.contents().is_empty(), !enabled(tty, quiet, term));
                    p.finish_and_clear();
                    assert!(screen.contents().is_empty());
                }
            }
        }
    }

    #[test]
    fn exact_callback_counts_are_drawn_without_a_clock() {
        let (p, screen) = visible();
        for (done, total) in [(0, 9), (3, 9), (4, 12), (12, 12)] {
            p.update(done, total);
            p.bar.force_draw();
            assert_eq!(p.bar.position(), done as u64);
            assert_eq!(p.bar.length(), Some(total as u64));
            assert_eq!(screen.contents(), format!("jg: {done}/{total} chunks"));
        }
        p.finish_and_clear();
        assert!(p.bar.is_finished());
        assert_eq!(screen.contents(), "");
        p.update(13, 13);
        p.bar.force_draw();
        p.finish_and_clear();
        assert_eq!(p.bar.position(), 12);
        assert_eq!(screen.contents(), "");
    }

    #[test]
    fn notes_survive_suspension_and_cleanup_leaves_only_notes() {
        let (p, screen) = visible();
        p.update(2, 5);
        p.bar.force_draw();
        p.note("path triage kept 5 of 9 files", &mut TerminalWriter(screen.clone())).unwrap();
        p.bar.force_draw();
        assert_eq!(screen.contents(), "jg: path triage kept 5 of 9 files\njg: 2/5 chunks");
        p.finish_and_clear();
        assert_eq!(screen.contents(), "jg: path triage kept 5 of 9 files");
    }

    #[test]
    fn redirected_notes_remain_plain_and_quiet_suppresses_them() {
        for quiet in [false, true] {
            let p = Progress::with_draw_target(false, quiet, None, ProgressDrawTarget::hidden());
            let mut out = Vec::new();
            p.update(1, 5);
            p.note("ordinary note", &mut out).unwrap();
            p.finish_and_clear();
            assert_eq!(out, if quiet { b"".as_slice() } else { b"jg: ordinary note\n".as_slice() });
        }
    }

    struct FailingWriter {
        fail_flush: bool,
    }

    impl Write for FailingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.fail_flush {
                Ok(bytes.len())
            } else {
                Err(io::Error::other("write failed"))
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("flush failed"))
        }
    }

    #[test]
    fn failed_note_io_restores_bar_then_explicit_cleanup_clears_it() {
        for fail_flush in [false, true] {
            let (p, screen) = visible();
            p.update(1, 2);
            p.bar.force_draw();
            let err = p.note("note", &mut FailingWriter { fail_flush }).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::Other);
            p.bar.force_draw();
            assert_eq!(screen.contents(), "jg: 1/2 chunks");
            p.finish_and_clear();
            assert!(p.bar.is_finished());
            assert_eq!(screen.contents(), "");
        }
    }

    #[test]
    fn cleanup_precedes_final_success_or_error_diagnostics() {
        for diagnostic in ["jg: no matches", "jg: TypeSafe API error: failed", "jg: TypeSafe rejected the API key", "jg: stats"] {
            let (p, screen) = visible();
            p.update(1, 2);
            p.bar.force_draw();
            p.finish_and_clear();
            screen.write_line(diagnostic).unwrap();
            assert_eq!(screen.contents(), diagnostic);
            drop(p);
            assert_eq!(screen.contents(), diagnostic);
        }
    }

    #[test]
    fn drop_is_a_cleanup_safety_net() {
        let (p, screen) = visible();
        p.update(1, 2);
        p.bar.force_draw();
        drop(p);
        assert_eq!(screen.contents(), "");
    }
}
