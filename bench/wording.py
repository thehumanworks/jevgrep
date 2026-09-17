"""A/B question wordings inside the same requests (Jev evaluates questions independently).

Usage: fnox exec -- uv run python bench/wording.py
"""
import concurrent.futures as cf, glob
from jevgrep.client import JevClient, resolve_api_key
from jevgrep.files import chunk_lines, discover, display_path, read_lines

HTTPX = glob.glob(".venv/lib/python3*/site-packages/httpx")[0]
BLOCK = {
    "contain": "Do lines {a}-{b} contain the code that the query is looking for?",
    "read": "Would a developer need to read lines {a}-{b} to answer the query?",
}
LINE = {
    "answer": "Does line {n} of the code directly answer the query?",
    "key": "Is line {n} one of the key lines a developer should look at first to answer the query?",
}
# (root, query, truth file suffix, truth ranges) ; truth None = diffuse query, just print
CASES = [
    (["src", "bench", "tests", "README.md"], "where is the integration with jev defined", "client.py", [(122, 199)]),
    (["src", "bench", "tests", "README.md"], "where is the code that integrates with jev", "client.py", [(122, 199)]),
    ([HTTPX], "where is the command line interface implemented", "_main.py", [(1, 600)]),
    ([HTTPX], "how does digest authentication work", "_auth.py", [(175, 348)]),
    ([HTTPX], "where is the integration with httpcore defined", "_transports/default.py", [(1, 500)]),
    ([HTTPX], "where does the client give up after following too many redirects", "_client.py", [(964, 1000), (1679, 1715)]),
    ([HTTPX], "what is the default request timeout", "_config.py", [(246, 246)]),
    ([HTTPX], "which HTTP method is used after a 303 redirect", "_client.py", [(494, 520)]),
    ([HTTPX], "how is the multipart form boundary string generated", "_multipart.py", [(232, 240)]),
    ([HTTPX], "how cookies set by a response get saved into the jar", "_models.py", [(1101, 1112)]),
]


def run(client, roots, query):
    chunks = [c for p in discover(roots) for c in chunk_lines(display_path(p), read_lines(p) or [])]
    def ask(chunk):
        w = len(str(chunk.end))
        state = {"task": "code search: find where a codebase answers a natural-language query", "query": query,
                 "file": chunk.path, "code": "\n".join(f"{n:>{w}}| {chunk.text(n)}" for n in range(chunk.ctx_start, chunk.end + 1))}
        qs = {}
        for name, tpl in BLOCK.items():
            for a, b in chunk.blocks:
                qs[f"B.{name}.{a}.{b}"] = {"type": "noul", "instructions": tpl.format(a=a, b=b)}
        for name, tpl in LINE.items():
            for n in chunk.askable():
                qs[f"L.{name}.{n}"] = {"type": "noul", "instructions": tpl.format(n=n)}
        return chunk, client.ask(state, qs)
    blocks, lines = {k: [] for k in BLOCK}, {k: [] for k in LINE}
    with cf.ThreadPoolExecutor(32) as ex:
        for chunk, ans in ex.map(ask, chunks):
            for qid, a in ans.items():
                kind, name, *rest = qid.split(".")
                if kind == "B":
                    blocks[name].append((a["noul"], chunk.path, int(rest[0]), int(rest[1]), chunk.text(int(rest[0]))))
                else:
                    lines[name].append((a["noul"], chunk.path, int(rest[0]), chunk.text(int(rest[0]))))
    return blocks, lines


with JevClient(resolve_api_key()) as client:
    for roots, query, suffix, ranges in CASES:
        blocks, lines = run(client, roots, query)
        print(f"\n=== {query}   [truth: {suffix} {ranges}]")
        hit = lambda path, a, b: path.endswith(suffix) and any(a <= hi and b >= lo for lo, hi in ranges)
        for name, rows in blocks.items():
            rows.sort(reverse=True)
            n5 = sum(1 for r in rows if r[0] >= .5)
            wrong5 = sum(1 for r in rows if r[0] >= .5 and not hit(r[1], r[2], r[3]))
            rank = next((i + 1 for i, r in enumerate(rows) if hit(r[1], r[2], r[3])), 0)
            print(f" block/{name}: first-truth-rank={rank}  blocks>=.5: {n5} (outside truth: {wrong5})")
            for p, path, a, b, text in rows[:5]:
                print(f"    {p:.2f} {'*' if hit(path, a, b) else ' '} {path.split('site-packages/')[-1]}:{a}-{b}  {text.strip()[:60]}")
        for name, rows in lines.items():
            rows.sort(reverse=True)
            n5 = sum(1 for r in rows if r[0] >= .5)
            wrong5 = sum(1 for r in rows if r[0] >= .5 and not hit(r[1], r[2], r[2]))
            print(f" line/{name}: lines>=.5: {n5} (outside truth: {wrong5})  top: " + " | ".join(f"{p:.2f}{'*' if hit(path, n, n) else ' '}{path.rsplit('/', 1)[-1]}:{n}" for p, path, n, _ in rows[:5]))
    print(f"\n{client.usage.requests} requests, {client.usage.input_tokens:,} tokens, ${client.usage.cost_usd:.3f}, retries={client.usage.retries}")
