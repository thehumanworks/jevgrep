"""Turns raw per-file answers into what gets displayed. Pure functions, no I/O.

Display rules, chosen so the output never contradicts itself:
  1. A row is printed only if its own probability clears the bar. Regions and lines need
     `threshold`. Nothing rides in on the strength of a neighbour.
  2. Each source line is printed at most once. Jev scores a line twice in effect: alone (the line
     question) and with its surroundings (the block question), and the two can disagree. So a
     pinpointed line that is already its region's label is not repeated, and a pinpointed line
     whose block did not clear the bar stands alone, without a region row.
  3. A file is shown only with at least one row. No "relevant, but nothing to show".
  4. A file whose section relevance is high but where nothing cleared the bar goes to a separate
     weak tier, showing its best blocks if they reach `WEAK_RATIO * threshold`. Broad answers
     spread thin, so a near miss in a clearly relevant file is still worth a look.
  5. With a --filter, a region or line is shown only if it passes the rules (`keep` >= FILTER_GATE).
     Filtering happens before everything else, so filtered-out code cannot put a file on the list.
  6. Matched files come first, then the weak tier. Within a tier, the number in a file's header
     is the number files are sorted by.
"""

from __future__ import annotations

from dataclasses import dataclass, field

from .filters import FILTER_GATE
from .search import BlockHit, FileResult, LineHit

WEAK_RATIO = 0.7         # rule 4: a weak-tier region needs at least this fraction of the threshold
WEAK_REGIONS = 2         # rule 4: how many best blocks a weak file may show
MAX_WEAK_FILES = 5


@dataclass
class Region:
    start: int
    end: int
    p: float
    label: str
    label_line: int = 0
    lines: list[LineHit] = field(default_factory=list)   # pinpointed lines inside, never the label line


@dataclass
class FileView:
    path: str
    relevance: float            # headline and sort key: best evidence anywhere in the file
    section_relevance: float    # Jev's 0..1 relevance score for the file's best section
    confidence: float
    regions: list[Region]
    lines: list[LineHit] = field(default_factory=list)   # pinpointed lines outside any shown region
    more_regions: int = 0
    more_lines: int = 0
    weak: bool = False          # shown on file-level relevance alone; nothing cleared the threshold

    def rows(self) -> list[Region | LineHit]:
        """Regions and stand-alone lines in file order."""
        return sorted([*self.regions, *self.lines], key=lambda r: r.start if isinstance(r, Region) else r.line)


def _merge(blocks: list[BlockHit]) -> list[Region]:
    regions: list[Region] = []
    for b in sorted(blocks, key=lambda b: b.start):
        if regions and b.start <= regions[-1].end + 2:
            last = regions[-1]
            last.end, last.p = max(last.end, b.end), max(last.p, b.p)
        else:
            regions.append(Region(b.start, b.end, b.p, b.label.strip(), b.label_line or b.start))
    return regions


def view_file(fr: FileResult, *, threshold: float, file_threshold: float,
              max_regions: int, max_lines: int) -> FileView | None:
    blocks = [b for b in fr.blocks if b.keep >= FILTER_GATE]
    best_per_line: dict[int, LineHit] = {}
    for h in (h for h in fr.lines if h.keep >= FILTER_GATE):  # guard: one score per source line, whatever produced the answers
        if h.line not in best_per_line or h.p > best_per_line[h.line].p:
            best_per_line[h.line] = h
    hits = [h for h in best_per_line.values() if h.p >= threshold]
    regions = _merge([b for b in blocks if b.p >= threshold])
    weak = not regions and not hits
    if weak:
        if fr.score < file_threshold:
            return None
        best = sorted(blocks, key=lambda b: -b.p)[:WEAK_REGIONS]
        regions = _merge([b for b in best if b.p >= WEAK_RATIO * threshold])
        if not regions:
            return None

    kept = sorted(sorted(regions, key=lambda r: -r.p)[:max_regions], key=lambda r: r.start)
    dropped = [r for r in regions if r not in kept]
    inside = lambda h, rs: any(r.start <= h.line <= r.end for r in rs)
    labels = {r.label_line for r in kept}
    candidates = [h for h in hits if h.line not in labels and not inside(h, dropped)]
    shown = sorted(sorted(candidates, key=lambda h: -h.p)[:max_lines], key=lambda h: h.line)
    for r in kept:
        r.lines = [h for h in shown if r.start <= h.line <= r.end]
    alone = [h for h in shown if not inside(h, kept)]
    relevance = max([fr.score] + [r.p for r in kept] + [h.p for h in shown])
    return FileView(fr.path, relevance, fr.score, fr.confidence, kept, alone,
                    more_regions=len(dropped), more_lines=len(candidates) - len(shown), weak=weak)


def select(results: list[FileResult], *, threshold: float = 0.5, file_threshold: float = 0.6,
           top: int = 15, max_regions: int = 5, max_lines: int = 10) -> list[FileView]:
    views = [v for fr in results if (v := view_file(
        fr, threshold=threshold, file_threshold=file_threshold, max_regions=max_regions, max_lines=max_lines))]
    views.sort(key=lambda v: (v.weak, -v.relevance, v.path))
    strong = [v for v in views if not v.weak][:top]
    weak = [v for v in views if v.weak][: max(0, min(MAX_WEAK_FILES, top - len(strong)))]
    return strong + weak


def filtered_out(results: list[FileResult], threshold: float) -> tuple[int, int]:
    """(regions, lines) that matched the query but were removed by the filter. For the stats line."""
    regions = sum(1 for fr in results for b in fr.blocks if b.p >= threshold and b.keep < FILTER_GATE)
    lines = sum(1 for fr in results for h in fr.lines if h.p >= threshold and h.keep < FILTER_GATE)
    return regions, lines


def select_files(results: list[FileResult], *, file_threshold: float = 0.6, top: int = 15) -> list[FileResult]:
    """Files-only mode: rank purely by section relevance."""
    keep = [fr for fr in results if fr.score >= file_threshold and fr.keep >= FILTER_GATE]
    return sorted(keep, key=lambda fr: (-fr.score, fr.path))[:top]
