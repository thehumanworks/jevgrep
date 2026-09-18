# ADR 0002: Terminal presentation and progress

- Status: accepted
- Date: 2026-09-18

## Context

The baseline CLI mixes selection, rendering, and terminal detection. Its truncation
counts Unicode scalar values rather than display columns and progress uses manual
carriage returns and erase sequences. We need terminal presentation without changing
selection, request scheduling, source payloads, or machine-output contracts.

## Decision

Use `console` for per-style presentation and display-width truncation. Keep the
public `render_files`, `render_text`, and `render_json` signatures; the first two
are plain wrappers around styled variants. Renderers retain injected `Write`
destinations and propagate all write failures. Execution retains broken-pipe exit
handling. Help/version now also check write and flush errors: a broken pipe is
success, and other output errors exit 2 instead of being discarded. JSON construction, key order, rounding, and payload text remain unchanged.
Flat rows and JSON bypass all application styling, including forced color.

Resolve a palette from pure inputs separately for each stream. Explicit `never`
disables and explicit `always` enables eligible human presentation. In `auto`,
nonempty `NO_COLOR` or `TERM=dumb` disables it; otherwise nonempty/nonzero
`CLICOLOR_FORCE` enables it, then `CLICOLOR=0` disables it, and TTY detection is the
default. Apply forced enable/disable to individual console styles, not global
switches. Headings, metadata, and weaker-tier notices retain their textual labels.
Clap help and usage errors remain plain; `--color` controls application output only.

Keep fixed label/source budgets of 110/200 display columns, using console width
utilities. ASCII remains byte-compatible. Tiny budgets shorten the `...` marker
rather than underflowing. Truncate payloads before applying application styles.

Use an uncolored indicatif chunks-completed display on stderr only when stderr is
a TTY, quiet is false, and TERM is not dumb. Stdout's terminal state is irrelevant.
Inject draw targets and use deterministic state/terminal checks, not timers or
animation snapshots. Route notes through `suspend` with an injected writer so plain
notes survive redirected stderr; quiet suppresses notes/progress, not real errors.
Execution must explicitly finish and clear progress before final diagnostics,
stats, and output, on success, no matches, API/auth failure, and output failure.
Dropping the adapter is only a safety net, not the normal lifecycle contract.
Worker-thread `JG_DEBUG` retry diagnostics also suspend the bar through an optional
client diagnostic reporter. The default library behavior and message text are
unchanged. Acquire the stderr lock inside suspension to avoid lock inversion.
Explicitly requested debug logs still appear in quiet mode, as before.
No scheduling, retry, or search callback semantics change.

## Alternatives

Keeping manual ANSI progress would retain cleanup and stream-policy duplication.
Keeping scalar-count truncation would mismeasure wide and combining characters.
Process-global console switches would couple streams and make parallel tests racy.
A full TUI is unnecessary for a streaming grep-like command. Retaining lexopt is
not a presentation solution; parser migration is covered separately in ADR 0001.

## Consequences and compatibility impact

Two dependencies add build time and binary size. Styled human output and corrected
Unicode truncation are intentional changes; plain ASCII, JSON, flat payloads, and
selection remain compatible. The dev-only indicatif `in_memory` feature supplies a
terminal emulator for deterministic screen and cleanup assertions without sleeps.
Execution owns environment capture, TTY detection, callback wiring, and
explicit final cleanup.

## Tests and references

- `src/cli/render.rs`: policy precedence, separate streams, forced styles, Unicode
  and tiny budgets, ASCII/JSON/flat fixtures, and failing writers.
- `src/cli/progress.rs`: visibility, injected counts, suspended notes, quiet,
  and explicit cleanup using an injected draw target.
- `tests/cli.rs`, `tests/results.rs`: output and selection contracts.
- `tests/client_search.rs`: retry diagnostic routing and unchanged callback counts.
- `scripts/terminal-smoke.py --pty` covers real stream wiring, notes, quiet/dumb
  terminals, API failures, no matches, broken pipes and final cleanup with timeouts.
- `docs/plans/cli-modernization.md`, phases D/E and the required test matrix.
- console `Style::force_styling`, `measure_text_width`, and `truncate_str` APIs.
- indicatif `ProgressBar`, `ProgressDrawTarget`, `TermLike`, and `suspend` APIs.
