"""File discovery (gitignore-aware) and line chunking."""

from __future__ import annotations

import fnmatch
import os
import re
import subprocess
from dataclasses import dataclass
from pathlib import Path

SKIP_DIRS = {
    ".git", ".hg", ".svn", "node_modules", ".venv", "venv", "__pycache__", "dist", "build",
    "target", ".next", ".nuxt", ".cache", ".pytest_cache", ".mypy_cache", ".ruff_cache",
    ".tox", "vendor", ".idea", ".vscode", "coverage", ".terraform",
}
SKIP_NAMES = {
    "package-lock.json", "pnpm-lock.yaml", "yarn.lock", "bun.lock", "bun.lockb", "Cargo.lock",
    "poetry.lock", "uv.lock", "Pipfile.lock", "composer.lock", "Gemfile.lock", "go.sum",
    "flake.lock",
}
SKIP_SUFFIXES = (
    ".min.js", ".min.css", ".map", ".png", ".jpg", ".jpeg", ".gif", ".webp", ".ico", ".svg",
    ".pdf", ".zip", ".gz", ".tar", ".tgz", ".bz2", ".xz", ".7z", ".jar", ".war", ".class",
    ".so", ".dylib", ".dll", ".exe", ".o", ".a", ".wasm", ".pyc", ".woff", ".woff2", ".ttf",
    ".eot", ".otf", ".mp3", ".mp4", ".mov", ".avi", ".webm", ".bin", ".dat", ".db", ".sqlite",
    ".parquet", ".npy", ".npz", ".pkl", ".onnx", ".pt", ".safetensors", ".snap",
)
MAX_LINE_CHARS = 300
_HAS_WORD = re.compile(r"[A-Za-z0-9]")


@dataclass(frozen=True)
class Chunk:
    path: str            # display path
    ctx_start: int       # 1-based first line included as leading context
    start: int           # 1-based first line we ask questions about
    end: int             # 1-based last line, inclusive
    lines: tuple[str, ...]  # text for ctx_start..end
    blocks: tuple[tuple[int, int], ...] = ()  # logical blocks inside start..end

    def text(self, n: int) -> str:
        return self.lines[n - self.ctx_start]

    def askable(self) -> list[int]:
        """Line numbers worth a question: skips blanks and pure punctuation."""
        return [n for n in range(self.start, self.end + 1) if _HAS_WORD.search(self.text(n))]


