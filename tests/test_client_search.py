import json
import threading

import httpx
import pytest

from jevgrep import client as C
from jevgrep import search as S
from jevgrep.cli import main
from jevgrep.files import chunk_lines


def make_client(handler, **kw) -> C.JevClient:
    c = C.JevClient("test-key", **kw)
    c.limiter.pause = 0.0
    c._http = httpx.Client(transport=httpx.MockTransport(handler), headers={"Authorization": "Bearer test-key"})
    return c


def answer_all(request: httpx.Request, hot=("needle",)) -> httpx.Response:
    """Fake Jev: a line is a hit when its text contains one of `hot`; a block when any of its lines does."""
    body = json.loads(request.content)
    code = {}
    for row in body["state"].get("code", "").splitlines():
        num, _, text = row.partition("| ")
        code[int(num)] = text
    is_hot = lambda n: any(h in code.get(n, "") for h in hot)
    answers = {}
    for qid, q in body["questions"].items():
        kind = qid.split(".", 1)[1]
        if q["type"] == "score":
            answers[qid] = {"type": "score", "score": 3.0 if any(map(is_hot, code)) else 0.0,
                            "confidence": 0.9, "probabilities": {}, "legend": {}}
        elif kind.startswith("B"):
            lo, hi = map(int, kind[1:].split("-"))
            answers[qid] = {"type": "noul", "noul": 0.9 if any(is_hot(n) for n in range(lo, hi + 1)) else 0.03}
        else:
            answers[qid] = {"type": "noul", "noul": 0.95 if is_hot(int(kind[1:])) else 0.02}
    return httpx.Response(200, json={"model": "jev-test", "answers": answers, "usage": {"input_tokens": 100, "output_tokens": 10}})


@pytest.fixture(autouse=True)
def no_sleep(monkeypatch):
    monkeypatch.setattr(C.time, "sleep", lambda s: None)


def test_build_request_single_and_multi_query():
    chunk = chunk_lines("a.py", ["import os", "", "def f():", "    return os.getcwd()"])[0]
    state, qs = S.build_request(["where is cwd read"], chunk, files_only=False)
    assert state["query"] == "where is cwd read" and "4|     return os.getcwd()" in state["code"]
    assert set(qs) == {"q0.rel", "q0.B1-1", "q0.B3-4", "q0.L1", "q0.L3", "q0.L4"}  # blank line 2 is never asked about
    assert "lines 3-4 contain" in qs["q0.B3-4"]["instructions"]
    assert qs["q0.rel"]["type"] == "score" and len(qs["q0.rel"]["criteria"]) == 4
    assert "directly answer" in qs["q0.L4"]["instructions"]
    assert "relevant to" in S.build_request(["q"], chunk, False, broad=True)[1]["q0.L4"]["instructions"]

    state, qs = S.build_request(["first", "second"], chunk, files_only=False)
    assert state["queries"] == {"q0": "first", "q1": "second"}
    assert {"q0.rel", "q1.rel", "q0.B3-4", "q1.B3-4", "q0.L4", "q1.L4"} <= set(qs)
    assert "second" in qs["q1.L4"]["instructions"]

    _, qs = S.build_request(["q"], chunk, files_only=True)
    assert set(qs) == {"q0.rel"}


def test_search_aggregates_ranks_and_handles_multiple_queries(tmp_path):
    (tmp_path / "hay.py").write_text("\n".join(f"hay_{i} = {i}" for i in range(400)))
    (tmp_path / "target.py").write_text("\n".join(["a = 1"] * 200 + ["needle = find()"] + ["b = 2"] * 50))
    seen = []
    def handler(req):
        seen.append(threading.get_ident())
        return answer_all(req)
    client = make_client(handler)
    res = S.search(client, ["find the needle", "unrelated"], sorted(tmp_path.iterdir()), S.Options(jobs=8))
    assert len(res) == 2
    top = res[0][0]
    assert top.path.endswith("target.py") and top.score == 1.0
    assert [h.line for h in top.lines if h.p >= 0.5] == [201]
    assert [(b.start <= 201 <= b.end) for b in top.blocks if b.p >= 0.5] == [True]
    assert all(h.p < 0.5 for fr in res[0][1:] for h in fr.lines)
    assert client.usage.requests == len({(id(r)) for r in seen}) or client.usage.requests >= 4  # both queries share each request
    assert len(set(seen)) > 1  # requests really ran on multiple threads


def test_token_limit_splits_chunk_and_retries_halves(tmp_path):
    (tmp_path / "big.py").write_text("\n".join(["x = 1"] * 99 + ["needle()"]))
    sizes = []
    def handler(req):
        n = sum(1 for q in json.loads(req.content)["questions"] if ".L" in q)
        sizes.append(n)
        if n > 30:
            return httpx.Response(400, json={"detail": {"error_type": "max_tokens_exceeded"}})
        return answer_all(req)
    res = S.search(make_client(handler), ["needle"], [tmp_path / "big.py"], S.Options(jobs=1))
    assert sizes[0] == 100 and all(n <= 30 for n in sizes if n not in sizes[:1] + [50])  # halved until it fits
    assert [h.line for h in res[0][0].lines if h.p >= 0.5] == [100]
    assert len(res[0][0].lines) == 100  # every line scored exactly once despite splitting


