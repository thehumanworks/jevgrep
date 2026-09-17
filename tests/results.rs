//! The display rules. These exist so jg's output never contradicts itself.
use jevgrep::files::{chunk_lines, Chunking};
use jevgrep::results::{filtered_out, select, select_files, Limits, Row};
use jevgrep::search::{block_label, region_label, BlockHit, FileResult, LineHit};

/// blocks: (start, end, p, keep); lines: (line, p, keep)
fn filtered(path: &str, score: f64, keep: f64, blocks: &[(usize, usize, f64, f64)], lines: &[(usize, f64, f64)]) -> FileResult {
    FileResult {
        path: path.into(),
        score,
        confidence: 0.8,
        keep,
        blocks: blocks
            .iter()
            .map(|&(a, b, p, k)| BlockHit { start: a, end: b, p, label: format!("def f{a}():"), label_line: a, keep: k })
            .collect(),
        lines: lines.iter().map(|&(n, p, k)| LineHit { line: n, p, text: format!("line {n}"), section: 1.0, keep: k }).collect(),
    }
}

fn file(path: &str, score: f64, blocks: &[(usize, usize, f64)], lines: &[(usize, f64)]) -> FileResult {
    let blocks: Vec<_> = blocks.iter().map(|&(a, b, p)| (a, b, p, 1.0)).collect();
    let lines: Vec<_> = lines.iter().map(|&(n, p)| (n, p, 1.0)).collect();
    filtered(path, score, 1.0, &blocks, &lines)
}

fn spans(v: &jevgrep::results::FileView) -> Vec<(usize, usize, f64)> {
    v.regions.iter().map(|r| (r.start, r.end, r.p)).collect()
}

const D: Limits = Limits { threshold: 0.5, file_threshold: 0.6, top: 15, max_regions: 5, max_lines: 10 };

#[test]
fn pinpoint_match_shows_region_with_its_lines_nested() {
    let views = select(&[file("a.py", 0.9, &[(10, 30, 0.96), (40, 60, 0.1)], &[(12, 0.93), (13, 0.2), (50, 0.1)])], &D);
    assert_eq!(views.len(), 1);
    assert_eq!(spans(&views[0]), [(10, 30, 0.96)]);
    assert_eq!(views[0].regions[0].lines.iter().map(|h| h.line).collect::<Vec<_>>(), [12]);
    assert!(!views[0].weak);
}

/// Regression: `jg "where is the integration with jev defined"` showed relevance=0.91 files with
/// only "(no single line above threshold)". A shown file must always carry a location.
#[test]
fn broad_query_relevant_file_without_any_strong_line_still_points_somewhere() {
    let diffuse = file("client.py", 0.91, &[(1, 7, 0.3), (122, 144, 0.58), (160, 199, 0.41)], &[(1, 0.40), (171, 0.31), (122, 0.25)]);
    let views = select(&[diffuse], &D);
    assert_eq!(spans(&views[0]), [(122, 144, 0.58)]);
    assert!(!views[0].weak && views[0].regions[0].lines.is_empty()); // no line cleared 0.5, and none is invented
}

#[test]
fn relevant_file_with_nothing_above_threshold_lands_in_weak_tier_with_near_misses_only() {
    let strong = file("cli.py", 0.71, &[(175, 190, 0.70)], &[(178, 0.61)]);
    let weak = file("search.py", 0.78, &[(87, 121, 0.43), (1, 20, 0.30), (130, 150, 0.05)], &[]);
    let views = select(&[weak, strong], &D);
    // Evidence beats a higher header.
    assert_eq!(views.iter().map(|v| (v.path.as_str(), v.weak)).collect::<Vec<_>>(), [("cli.py", false), ("search.py", true)]);
    assert_eq!(spans(&views[1]), [(87, 121, 0.43)]); // 0.30 is under 0.7 x 0.5, so it stays hidden
}

#[test]
fn weak_tier_bar_follows_the_threshold() {
    let f = [file("s.py", 0.9, &[(1, 9, 0.30)], &[])];
    assert!(select(&f, &D).is_empty()); // 0.30 < 0.35
    let views = select(&f, &Limits { threshold: 0.4, ..D }); // 0.30 >= 0.28
    assert!(views[0].weak && views[0].regions[0].p == 0.30);
}

#[test]
fn files_with_no_locatable_evidence_are_hidden() {
    let vague = file("vague.py", 0.95, &[(1, 40, 0.1)], &[]); // relevant "overall", but nowhere in particular
    let tangential = file("t.py", 0.4, &[(1, 40, 0.45)], &[]); // below both thresholds
    assert!(select(&[vague, tangential], &D).is_empty());
}

/// Regression: `client.py 1-7 0.19` was printed only because line 1 scored 0.54.
#[test]
fn a_strong_line_in_a_weak_block_stands_alone_without_a_sub_threshold_region_row() {
    let views = select(&[file("a.py", 0.3, &[(1, 20, 0.19), (22, 40, 0.1)], &[(5, 0.8)])], &D);
    let v = &views[0];
    assert!(v.regions.is_empty());
    assert_eq!(v.lines.iter().map(|h| (h.line, h.p)).collect::<Vec<_>>(), [(5, 0.8)]);
    assert!(v.relevance == 0.8 && !v.weak); // header shows the best evidence
}