def _git_files(root: Path, no_ignore: bool) -> list[Path] | None:
    if no_ignore:
        return None
    try:
        out = subprocess.run(
            ["git", "-C", str(root), "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
            capture_output=True, timeout=60,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if out.returncode != 0:
        return None
    return [root / p for p in out.stdout.decode("utf-8", "replace").split("\0") if p]


def _walk(root: Path, hidden: bool) -> list[Path]:
    found: list[Path] = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(
            d for d in dirnames if d not in SKIP_DIRS and (hidden or not d.startswith("."))
        )
        for name in sorted(filenames):
            if hidden or not name.startswith("."):
                found.append(Path(dirpath) / name)
    return found


def _matches(rel: str, patterns: list[str]) -> bool:
    name = rel.rsplit("/", 1)[-1]
    return any(
        fnmatch.fnmatch(rel, p) or fnmatch.fnmatch(name, p) or fnmatch.fnmatch(rel, f"*/{p}")
        for p in patterns
    )


def display_path(p: Path) -> str:
    """Relative to the cwd when the file is inside it, otherwise as the user spelled it."""
    try:
        rel = os.path.relpath(p)
    except ValueError:
        return str(p)
    return str(p) if rel.startswith("..") else rel


def discover(
    paths: list[str],
    *,
    globs: list[str] | None = None,
    excludes: list[str] | None = None,
    max_bytes: int = 512_000,
    hidden: bool = False,
    no_ignore: bool = False,
) -> list[Path]:
    """Expand paths into searchable text files, honoring .gitignore inside git repos."""
    seen: set[Path] = set()
    result: list[Path] = []
    for raw in paths or ["."]:
        root = Path(raw)
        if root.is_file():
            candidates, explicit = [root], True
        elif root.is_dir():
            # An explicitly named directory is searched even if git ignores all of it.
            candidates = _git_files(root, no_ignore) or _walk(root, hidden)
            explicit = False
        else:
            raise FileNotFoundError(raw)
        for p in candidates:
            rp = p.resolve()
            if rp in seen or not p.is_file():
                continue
            rel = display_path(p).replace(os.sep, "/")
            if not explicit:
                try:  # skip rules apply below the root the user named, not to the root itself
                    parts = list(p.relative_to(root).parts)
                except ValueError:
                    parts = rel.split("/")
                if any(part in SKIP_DIRS for part in parts[:-1]):
                    continue
                if not hidden and any(part.startswith(".") and part not in (".", "..") for part in parts):
                    continue
                if p.name in SKIP_NAMES or p.name.lower().endswith(SKIP_SUFFIXES):
                    continue
                if globs and not _matches(rel, globs):
                    continue
                try:
                    if p.stat().st_size > max_bytes or p.stat().st_size == 0:
                        continue
                except OSError:
                    continue
            if excludes and _matches(rel, excludes):
                continue
            seen.add(rp)
            result.append(p)
    return result


def read_lines(path: Path) -> list[str] | None:
    """Returns the file's lines, or None for binary/unreadable files."""
    try:
        data = path.read_bytes()
    except OSError:
        return None
    if b"\0" in data[:8192]:
        return None
    return data.decode("utf-8", "replace").splitlines()


def clip(line: str) -> str:
    line = line.rstrip()
    return line if len(line) <= MAX_LINE_CHARS else line[:MAX_LINE_CHARS] + " ..."


QUESTION_TOKENS = 30  # rough cost of one per-line question, measured against live usage


def estimate_tokens(line: str, questions_per_line: float = 1) -> int:
    return 6 + len(line) // 3 + round(QUESTION_TOKENS * questions_per_line)


DEFINITION = re.compile(
    r"^\s*(?:(?:export|default|pub(?:\([a-z]+\))?|public|private|protected|internal|static|final|abstract|"
    r"async|unsafe|extern|inline|virtual|override|const)\s+)*"
    r"(?:def|class|fn|func|function|impl|struct|enum|trait|interface|type|module|mod|namespace|object|record|"
    r"macro_rules!|sub|proc|procedure|package)\b"
)


def _indent(line: str) -> int:
    return len(line) - len(line.lstrip())


def split_blocks(lines: list[str], *, min_len: int = 6, member_len: int = 12, max_len: int = 40) -> list[tuple[int, int]]:
    """Splits a file into logical blocks, returned as 1-based inclusive (start, end) ranges.

    Language-agnostic, driven by blank lines and indentation. A new block starts at a paragraph
    (a non-blank line after a blank one) when that line is:
      - no deeper than the block's first line (next function, class, top-level statement), or
      - at the block's member level (next method of a class) once the block has `member_len` lines, or
      - anything at all once the block has `max_len` lines.
    """
    blocks: list[tuple[int, int]] = []
    start: int | None = None   # 0-based start of the current block
    base = inner = 0           # indent of the block's first line / shallowest nested indent
    for i, line in enumerate(lines):
        if not line.strip():
            continue
        ind = _indent(line)
        if start is None:
            start, base, inner = i, ind, 10**6
            continue
        length = i - start
        para = i > 0 and not lines[i - 1].strip()
        if para and (
            ind < base
            or (ind <= min(inner, base + 8) and DEFINITION.match(line) and (ind <= base or length >= min_len))
            or (length >= min_len and (ind <= base or (ind <= inner and length >= member_len) or length >= max_len))
        ) or length >= max_len + 10:
            blocks.append((start + 1, i))
            start, base, inner = i, ind, 10**6
        elif ind > base:
            inner = min(inner, ind)
    if start is not None:
        blocks.append((start + 1, len(lines)))
    trimmed = []
    for a, b in blocks:
        while b > a and not lines[b - 1].strip():
            b -= 1
        trimmed.append((a, b))
    return trimmed


def chunk_lines(
    path: str, lines: list[str], *, max_lines: int = 150, max_tokens: int = 14_000, context: int = 12,
    questions_per_line: float = 1,
) -> list[Chunk]:
    """Packs a file's logical blocks into windows bounded by line count and a token budget.

    Windows break between blocks, so a function is not cut in half unless it alone is too big.
    Each window carries up to `context` preceding lines so the model can see what encloses it,
    but questions are only asked about the window itself.
    """
    clipped = [clip(l) for l in lines]
    cost = [estimate_tokens(l, questions_per_line) for l in clipped]
    pieces: list[tuple[int, int]] = []
    for a, b in split_blocks(clipped):
        # A block too large for one window is cut into window-sized pieces.
        i = a
        while i <= b:
            j, budget = i, max_tokens
            while j <= b and j - i < max_lines and (cost[j - 1] <= budget or j == i):
                budget -= cost[j - 1]
                j += 1
            pieces.append((i, j - 1))
            i = j
    chunks: list[Chunk] = []
    group: list[tuple[int, int]] = []

    def flush() -> None:
        if group:
            first, last = group[0][0], group[-1][1]
            ctx = max(1, first - context)
            chunks.append(Chunk(path, ctx, first, last, tuple(clipped[ctx - 1 : last]), tuple(group)))
            group.clear()

    used = 0
    for a, b in pieces:
        need = sum(cost[a - 1 : b])
        if group and (b - group[0][0] + 1 > max_lines or used + need > max_tokens):
            flush()
            used = 0
        group.append((a, b))
        used += need
    flush()
    return chunks


def split_chunk(chunk: Chunk) -> list[Chunk]:
    """Halves a chunk (used when the API reports the request was too large)."""
    span = chunk.end - chunk.start + 1
    if span < 2:
        return []
    mid = chunk.start + span // 2
    if len(chunk.blocks) > 1:  # prefer the block boundary nearest the middle
        mid = min((a for a, _ in chunk.blocks[1:]), key=lambda a: abs(a - mid))
    off = chunk.ctx_start

    def part(ctx: int, start: int, end: int) -> Chunk:
        blocks = tuple((max(a, start), min(b, end)) for a, b in chunk.blocks if a <= end and b >= start)
        return Chunk(chunk.path, ctx, start, end, chunk.lines[ctx - off : end - off + 1], blocks)

    return [part(chunk.ctx_start, chunk.start, mid - 1), part(max(chunk.start, mid - 12), mid, chunk.end)]