def test_retries_on_429_then_succeeds_and_limiter_recovers():
    calls = []
    def handler(req):
        calls.append(1)
        if len(calls) < 3:
            return httpx.Response(429, json={"detail": {"message": "Rate limit exceeded"}})
        return answer_all(req)
    c = make_client(handler, pool_size=16)
    chunk = chunk_lines("a.py", ["needle"])[0]
    answers = c.ask(*S.build_request(["q"], chunk, False))
    assert answers["q0.L1"]["noul"] == 0.95
    assert c.usage.retries == 2 and c.usage.requests == 1
    assert c.limiter.limit < 16 and c.limiter.in_flight == 0


def test_auth_error_is_not_retried():
    calls = []
    def handler(req):
        calls.append(1)
        return httpx.Response(401, json={"detail": "bad key"})
    with pytest.raises(C.JevAuthError):
        make_client(handler).ask({}, {})
    assert len(calls) == 1


def test_gives_up_after_max_retries():
    c = make_client(lambda req: httpx.Response(529, text="overloaded"), max_retries=2)
    with pytest.raises(C.JevError, match="gave up"):
        c.ask({}, {})
    assert c.usage.retries == 2


def test_path_triage_keeps_likely_files(tmp_path):
    def handler(req):
        body = json.loads(req.content)
        if "paths" in body["state"]:
            return httpx.Response(200, json={"answers": {k: {"type": "noul", "noul": 0.9 if "auth" in v else 0.01}
                                                         for k, v in body["state"]["paths"].items()}, "usage": {}})
        return answer_all(req, hot=("token",))
    for name in ["auth.py", "billing.py", "ui.py"]:
        (tmp_path / name).write_text("token = load()\n")
    notes = []
    res = S.search(make_client(handler), ["auth token"], sorted(tmp_path.iterdir()), S.Options(triage=True), note=notes.append)
    assert [fr.path.rsplit("/", 1)[-1] for fr in res[0]] == ["auth.py"]
    assert "kept 1 of 3" in notes[0]


def test_cli_end_to_end_text_json_and_exit_codes(tmp_path, monkeypatch, capsys):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("TYPESAFE_API_KEY", "test-key")
    (tmp_path / "t.py").write_text("a = 1\nneedle = 2\nb = 3\n")
    real = C.JevClient.__init__
    def patched(self, *a, **kw):
        real(self, *a, **kw)
        self._http = httpx.Client(transport=httpx.MockTransport(answer_all))
    monkeypatch.setattr(C.JevClient, "__init__", patched)

    assert main(["find needle", "-C", "1"]) == 0
    out = capsys.readouterr()
    assert out.out.splitlines() == [
        "t.py  relevance=1.00",
        "        1-3  0.90  a = 1",          # line 1 is the region label, so -C does not print it again
        "          2  0.95  needle = 2",
        "          3        b = 3",
        "",
    ]
    assert "1 files, 1 requests" in out.err

    assert main(["find needle", "--json", "-q"]) == 0
    row = json.loads(capsys.readouterr().out)
    assert row["path"] == "t.py" and row["match"] == "strong"
    assert row["regions"] == [{"start": 1, "end": 3, "p": 0.9, "label": "a = 1",
                               "lines": [{"line": 2, "p": 0.95, "text": "needle = 2"}]}]
    assert row["lines"] == []

    assert main(["find needle", "--no-heading", "-q"]) == 0
    assert capsys.readouterr().out == "t.py:1-3:0.90:a = 1\nt.py:2:0.95:needle = 2\n"

    assert main(["find needle", "-l", "-q"]) == 0
    assert capsys.readouterr().out == "1.00  t.py\n"

    (tmp_path / "t.py").write_text("nothing here\n")
    assert main(["find needle", "-q"]) == 1
    assert main(["find needle", "missing-dir", "-q"]) == 2


def test_flat_output_never_carries_weak_tier_rows(tmp_path, monkeypatch, capsys):
    """Weak rows need the "weaker" separator to be read correctly; flat output has none."""
    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("TYPESAFE_API_KEY", "test-key")
    (tmp_path / "t.py").write_text("a = 1\nb = 2\n")
    def near_miss(request):
        qs = json.loads(request.content)["questions"]
        answers = {k: ({"type": "score", "score": 2.7, "confidence": 0.9} if q["type"] == "score"
                       else {"type": "noul", "noul": 0.41 if ".B" in k else 0.1}) for k, q in qs.items()}
        return httpx.Response(200, json={"answers": answers, "usage": {}})
    real = C.JevClient.__init__
    def patched(self, *a, **kw):
        real(self, *a, **kw)
        self._http = httpx.Client(transport=httpx.MockTransport(near_miss))
    monkeypatch.setattr(C.JevClient, "__init__", patched)

    assert main(["q", "-q"]) == 0
    text = capsys.readouterr().out
    assert text.startswith("-- weaker:") and "        1-2  0.41  a = 1" in text
    assert main(["q", "-q", "--no-heading"]) == 1            # nothing cleared -t, so flat mode has no rows
    assert capsys.readouterr().out == ""
    assert main(["q", "-q", "--json"]) == 0
    assert json.loads(capsys.readouterr().out)["match"] == "weak"


def test_missing_key_is_a_clear_error(monkeypatch, tmp_path, capsys):
    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("TYPESAFE_API_KEY", raising=False)
    monkeypatch.setattr(C.shutil, "which", lambda name: None)
    (tmp_path / "t.py").write_text("x = 1\n")
    assert main(["q"]) == 2
    assert "TYPESAFE_API_KEY is not set" in capsys.readouterr().err
