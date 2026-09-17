"""The display rules. These exist so jg's output never contradicts itself."""
from jevgrep.results import select, select_files
from jevgrep.files import chunk_lines
from jevgrep.search import BlockHit, FileResult, LineHit, block_label, region_label


def file(path, score, blocks=(), lines=()):
    return FileResult(path, score=score, confidence=0.8,
                      blocks=[BlockHit(a, b, p, f"def f{a}():") for a, b, p in blocks],
                      lines=[LineHit(n, p, f"line {n}") for n, p in lines])


def test_pinpoint_match_shows_region_with_its_lines_nested():
    [v] = select([file("a.py", 0.9, blocks=[(10, 30, 0.96), (40, 60, 0.1)], lines=[(12, 0.93), (13, 0.2), (50, 0.1)])])
    assert [(r.start, r.end, r.p) for r in v.regions] == [(10, 30, 0.96)]
    assert [h.line for h in v.regions[0].lines] == [12]
    assert not v.weak


def test_broad_query_relevant_file_without_any_strong_line_still_points_somewhere():
    """Regression: `jg "where is the integration with jev defined"` showed relevance=0.91 files
    with only "(no single line above threshold)". A shown file must always carry a location."""
    diffuse = file("client.py", 0.91, blocks=[(1, 7, 0.3), (122, 144, 0.58), (160, 199, 0.41)],
                   lines=[(1, 0.40), (171, 0.31), (122, 0.25)])
    [v] = select([diffuse])
    assert [(r.start, r.end) for r in v.regions] == [(122, 144)] and not v.weak
    assert v.regions[0].lines == []            # no line cleared 0.5, and none is invented


def test_relevant_file_with_nothing_above_threshold_lands_in_weak_tier_with_near_misses_only():
    strong = file("cli.py", 0.71, blocks=[(175, 190, 0.70)], lines=[(178, 0.61)])
    weak = file("search.py", 0.78, blocks=[(87, 121, 0.43), (1, 20, 0.30), (130, 150, 0.05)])
    views = select([weak, strong])
    assert [(v.path, v.weak) for v in views] == [("cli.py", False), ("search.py", True)]  # evidence beats a higher header
    assert [(r.start, r.p) for r in views[1].regions] == [(87, 0.43)]   # 0.30 is under 0.7 x 0.5, so it stays hidden


def test_weak_tier_bar_follows_the_threshold():
    f = file("s.py", 0.9, blocks=[(1, 9, 0.30)])
    assert select([f]) == []                                         # 0.30 < 0.35
    [v] = select([f], threshold=0.4)                                 # 0.30 >= 0.28
    assert v.weak and v.regions[0].p == 0.30


def test_files_with_no_locatable_evidence_are_hidden():
    vague = file("vague.py", 0.95, blocks=[(1, 40, 0.1)])          # relevant "overall", but nowhere in particular
    tangential = file("t.py", 0.4, blocks=[(1, 40, 0.45)])          # below both thresholds
    assert select([vague, tangential]) == []


def test_a_strong_line_in_a_weak_block_stands_alone_without_a_sub_threshold_region_row():
    """Regression: `client.py 1-7 0.19` was printed only because line 1 scored 0.54."""
    [v] = select([file("a.py", 0.3, blocks=[(1, 20, 0.19), (22, 40, 0.1)], lines=[(5, 0.8)])])
    assert v.regions == [] and [(h.line, h.p) for h in v.lines] == [(5, 0.8)]
    assert v.relevance == 0.8 and not v.weak                        # header shows the best evidence


def test_a_line_that_is_its_regions_label_is_not_printed_twice():
    """Regression: `101-119 0.98 def resolve_api_key()` was followed by `101 0.88 def resolve_api_key()`.
    Jev scores the line alone and within its block; the two numbers differ, the text is the same."""
    [v] = select([file("c.py", 1.0, blocks=[(101, 119, 0.98)], lines=[(101, 0.88), (103, 0.90)])])
    assert [r.label_line for r in v.regions] == [101]
    assert [h.line for h in v.regions[0].lines] == [103]
    assert v.more_lines == 0                                        # the folded line is not reported as hidden


def test_same_line_scored_twice_keeps_one_row():
    [v] = select([file("d.py", 0.9, lines=[(7, 0.6), (7, 0.9)])])
    assert [(h.line, h.p) for h in v.lines] == [(7, 0.9)]


