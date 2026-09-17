"""Natural-language filters: parsing into polar rules, question shape, and gating of displayed rows."""
import json

import httpx
import pytest

from jevgrep import client as C
from jevgrep import search as S
from jevgrep.cli import main
from jevgrep.files import chunk_lines
from jevgrep.filters import Rules, parse_filter
from jevgrep.results import filtered_out, select, select_files
from jevgrep.search import BlockHit, FileResult, LineHit


@pytest.mark.parametrize("text, only, exclude", [
    ("Source code files only. No documentation", [["Source code files"]], ["documentation"]),
    ("No tests", [], ["tests"]),
    ("Exclude documentation, examples and config files", [], ["documentation", "examples", "config files"]),
    ("Only documentation", [["documentation"]], []),
    ("source code only, no docs or tests", [["source code"]], ["docs", "tests"]),
    ("async code", [["async code"]], []),                                   # a bare phrase is an inclusion
    ("Only Rust or Python files; skip generated code", [["Rust", "Python files"]], ["generated code"]),
    ("Don't include vendored code. Just the public API", [["public API"]], ["vendored code"]),
    ("python files but not tests", [["python files"]], ["tests"]),
    ("Ignore comments and docstrings", [], ["comments", "docstrings"]),
    ("definitions only, not call sites or imports", [["definitions"]], ["call sites", "imports"]),
    ("without any tests, excluding the benchmarks", [], ["tests", "benchmarks"]),
    ("no tests. no tests.", [], ["tests"]),                                  # duplicates collapse
    ("  .  ", [], []),
])
def test_parse_filter(text, only, exclude):
    rules = parse_filter(text)
    assert (rules.only, rules.exclude) == (only, exclude)


def test_rules_apply_polarity_in_code():
    rules = parse_filter("Only Rust or Python. No tests")
    assert rules.terms() == ["Rust", "Python", "tests"]
    assert rules.passes({"Rust": 0.1, "Python": 0.9, "tests": 0.2}) == pytest.approx(0.8)   # any alternative, and not a test
    assert rules.passes({"Rust": 0.1, "Python": 0.9, "tests": 0.95}) == pytest.approx(0.05)  # excluded
    assert rules.passes({"Rust": 0.1, "Python": 0.2, "tests": 0.0}) == pytest.approx(0.2)    # no inclusion clause holds
    assert Rules().passes({}) == 1.0 and not Rules()
    assert rules.describe() == "only Rust or Python; not tests"


def test_filter_questions_are_positive_atomic_and_outside_state():
    chunk = chunk_lines("a.py", ["import os", "", "def f():", "    return os.getcwd()"])[0]
    state, qs = S.build_request(["q1", "q2"], chunk, False, rules=parse_filter("source code only. no tests"))
    assert "filter" not in json.dumps(state) and "tests" not in json.dumps(state)   # shared state stays clean
    assert qs["F0.S"]["instructions"] == "Does this file fall under the category: source code?"
    assert qs["F1.B3-4"]["instructions"] == "Do lines 3-4 fall under the category: tests?"
    assert all(" no " not in q["instructions"].lower() and "not" not in q["instructions"].lower()
               for k, q in qs.items() if k.startswith("F"))                          # never a negated question
    assert len([k for k in qs if k.startswith("F")]) == 2 * (1 + len(chunk.blocks))  # asked once, not per query
    _, qs = S.build_request(["q"], chunk, True, rules=parse_filter("no tests"))
    assert set(qs) == {"F0.S", "q0.rel"}                                             # files-only: section level only


def fr(path, score, blocks=(), lines=(), keep=1.0):
    return FileResult(path, score=score, confidence=0.8, keep=keep,
                      blocks=[BlockHit(a, b, p, f"def f{a}():", a, k) for a, b, p, k in blocks],
                      lines=[LineHit(n, p, f"line {n}", 1.0, k) for n, p, k in lines])


