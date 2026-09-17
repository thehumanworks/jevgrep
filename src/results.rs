//! Turns raw per-file answers into what gets displayed. Pure functions, no I/O.
//!
//! Display rules, chosen so the output never contradicts itself:
//!   1. A row is printed only if its own probability clears the bar. Regions and lines need
//!      `threshold`. Nothing rides in on the strength of a neighbour.
//!   2. Each source line is printed at most once. Jev scores a line twice in effect: alone (the line
//!      question) and with its surroundings (the block question), and the two can disagree. So a
//!      pinpointed line that is already its region's label is not repeated, and a pinpointed line
//!      whose block did not clear the bar stands alone, without a region row.
//!   3. A file is shown only with at least one row. No "relevant, but nothing to show".
//!   4. A file whose section relevance is high but where nothing cleared the bar goes to a separate
//!      weak tier, showing its best blocks if they reach `WEAK_RATIO * threshold`. Broad answers
//!      spread thin, so a near miss in a clearly relevant file is still worth a look.
//!   5. With a --filter, a region or line is shown only if it passes the rules (`keep` >= FILTER_GATE).
//!      Filtering happens before everything else, so filtered-out code cannot put a file on the list.
//!   6. Matched files come first, then the weak tier. Within a tier, the number in a file's header
//!      is the number files are sorted by.

use std::collections::{BTreeMap, HashSet};

use crate::filters::FILTER_GATE;
use crate::search::{BlockHit, FileResult, LineHit};

/// Rule 4: a weak-tier region needs at least this fraction of the threshold.
pub const WEAK_RATIO: f64 = 0.7;
/// Rule 4: how many best blocks a weak file may show.
pub const WEAK_REGIONS: usize = 2;
pub const MAX_WEAK_FILES: usize = 5;

#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub start: usize,
    pub end: usize,
    pub p: f64,
    pub label: String,
    pub label_line: usize,
    /// Pinpointed lines inside, never the label line.
    pub lines: Vec<LineHit>,
}

/// One printed row: a region (with its nested lines) or a stand-alone line.
#[derive(Debug, Clone, Copy)]
pub enum Row<'a> {
    Region(&'a Region),
    Line(&'a LineHit),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileView {
    pub path: String,
    /// Headline and sort key: best evidence anywhere in the file.
    pub relevance: f64,
    /// Jev's 0..1 relevance score for the file's best section.
    pub section_relevance: f64,
    pub confidence: f64,
    pub regions: Vec<Region>,
    /// Pinpointed lines outside any shown region.
    pub lines: Vec<LineHit>,
    pub more_regions: usize,
    pub more_lines: usize,
    /// Shown on file-level relevance alone; nothing cleared the threshold.
    pub weak: bool,
}

impl FileView {
    /// Regions and stand-alone lines in file order.
    pub fn rows(&self) -> Vec<Row<'_>> {
        let mut rows: Vec<Row> = self.regions.iter().map(Row::Region).chain(self.lines.iter().map(Row::Line)).collect();
        rows.sort_by_key(|row| match row {
            Row::Region(r) => r.start,
            Row::Line(h) => h.line,
        });
        rows
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub threshold: f64,
    pub file_threshold: f64,
    pub top: usize,
    pub max_regions: usize,
    pub max_lines: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { threshold: 0.5, file_threshold: 0.6, top: 15, max_regions: 5, max_lines: 10 }
    }
}

fn merge(mut blocks: Vec<&BlockHit>) -> Vec<Region> {
    blocks.sort_by_key(|b| b.start);
    let mut regions: Vec<Region> = Vec::new();
    for b in blocks {
        match regions.last_mut() {
            Some(last) if b.start <= last.end + 2 => {
                last.end = last.end.max(b.end);
                last.p = last.p.max(b.p);
            }
            _ => regions.push(Region {
                start: b.start,
                end: b.end,
                p: b.p,
                label: b.label.trim().to_owned(),
                label_line: if b.label_line > 0 { b.label_line } else { b.start },
                lines: vec![],
            }),
        }
    }
    regions
}

