"""Turns (queries, files) into Jev requests, fans them out over threads, aggregates answers.

One request per file chunk. Each request carries, per query:
  - one Score question: how relevant is this file section to the query (ranks files)
  - one Noul question per logical block: do these lines contain what the query seeks (regions)
  - one Noul question per line: does this line directly answer the query (anchors)
Answers are often a whole function or class rather than a line. Then no single line scores high,
but its block does, which is why regions and not lines are the primary unit of output.
Jev evaluates every question in a request in parallel, so extra queries and extra
lines add tokens but almost no latency. Extra queries reuse the same state.
"""

from __future__ import annotations

import concurrent.futures as cf
import threading
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable

from .client import JevClient, JevError, TokenLimitError
from .filters import FILTER_GATE, Rules
from .files import DEFINITION, Chunk, chunk_lines, display_path, read_lines, split_chunk

RELEVANCE_LEVELS = [
    "Irrelevant: nothing here relates to the query",
    "Tangential: touches the same area but would not help answer the query",
    "Relevant: contains code or text that helps answer the query",
    "Direct hit: this is the code or text the query is looking for",
]
# Wording was chosen by benchmark (bench/bench.py). "Directly answer" keeps the target lines above
# 0.5 while flagging 10-30x fewer lines elsewhere than the looser "relevant to" phrasing.
LINE_INSTRUCTION = "Does line {n} of the code directly answer {which}?"
LINE_INSTRUCTION_BROAD = "Is line {n} of the code relevant to {which}?"
BLOCK_INSTRUCTION = "Do lines {a}-{b} contain the code that {which} is looking for?"
# Filter terms are asked as positive category questions; polarity is applied in code (filters.py).
# Wording chosen by bench/filter_templates.py. It must cope with raw phrases like "source code files".
FILTER_SECTION_INSTRUCTION = "Does this file fall under the category: {term}?"
FILTER_BLOCK_INSTRUCTION = "Do lines {a}-{b} fall under the category: {term}?"
LINE_CRITERIA: dict | None = None


@dataclass
class Options:
    files_only: bool = False
    broad: bool = False
    jobs: int = 32
    chunk_lines: int = 150
    chunk_tokens: int = 14_000
    max_files: int = 1500
    triage: bool = False
    triage_threshold: float = 0.2
    rules: Rules | None = None   # natural-language include/exclude filter


@dataclass
class LineHit:
    line: int
    p: float
    text: str
    section: float = 1.0  # relevance (0..1) of the chunk this line was scored in
    keep: float = 1.0     # P(passes the --filter rules), inherited from the line's block


@dataclass
class BlockHit:
    start: int
    end: int
    p: float      # P(these lines contain the code the query is looking for)
    label: str    # first meaningful line of the block, usually a signature
    label_line: int = 0  # line number the label text comes from
    keep: float = 1.0    # P(passes the --filter rules)


@dataclass
class FileResult:
    path: str
    score: float = 0.0        # 0..1, best chunk relevance
    confidence: float = 0.0   # Jev's confidence in that chunk's relevance score
    keep: float = 1.0         # P(file passes the --filter rules): mean over its sections
    blocks: list[BlockHit] = field(default_factory=list)
    lines: list[LineHit] = field(default_factory=list)

    @property
    def best_evidence(self) -> float:
        return max([b.p for b in self.blocks] + [h.p for h in self.lines], default=0.0)

    @property
    def rank(self) -> float:
        return max(self.score, self.best_evidence) + 0.25 * min(self.score, self.best_evidence)


def block_label(lines: list[str]) -> str:
    """The line that best names a block: its definition if one opens it, else its first real line.

    Skips leading decorators, attributes and comments, so `@property` / `#[derive]` / a doc
    comment do not hide the `def` or `struct` right below them.
    """
    real = [l for l in lines if any(c.isalnum() for c in l)]
    for line in real[:6]:
        if DEFINITION.match(line):
            return line
    return real[0] if real else ""


def region_label(chunk: Chunk, a: int, b: int) -> tuple[str, int]:
    """A block's label and the line it came from. Mid-function blocks get the enclosing definition as a prefix."""
    label = block_label([chunk.text(n) for n in range(a, b + 1)])
    at = next((n for n in range(a, b + 1) if chunk.text(n) == label), a)
    return _with_owner(chunk, a, label), at


def _with_owner(chunk: Chunk, a: int, label: str) -> str:
    if not label or DEFINITION.match(label):
        return label.strip()
    depth = len(label) - len(label.lstrip())
    for n in range(a - 1, chunk.ctx_start - 1, -1):
        line = chunk.text(n)
        if line.strip() and len(line) - len(line.lstrip()) < depth:
            if DEFINITION.match(line):
                owner = line.strip()
                return f"{owner if len(owner) <= 60 else owner[:57] + '...'} > {label.strip()}"
            depth = len(line) - len(line.lstrip())
    return label.strip()


