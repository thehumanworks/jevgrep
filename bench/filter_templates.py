"""A/B question templates for polar filter terms at region (block) and section level.

Usage: fnox exec -- uv run python bench/filter_templates.py
Terms come straight from parse_filter(), so templates must cope with raw phrases ("source code files").
"""
import concurrent.futures as cf, os, re
from jevgrep.client import JevClient, resolve_api_key
from jevgrep.files import chunk_lines, discover, display_path, read_lines
from jevgrep.filters import parse_filter

UREQ = os.path.expanduser("~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ureq-3.4.2")
BLOCK = {
    "category": "Do lines {a}-{b} fall under the category: {term}?",
}
SECTION = {
    "is": "Is this file {term}?",
    "category": "Does this file fall under the category: {term}?",
}


def file_kind(p: str) -> str:
    p = p.replace(UREQ + "/", ""); name = p.rsplit("/", 1)[-1]
    if p.startswith("tests/") or re.search(r"(^|/)test[_.]|_test\.|/testdata/", p): return "test"
    if p.startswith("bench/"): return "bench"
    if p.startswith("examples/"): return "example"
    if name.endswith((".md", ".tpl")) or name == "RELEASE.txt": return "doc"
    if name.startswith("LICENSE"): return "license"
    if name.endswith((".toml", ".orig", ".yml", ".yaml", ".lock")): return "config"
    return "code"


COMMENT = ("//", "#", "*", "/*", '"""', "''''")


def block_kind(path, lines, a, b) -> set[str]:
    kinds = {file_kind(path)}
    body = [l.strip() for l in lines[a - 1 : b] if l.strip()]
    if kinds == {"code"} and body:
        ratio = sum(l.startswith(COMMENT) and not l.startswith("#[") for l in body) / len(body)
        if ratio >= 0.85: kinds = {"doc"}            # a pure doc-comment block is documentation
        elif ratio > 0.25: kinds = {"mixed"}         # doc comment + the code it documents
    if any("#[cfg(test)]" in l for l in lines[: a]) and path.endswith(".rs"):
        kinds = {"test"}
    if body and all(re.match(r"(import |from \S+ import |use |pub use |extern crate |mod \w+;|pub mod \w+;|#!?\[|\)|[\w{}, ]+,?$)", l) for l in body) \
            and any(re.match(r"(import |from |use |pub use )", l) for l in body):
        kinds.add("imports")
    return kinds

# filter -> (block passes if kinds & must_pass, fails if kinds & must_fail; others unscored)
FILTERS = {
    "No documentation": ({"code", "test", "example", "bench", "mixed"}, {"doc", "license"}),
    "Source code files only. No documentation": ({"code", "test", "example", "bench", "mixed"}, {"doc", "license"}),
    "No tests": ({"code", "doc", "example", "config", "mixed"}, {"test"}),
    "Only documentation": ({"doc"}, {"code", "test", "example", "config", "bench"}),
    "Ignore imports": (None, {"imports"}),
}