pub fn view_file(fr: &FileResult, lim: &Limits) -> Option<FileView> {
    let blocks: Vec<&BlockHit> = fr.blocks.iter().filter(|b| b.keep >= FILTER_GATE).collect();
    // Guard: one score per source line, whatever produced the answers.
    let mut best_per_line: BTreeMap<usize, &LineHit> = BTreeMap::new();
    for h in fr.lines.iter().filter(|h| h.keep >= FILTER_GATE) {
        let best = best_per_line.entry(h.line).or_insert(h);
        if h.p > best.p {
            *best = h;
        }
    }
    let hits: Vec<&LineHit> = best_per_line.into_values().filter(|h| h.p >= lim.threshold).collect();
    let mut regions = merge(blocks.iter().copied().filter(|b| b.p >= lim.threshold).collect());
    let weak = regions.is_empty() && hits.is_empty();
    if weak {
        if fr.score < lim.file_threshold {
            return None;
        }
        let mut best = blocks.clone();
        best.sort_by(|a, b| b.p.total_cmp(&a.p));
        best.truncate(WEAK_REGIONS);
        regions = merge(best.into_iter().filter(|b| b.p >= WEAK_RATIO * lim.threshold).collect());
        if regions.is_empty() {
            return None;
        }
    }

    regions.sort_by(|a, b| b.p.total_cmp(&a.p));
    let dropped = regions.split_off(lim.max_regions.min(regions.len()));
    let mut kept = regions;
    kept.sort_by_key(|r| r.start);
    let inside = |h: &LineHit, rs: &[Region]| rs.iter().any(|r| r.start <= h.line && h.line <= r.end);
    let labels: HashSet<usize> = kept.iter().map(|r| r.label_line).collect();
    let mut candidates: Vec<&LineHit> = hits.into_iter().filter(|h| !labels.contains(&h.line) && !inside(h, &dropped)).collect();
    let more_lines = candidates.len().saturating_sub(lim.max_lines);
    candidates.sort_by(|a, b| b.p.total_cmp(&a.p));
    candidates.truncate(lim.max_lines);
    candidates.sort_by_key(|h| h.line);
    for r in &mut kept {
        r.lines = candidates.iter().filter(|h| r.start <= h.line && h.line <= r.end).map(|&h| h.clone()).collect();
    }
    let alone: Vec<LineHit> = candidates.iter().filter(|h| !inside(h, &kept)).map(|&h| h.clone()).collect();
    let relevance = kept.iter().map(|r| r.p).chain(candidates.iter().map(|h| h.p)).fold(fr.score, f64::max);
    Some(FileView {
        path: fr.path.clone(),
        relevance,
        section_relevance: fr.score,
        confidence: fr.confidence,
        regions: kept,
        lines: alone,
        more_regions: dropped.len(),
        more_lines,
        weak,
    })
}

pub fn select(results: &[FileResult], lim: &Limits) -> Vec<FileView> {
    let mut views: Vec<FileView> = results.iter().filter_map(|fr| view_file(fr, lim)).collect();
    views.sort_by(|a, b| a.weak.cmp(&b.weak).then(b.relevance.total_cmp(&a.relevance)).then_with(|| a.path.cmp(&b.path)));
    let (mut strong, mut weak): (Vec<_>, Vec<_>) = views.into_iter().partition(|v| !v.weak);
    strong.truncate(lim.top);
    weak.truncate(MAX_WEAK_FILES.min(lim.top - strong.len()));
    strong.extend(weak);
    strong
}

/// (regions, lines) that matched the query but were removed by the filter. For the stats line.
pub fn filtered_out(results: &[FileResult], threshold: f64) -> (usize, usize) {
    let regions = results.iter().flat_map(|fr| &fr.blocks).filter(|b| b.p >= threshold && b.keep < FILTER_GATE).count();
    let lines = results.iter().flat_map(|fr| &fr.lines).filter(|h| h.p >= threshold && h.keep < FILTER_GATE).count();
    (regions, lines)
}

/// Files-only mode: rank purely by section relevance.
pub fn select_files(results: &[FileResult], file_threshold: f64, top: usize) -> Vec<&FileResult> {
    let mut keep: Vec<&FileResult> = results.iter().filter(|fr| fr.score >= file_threshold && fr.keep >= FILTER_GATE).collect();
    keep.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
    keep.truncate(top);
    keep
}
