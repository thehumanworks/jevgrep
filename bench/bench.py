"""Known-answer benchmark on the httpx source, scored on what jg would actually display.

Usage: fnox exec -- uv run python bench/bench.py
Pinpoint cases have a narrow truth range; broad cases accept any region in the right part of a file.
"""
import glob, time
from jevgrep import search as S
from jevgrep.client import JevClient, resolve_api_key
from jevgrep.files import discover
from jevgrep.results import select

ROOT = glob.glob(".venv/lib/python3*/site-packages/httpx")[0]
CASES = [  # (kind, query, expected file suffix, acceptable line ranges)
    ("pin", "where does the client give up after following too many redirects", "_client.py", [(964, 1000), (1679, 1715)]),
    ("pin", "how are proxy settings picked up from environment variables", "_utils.py", [(30, 80)]),
    ("pin", "parsing the server's digest auth challenge header", "_auth.py", [(224, 254)]),
    ("pin", "how cookies set by a response get saved into the jar", "_models.py", [(1101, 1112)]),
    ("pin", "decompressing gzip response bodies", "_decoders.py", [(85, 107)]),
    ("pin", "what is the default request timeout", "_config.py", [(246, 246)]),
    ("pin", "how is the multipart form boundary string generated", "_multipart.py", [(232, 240)]),
    ("pin", "which HTTP method is used after a 303 redirect", "_client.py", [(494, 520)]),
    ("pin", "how is the character encoding of a response guessed when the server does not declare one", "_models.py", [(653, 675)]),
    ("broad", "how does digest authentication work", "_auth.py", [(175, 348)]),
    ("broad", "where is the integration with httpcore defined", "_transports/default.py", [(1, 500)]),
    ("broad", "where is the command line interface implemented", "_main.py", [(1, 600)]),
    ("broad", "how are URLs parsed and normalized", "_urlparse.py", [(1, 600)]),
    ("broad", "where are the exception types declared", "_exceptions.py", [(1, 400)]),
    ("broad", "how does the client support ASGI apps without a network", "_transports/asgi.py", [(1, 300)]),
]

files = discover([ROOT])
t = time.time()
with JevClient(resolve_api_key()) as client:
    ranked = S.search(client, [c[1] for c in CASES], files, S.Options())
    u = client.usage
print(f"{len(files)} files, {len(CASES)} queries, {u.requests} req, {u.retries} retries, {u.input_tokens:,} tok (${u.cost_usd:.4f}), {time.time() - t:.1f}s\n")
print(f"{'kind':5} {'file@':>5} {'region':>6} {'line':>5} | {'strong':>6} {'weak':>4} {'regions':>7} {'lines':>5} | query")
ok_file = ok_region = ok_line = pins = 0
for (kind, query, suffix, ranges), results in zip(CASES, ranked):
    views = select(results)
    overlap = lambda a, b: any(a <= hi and b >= lo for lo, hi in ranges)
    pos = next((i + 1 for i, v in enumerate(views) if v.path.endswith(suffix)), 0)
    truth = [v for v in views if v.path.endswith(suffix) and not v.weak]
    region = max((r.p for v in truth for r in v.regions if overlap(r.start, r.end)), default=0)
    # A pinpoint counts when any printed row sits on a truth line: a nested line, a stand-alone line,
    # or a region whose label line it is (that line is folded into the region row, not repeated).
    line = max([h.p for v in truth for h in [*v.lines, *(h for r in v.regions for h in r.lines)] if overlap(h.line, h.line)]
               + [r.p for v in truth for r in v.regions if overlap(r.label_line, r.label_line)], default=0)
    strong = [v for v in views if not v.weak]
    ok_file += pos == 1; ok_region += region >= .5
    if kind == "pin":
        pins += 1; ok_line += line >= .5
    print(f"{kind:5} {pos:>5} {region:>6.2f} {line:>5.2f} | {len(strong):>6} {len(views) - len(strong):>4} "
          f"{sum(len(v.regions) for v in strong):>7} {sum(len(v.lines) + sum(len(r.lines) for r in v.regions) for v in strong):>5} | {query}")
n = len(CASES)
print(f"\nexpected file shown first: {ok_file}/{n}   expected region shown as a match: {ok_region}/{n}   "
      f"pinpoint line shown: {ok_line}/{pins}")