def test_invariants_hold_for_arbitrary_answers():
    import random
    rng = random.Random(7)
    for _ in range(300):
        t = rng.choice([0.3, 0.5, 0.8])
        results = []
        for f in range(rng.randint(1, 5)):
            starts = sorted(rng.sample(range(1, 400, 5), rng.randint(0, 12)))
            blocks = [(a, a + rng.randint(0, 4), round(rng.random(), 2)) for a in starts]
            lines = [(rng.randint(1, 410), round(rng.random(), 2)) for _ in range(rng.randint(0, 40))]
            results.append(file(f"f{f}.py", round(rng.random(), 2), blocks, lines))
        views = select(results, threshold=t, max_regions=rng.randint(1, 6), max_lines=rng.randint(1, 12))
        for v in views:
            rows = v.rows()
            assert rows, "a shown file always has a row"
            for r in v.regions:
                assert r.p >= (0.7 * t if v.weak else t)
                assert all(r.start <= h.line <= r.end for h in r.lines)
            shown_lines = [h for r in v.regions for h in r.lines] + v.lines
            assert all(h.p >= t for h in shown_lines) and not (v.weak and shown_lines)
            printed = [r.label_line for r in v.regions] + [h.line for h in shown_lines]
            assert len(printed) == len(set(printed)), "no source line printed twice"
            assert v.relevance >= max([r.p for r in v.regions] + [h.p for h in shown_lines])
        keys = [(v.weak, -v.relevance) for v in views]
        assert keys == sorted(keys)


def test_adjacent_matching_blocks_merge_into_one_region():
    [v] = select([file("auth.py", 0.99, blocks=[(255, 268, 0.76), (270, 276, 0.66), (278, 284, 0.68), (300, 310, 0.9)])])
    assert [(r.start, r.end, r.p) for r in v.regions] == [(255, 284, 0.76), (300, 310, 0.9)]


def test_files_are_sorted_by_the_number_in_their_header_within_a_tier():
    views = select([file("b.py", 0.6, blocks=[(1, 9, 0.7)]), file("a.py", 0.5, blocks=[(1, 9, 0.95)]),
                    file("c.py", 0.99, blocks=[(1, 9, 0.55)])])
    assert [(v.path, v.relevance) for v in views] == [("c.py", 0.99), ("a.py", 0.95), ("b.py", 0.7)]


def test_caps_report_what_was_left_out():
    busy = file("a.py", 0.9, blocks=[(i * 10, i * 10 + 5, 0.6 + i / 100) for i in range(8)],
                lines=[(i * 10 + 1, 0.9) for i in range(8)])
    [v] = select([busy], max_regions=3, max_lines=2)
    assert len(v.regions) == 3 and v.more_regions == 5
    assert sum(len(r.lines) for r in v.regions) == 2 and v.more_lines == 1  # lines of dropped regions go with them
    assert [r.start for r in v.regions] == sorted(r.start for r in v.regions)


def test_files_only_mode_ranks_by_section_relevance():
    got = select_files([file("a.py", 0.61), file("b.py", 0.9), file("c.py", 0.59)])
    assert [f.path for f in got] == ["b.py", "a.py"]


def test_block_label_prefers_the_definition_under_decorators_and_comments():
    assert block_label(["    @property", "    def encoding(self) -> str:", "        return 1"]).strip() == "def encoding(self) -> str:"
    assert block_label(["#[derive(Debug)]", "pub struct Walk {", "    x: u8,"]) == "pub struct Walk {"
    assert block_label(["/// Docs.", "pub(crate) async fn run() {"]) == "pub(crate) async fn run() {"
    assert block_label(["export default function App() {"]) == "export default function App() {"
    assert block_label(["", "  x = compute()", "  y = 2"]) == "  x = compute()"
    assert block_label(["", "}"]) == ""


def test_region_label_names_the_enclosing_definition_for_mid_function_blocks():
    src = ["class Builder:", "    def build(self, spec):", "        state = {}", "        for k in spec:", "            state[k] = 1",
           "", "        questions = {}", "        return state, questions"]
    chunk = chunk_lines("b.py", src)[0]
    assert region_label(chunk, 7, 8) == ("def build(self, spec): > questions = {}", 7)
    assert region_label(chunk, 2, 5) == ("def build(self, spec):", 2)      # already a definition: no prefix
    assert region_label(chunk_lines("t.txt", ["plain words", "more words"])[0], 1, 2) == ("plain words", 1)
    decorated = chunk_lines("p.py", ["@property", "def size(self):", "    return 1"])[0]
    assert region_label(decorated, 1, 3) == ("def size(self):", 2)         # label line is the def, not the decorator