def test_filtered_rows_never_display_and_cannot_carry_a_file():
    code = fr("src/a.py", 0.9, blocks=[(1, 9, 0.8, 0.9), (20, 29, 0.9, 0.1)], lines=[(3, 0.7, 0.9), (22, 0.95, 0.1)])
    docs = fr("README.md", 0.9, blocks=[(1, 9, 0.9, 0.02)], lines=[(4, 0.9, 0.02)])
    near = fr("tests/t.py", 0.9, blocks=[(1, 9, 0.45, 0.1)])                  # would be weak tier, but filtered
    views = select([code, docs, near])
    assert [v.path for v in views] == ["src/a.py"]
    assert [(r.start, [h.line for h in r.lines]) for r in views[0].regions] == [(1, [3])]
    assert filtered_out([code, docs, near], 0.5) == (2, 2)


def test_files_only_mode_gates_on_file_keep():
    got = select_files([fr("a.py", 0.9, keep=0.8), fr("README.md", 0.95, keep=0.1)])
    assert [f.path for f in got] == ["a.py"]


def fake_jev(request: httpx.Request) -> httpx.Response:
    """Everything matches the query. `documentation` applies to .md files, `imports` to import lines."""
    body = json.loads(request.content)
    path, code = body["state"]["file"], {}
    for row in body["state"]["code"].splitlines():
        num, _, text = row.partition("| ")
        code[int(num)] = text
    answers = {}
    for qid, q in body["questions"].items():
        text = q["instructions"]
        if q["type"] == "score":
            answers[qid] = {"type": "score", "score": 3.0, "confidence": 0.9}
        elif qid.startswith("F"):
            if "documentation" in text:
                p = 0.97 if path.endswith(".md") else 0.03
            else:  # imports
                lo, hi = (map(int, qid.split(".B")[1].split("-"))) if ".B" in qid else (0, -1)
                p = 0.95 if any(code.get(n, "").startswith("import") for n in range(lo, hi + 1)) else 0.04
            answers[qid] = {"type": "noul", "noul": p}
        else:
            answers[qid] = {"type": "noul", "noul": 0.9}
    return httpx.Response(200, json={"answers": answers, "usage": {"input_tokens": 50}})


@pytest.fixture
def repo(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("TYPESAFE_API_KEY", "test-key")
    (tmp_path / "app.py").write_text("import os\nimport sys\n\n\ndef run():\n    return os.getcwd()\n")
    (tmp_path / "README.md").write_text("# App\n\nRun it.\n")
    real = C.JevClient.__init__
    def patched(self, *a, **kw):
        real(self, *a, **kw)
        self._http = httpx.Client(transport=httpx.MockTransport(fake_jev))
    monkeypatch.setattr(C.JevClient, "__init__", patched)


def shown_paths(out: str) -> list[str]:
    return [l.split()[0] for l in out.splitlines() if "relevance=" in l]


def test_cli_filter_end_to_end(repo, capsys):
    assert main(["anything"]) == 0
    assert shown_paths(capsys.readouterr().out) == ["README.md", "app.py"]

    assert main(["anything", "--filter", "Source code only. No documentation"]) == 0
    out = capsys.readouterr()
    assert shown_paths(out.out) == ["app.py"]
    assert "jg: filter: only Source code; not documentation" in out.err        # shows how the filter was read
    assert "filter removed" in out.err

    assert main(["anything", "--not", "imports", "--not", "documentation", "-q", "--no-heading"]) == 0
    rows = capsys.readouterr().out.splitlines()
    assert rows and all(r.startswith("app.py:") for r in rows)
    assert not any(r.split(":")[1] in ("1", "2", "1-2") for r in rows)          # the import block is gone: region-level filter

    assert main(["anything", "-l", "--only", "documentation", "-q"]) == 0
    assert capsys.readouterr().out.split()[1::2] == ["README.md"]

    assert main(["anything", "--filter", " . ", "-q"]) == 2                     # a filter that names nothing is an error