/// Regression: `101-119 0.98 def resolve_api_key()` was followed by `101 0.88 def resolve_api_key()`.
/// Jev scores the line alone and within its block; the two numbers differ, the text is the same.
#[test]
fn a_line_that_is_its_regions_label_is_not_printed_twice() {
    let views = select(&[file("c.py", 1.0, &[(101, 119, 0.98)], &[(101, 0.88), (103, 0.90)])], &D);
    let v = &views[0];
    assert_eq!(v.regions.iter().map(|r| r.label_line).collect::<Vec<_>>(), [101]);
    assert_eq!(v.regions[0].lines.iter().map(|h| h.line).collect::<Vec<_>>(), [103]);
    assert_eq!(v.more_lines, 0); // the folded line is not reported as hidden
}

#[test]
fn same_line_scored_twice_keeps_one_row() {
    let views = select(&[file("d.py", 0.9, &[], &[(7, 0.6), (7, 0.9)])], &D);
    assert_eq!(views[0].lines.iter().map(|h| (h.line, h.p)).collect::<Vec<_>>(), [(7, 0.9)]);
}

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % n
    }

    fn unit(&mut self) -> f64 {
        self.below(101) as f64 / 100.0
    }
}

#[test]
fn invariants_hold_for_arbitrary_answers() {
    let mut rng = Rng(7);
    let (mut strong_seen, mut weak_seen) = (0, 0);
    for _ in 0..500 {
        let t = [0.3, 0.5, 0.8][rng.below(3)];
        let mut results = Vec::new();
        for f in 0..1 + rng.below(5) {
            let mut starts: Vec<usize> = (0..rng.below(13)).map(|_| 1 + 5 * rng.below(80)).collect();
            starts.sort_unstable();
            starts.dedup();
            // Every other file is dim: nothing in it clears the threshold, so it can only be weak.
            let scale = if rng.below(2) == 0 { 1.0 } else { t * 0.99 };
            let blocks: Vec<_> = starts.iter().map(|&a| (a, a + rng.below(5), rng.unit() * scale)).collect();
            let lines: Vec<_> = (0..rng.below(41)).map(|_| (1 + rng.below(410), rng.unit() * scale)).collect();
            results.push(file(&format!("f{f}.py"), rng.unit(), &blocks, &lines));
        }
        let views = select(&results, &Limits { threshold: t, max_regions: 1 + rng.below(6), max_lines: 1 + rng.below(12), ..D });
        for v in &views {
            assert!(!v.rows().is_empty(), "a shown file always has a row");
            for r in &v.regions {
                assert!(r.p >= if v.weak { 0.7 * t } else { t });
                assert!(r.lines.iter().all(|h| r.start <= h.line && h.line <= r.end));
            }
            let shown: Vec<&LineHit> = v.regions.iter().flat_map(|r| &r.lines).chain(&v.lines).collect();
            assert!(shown.iter().all(|h| h.p >= t) && !(v.weak && !shown.is_empty()));
            let mut printed: Vec<usize> = v.regions.iter().map(|r| r.label_line).chain(shown.iter().map(|h| h.line)).collect();
            let count = printed.len();
            printed.sort_unstable();
            printed.dedup();
            assert_eq!(printed.len(), count, "no source line printed twice");
            assert!(v.regions.iter().map(|r| r.p).chain(shown.iter().map(|h| h.p)).all(|p| v.relevance >= p));
            let order: Vec<usize> = v
                .rows()
                .iter()
                .map(|row| match row {
                    Row::Region(r) => r.start,
                    Row::Line(h) => h.line,
                })
                .collect();
            assert!(order.windows(2).all(|w| w[0] <= w[1]));
            if v.weak {
                weak_seen += 1
            } else {
                strong_seen += 1
            }
        }
        assert!(views.windows(2).all(|w| (w[0].weak, -w[0].relevance) <= (w[1].weak, -w[1].relevance)));
    }
    assert!(strong_seen > 100 && weak_seen > 10, "the generator must exercise both tiers ({strong_seen}, {weak_seen})");
}

#[test]
fn adjacent_matching_blocks_merge_into_one_region() {
    let views = select(&[file("auth.py", 0.99, &[(255, 268, 0.76), (270, 276, 0.66), (278, 284, 0.68), (300, 310, 0.9)], &[])], &D);
    assert_eq!(spans(&views[0]), [(255, 284, 0.76), (300, 310, 0.9)]);
}

#[test]
fn files_are_sorted_by_the_number_in_their_header_within_a_tier() {
    let views = select(
        &[file("b.py", 0.6, &[(1, 9, 0.7)], &[]), file("a.py", 0.5, &[(1, 9, 0.95)], &[]), file("c.py", 0.99, &[(1, 9, 0.55)], &[])],
        &D,
    );
    assert_eq!(views.iter().map(|v| (v.path.as_str(), v.relevance)).collect::<Vec<_>>(), [("c.py", 0.99), ("a.py", 0.95), ("b.py", 0.7)]);
}

