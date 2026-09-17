"""Dump raw Jev answers for a query: section score distributions and top lines per file.

Usage: fnox exec -- uv run python bench/inspect_query.py "query" [path ...] [--broad]
"""
import sys
from jevgrep import search as S
from jevgrep.client import JevClient, resolve_api_key
from jevgrep.files import discover

args = [a for a in sys.argv[1:] if a != "--broad"]
query, paths = args[0], args[1:] or ["."]
raw = []
orig = JevClient.ask
def spy(self, state, questions):
    ans = orig(self, state, questions)
    raw.append((state.get("file"), state.get("shown_lines"), ans.get("q0.rel")))
    return ans
JevClient.ask = spy
with JevClient(resolve_api_key()) as c:
    res = S.search(c, [query], discover(paths), S.Options(broad="--broad" in sys.argv))[0]
for fr in res[:8]:
    print(f"\n{fr.path}  score={fr.score:.2f} conf={fr.confidence:.2f}")
    for f, shown, rel in raw:
        if f == fr.path and rel:
            print(f"   section {shown}: score={rel['score']:.2f} probs={ {k: round(v, 2) for k, v in sorted(rel['probabilities'].items())} }")
    for h in sorted(fr.lines, key=lambda h: -h.p)[:8]:
        print(f"   {h.line:>5} {h.p:.2f}  {h.text[:100]}")
