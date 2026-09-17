"""jg command line."""

from __future__ import annotations

import argparse
import json
import os
import sys
import time

from . import __version__
from .client import DEFAULT_BASE_URL, DEFAULT_MODEL, JevAuthError, JevClient, JevError, resolve_api_key
from .files import discover
from .filters import Rules, parse_filter
from .results import WEAK_RATIO, FileView, Region, filtered_out, select, select_files
from .search import FileResult, Options, search

EPILOG = """\
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
only with near misses (at least 0.7 x -t). Probabilities are calibrated: 0.9 means right about 9 times in 10. Raise -t
for precision, lower it for recall. Exit status: 0 matches found, 1 none, 2 error.
"""


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    p = argparse.ArgumentParser(
        prog="jg",
        description="jevgrep: search code with a natural-language query. Every file chunk is scored "
                    "line by line by TypeSafe's Jev model, in parallel.",
        epilog=EPILOG,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    p.add_argument("query", help="what you are looking for, in plain language")
    p.add_argument("paths", nargs="*", help="files or directories (default: .)")
    p.add_argument("-e", "--query", dest="extra", action="append", default=[], metavar="QUERY",
                   help="additional query answered in the same pass (repeatable)")
    p.add_argument("-l", "--files", action="store_true",
                   help="rank relevant files only, no region or line scoring (about 3x cheaper)")
    p.add_argument("-f", "--filter", action="append", default=[], metavar="TEXT",
                   help='plain-language include/exclude rules, e.g. "Source code only. No tests or documentation" (repeatable)')
    p.add_argument("--only", action="append", default=[], metavar="CATEGORY",
                   help='keep only code in this category, e.g. --only "async code" (repeatable: all must hold)')
    p.add_argument("--not", dest="exclude_terms", action="append", default=[], metavar="CATEGORY",
                   help='drop code in this category, e.g. --not tests --not "generated code" (repeatable)')
    p.add_argument("-b", "--broad", action="store_true",
                   help="flag every line related to the query, not just the lines that answer it (more recall, more noise)")
    p.add_argument("-t", "--threshold", type=float, default=0.5,
                   help="min probability for a region or line to count as a match (default 0.5)")
    p.add_argument("-T", "--file-threshold", type=float, default=0.6,
                   help="file relevance needed to list a file with -l, or to put a file with nothing above -t "
                        "into the weaker tier (default 0.6)")
    p.add_argument("-n", "--top", type=int, default=15, help="max files per query (default 15)")
    p.add_argument("-m", "--max-lines", type=int, default=10, help="max pinpointed lines per file (default 10)")
    p.add_argument("--max-regions", type=int, default=5, help="max regions per file (default 5)")
    p.add_argument("-C", "--context", type=int, default=0, help="lines of context around each pinpointed line")
    p.add_argument("-g", "--glob", action="append", default=[], help="only search files matching GLOB (repeatable)")
    p.add_argument("-x", "--exclude", action="append", default=[], help="skip files matching GLOB (repeatable)")
    p.add_argument("-j", "--jobs", type=int, default=32, help="concurrent Jev requests (default 32)")
    p.add_argument("--json", action="store_true", help="JSON lines, one object per matching file")
    p.add_argument("--no-heading", action="store_true",
                   help="flat output: path:START-END:prob:label for regions, path:LINE:prob:text for lines; "
                        "matches only, no weaker tier")
    p.add_argument("--triage", action="store_true",
                   help="pre-filter files by path with Jev first (automatic above --max-files)")
    p.add_argument("--max-files", type=int, default=1500, help="file cap before path triage kicks in (default 1500)")
    p.add_argument("--max-filesize", type=int, default=512_000, help="skip files larger than BYTES (default 512000)")
    p.add_argument("--chunk-lines", type=int, default=150, help="lines per Jev request (default 150)")
    p.add_argument("--hidden", action="store_true", help="include dotfiles")
    p.add_argument("--no-ignore", action="store_true", help="do not honor .gitignore")
    p.add_argument("-q", "--quiet", action="store_true", help="no progress or stats on stderr")
    p.add_argument("--model", default=os.environ.get("JG_MODEL", DEFAULT_MODEL))
    p.add_argument("--base-url", default=os.environ.get("JG_BASE_URL", DEFAULT_BASE_URL))
    p.add_argument("-V", "--version", action="version", version=f"jg {__version__}")
    return p.parse_args(argv)


def _file_lines(path: str) -> list[str]:
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            return f.read().splitlines()
    except OSError:
        return []


def render_files(query: str | None, files: list[FileResult], as_json: bool, out) -> None:
    for fr in files:
        if as_json:
            out.write(json.dumps({"query": query, "path": fr.path, "relevance": round(fr.score, 4),
                                  "confidence": round(fr.confidence, 4)}) + "\n")
        else:
            out.write(f"{fr.score:.2f}  {fr.path}\n")


def render_text(views: list[FileView], args: argparse.Namespace, out) -> None:
    """One row shape throughout: location, probability, text. Regions are `a-b`, lines are `n`.

    Every printed probability clears its bar and no source line is printed twice; see results.py.
    """
    def cut(text: str, width: int) -> str:
        return text if len(text) <= width else text[: width - 3] + "..."

    announced = False
    for v in views:
        if args.no_heading:
            if v.weak:  # flat rows cannot carry the "weaker" caveat, so they carry only real matches
                continue
            for row in v.rows():
                if isinstance(row, Region):
                    out.write(f"{v.path}:{row.start}-{row.end}:{row.p:.2f}:{row.label}\n")
                for h in (row.lines if isinstance(row, Region) else [row]):
                    out.write(f"{v.path}:{h.line}:{h.p:.2f}:{h.text}\n")
            continue
        if v.weak and not announced:
            announced = True
            out.write(f"-- weaker: these files look related overall, but nothing in them reached {args.threshold:.2f}; "
                      f"near misses (>= {WEAK_RATIO * args.threshold:.2f}) shown\n\n")
        out.write(f"{v.path}  relevance={v.relevance:.2f}\n")
        source = _file_lines(v.path) if args.context else []
        printed: set[int] = set()
        for row in v.rows():
            if isinstance(row, Region):
                out.write(f"{f'{row.start}-{row.end}':>11}  {row.p:.2f}  {cut(row.label, 110)}\n")
                printed.add(row.label_line)
            hits = {h.line: h for h in (row.lines if isinstance(row, Region) else [row])}
            around = sorted({n for h in hits.values() for n in range(max(1, h.line - args.context), h.line + args.context + 1)})
            for n in around:
                if n in printed:
                    continue
                printed.add(n)
                if n in hits:
                    out.write(f"{n:>11}  {hits[n].p:.2f}  {cut(hits[n].text, 200)}\n")
                elif 0 < n <= len(source) and source[n - 1].strip():
                    out.write(f"{n:>11}        {cut(source[n - 1].rstrip(), 200)}\n")
        more = [f"{v.more_regions} more region{'s' * (v.more_regions > 1)} (raise --max-regions)" if v.more_regions else "",
                f"{v.more_lines} more line{'s' * (v.more_lines > 1)} (raise -m)" if v.more_lines else ""]
        if any(more):
            out.write(f"{'':>11}  not shown: {', '.join(m for m in more if m)}\n")
        out.write("\n")


def render_json(query: str, views: list[FileView], out) -> None:
    for v in views:
        out.write(json.dumps({
            "query": query,
            "path": v.path,
            "relevance": round(v.relevance, 4),
            "section_relevance": round(v.section_relevance, 4),
            "confidence": round(v.confidence, 4),
            "match": "weak" if v.weak else "strong",
            "regions": [{
                "start": r.start, "end": r.end, "p": round(r.p, 4), "label": r.label,
                "lines": [{"line": h.line, "p": round(h.p, 4), "text": h.text} for h in r.lines],
            } for r in v.regions],
            "lines": [{"line": h.line, "p": round(h.p, 4), "text": h.text} for h in v.lines],
            "more_regions": v.more_regions,
            "more_lines": v.more_lines,
        }) + "\n")


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    queries = [q.strip() for q in [args.query, *args.extra] if q.strip()]
    if not queries:
        print("jg: empty query", file=sys.stderr)
        return 2
    err = sys.stderr
    tty = err.isatty() and not args.quiet
    started = time.time()

    def progress(done: int, total: int) -> None:
        if tty:
            err.write(f"\rjg: {done}/{total} chunks")
            err.flush()

    def note(msg: str) -> None:
        if not args.quiet:
            print(f"{chr(13) + chr(27) + '[K' if tty else ''}jg: {msg}", file=err)

    try:
        files = discover(args.paths, globs=args.glob, excludes=args.exclude,
                         max_bytes=args.max_filesize, hidden=args.hidden, no_ignore=args.no_ignore)
    except FileNotFoundError as e:
        print(f"jg: {e}: no such file or directory", file=err)
        return 2
    if not files:
        if not args.quiet:
            print("jg: no searchable files", file=err)
        return 1

    rules = Rules([[t] for t in args.only], list(args.exclude_terms))
    for text in args.filter:
        rules = rules.merge(parse_filter(text))
    if (args.filter or args.only or args.exclude_terms) and not rules:
        print("jg: the filter names no category; try e.g. --filter \"no tests\"", file=err)
        return 2
    if rules:
        note(f"filter: {rules.describe()}")

    opts = Options(files_only=args.files, rules=rules or None, broad=args.broad, jobs=args.jobs, chunk_lines=args.chunk_lines,
                   max_files=args.max_files, triage=args.triage)
    try:
        with JevClient(resolve_api_key(), base_url=args.base_url, model=args.model,
                       pool_size=max(8, args.jobs)) as client:
            ranked = search(client, queries, files, opts, progress, note)
            usage = client.usage
    except JevAuthError as e:
        print(f"jg: {e}", file=err)
        return 2
    except JevError as e:
        print(f"jg: TypeSafe API error: {e}", file=err)
        return 2
    except KeyboardInterrupt:
        print("\njg: interrupted", file=err)
        return 130

    if tty:
        err.write("\r\033[K")  # wipe the progress line so results start on a clean row
        err.flush()
    found = False
    for query, results in zip(queries, ranked):
        heading = len(queries) > 1 and not args.json
        if heading:
            sys.stdout.write(f"== {query}\n")
        if args.files:
            picked = select_files(results, file_threshold=args.file_threshold, top=args.top)
            render_files(query, picked, args.json, sys.stdout)
        else:
            picked = select(results, threshold=args.threshold, file_threshold=args.file_threshold,
                            top=args.top, max_regions=args.max_regions, max_lines=args.max_lines)
            (render_json if args.json else render_text)(*((query, picked, sys.stdout) if args.json else (picked, args, sys.stdout)))
        if args.no_heading and not args.files and not args.json:
            picked = [v for v in picked if not v.weak]
        found = found or bool(picked)
        if heading and not picked:
            sys.stdout.write("(no matches)\n\n")
        elif heading and args.files:
            sys.stdout.write("\n")
    sys.stdout.flush()

    if rules and not args.quiet and not args.files:
        gone = [filtered_out(results, args.threshold) for results in ranked]
        regions, lines = sum(g[0] for g in gone), sum(g[1] for g in gone)
        print(f"jg: filter removed {regions} matching region{'s' * (regions != 1)} and {lines} line{'s' * (lines != 1)}", file=err)
    if not args.quiet:
        retries = f", {usage.retries} retries" if usage.retries else ""
        print(f"jg: {len(files)} files, {usage.requests} requests{retries}, "
              f"{usage.input_tokens:,} tokens (~${usage.cost_usd:.4f}), {time.time() - started:.1f}s", file=err)
    return 0 if found else 1


if __name__ == "__main__":
    sys.exit(main())