#[test]
fn caps_report_what_was_left_out() {
    let blocks: Vec<_> = (0..8).map(|i| (i * 10, i * 10 + 5, 0.6 + i as f64 / 100.0)).collect();
    let lines: Vec<_> = (0..8).map(|i| (i * 10 + 1, 0.9)).collect();
    let views = select(&[file("a.py", 0.9, &blocks, &lines)], &Limits { max_regions: 3, max_lines: 2, ..D });
    let v = &views[0];
    assert!(v.regions.len() == 3 && v.more_regions == 5);
    // Lines of dropped regions go with them.
    assert!(v.regions.iter().map(|r| r.lines.len()).sum::<usize>() == 2 && v.more_lines == 1);
    assert!(v.regions.windows(2).all(|w| w[0].start < w[1].start));
}

#[test]
fn top_caps_strong_files_and_the_weak_tier_separately() {
    let mut results: Vec<_> = (0..20).map(|i| file(&format!("s{i:02}.py"), 0.9, &[(1, 9, 0.9)], &[])).collect();
    results.extend((0..9).map(|i| file(&format!("w{i}.py"), 0.9, &[(1, 9, 0.4)], &[])));
    assert_eq!(select(&results, &D).iter().filter(|v| v.weak).count(), 0); // 15 strong files fill the page
    let views = select(&results[12..], &D);
    assert_eq!((views.iter().filter(|v| !v.weak).count(), views.iter().filter(|v| v.weak).count()), (8, 5));
}

#[test]
fn files_only_mode_ranks_by_section_relevance_and_gates_on_file_keep() {
    let plain = [file("a.py", 0.61, &[], &[]), file("b.py", 0.9, &[], &[]), file("c.py", 0.59, &[], &[])];
    assert_eq!(select_files(&plain, 0.6, 15).iter().map(|f| &*f.path).collect::<Vec<_>>(), ["b.py", "a.py"]);
    let gated = [filtered("a.py", 0.9, 0.8, &[], &[]), filtered("README.md", 0.95, 0.1, &[], &[])];
    assert_eq!(select_files(&gated, 0.6, 15).iter().map(|f| &*f.path).collect::<Vec<_>>(), ["a.py"]);
}

#[test]
fn filtered_rows_never_display_and_cannot_carry_a_file() {
    let code = filtered("src/a.py", 0.9, 1.0, &[(1, 9, 0.8, 0.9), (20, 29, 0.9, 0.1)], &[(3, 0.7, 0.9), (22, 0.95, 0.1)]);
    let docs = filtered("README.md", 0.9, 1.0, &[(1, 9, 0.9, 0.02)], &[(4, 0.9, 0.02)]);
    let near = filtered("tests/t.py", 0.9, 1.0, &[(1, 9, 0.45, 0.1)], &[]); // would be weak tier, but filtered
    let all = [code, docs, near];
    let views = select(&all, &D);
    assert_eq!(views.iter().map(|v| &*v.path).collect::<Vec<_>>(), ["src/a.py"]);
    let got: Vec<_> = views[0].regions.iter().map(|r| (r.start, r.lines.iter().map(|h| h.line).collect::<Vec<_>>())).collect();
    assert_eq!(got, [(1, vec![3])]);
    assert_eq!(filtered_out(&all, 0.5), (2, 2));
}

#[test]
fn block_label_prefers_the_definition_under_decorators_and_comments() {
    assert_eq!(block_label(&["    @property", "    def encoding(self) -> str:", "        return 1"]).trim(), "def encoding(self) -> str:");
    assert_eq!(block_label(&["#[derive(Debug)]", "pub struct Walk {", "    x: u8,"]), "pub struct Walk {");
    assert_eq!(block_label(&["/// Docs.", "pub(crate) async fn run() {"]), "pub(crate) async fn run() {");
    assert_eq!(block_label(&["export default function App() {"]), "export default function App() {");
    assert_eq!(block_label(&["", "  x = compute()", "  y = 2"]), "  x = compute()");
    assert_eq!(block_label(&["", "}"]), "");
}

#[test]
fn region_label_names_the_enclosing_definition_for_mid_function_blocks() {
    let src = [
        "class Builder:",
        "    def build(self, spec):",
        "        state = {}",
        "        for k in spec:",
        "            state[k] = 1",
        "",
        "        questions = {}",
        "        return state, questions",
    ];
    let chunk = &chunk_lines("b.py", &src, Chunking::default())[0];
    assert_eq!(region_label(chunk, 7, 8), ("def build(self, spec): > questions = {}".to_string(), 7));
    assert_eq!(region_label(chunk, 2, 5), ("def build(self, spec):".to_string(), 2)); // already a definition: no prefix
    let text = &chunk_lines("t.txt", &["plain words", "more words"], Chunking::default())[0];
    assert_eq!(region_label(text, 1, 2), ("plain words".to_string(), 1));
    let decorated = &chunk_lines("p.py", &["@property", "def size(self):", "    return 1"], Chunking::default())[0];
    assert_eq!(region_label(decorated, 1, 3), ("def size(self):".to_string(), 2));
    // label line is the def, not the decorator
}