def _qid(i: int) -> str:
    return f"q{i}"


def build_request(queries: list[str], chunk: Chunk, files_only: bool, broad: bool = False,
                  rules: Rules | None = None) -> tuple[dict, dict]:
    width = len(str(chunk.end))
    listing = "\n".join(
        f"{n:>{width}}| {chunk.text(n)}" for n in range(chunk.ctx_start, chunk.end + 1)
    )
    state: dict = {"task": "code search: find where a codebase answers a natural-language query"}
    if len(queries) == 1:
        state["query"] = queries[0]
    else:
        state["queries"] = {_qid(i): q for i, q in enumerate(queries)}
    state["file"] = chunk.path
    state["shown_lines"] = f"{chunk.ctx_start}-{chunk.end}"
    state["code"] = listing

    questions: dict = {}
    # Filter questions are independent of the query, so they are asked once per chunk. The filter
    # text stays out of `state`: state is shared, and there it skews the relevance answers.
    for t, term in enumerate(rules.terms() if rules else []):
        questions[f"F{t}.S"] = {"type": "noul", "instructions": FILTER_SECTION_INSTRUCTION.format(term=term)}
        if not files_only:
            for a, b in chunk.blocks:
                questions[f"F{t}.B{a}-{b}"] = {
                    "type": "noul", "instructions": FILTER_BLOCK_INSTRUCTION.format(a=a, b=b, term=term),
                }
    askable = [] if files_only else chunk.askable()
    for i, q in enumerate(queries):
        questions[f"{_qid(i)}.rel"] = {
            "type": "score",
            "instructions": f"How relevant is this section of {chunk.path} to the query: {q}",
            "criteria": RELEVANCE_LEVELS,
        }
        which = "the query" if len(queries) == 1 else f"query {_qid(i)} ({q})"
        if not files_only:
            for a, b in chunk.blocks:
                questions[f"{_qid(i)}.B{a}-{b}"] = {
                    "type": "noul", "instructions": BLOCK_INSTRUCTION.format(a=a, b=b, which=which),
                }
        for n in askable:
            wording = LINE_INSTRUCTION_BROAD if broad else LINE_INSTRUCTION
            question = {"type": "noul", "instructions": wording.format(n=n, which=which)}
            if LINE_CRITERIA:
                question["criteria"] = LINE_CRITERIA
            questions[f"{_qid(i)}.L{n}"] = question
    return state, questions


def _ask_chunk(client: JevClient, queries: list[str], chunk: Chunk, files_only: bool, broad: bool = False,
               rules: Rules | None = None) -> list[tuple[Chunk, dict]]:
    state, questions = build_request(queries, chunk, files_only, broad, rules)
    try:
        return [(chunk, client.ask(state, questions))]
    except TokenLimitError:
        parts = split_chunk(chunk)
        if not parts:
            return []
        out: list[tuple[Chunk, dict]] = []
        for part in parts:
            out.extend(_ask_chunk(client, queries, part, files_only, broad, rules))
        return out


def triage_paths(client: JevClient, queries: list[str], paths: list[str], jobs: int) -> dict[str, float]:
    """Cheap pre-filter on file paths alone. Returns path -> P(worth opening)."""
    batches = [paths[i : i + 250] for i in range(0, len(paths), 250)]
    wanted = queries[0] if len(queries) == 1 else " | ".join(queries)

    def run(batch: list[str]) -> dict[str, float]:
        state = {
            "task": "decide which files are worth opening for a code search",
            "query": wanted,
            "paths": {f"p{i}": p for i, p in enumerate(batch)},
        }
        questions = {
            f"p{i}": {
                "type": "noul",
                "instructions": f"Judging by its path, could the file paths.p{i} ({p}) plausibly "
                                f"contain code relevant to the query?",
            }
            for i, p in enumerate(batch)
        }
        answers = client.ask(state, questions)
        return {p: float(answers.get(f"p{i}", {}).get("noul", 1.0)) for i, p in enumerate(batch)}

    scores: dict[str, float] = {}
    with cf.ThreadPoolExecutor(max_workers=max(1, min(jobs, len(batches)))) as pool:
        for part in pool.map(run, batches):
            scores.update(part)
    return scores


