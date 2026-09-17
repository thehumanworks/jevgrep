#!/usr/bin/env python3
"""Known-answer benchmark on the httpx 0.28.1 source, scored on what jg actually displays.

Usage:  cargo build --release && fnox exec -- python3 bench/bench.py [CORPUS_DIR]
Corpus: pip install --no-deps --target bench/corpus httpx==0.28.1   (bench/corpus is gitignored)
Pinpoint cases have a narrow truth range; broad cases accept any region in the right part of a file.
Standard library only: it drives the jg binary through --json.
"""
import json, os, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
JG = os.environ.get("JG", os.path.join(HERE, "..", "target", "release", "jg"))
ROOT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, "corpus", "httpx")
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

queries = [c[1] for c in CASES]
cmd = [JG, queries[0], ROOT, "--json"] + [x for q in queries[1:] for x in ("-e", q)]
t = time.time()
run = subprocess.run(cmd, capture_output=True, text=True)
print(f"{run.stderr.strip().splitlines()[-1] if run.stderr.strip() else ''}  (wall {time.time() - t:.1f}s, exit {run.returncode})\n")
if run.returncode not in (0, 1):
    sys.exit(run.stderr)
views_by_query = {}
for line in run.stdout.splitlines():
    row = json.loads(line)
    views_by_query.setdefault(row["query"], []).append(row)

print(f"{'kind':5} {'file@':>5} {'region':>6} {'line':>5} | {'strong':>6} {'weak':>4} {'regions':>7} {'lines':>5} | query")
ok_file = ok_region = ok_line = pins = below_bar = twice = 0
for kind, query, suffix, ranges in CASES:
    views = views_by_query.get(query, [])
    overlap = lambda a, b: any(a <= hi and b >= lo for lo, hi in ranges)
    pos = next((i + 1 for i, v in enumerate(views) if v["path"].endswith(suffix)), 0)
    truth = [v for v in views if v["path"].endswith(suffix) and v["match"] == "strong"]
    region = max((r["p"] for v in truth for r in v["regions"] if overlap(r["start"], r["end"])), default=0)
    # A pinpoint counts when any printed row sits on a truth line: a nested line, a stand-alone line,
    # or a region whose label line it is (that line is folded into the region row, not repeated).
    line = max([h["p"] for v in truth for h in [*v["lines"], *(h for r in v["regions"] for h in r["lines"])] if overlap(h["line"], h["line"])]
               + [r["p"] for v in truth for r in v["regions"] if overlap(r["label_line"], r["label_line"])], default=0)
    strong = [v for v in views if v["match"] == "strong"]
    ok_file += pos == 1; ok_region += region >= .5
    if kind == "pin":
        pins += 1; ok_line += line >= .5
    for v in views:  # audit the display rules on live answers
        bar = 0.35 if v["match"] == "weak" else 0.5
        shown = [h for r in v["regions"] for h in r["lines"]] + v["lines"]
        below_bar += sum(r["p"] < bar for r in v["regions"]) + sum(h["p"] < 0.5 for h in shown)
        printed = [r["label_line"] for r in v["regions"]] + [h["line"] for h in shown]
        twice += len(printed) - len(set(printed))
    print(f"{kind:5} {pos:>5} {region:>6.2f} {line:>5.2f} | {len(strong):>6} {len(views) - len(strong):>4} "
          f"{sum(len(v['regions']) for v in strong):>7} {sum(len(v['lines']) + sum(len(r['lines']) for r in v['regions']) for v in strong):>5} | {query}")
n = len(CASES)
print(f"\nexpected file shown first: {ok_file}/{n}   expected region shown as a match: {ok_region}/{n}   "
      f"pinpoint line shown: {ok_line}/{pins}\nrows below their bar: {below_bar}   source lines printed twice: {twice}")
