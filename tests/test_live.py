"""Live tests against the real TypeSafe API. Run: fnox exec -- uv run pytest -m live"""
import json

import pytest

from jevgrep.cli import main

pytestmark = pytest.mark.live

POOL = '''\
import asyncpg

POOL_MIN = 2
POOL_MAX = 20


async def create_pool(dsn: str):
    return await asyncpg.create_pool(dsn, min_size=POOL_MIN, max_size=POOL_MAX)
'''
STRINGS = '''\
def slugify(title: str) -> str:
    return "-".join(title.lower().split())


def truncate(text: str, width: int) -> str:
    return text if len(text) <= width else text[: width - 1] + "..."
'''
BILLING = '''\
TAX_RATE = 0.21


def invoice_total(items):
    subtotal = sum(i.price * i.quantity for i in items)
    return round(subtotal * (1 + TAX_RATE), 2)
'''


@pytest.fixture
def repo(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    for name, text in {"db/pool.py": POOL, "util/strings.py": STRINGS, "billing/invoice.py": BILLING}.items():
        (tmp_path / name).parent.mkdir(parents=True, exist_ok=True)
        (tmp_path / name).write_text(text)
    return tmp_path


def rows(capsys):
    return [json.loads(l) for l in capsys.readouterr().out.splitlines()]


def test_finds_the_right_file_and_line(repo, capsys):
    assert main(["where is the maximum number of database connections configured", "--json", "-q"]) == 0
    found = rows(capsys)
    assert found[0]["path"] == "db/pool.py" and found[0]["relevance"] > 0.8
    assert found[0]["match"] == "strong", found[0]
    regions = found[0]["regions"]
    located = {l["line"] for r in regions for l in r["lines"]} | {l["line"] for l in found[0]["lines"]}
    assert located & {4, 8} or any(r["start"] <= 8 <= r["end"] for r in regions), found[0]  # POOL_MAX, or create_pool
    assert all(r["start"] <= l["line"] <= r["end"] for r in found[0]["regions"] for l in r["lines"])
    assert all(r["path"] != "util/strings.py" for r in found)  # calibrated: unrelated file stays out


def test_several_queries_in_one_pass(repo, capsys):
    assert main(["how is sales tax applied to an invoice", "-e", "turning a title into a URL slug", "--json", "-q"]) == 0
    best = {}
    for r in rows(capsys):
        best.setdefault(r["query"], r["path"])
    assert best == {
        "how is sales tax applied to an invoice": "billing/invoice.py",
        "turning a title into a URL slug": "util/strings.py",
    }


def test_files_only_mode_ranks_files(repo, capsys):
    assert main(["database connection pooling", "-l", "-q"]) == 0
    first = capsys.readouterr().out.splitlines()[0]
    assert first.split()[1] == "db/pool.py" and float(first.split()[0]) > 0.6


def test_broad_query_returns_a_region_even_when_no_single_line_answers_it(repo, capsys):
    assert main(["how does invoicing work", "--json", "-q"]) == 0
    found = rows(capsys)
    assert found[0]["path"] == "billing/invoice.py"
    assert all(r["regions"] or r["lines"] for r in found)    # every file shown points at a location
    assert all(reg["p"] >= 0.35 for r in found for reg in r["regions"])   # and never at a long shot


def test_every_shown_file_has_a_location_in_text_output(repo, capsys):
    assert main(["database connection settings", "-q"]) == 0
    blocks = [b for b in capsys.readouterr().out.split("\n\n") if b.strip() and not b.startswith("--")]
    for block in blocks:
        header, *body = block.splitlines()
        assert "relevance=" in header and body, block
        assert all(row.split()[0].replace("-", "").isdigit() for row in body), block


def test_natural_language_filter_excludes_documentation(repo, capsys):
    (repo / "README.md").write_text("# Pooling\n\nThe database connection pool holds at most 20 connections.\n"
                                    "Set POOL_MAX in db/pool.py to change the maximum.\n")
    assert main(["what is the maximum number of database connections", "--json", "-q"]) == 0
    assert {"README.md", "db/pool.py"} <= {r["path"] for r in rows(capsys)}       # unfiltered: docs and code both match

    assert main(["what is the maximum number of database connections", "--json", "-q",
                 "--filter", "Source code files only. No documentation"]) == 0
    paths = {r["path"] for r in rows(capsys)}
    assert "db/pool.py" in paths and "README.md" not in paths

    assert main(["what is the maximum number of database connections", "-l", "-q", "--only", "documentation"]) == 0
    assert [l.split()[1] for l in capsys.readouterr().out.splitlines()] == ["README.md"]


def test_no_match_exits_1(repo, capsys):
    assert main(["kubernetes pod autoscaling policy", "-q"]) == 1
    assert capsys.readouterr().out == ""


def test_bad_key_exits_2(repo, capsys, monkeypatch):
    monkeypatch.setenv("TYPESAFE_API_KEY", "apikey_invalid")
    assert main(["anything"]) == 2
    assert "rejected the API key" in capsys.readouterr().err