with JevClient(resolve_api_key()) as client:
    files = discover([UREQ]) + discover(["src", "tests", "bench", "README.md", "pyproject.toml"])
    work = []
    for p in files:
        lines = read_lines(p) or []
        for c in chunk_lines(display_path(p), lines)[:3]:
            work.append((p, lines, c))
    mixed = {}
    sweep = []
    totals = {("B", k): [0, 0, 0, 0] for k in BLOCK} | {("S", k): [0, 0, 0, 0] for k in SECTION}
    for ftext, (must_pass, must_fail) in FILTERS.items():
        rules = parse_filter(ftext)
        def ask(item):
            p, lines, c = item
            w = len(str(c.end))
            state = {"task": "code search", "query": "how are HTTP proxies configured", "file": c.path.replace(UREQ + "/", ""),
                     "code": "\n".join(f"{n:>{w}}| {c.text(n)}" for n in range(c.ctx_start, c.end + 1))}
            qs = {}
            for ti, term in enumerate(rules.terms()):
                for k, tpl in SECTION.items():
                    qs[f"S|{k}|{ti}"] = {"type": "noul", "instructions": tpl.format(term=term)}
                for k, tpl in BLOCK.items():
                    for a, b in c.blocks:
                        qs[f"B|{k}|{ti}|{a}|{b}"] = {"type": "noul", "instructions": tpl.format(a=a, b=b, term=term)}
            return item, client.ask(state, qs)
        per = {key: [0, 0, 0, 0] for key in totals}   # ok_pass, n_pass, ok_fail, n_fail
        wrong = {key: [] for key in totals}
        with cf.ThreadPoolExecutor(24) as ex:
            for (p, lines, c), ans in ex.map(ask, work):
                terms = rules.terms()
                for k in SECTION:
                    score = rules.passes({t: ans[f"S|{k}|{i}"]["noul"] for i, t in enumerate(terms)})
                    fk = file_kind(str(p))
                    truth = True if must_pass and fk in must_pass else False if fk in must_fail else None
                    if truth is not None and "imports" not in must_fail:
                        s = per[("S", k)]; s[1 if truth else 3] += 1; s[0 if truth else 2] += (score >= .5) == truth
                for k in BLOCK:
                    for a, b in c.blocks:
                        score = rules.passes({t: ans[f"B|{k}|{i}|{a}|{b}"]["noul"] for i, t in enumerate(terms)})
                        combined = rules.passes({t: max(ans[f"B|{k}|{i}|{a}|{b}"]["noul"], ans[f"S|category|{i}"]["noul"]) for i, t in enumerate(terms)})
                        kinds = block_kind(str(p), lines, a, b)
                        if kinds & must_fail: truth = False
                        elif must_pass is None: truth = True if file_kind(str(p)) in {"code", "test", "bench", "example"} else None
                        elif kinds & must_pass: truth = True
                        else: truth = None
                        if truth is None: continue
                        sweep.append(("mixed" if "mixed" in kinds else truth, combined))
                        if "mixed" in kinds:
                            m = mixed.setdefault((ftext, k), [0, 0]); m[1] += 1; m[0] += score >= .5
                            continue
                        s = per[("B", k)]; s[1 if truth else 3] += 1
                        good = (score >= .5) == truth
                        s[0 if truth else 2] += good
                        if not good and len(wrong[("B", k)]) < 3:
                            wrong[("B", k)].append(f"{c.path.replace(UREQ + '/', '')}:{a}-{b}={score:.2f}({'pass' if truth else 'fail'} expected) {c.text(a).strip()[:40]!r}")
        print(f"\n=== {ftext}   -> {rules.describe()}")
        for key, (okp, np_, okf, nf) in per.items():
            if np_ + nf == 0: continue
            for i, v in enumerate((okp, np_, okf, nf)): totals[key][i] += v
            print(f"  {key[0]}/{key[1]:9} kept {okp}/{np_} that should pass ({okp / max(np_, 1):.0%}), dropped {okf}/{nf} that should fail ({okf / max(nf, 1):.0%})   e.g. wrong: {wrong[key][:2]}")
    print("\nMIXED blocks (doc comment + the code it documents), share kept:")
    for (ftext, k), (kept, n) in mixed.items():
        print(f"  {ftext[:42]:42} B/{k:9} kept {kept}/{n} ({kept / n:.0%})")
    print("\nGATE SWEEP on the combined score the tool uses (max of section and block per term):")
    for gate in (0.5, 0.45, 0.4, 0.35, 0.3, 0.25):
        keep = [sc >= gate for t, sc in sweep if t is True]; drop = [sc < gate for t, sc in sweep if t is False]
        mix = [sc >= gate for t, sc in sweep if t == "mixed"]
        print(f"  gate {gate:.2f}: keeps {sum(keep) / len(keep):.1%} of rows that should pass, drops {sum(drop) / len(drop):.1%} of rows that should fail, keeps {sum(mix) / len(mix):.0%} of mixed doc+code blocks")
    print("\nTOTAL")
    for key, (okp, np_, okf, nf) in totals.items():
        print(f"  {key[0]}/{key[1]:9} keep-rate {okp / np_:.1%} ({okp}/{np_})   drop-rate {okf / nf:.1%} ({okf}/{nf})   balanced accuracy {(okp / np_ + okf / nf) / 2:.1%}")
    print(f"{client.usage.requests} requests, {client.usage.input_tokens:,} tokens, ${client.usage.cost_usd:.3f}, retries={client.usage.retries}")
