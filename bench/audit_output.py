"""Audit what jg displays for invariants: no sub-floor rows, no source line printed twice.

Usage: fnox exec -- uv run python bench/audit_output.py
"""
import glob
from collections import Counter
from jevgrep import search as S
from jevgrep.client import JevClient, resolve_api_key
from jevgrep.files import discover
from jevgrep.results import select

HTTPX = glob.glob(".venv/lib/python3*/site-packages/httpx")[0]
RUNS = [
    (["src", "bench", "tests", "README.md"], ["where is the integration with jev defined", "where is the API key looked up", "how are rate limits handled"]),
    ([HTTPX], ["what is the default request timeout", "how does digest authentication work", "which HTTP method is used after a 303 redirect",
               "where does the client give up after following too many redirects", "how are URLs parsed and normalized"]),
]
with JevClient(resolve_api_key()) as client:
    for roots, queries in RUNS:
        for query, results in zip(queries, S.search(client, queries, discover(roots), S.Options())):
            raw_dupes = sum(c - 1 for fr in results for c in Counter(h.line for h in fr.lines).values() if c > 1)
            rows, low, printed_twice = 0, [], []
            for v in select(results):
                seen = Counter()
                for r in v.regions:
                    rows += 1
                    seen[getattr(r, "label_line", r.start)] += 1
                    if r.p < (0.35 if v.weak else 0.5): low.append(f"{v.path}:{r.start}-{r.end}={r.p:.2f}")
                    for h in r.lines:
                        rows += 1; seen[h.line] += 1
                for h in getattr(v, "lines", []):
                    rows += 1; seen[h.line] += 1
                printed_twice += [f"{v.path}:{n}" for n, c in seen.items() if c > 1]
            print(f"{query[:60]:60} rows={rows:>3}  same line scored twice by API={raw_dupes}  rows below their bar={len(low)} {low[:3]}  line printed twice={len(printed_twice)} {printed_twice[:3]}")
