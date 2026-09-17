import subprocess

from jevgrep.files import Chunk, chunk_lines, discover, read_lines, split_blocks, split_chunk


def test_chunks_cover_every_line_once_with_leading_context():
    lines = [f"x{i} = {i}" for i in range(1, 401)]
    chunks = chunk_lines("f.py", lines, max_lines=150, context=10)
    assert chunks[0].start == 1 and chunks[-1].end == 400
    assert all(a.end + 1 == b.start for a, b in zip(chunks, chunks[1:]))
    assert all(c.end - c.start + 1 <= 150 for c in chunks)
    second = chunks[1]
    assert second.ctx_start == second.start - 10 and second.text(second.start) == f"x{second.start} = {second.start}"
    assert [n for c in chunks for n in c.askable()] == list(range(1, 401))
    assert [n for c in chunks for a, b in c.blocks for n in range(a, b + 1)] == list(range(1, 401))


PY = """\
import os

LIMIT = 3


class Greeter:
    \"\"\"Says hello.\"\"\"

    def __init__(self, name):
        self.name = name
        self.count = 0
        self.log = []
        self.extra = None
        self.more = None
        self.even_more = None
        self.last = None

    def greet(self):
        self.count += 1
        return f"hello {self.name}"


def main():
    g = Greeter(os.environ["USER"])
    print(g.greet())
    print(g.greet())
    print(g.greet())
    print(g.greet())
""".splitlines()


def test_split_blocks_follows_code_structure():
    blocks = split_blocks(PY)
    starts = [PY[a - 1].strip() for a, _ in blocks]
    assert "class Greeter:" in starts and "def main():" in starts
    assert "def greet(self):" in starts            # a method becomes its own block once the class is long enough
    assert all(PY[b - 1].strip() for _, b in blocks)  # no block ends on a blank line
    covered = {n for a, b in blocks for n in range(a, b + 1)}
    assert all(n in covered for n, line in enumerate(PY, 1) if line.strip())


def test_chunks_break_between_blocks_not_inside_them():
    lines = []
    for i in range(12):
        lines += [f"def f{i}():"] + [f"    x{j} = {j}" for j in range(20)] + [""]
    chunks = chunk_lines("f.py", lines, max_lines=100)
    assert len(chunks) > 1
    assert all(lines[c.start - 1].startswith("def ") for c in chunks)


def test_chunk_budget_shrinks_with_more_queries():
    lines = ["value = compute(thing)"] * 300
    one = chunk_lines("f.py", lines, max_tokens=5000, questions_per_line=1)
    nine = chunk_lines("f.py", lines, max_tokens=5000, questions_per_line=9)
    assert len(nine) > len(one)
    assert sum(c.end - c.start + 1 for c in nine) == 300


def test_askable_skips_blank_and_punctuation_lines():
    c = chunk_lines("f.js", ["function f() {", "", "  return 1;", "}", "});"])[0]
    assert c.askable() == [1, 3]


def test_long_lines_are_clipped():
    c = chunk_lines("f.txt", ["a" * 5000])[0]
    assert len(c.text(1)) < 400


def test_split_chunk_halves_at_a_block_boundary_and_preserves_text():
    lines = [f"l{i}" for i in range(1, 101)]
    c = chunk_lines("f.py", lines)[0]
    a, b = split_chunk(c)
    assert a.start == 1 and b.end == 100 and a.end + 1 == b.start
    assert b.start in {s for s, _ in c.blocks}
    assert a.text(a.end) == f"l{a.end}" and b.text(b.start) == f"l{b.start}" and b.text(100) == "l100"
    assert b.ctx_start < b.start
    assert [n for part in (a, b) for s, e in part.blocks for n in range(s, e + 1)] == list(range(1, 101))
    assert split_chunk(Chunk("f", 1, 1, 1, ("x",))) == []


def test_discover_honors_gitignore_and_skips_binaries_and_lockfiles(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    subprocess.run(["git", "init", "-q"], check=True)
    (tmp_path / ".gitignore").write_text("ignored/\n")
    for rel, data in {
        "src/a.py": b"print(1)\n", "ignored/b.py": b"print(2)\n", "node_modules/x/i.js": b"x\n",
        "uv.lock": b"lock\n", "logo.png": b"\x89PNG", "empty.py": b"", "docs/readme.md": b"# hi\n",
    }.items():
        (tmp_path / rel).parent.mkdir(parents=True, exist_ok=True)
        (tmp_path / rel).write_bytes(data)
    found = sorted(str(p) for p in discover(["."]))
    assert found == ["docs/readme.md", "src/a.py"]
    assert [str(p) for p in discover(["."], globs=["*.py"])] == ["src/a.py"]
    assert [str(p) for p in discover(["."], excludes=["docs/*"])] == ["src/a.py"]
    # An explicitly named ignored directory is still searched.
    assert [str(p) for p in discover(["ignored"])] == ["ignored/b.py"]
    # An explicitly named file always wins.
    assert [str(p) for p in discover(["uv.lock"])] == ["uv.lock"]


def test_discover_without_git(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    (tmp_path / "a.py").write_text("x = 1\n")
    (tmp_path / ".hidden.py").write_text("y = 2\n")
    assert [str(p) for p in discover(["."], no_ignore=True)] == ["a.py"]
    assert sorted(str(p) for p in discover(["."], no_ignore=True, hidden=True)) == [".hidden.py", "a.py"]


def test_read_lines_rejects_binary(tmp_path):
    (tmp_path / "b").write_bytes(b"ab\0cd")
    (tmp_path / "t").write_text("one\ntwo\n")
    assert read_lines(tmp_path / "b") is None
    assert read_lines(tmp_path / "t") == ["one", "two"]
