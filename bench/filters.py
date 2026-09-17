"""A/B phrasings for natural-language include/exclude filters. Ground truth comes from file paths.

Usage: fnox exec -- uv run python bench/filters.py
Variants (all asked about the same file chunk):
  embed      plain question, filter text inside the instruction
  struct     structured instruction {question, filter, focus}
  crit       embed + true/false criteria
  state      filter lives in state, instruction references it by backticked path (docs' refund_policy pattern)
  split      the filter split into sentences, one atomic question each, combined with min() in code
  polar      hand-written only/not terms, each asked positively ("Is this file X?"), polarity applied in code
"""
import concurrent.futures as cf, os, re, statistics as st
from jevgrep.client import JevClient, resolve_api_key
from jevgrep.files import chunk_lines, discover, display_path, read_lines

UREQ = os.path.expanduser("~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ureq-3.4.2")
QUERY = "how are HTTP proxies configured"


def kind(path: str) -> str:
    p = path.replace(UREQ + "/", "")
    name = p.rsplit("/", 1)[-1]
    if p.startswith("tests/") or re.search(r"(^|/)test[_.]|_test\.|/testserver", p): return "test"
    if p.startswith("bench/"): return "bench"
    if p.startswith("examples/"): return "example"
    if name.endswith((".md", ".tpl")) or name == "RELEASE.txt": return "doc"
    if name.startswith("LICENSE"): return "license"
    if name.endswith((".toml", ".orig", ".yml", ".yaml", ".lock")) or name.startswith("."): return "config"
    return "code"

# filter text -> (kinds that must pass, kinds that must fail, polar form)
FILTERS = {
    "Source code files only. No documentation": ({"code", "test", "example", "bench"}, {"doc", "license"}, (["source code"], ["documentation"])),
    "No tests": ({"code", "doc", "example", "config", "license", "bench"}, {"test"}, ([], ["a test file"])),
    "Exclude documentation, examples and config files": ({"code", "test"}, {"doc", "example", "config"}, ([], ["documentation", "an example program", "a config file"])),
    "Only documentation": ({"doc"}, {"code", "test", "example", "config", "bench"}, (["documentation"], [])),
    "Exclude tests and benchmarks": ({"code", "doc", "example", "config"}, {"test", "bench"}, ([], ["a test file", "a benchmark script"])),
}
FOCUS = "Judge what kind of file or code this is. Ignore whether it is relevant to any search query."
CRIT = {"true": "Meets every inclusion rule and breaks no exclusion rule of the filter",
        "false": "Breaks at least one rule of the filter"}


def questions(ftext, polar):
    qs = {
        "embed": {"type": "noul", "instructions": f"Does this file satisfy the search filter: {ftext}"},
        "struct": {"type": "noul", "instructions": {"question": "Does this file satisfy every rule in `filter`?", "filter": ftext, "focus": FOCUS}},
        "crit": {"type": "noul", "instructions": f"Does this file satisfy the search filter: {ftext}", "criteria": CRIT},
    }
    for i, rule in enumerate(r.strip() for r in re.split(r"(?<=[.;])\s+", ftext) if r.strip()):
        qs[f"split{i}"] = {"type": "noul", "instructions": f"Does this file satisfy the search filter: {rule}"}
    for i, term in enumerate(polar[0]):
        qs[f"only{i}"] = {"type": "noul", "instructions": f"Is this file {term}?"}
    for i, term in enumerate(polar[1]):
        qs[f"not{i}"] = {"type": "noul", "instructions": f"Is this file {term}?"}
    return qs


REL = {"type": "score", "instructions": f"How relevant is this section to the query: {QUERY}",
       "criteria": ["Irrelevant", "Tangential", "Relevant", "Direct hit"]}


def run(client, files, ftext, polar):
    chunks = [c for p in files for c in chunk_lines(display_path(p), read_lines(p) or [])[:2]]
    def ask(c):
        w = len(str(c.end))
        base = {"task": "code search", "query": QUERY, "file": c.path.replace(UREQ + "/", ""),
                "code": "\n".join(f"{n:>{w}}| {c.text(n)}" for n in range(c.ctx_start, c.end + 1))}
        a0 = client.ask(base, {**questions(ftext, polar), "rel": REL})
        a1 = client.ask({**base, "filter": ftext}, {
            "state": {"type": "noul", "instructions": "Does the file shown in `file` and `code` satisfy `filter`?"},
            "rel": REL})
        return c.path, a0, a1
    per = {}
    with cf.ThreadPoolExecutor(24) as ex:
        for path, a0, a1 in ex.map(ask, chunks):
            d = per.setdefault(path, {})
            for k, a in a0.items():
                d.setdefault(k, []).append(a.get("noul", a.get("score")))
            d.setdefault("state", []).append(a1["state"]["noul"])
            d.setdefault("rel_with_filter_in_state", []).append(a1["rel"]["score"])
    out = {}
    for path, d in per.items():
        m = {k: st.mean(v) for k, v in d.items()}
        splits = [v for k, v in m.items() if k.startswith("split")]
        polars = [v for k, v in m.items() if k.startswith("only")] + [1 - v for k, v in m.items() if k.startswith("not")]
        out[path] = {"embed": m["embed"], "struct": m["struct"], "crit": m["crit"], "state": m["state"],
                     "split": min(splits), "polar": min(polars), "rel": m["rel"], "rel_s": m["rel_with_filter_in_state"]}
    return out


VARIANTS = ["embed", "struct", "crit", "state", "split", "polar"]
with JevClient(resolve_api_key()) as client:
    repos = {"ureq": discover([UREQ], hidden=False), "jevgrep": discover(["src", "tests", "bench", "README.md", "pyproject.toml"])}
    total = {v: [0, 0] for v in VARIANTS}
    for ftext, (must_pass, must_fail, polar) in FILTERS.items():
        for repo, files in repos.items():
            res = run(client, files, ftext, polar)
            labelled = [(p, kind(p)) for p in res if kind(p) in must_pass | must_fail]
            if not any(k in must_fail for _, k in labelled) or not any(k in must_pass for _, k in labelled):
                continue
            print(f"\n=== [{repo}] {ftext}   ({len(labelled)} labelled files)")
            for v in VARIANTS:
                ok = [(res[p][v] >= .5) == (k in must_pass) for p, k in labelled]
                lo_pass = min(res[p][v] for p, k in labelled if k in must_pass)
                hi_fail = max(res[p][v] for p, k in labelled if k in must_fail)
                wrong = [f"{p.replace(UREQ + '/', '')}={res[p][v]:.2f}" for (p, k), o in zip(labelled, ok) if not o]
                total[v][0] += sum(ok); total[v][1] += len(ok)
                print(f"  {v:7} acc={sum(ok)}/{len(ok)}  lowest pass={lo_pass:.2f}  highest fail={hi_fail:.2f}  margin={lo_pass - hi_fail:+.2f}  wrong: {wrong[:4]}")
            drift = [abs(r["rel"] - r["rel_s"]) for r in res.values()]
            flip = sum((r["rel"] >= 1.8) != (r["rel_s"] >= 1.8) for r in res.values())
            print(f"  relevance drift when `filter` sits in state: mean |d|={st.mean(drift):.2f} of 3, max={max(drift):.2f}, files crossing the display bar={flip}")
    print("\nTOTAL accuracy: " + "   ".join(f"{v}={a}/{n} ({a / n:.0%})" for v, (a, n) in total.items()))
    print(f"{client.usage.requests} requests, {client.usage.input_tokens:,} tokens, ${client.usage.cost_usd:.3f}")
