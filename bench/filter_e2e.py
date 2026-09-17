"""End-to-end check of --filter on displayed rows: leaks (forbidden kinds shown) and retention.

Usage: fnox exec -- uv run python bench/filter_e2e.py
One search per case computes both views: gating applied, and gating ignored (the no-filter baseline).
"""
import copy, os, re
from jevgrep import search as S
from jevgrep.client import JevClient, resolve_api_key
from jevgrep.files import discover
from jevgrep.filters import parse_filter
from jevgrep.results import select

UREQ = os.path.expanduser("~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ureq-3.4.2")
REPOS = {"ureq": [UREQ], "jevgrep": ["src", "tests", "bench", "README.md", "pyproject.toml"]}


def kind(p: str) -> str:
    p = p.replace(UREQ + "/", ""); name = p.rsplit("/", 1)[-1]
    if p.startswith("tests/") or re.search(r"(^|/)test[_.]|_test\.|/testdata/", p): return "test"
    if p.startswith("bench/"): return "bench"
    if p.startswith("examples/"): return "example"
    if name.endswith((".md", ".tpl", ".txt")): return "doc"
    if name.endswith((".toml", ".orig", ".yml", ".lock")): return "config"
    return "code"

CASES = [  # repo, queries, filter, kinds that must not appear
    ("ureq", ["how are HTTP proxies configured", "how do I set a timeout on a request", "how are cookies stored"],
     "Source code only. No documentation", {"doc"}),
    ("ureq", ["how are HTTP proxies configured", "how do I set a timeout on a request", "how are cookies stored"],
     "Exclude documentation, examples and config files", {"doc", "example", "config"}),
    ("ureq", ["how are HTTP proxies configured", "how do I set a timeout on a request"], "only documentation", {"code", "example", "config", "test"}),
    ("jevgrep", ["how are rate limits handled", "where is the integration with jev defined", "how are files split into blocks"],
     "No tests or benchmarks", {"test", "bench"}),
    ("jevgrep", ["how are rate limits handled", "where is the integration with jev defined", "how are files split into blocks"],
     "source code only, no docs, no tests, no benchmarks", {"doc", "test", "bench"}),
]


COMMENT = ("///", "//!", "//", '"""', "#", "/*", "*")


def row_kind(path: str, start: int, end: int) -> str:
    """Path kind, except that a row made of comment lines inside a code file is documentation,
    which is how Jev (rightly) treats it. A doc comment plus the code it documents stays code."""
    k = kind(path)
    if k != "code":
        return k
    try:
        body = [l.strip() for l in open(path, encoding="utf-8", errors="replace").read().splitlines()[start - 1 : end] if l.strip()]
    except OSError:
        return k
    ratio = sum(l.startswith(COMMENT) and not l.startswith("#[") for l in body) / max(len(body), 1)
    return "doc" if ratio >= 0.85 else k


def code_lines(row_set, forbidden) -> set:
    """Non-comment, non-blank lines of allowed-kind files that the displayed rows cover."""
    out = set()
    for path, a, b in row_set:
        if kind(path) in forbidden:
            continue
        try:
            src = open(path, encoding="utf-8", errors="replace").read().splitlines()
        except OSError:
            continue
        for n in range(a, min(b, len(src)) + 1):
            t = src[n - 1].strip()
            if t and (kind(path) != "code" or not (t.startswith(COMMENT) and not t.startswith("#["))):
                out.add((path, n))
    return out


def rows(views):
    return {(v.path, r.start, r.end) if hasattr(r, "start") else (v.path, r.line, r.line)
            for v in views if not v.weak for r in v.rows()}


with JevClient(resolve_api_key()) as client:
    T = [0, 0, 0, 0]
    for repo, queries, ftext, forbidden in CASES:
        rules = parse_filter(ftext)
        ranked = S.search(client, queries, discover(REPOS[repo]), S.Options(rules=rules))
        print(f"\n=== [{repo}] {ftext}   -> {rules.describe()}")
        for q, results in zip(queries, ranked):
            ungated = copy.deepcopy(results)
            for fr in ungated:
                for x in [*fr.blocks, *fr.lines]:
                    x.keep = 1.0
            base, got = rows(select(ungated)), rows(select(results))
            bad_base = {r for r in base if row_kind(*r) in forbidden}
            leaks = {r for r in got if row_kind(*r) in forbidden}
            allowed = code_lines(base - bad_base, forbidden)
            kept = allowed & code_lines(got, forbidden)
            T[0] += len(leaks); T[1] += len(bad_base); T[2] += len(kept); T[3] += len(allowed)
            print(f"  forbidden rows shown: {len(leaks)} (was {len(bad_base)} unfiltered)   allowed lines still covered: {len(kept)}/{len(allowed)}"
                  f"   | {q}" + (f"   LEAKS {[(p.replace(UREQ + '/', ''), a, b) for p, a, b in sorted(leaks)[:3]]}" if leaks else ""))
    print(f"\nTOTAL forbidden rows shown: {T[0]} of {T[1]} that appear unfiltered ({1 - T[0] / max(T[1], 1):.0%} removed);  allowed lines still covered: {T[2]}/{T[3]} ({T[2] / max(T[3], 1):.0%})")
    print(f"{client.usage.requests} requests, {client.usage.input_tokens:,} tokens, ${client.usage.cost_usd:.3f}")