def search(
    client: JevClient,
    queries: list[str],
    files: list[Path],
    opts: Options,
    progress: Callable[[int, int], None] | None = None,
    note: Callable[[str], None] | None = None,
) -> list[list[FileResult]]:
    """Returns one ranked FileResult list per query (unfiltered; callers apply thresholds)."""
    named = [(display_path(p), p) for p in files]
    if opts.triage or len(named) > opts.max_files:
        scores = triage_paths(client, queries, [d for d, _ in named], opts.jobs)
        kept = sorted(
            (x for x in named if scores.get(x[0], 1.0) >= opts.triage_threshold),
            key=lambda x: -scores.get(x[0], 1.0),
        )[: opts.max_files]
        if note:
            note(f"path triage kept {len(kept)} of {len(named)} files")
        named = kept

    per_query: list[dict[str, FileResult]] = [{} for _ in queries]
    lock = threading.Lock()
    errors: list[str] = []

    rules = opts.rules
    terms = rules.terms() if rules else []
    section_keeps: dict[str, list[float]] = {}

    def absorb(chunk: Chunk, answers: dict) -> None:
        # A term applies to a block if either the section or the block itself falls under it:
        # sections decide file kinds (tests, docs), blocks decide constructs (imports, comments).
        section_p = {t: float((answers.get(f"F{k}.S") or {}).get("noul", 0.5)) for k, t in enumerate(terms)}
        section_keep = rules.passes(section_p) if rules else 1.0
        block_keep: dict[tuple[int, int], float] = {}
        for a, b in chunk.blocks:
            if rules:
                own = {t: float((answers.get(f"F{k}.B{a}-{b}") or {}).get("noul", 0.0)) for k, t in enumerate(terms)}
                block_keep[(a, b)] = rules.passes({t: max(section_p[t], own[t]) for t in terms})
            else:
                block_keep[(a, b)] = 1.0
        keep_of = lambda n: next((k for (a, b), k in block_keep.items() if a <= n <= b), section_keep)
        with lock:
            section_keeps.setdefault(chunk.path, []).append(section_keep)
        for i in range(len(queries)):
            rel = answers.get(f"{_qid(i)}.rel") or {}
            top = max(1, len(RELEVANCE_LEVELS) - 1)
            score = float(rel.get("score", 0.0)) / top
            hits = []
            for n in range(chunk.start, chunk.end + 1):
                a = answers.get(f"{_qid(i)}.L{n}")
                if a is not None:
                    hits.append(LineHit(n, float(a.get("noul", 0.0)), chunk.text(n), score, keep_of(n)))
            blocks = []
            for a, b in chunk.blocks:
                ans = answers.get(f"{_qid(i)}.B{a}-{b}")
                if ans is not None:
                    label, at = region_label(chunk, a, b)
                    blocks.append(BlockHit(a, b, float(ans.get("noul", 0.0)), label, at, block_keep[(a, b)]))
            with lock:
                fr = per_query[i].setdefault(chunk.path, FileResult(chunk.path))
                if score >= fr.score and section_keep >= FILTER_GATE:  # a filtered-out section cannot carry the file
                    fr.score = score
                    fr.confidence = float(rel.get("confidence", 0.0))
                fr.lines.extend(hits)
                fr.blocks.extend(blocks)

    def work(chunk: Chunk) -> None:
        for part, answers in _ask_chunk(client, queries, chunk, opts.files_only, opts.broad, opts.rules):
            absorb(part, answers)

    done = 0
    with cf.ThreadPoolExecutor(max_workers=max(1, opts.jobs)) as pool:
        futures = []
        for shown, path in named:
            lines = read_lines(path)
            if not lines:
                continue
            per_line = 0 if opts.files_only else len(queries) + 0.12 * len(terms)  # ~1 block question per 8 lines
            for chunk in chunk_lines(shown, lines, max_lines=opts.chunk_lines, max_tokens=opts.chunk_tokens,
                                     questions_per_line=per_line):
                futures.append(pool.submit(work, chunk))
        total = len(futures)
        try:
            for fut in cf.as_completed(futures):
                try:
                    fut.result()
                except JevError as e:
                    if type(e).__name__ == "JevAuthError":
                        raise
                    errors.append(str(e))
                done += 1
                if progress:
                    progress(done, total)
        except BaseException:
            for f in futures:
                f.cancel()
            raise

    if errors and note:
        note(f"{len(errors)} request(s) failed, results may be incomplete; first error: {errors[0]}")
    if errors and len(errors) == len(futures):
        raise JevError(errors[0])

    ranked = []
    for table in per_query:
        for fr in table.values():
            keeps = section_keeps.get(fr.path) or [1.0]
            fr.keep = sum(keeps) / len(keeps)
            fr.lines.sort(key=lambda h: h.line)
            fr.blocks.sort(key=lambda b: b.start)
        ranked.append(sorted(table.values(), key=lambda fr: (-fr.rank, fr.path)))
    return ranked
