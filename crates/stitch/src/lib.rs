//! Scrolling capture stitcher. Pure image logic; the app drives scrolling and capture.
//!
//! Driver loop (in the app): capture region -> `push` -> if `Appended`/`Unchanged` send
//! one scroll step -> wait `scroll_delay_ms` -> repeat until `End` (two consecutive
//! unchanged frames), `max_height` reached, or the user clicks Stop. Then `finish`.
//!
//! Algorithm requirements:
//! - Detect a fixed header and footer (rows identical between consecutive frames at the
//!   same position) and exclude them from the matched band; keep the header once at the
//!   top and the footer once at the bottom of the result.
//! - Find the vertical offset between consecutive frames by matching the scrolling band
//!   (row-hash candidates, then verify with a per-pixel tolerance to survive subpixel AA
//!   and cursor blink). Handle partial last scroll (offset smaller than usual).
//! - Ignore narrow columns that change independently (scrollbar thumb): match on the
//!   middle 90% of width.
//! - Never panic on mismatched sizes; return `Failed`.

use stinkshot_core::Rgba;

pub mod drive;
pub mod settle;

#[derive(Clone, Debug)]
pub struct StitchOptions {
    pub max_height: u32,
    /// Max per-channel difference counted as "same pixel".
    pub tolerance: u8,
    /// Crop a scrollbar found at the right edge from the result (its thumb would otherwise
    /// leave a stripe of every position it had).
    pub trim_scrollbar: bool,
}

impl Default for StitchOptions {
    fn default() -> Self {
        Self {
            max_height: 30000,
            tolerance: 6,
            trim_scrollbar: true,
        }
    }
}

/// How settled a candidate frame looks, from `Stitcher::assess`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Assessment {
    /// Rows the frame would add (None when no overlap was found).
    pub new_rows: Option<u32>,
    /// Rows among the new ones that look like loading placeholders (skeleton bars, flat
    /// blocks in a color the page did not show at the start).
    pub placeholder_rows: u32,
}

impl Assessment {
    /// Enough placeholder rows to wait for the page to finish loading.
    pub fn looks_unloaded(&self) -> bool {
        let new = self.new_rows.unwrap_or(0);
        new > 0 && self.placeholder_rows >= 8.max(new * 5 / 100)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PushResult {
    /// First frame or new content appended; `added_rows` new rows.
    Appended { added_rows: u32 },
    /// Frame identical to previous (scroll had no effect yet).
    Unchanged,
    /// Two consecutive unchanged frames, or max height reached: stop scrolling.
    End,
    /// No reliable overlap found; caller should stop and `finish` with what it has.
    Failed,
}

/// Percent of the width ignored on each side when matching (scrollbars etc).
const SIDE_MARGIN_PCT: usize = 5;
/// A row matches when at most 1/N of its compared pixels differ.
const ROW_PIXEL_BUDGET_DIV: usize = 100;
/// A candidate offset is accepted when at most this percent of overlap rows mismatch.
const OVERLAP_BAD_ROW_PCT: usize = 3;
/// Max candidate offsets generated from row hashes before falling back to a full scan.
const MAX_CANDIDATES: usize = 96;
/// Max anchor rows used to generate candidates.
const MAX_ANCHORS: usize = 16;
/// Anchor rows used for the relaxed (lazy content) search, spread over the band.
const RELAXED_ANCHORS: usize = 48;
/// Offsets this close to the last step are always tried by the relaxed search.
const LAST_STEP_SLACK: usize = 8;
/// A row with fewer color runs than this is blank or nearly flat: content not loaded yet.
const UNLOADED_MAX_RUNS: usize = 4;

#[derive(Clone, Copy)]
struct Geom {
    w: usize,
    h: usize,
    x0: usize,
    x1: usize,
    tol: u8,
}

impl Geom {
    fn new(w: usize, h: usize, tol: u8) -> Self {
        let margin = w * SIDE_MARGIN_PCT / 100;
        let (x0, x1) = if w > 2 * margin + 1 {
            (margin, w - margin)
        } else {
            (0, w)
        };
        Self { w, h, x0, x1, tol }
    }

    /// Middle-width bytes of row `y`.
    #[inline]
    fn row<'a>(&self, data: &'a [u8], y: usize) -> &'a [u8] {
        let base = y * self.w;
        &data[(base + self.x0) * 4..(base + self.x1) * 4]
    }

    fn pixel_budget(&self) -> usize {
        (self.x1 - self.x0) / ROW_PIXEL_BUDGET_DIV
    }

    /// Rows are "close" when at most `budget` pixels differ by more than the tolerance.
    #[inline]
    fn rows_close(&self, a: &[u8], b: &[u8], budget: usize) -> bool {
        if a == b {
            return true;
        }
        let tol = i16::from(self.tol);
        let mut bad = 0usize;
        for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
            let far = (i16::from(pa[0]) - i16::from(pb[0])).abs() > tol
                || (i16::from(pa[1]) - i16::from(pb[1])).abs() > tol
                || (i16::from(pa[2]) - i16::from(pb[2])).abs() > tol;
            if far {
                bad += 1;
                if bad > budget {
                    return false;
                }
            }
        }
        true
    }

    /// Exact hash of a row's RGB values over the middle width (alpha ignored).
    fn hash_row(&self, data: &[u8], y: usize) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for px in self.row(data, y).chunks_exact(4) {
            let v = u64::from(u32::from_le_bytes([px[0], px[1], px[2], 0]));
            h = (h.rotate_left(5) ^ v).wrapping_mul(0x517c_c1b7_2722_0a95);
        }
        h
    }

    /// True when at most `allowed` rows in `[y0, y1)` differ between the two frames.
    fn same_region(&self, a: &[u8], b: &[u8], y0: usize, y1: usize, allowed: usize) -> bool {
        let budget = self.pixel_budget();
        let mut bad = 0usize;
        for y in y0..y1 {
            if !self.rows_close(self.row(a, y), self.row(b, y), budget) {
                bad += 1;
                if bad > allowed {
                    return false;
                }
            }
        }
        true
    }
}

/// Stateful scrolling stitcher.
pub struct Stitcher {
    opts: StitchOptions,
    /// Frame width/height of the session (0 until the first frame).
    width: usize,
    frame_h: usize,
    /// Stitched rows excluding the footer.
    body: Vec<u8>,
    body_rows: usize,
    /// Footer rows (taken from the newest frame).
    footer: Vec<u8>,
    footer_h: usize,
    header_h: usize,
    layout_known: bool,
    /// Previous accepted frame (RGBA).
    prev: Vec<u8>,
    frames: usize,
    unchanged: u32,
    last_step: Option<usize>,
    /// First frame (for the scrollbar check at `finish`).
    first: Vec<u8>,
    /// Background colors seen in the first frame (long runs), for placeholder detection.
    bg: Vec<[u8; 3]>,
    /// Layout changes and stale-tail repairs made after the first scroll (diagnostics).
    repairs: u32,
}

impl Stitcher {
    pub fn new(opts: StitchOptions) -> Self {
        Self {
            opts,
            width: 0,
            frame_h: 0,
            body: Vec::new(),
            body_rows: 0,
            footer: Vec::new(),
            footer_h: 0,
            header_h: 0,
            layout_known: false,
            prev: Vec::new(),
            frames: 0,
            unchanged: 0,
            last_step: None,
            first: Vec::new(),
            bg: Vec::new(),
            repairs: 0,
        }
    }

    pub fn options(&self) -> &StitchOptions {
        &self.opts
    }

    pub fn push(&mut self, frame: &Rgba) -> PushResult {
        let w = frame.width as usize;
        let h = frame.height as usize;
        if w == 0 || h == 0 || frame.data.len() != w * h * 4 {
            return PushResult::Failed;
        }
        if self.frames == 0 {
            return self.push_first(frame);
        }
        if w != self.width || h != self.frame_h {
            log::debug!(
                "stitch: frame {w}x{h} differs from session {}x{}",
                self.width,
                self.frame_h
            );
            return PushResult::Failed;
        }
        if self.height() >= self.opts.max_height {
            return PushResult::End;
        }
        let g = Geom::new(w, h, self.opts.tolerance);

        // Unchanged check, on the scrolling band once it is known.
        let (y0, y1) = if self.layout_known {
            (self.header_h, h - self.footer_h)
        } else {
            (0, h)
        };
        let allowed = ((y1 - y0) / 50).max(1);
        if g.same_region(&self.prev, &frame.data, y0, y1, allowed) {
            self.unchanged += 1;
            return if self.unchanged >= 2 {
                PushResult::End
            } else {
                PushResult::Unchanged
            };
        }

        let mut stale: Vec<usize> = Vec::new();
        let (hdr, ftr, d) = if self.layout_known {
            match self.find_offset(&g, &frame.data, self.header_h, self.footer_h) {
                Some(d) => (self.header_h, self.footer_h, d),
                None => match self.repair(&g, &frame.data) {
                    Some((hdr, ftr, d, rows)) => {
                        stale = rows;
                        (hdr, ftr, d)
                    }
                    None => {
                        log::debug!("stitch: no overlap found");
                        return PushResult::Failed;
                    }
                },
            }
        } else {
            let (dh, df) = detect_fixed(&g, &self.prev, &frame.data);
            let mut found = self
                .find_offset(&g, &frame.data, dh, df)
                .map(|d| (dh, df, d));
            if found.is_none() && (dh > 0 || df > 0) {
                found = self.find_offset(&g, &frame.data, 0, 0).map(|d| (0, 0, d));
            }
            if found.is_none()
                && let Some((a, d, rows)) = self.find_offset_relaxed(&g, &frame.data, dh, df)
            {
                stale = rows;
                self.repairs += 1;
                found = Some((dh + a, df, d));
            }
            match found {
                Some(t) => t,
                None => {
                    log::debug!("stitch: no overlap found for first scroll");
                    return PushResult::Failed;
                }
            }
        };

        if !self.layout_known {
            // Commit the layout: the first frame was stored whole, so cut its footer off.
            self.header_h = hdr;
            self.footer_h = ftr;
            self.layout_known = true;
            self.body_rows = self.body_rows.saturating_sub(ftr);
            self.body.truncate(self.body_rows * w * 4);
        }
        if self.layout_known && ftr != self.footer_h {
            log::debug!("stitch: footer {} -> {ftr} rows", self.footer_h);
        }
        let old_ftr = self.footer_h;
        self.header_h = hdr;
        self.footer_h = ftr;
        // Rows of the previous frame that changed in place (lazy content that loaded after
        // it was captured): overwrite them with this frame's version. Screen row r of this
        // frame showed row r + d of the previous one, whose band ended at the body's end.
        for r in stale {
            let b = (self.body_rows + old_ftr + r + d) as isize - h as isize;
            if b >= 0 && (b as usize) < self.body_rows {
                let dst = b as usize * w * 4;
                self.body[dst..dst + w * 4]
                    .copy_from_slice(&frame.data[r * w * 4..(r + 1) * w * 4]);
            }
        }
        // The footer always comes from the newest frame.
        self.footer.clear();
        self.footer
            .extend_from_slice(&frame.data[(h - ftr) * w * 4..]);

        let cap_rows = (self.opts.max_height as usize).saturating_sub(ftr);
        // New content starts right under the previous frame's band (row h - old_ftr - d of
        // this frame) and ends at this frame's footer.
        let start_row = (h - old_ftr).saturating_sub(d).max(hdr);
        let fresh = (h - ftr).saturating_sub(start_row);
        let take = fresh.min(cap_rows.saturating_sub(self.body_rows));
        let start = start_row * w * 4;
        self.body
            .extend_from_slice(&frame.data[start..start + take * w * 4]);
        self.body_rows += take;

        self.prev.clear();
        self.prev.extend_from_slice(&frame.data);
        self.frames += 1;
        self.unchanged = 0;
        self.last_step = Some(d);
        PushResult::Appended {
            added_rows: take as u32,
        }
    }

    fn push_first(&mut self, frame: &Rgba) -> PushResult {
        let w = frame.width as usize;
        let h = frame.height as usize;
        let keep = h.min(self.opts.max_height.max(1) as usize);
        self.width = w;
        self.frame_h = h;
        self.body = frame.data[..keep * w * 4].to_vec();
        self.body_rows = keep;
        self.footer.clear();
        self.footer_h = 0;
        self.header_h = 0;
        self.layout_known = false;
        self.prev = frame.data.clone();
        self.first = frame.data.clone();
        // Colors accepted before the first frame (a first step that timed out) stay.
        let accepted = std::mem::take(&mut self.bg);
        let g = Geom::new(w, h, self.opts.tolerance);
        self.bg = settle::background_colors((0..h).map(|y| g.row(&frame.data, y)));
        self.bg.extend(accepted);
        self.frames = 1;
        self.unchanged = 0;
        self.last_step = None;
        self.repairs = 0;
        PushResult::Appended {
            added_rows: keep as u32,
        }
    }

    /// Find the scroll distance `d >= 1` such that new band row `y` equals previous band
    /// row `y + d`. Returns `None` when nothing verifies.
    fn find_offset(&self, g: &Geom, new: &[u8], hdr: usize, ftr: usize) -> Option<usize> {
        if hdr + ftr + 4 > g.h {
            return None;
        }
        let bh = g.h - hdr - ftr;
        let min_ov = (bh / 10).max(4);
        if bh <= min_ov {
            return None;
        }
        let max_d = bh - min_ov;
        let prev = &self.prev;
        let budget = g.pixel_budget();
        let last_step = self.last_step;

        // best = (mismatching rows, d)
        let best: std::cell::Cell<Option<(usize, usize)>> = std::cell::Cell::new(None);
        let try_d = |d: usize| {
            let ov = bh - d;
            let mut limit = ov * OVERLAP_BAD_ROW_PCT / 100;
            if let Some((b, _)) = best.get() {
                limit = limit.min(b);
            }
            let mut bad = 0usize;
            for y in 0..ov {
                let a = g.row(new, hdr + y);
                let b = g.row(prev, hdr + y + d);
                if !g.rows_close(a, b, budget) {
                    bad += 1;
                    if bad > limit {
                        return;
                    }
                }
            }
            let better = match best.get() {
                None => true,
                Some((bb, bd)) => {
                    bad < bb
                        || (bad == bb
                            && match last_step {
                                Some(ls) => d.abs_diff(ls) < bd.abs_diff(ls),
                                None => d > bd,
                            })
                }
            };
            if better {
                best.set(Some((bad, d)));
            }
        };

        // Candidates from distinctive anchor rows near the top of the new band: a new row
        // at y can only match when the overlap is at least min_ov, so y < min_ov.
        let mut prev_hashes: Vec<(u64, u32)> = (0..bh)
            .map(|y| (g.hash_row(prev, hdr + y), y as u32))
            .collect();
        prev_hashes.sort_unstable();
        let new_hashes: Vec<u64> = (0..min_ov).map(|y| g.hash_row(new, hdr + y)).collect();
        let mut sorted_new = new_hashes.clone();
        sorted_new.sort_unstable();
        let mut anchors: Vec<usize> = (0..min_ov)
            .filter(|&y| {
                let hv = new_hashes[y];
                let n_new = sorted_new.partition_point(|&v| v <= hv)
                    - sorted_new.partition_point(|&v| v < hv);
                n_new == 1 && count_hash(&prev_hashes, hv) <= 8
            })
            .collect();
        if anchors.len() > MAX_ANCHORS {
            let n = anchors.len();
            anchors = (0..MAX_ANCHORS)
                .map(|i| anchors[i * n / MAX_ANCHORS])
                .collect();
        }
        let mut cands: Vec<usize> = Vec::new();
        for &y in &anchors {
            let hv = new_hashes[y];
            let lo = prev_hashes.partition_point(|&(v, _)| v < hv);
            for &(v, r) in &prev_hashes[lo..] {
                if v != hv {
                    break;
                }
                let r = r as usize;
                if r > y && r - y <= max_d {
                    cands.push(r - y);
                }
            }
        }
        cands.sort_unstable();
        cands.dedup();
        cands.truncate(MAX_CANDIDATES);
        for &d in &cands {
            try_d(d);
        }

        // Fallback (subpixel noise, blank-heavy content): scan every offset.
        if best.get().is_none() {
            for d in (1..=max_d).rev() {
                try_d(d);
            }
        }
        best.get().map(|(_, d)| d)
    }

    /// After a failed match with a known layout: the sticky bars may have changed (a
    /// header that collapses on scroll, a cookie bar that appears), or the previous frame's
    /// rows were still loading. Returns (header, footer, step, stale screen rows).
    #[allow(clippy::type_complexity)]
    fn repair(&mut self, g: &Geom, new: &[u8]) -> Option<(usize, usize, usize, Vec<usize>)> {
        let (hdr, ftr) = (self.header_h, self.footer_h);
        let (dh, df) = detect_fixed(g, &self.prev, new);
        for (h2, f2) in [(dh, df), (dh.max(hdr), df.max(ftr)), (hdr, df.max(ftr))] {
            if (h2, f2) != (hdr, ftr)
                && let Some(d) = self.find_offset(g, new, h2, f2)
            {
                log::debug!("stitch: layout {hdr}/{ftr} -> {h2}/{f2} rows");
                self.repairs += 1;
                return Some((h2, f2, d, Vec::new()));
            }
        }
        let (a, d, stale) = self.find_offset_relaxed(g, new, hdr, ftr)?;
        log::debug!(
            "stitch: relaxed match, header +{a}, {} stale rows, step {d}",
            stale.len()
        );
        self.repairs += 1;
        Some((hdr + a, ftr, d, stale))
    }

    /// Offset search for frames that only partly overlap: a run of mismatching rows at the
    /// top of the overlap is a sticky bar that grew; other mismatching rows are rows of the
    /// previous frame that changed in place since (lazy content that finished loading).
    /// At least a third of the rest must match, with 8 or more distinctive rows among them.
    /// A changed row counts as loaded only when its previous version looked unloaded (blank,
    /// nearly flat or a placeholder); more than a few changed detailed rows reject the offset,
    /// so a page with repeating structure (a comment thread) never matches a frame that does
    /// not overlap it. Only offsets suggested by exact matches of distinctive rows (and the
    /// last step's neighbourhood) are tried, which keeps this cheap on large frames.
    /// Returns (top run, step, stale rows as screen rows of the new frame), where the stale
    /// rows are only those whose previous version looked unloaded.
    fn find_offset_relaxed(
        &self,
        g: &Geom,
        new: &[u8],
        hdr: usize,
        ftr: usize,
    ) -> Option<(usize, usize, Vec<usize>)> {
        if hdr + ftr + 4 > g.h {
            return None;
        }
        let bh = g.h - hdr - ftr;
        let min_ov = (bh / 10).max(4);
        if bh <= min_ov * 2 {
            return None;
        }
        let max_run = bh / 3;
        let budget = g.pixel_budget();
        let prev = &self.prev;
        // New band rows near the top in the color of the sticky bar above them.
        let bar_rows: Vec<bool> = if hdr > 0 {
            let c = settle::dominant_color(g.row(new, 0));
            (0..max_run)
                .map(|r| {
                    let rc = settle::dominant_color(g.row(new, hdr + r));
                    (0..3).all(|i| rc[i].abs_diff(c[i]) <= 8)
                })
                .collect()
        } else {
            vec![false; max_run]
        };
        let bg = &self.bg;
        let unloaded = |row: &[u8]| {
            settle::row_runs(row, 10, UNLOADED_MAX_RUNS) < UNLOADED_MAX_RUNS
                || settle::placeholder_row(row, bg)
        };
        let max_d = bh - min_ov;
        let cands = self.relaxed_candidates(g, new, hdr, bh, max_d);
        // (matched rows, d, top)
        let mut best: Option<(usize, usize, usize)> = None;
        for d in cands {
            let ov = bh - d;
            let close =
                |y: usize| g.rows_close(g.row(new, hdr + y), g.row(prev, hdr + y + d), budget);
            let mut y = 0;
            while y < ov && y < max_run && !close(y) {
                y += 1;
            }
            if y >= max_run || y >= ov {
                continue;
            }
            // A sticky bar grew only when what it now covers was detailed content in the
            // previous frame; a run over placeholder or blank rows is stale content instead.
            let detailed = (0..y)
                .filter(|&r| settle::row_runs(g.row(prev, hdr + r + d), 10, 8) >= 8)
                .count();
            // Or it has the color of the bar it extends (a header that changed height).
            let bar_like = bar_rows[..y].iter().filter(|&&b| b).count() * 2 >= y;
            let top = if y > 0 && (detailed * 2 >= y || bar_like) {
                y
            } else {
                0
            };
            let need = min_ov.max((ov - top) / 3);
            if ov - top < need {
                continue;
            }
            let allowed = (ov - top) - need;
            let (mut matched, mut missed, mut distinct) = (0, y - top, 0);
            if missed > allowed {
                continue;
            }
            // Rows of the top run that are not a grown bar must have been unloaded too.
            let noise = ((ov - top) * OVERLAP_BAD_ROW_PCT / 100).max(1);
            let mut detailed_missed = (top..y)
                .filter(|&r| !unloaded(g.row(prev, hdr + r + d)))
                .count();
            if detailed_missed > noise {
                continue;
            }
            while y < ov {
                if close(y) {
                    matched += 1;
                    if settle::row_runs(g.row(new, hdr + y), 10, 4) >= 4 {
                        distinct += 1;
                    }
                } else {
                    missed += 1;
                    if !unloaded(g.row(prev, hdr + y + d)) {
                        detailed_missed += 1;
                    }
                    if missed > allowed || detailed_missed > noise {
                        break;
                    }
                }
                y += 1;
            }
            let ok = missed <= allowed
                && detailed_missed <= noise
                && matched >= need
                && distinct >= 8
                && (top > 0 || missed > 0);
            if !ok {
                continue;
            }
            let better = match best {
                None => true,
                Some((m, bd, _)) => {
                    matched > m
                        || (matched == m
                            && self
                                .last_step
                                .is_some_and(|ls| d.abs_diff(ls) < bd.abs_diff(ls)))
                }
            };
            if better {
                best = Some((matched, d, top));
            }
        }
        let (_, d, top) = best?;
        let ov = bh - d;
        // Never overwrite rows that were already detailed: only unloaded ones get replaced.
        let stale = (top..ov)
            .filter(|&y| {
                let old = g.row(prev, hdr + y + d);
                !g.rows_close(g.row(new, hdr + y), old, budget) && unloaded(old)
            })
            .map(|y| hdr + y)
            .collect();
        Some((top, d, stale))
    }

    /// Offsets worth trying in `find_offset_relaxed`: from distinctive new band rows (unique
    /// in the new band, rare in the previous one, not blank) whose exact hash appears in the
    /// previous band, plus the last step's neighbourhood. Sorted, at most `MAX_CANDIDATES`.
    fn relaxed_candidates(
        &self,
        g: &Geom,
        new: &[u8],
        hdr: usize,
        bh: usize,
        max_d: usize,
    ) -> Vec<usize> {
        let prev = &self.prev;
        let mut prev_hashes: Vec<(u64, u32)> = (0..bh)
            .map(|y| (g.hash_row(prev, hdr + y), y as u32))
            .collect();
        prev_hashes.sort_unstable();
        let new_hashes: Vec<u64> = (0..bh).map(|y| g.hash_row(new, hdr + y)).collect();
        let mut sorted_new = new_hashes.clone();
        sorted_new.sort_unstable();
        let anchors: Vec<usize> = (0..bh.saturating_sub(1))
            .filter(|&y| {
                let hv = new_hashes[y];
                let n_new = sorted_new.partition_point(|&v| v <= hv)
                    - sorted_new.partition_point(|&v| v < hv);
                n_new == 1
                    && (1..=8).contains(&count_hash(&prev_hashes, hv))
                    && settle::row_runs(g.row(new, hdr + y), 10, 4) >= 4
            })
            .collect();
        let step = (anchors.len() / RELAXED_ANCHORS).max(1);
        let mut cands: Vec<usize> = Vec::new();
        for &y in anchors.iter().step_by(step) {
            let hv = new_hashes[y];
            let lo = prev_hashes.partition_point(|&(v, _)| v < hv);
            for &(v, r) in &prev_hashes[lo..] {
                if v != hv {
                    break;
                }
                let r = r as usize;
                if r > y && r - y <= max_d {
                    cands.push(r - y);
                }
            }
        }
        if let Some(ls) = self.last_step {
            let lo = ls.saturating_sub(LAST_STEP_SLACK).max(1);
            cands.extend((lo..=ls + LAST_STEP_SLACK).filter(|&d| d <= max_d));
        }
        cands.sort_unstable();
        cands.dedup();
        if cands.len() > MAX_CANDIDATES * 2 {
            let n = cands.len();
            cands = (0..MAX_CANDIDATES * 2)
                .map(|i| cands[i * n / (MAX_CANDIDATES * 2)])
                .collect();
        }
        cands
    }

    /// Whether `frame` looks fully loaded, without changing any state: the rows it would
    /// add and how many of them look like loading placeholders.
    pub fn assess(&self, frame: &Rgba) -> Assessment {
        let w = frame.width as usize;
        let h = frame.height as usize;
        if w == 0 || h == 0 || frame.data.len() != w * h * 4 {
            return Assessment::default();
        }
        let g = Geom::new(w, h, self.opts.tolerance);
        if self.frames == 0 {
            // First frame: every row is new; the page colors come from the frame itself
            // (plus any accepted after a timeout).
            let mut bg = settle::background_colors((0..h).map(|y| g.row(&frame.data, y)));
            bg.extend_from_slice(&self.bg);
            let suspect = (0..h)
                .filter(|&y| settle::placeholder_row(g.row(&frame.data, y), &bg))
                .count();
            return Assessment {
                new_rows: Some(h as u32),
                placeholder_rows: suspect as u32,
            };
        }
        if w != self.width || h != self.frame_h {
            return Assessment::default();
        }
        let (hdr, ftr) = if self.layout_known {
            (self.header_h, self.footer_h)
        } else {
            detect_fixed(&g, &self.prev, &frame.data)
        };
        if hdr + ftr + 4 > h {
            return Assessment::default();
        }
        let bh = h - hdr - ftr;
        // An unchanged frame (the end of the page, the end grace) has no partial overlap to
        // look for, and the relaxed search is the expensive part.
        let unchanged = g.same_region(&self.prev, &frame.data, hdr, h - ftr, (bh / 50).max(1));
        let d = self.find_offset(&g, &frame.data, hdr, ftr).or_else(|| {
            if unchanged {
                return None;
            }
            self.find_offset_relaxed(&g, &frame.data, hdr, ftr)
                .map(|(_, d, _)| d)
        });
        // Without an overlap every band row is "new".
        let (y0, n) = match d {
            Some(d) => (hdr + bh - d, d),
            None => (hdr, bh),
        };
        let suspect = (y0..y0 + n)
            .filter(|&y| settle::placeholder_row(g.row(&frame.data, y), &self.bg))
            .count();
        Assessment {
            new_rows: d.map(|d| d as u32),
            placeholder_rows: suspect as u32,
        }
    }

    /// Treat the block colors of `frame`'s band as page colors from now on: a step waited
    /// the full settle time and they did not change, so they are content (a gray sidebar,
    /// a colored banner), not placeholders. Keeps later steps from waiting for them again.
    pub fn accept_colors(&mut self, frame: &Rgba) {
        let w = frame.width as usize;
        let h = frame.height as usize;
        let first = self.frames == 0;
        if w == 0
            || frame.data.len() != w * h * 4
            || (!first && (w != self.width || h != self.frame_h))
        {
            return;
        }
        let g = Geom::new(w, h, self.opts.tolerance);
        let (y0, y1) = if first {
            (0, h)
        } else {
            (self.header_h, h - self.footer_h)
        };
        let new = settle::placeholder_colors((y0..y1).map(|y| g.row(&frame.data, y)), &self.bg);
        if !new.is_empty() {
            log::debug!(
                "stitch: accepting {} page colors after a timeout",
                new.len()
            );
            self.bg.extend(new);
        }
    }

    /// Layout changes and stale-row repairs made so far (diagnostics).
    pub fn repairs(&self) -> u32 {
        self.repairs
    }

    /// Current stitched height in px (for the progress HUD).
    pub fn height(&self) -> u32 {
        (self.body_rows + self.footer_h) as u32
    }

    /// Number of frames that contributed (the first frame plus every `Appended`).
    pub fn frames(&self) -> usize {
        self.frames
    }

    pub fn finish(self) -> Rgba {
        if self.frames == 0 {
            return Rgba::new(0, 0);
        }
        let trim = if self.opts.trim_scrollbar && self.frames > 1 {
            let skip = (self.header_h, self.footer_h);
            settle::scrollbar_width(&self.first, &self.prev, self.width, self.frame_h, skip)
        } else {
            0
        };
        let mut data = self.body;
        data.extend_from_slice(&self.footer);
        let h = (self.body_rows + self.footer_h) as u32;
        let img = Rgba {
            width: self.width as u32,
            height: h,
            data,
        };
        if trim == 0 {
            return img;
        }
        log::debug!("stitch: cropping a {trim} px scrollbar");
        img.crop(stinkshot_core::PxRect::new(
            0,
            0,
            (self.width - trim) as i32,
            h as i32,
        ))
    }
}

fn count_hash(sorted: &[(u64, u32)], hv: u64) -> usize {
    sorted.partition_point(|&(v, _)| v <= hv) - sorted.partition_point(|&(v, _)| v < hv)
}

/// Leading and trailing rows identical between two frames at the same position.
fn detect_fixed(g: &Geom, a: &[u8], b: &[u8]) -> (usize, usize) {
    let budget = g.pixel_budget();
    let cap = g.h / 3;
    let same = |y: usize| g.rows_close(g.row(a, y), g.row(b, y), budget);
    let mut hdr = 0;
    while hdr < cap && same(hdr) {
        hdr += 1;
    }
    let mut ftr = 0;
    while ftr < cap && same(g.h - 1 - ftr) {
        ftr += 1;
    }
    (hdr, ftr)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 33) as u32
        }
    }

    /// The synthetic frames carry a 6 px scrollbar whose thumb moves; most tests compare
    /// the full width, so they keep it.
    fn no_trim() -> StitchOptions {
        StitchOptions {
            trim_scrollbar: false,
            ..StitchOptions::default()
        }
    }

    const W: usize = 120;
    const SB: usize = 6; // scrollbar columns (right edge)

    fn noise_row(rng: &mut Lcg, w: usize) -> Vec<u8> {
        let mut row = Vec::with_capacity(w * 4);
        let mut x = 0;
        while x < w {
            let len = 2 + (rng.next() % 9) as usize;
            let ink = !rng.next().is_multiple_of(3);
            let v = if ink {
                20 + (rng.next() % 120) as u8
            } else {
                250
            };
            let tint = (rng.next() % 40) as u8;
            for _ in 0..len.min(w - x) {
                row.extend_from_slice(&[v, v.saturating_add(tint), v, 255]);
            }
            x += len;
        }
        row
    }

    fn make_rows(seed: u64, n: usize, w: usize) -> Vec<Vec<u8>> {
        let mut rng = Lcg(seed);
        (0..n).map(|_| noise_row(&mut rng, w)).collect()
    }

    /// Page with repeated rows and a blank block to tempt false matches.
    fn make_page(n: usize) -> Vec<Vec<u8>> {
        let mut page = make_rows(1, n, W);
        for r in 40..48 {
            page[r] = page[39].clone();
        }
        for r in (97..n).step_by(97) {
            page[r] = page[r - 1].clone();
        }
        for row in page.iter_mut().take(760.min(n)).skip(700) {
            *row = vec![255; W * 4];
        }
        page
    }

    fn frame(
        page: &[Vec<u8>],
        header: &[Vec<u8>],
        footer: &[Vec<u8>],
        s: usize,
        v: usize,
        phase: usize,
        jitter: bool,
    ) -> Rgba {
        let band = v - header.len() - footer.len();
        let mut rows: Vec<&Vec<u8>> = Vec::new();
        rows.extend(header.iter());
        rows.extend(page[s..s + band].iter());
        rows.extend(footer.iter());
        let mut data = Vec::with_capacity(W * v * 4);
        for (y, row) in rows.iter().enumerate() {
            let mut row = (*row).clone();
            // Scrollbar thumb that moves with the phase.
            let thumb = (phase * 13) % (v - 20);
            for x in W - SB..W {
                let on = y >= thumb && y < thumb + 20;
                let c = if on { 90 } else { 230 };
                row[x * 4..x * 4 + 4].copy_from_slice(&[c, c, c, 255]);
            }
            if jitter && y >= header.len() && y < v - footer.len() {
                // Subpixel-AA style jitter on a few percent of pixels.
                for x in (7..W - SB - 1).step_by(1 + (y + phase) % 67) {
                    let j = 1 + ((x + y + phase) % 4) as u8;
                    row[x * 4] = row[x * 4].saturating_add(j);
                }
            }
            data.extend_from_slice(&row);
        }
        Rgba::from_raw(W as u32, v as u32, data)
    }

    /// Compare stitched output with the expected rows, ignoring scrollbar columns.
    fn assert_matches(out: &Rgba, expected: &[&Vec<u8>]) {
        assert_matches_tol(out, expected, 0);
    }

    fn assert_matches_tol(out: &Rgba, expected: &[&Vec<u8>], tol: u8) {
        assert_eq!(out.height as usize, expected.len(), "height");
        assert_eq!(out.width as usize, W);
        for (y, exp) in expected.iter().enumerate() {
            let a = &out.data[y * W * 4..y * W * 4 + (W - SB) * 4];
            let b = &exp[..(W - SB) * 4];
            let ok = a.iter().zip(b).all(|(p, q)| p.abs_diff(*q) <= tol);
            assert!(ok, "row {y} differs");
        }
    }

    struct Setup {
        page: Vec<Vec<u8>>,
        header: Vec<Vec<u8>>,
        footer: Vec<Vec<u8>>,
        v: usize,
    }

    fn setup(page_rows: usize) -> Setup {
        Setup {
            page: make_page(page_rows),
            header: make_rows(777, 30, W),
            footer: make_rows(888, 24, W),
            v: 300,
        }
    }

    fn run(
        s: &Setup,
        step: usize,
        jitter: bool,
        opts: StitchOptions,
    ) -> (Stitcher, Vec<PushResult>) {
        let band = s.v - s.header.len() - s.footer.len();
        let last = s.page.len() - band;
        let mut st = Stitcher::new(opts);
        let mut results = Vec::new();
        let mut off = 0;
        let mut phase = 0;
        loop {
            results.push(st.push(&frame(
                &s.page, &s.header, &s.footer, off, s.v, phase, jitter,
            )));
            phase += 1;
            if off == last {
                break;
            }
            off = (off + step).min(last);
        }
        (st, results)
    }

    fn expected(s: &Setup) -> Vec<&Vec<u8>> {
        s.header
            .iter()
            .chain(s.page.iter())
            .chain(s.footer.iter())
            .collect()
    }

    #[test]
    fn stitches_page_with_sticky_header_footer_and_partial_last_scroll() {
        let s = setup(1500);
        let (mut st, results) = run(&s, 200, false, no_trim());
        assert!(
            results
                .iter()
                .all(|r| matches!(r, PushResult::Appended { .. })),
            "{results:?}"
        );
        // Last scroll is partial: (1500 - 246) % 200 = 54.
        assert_eq!(
            results.last(),
            Some(&PushResult::Appended { added_rows: 54 })
        );
        // Two more identical frames end the capture.
        let again = frame(&s.page, &s.header, &s.footer, 1500 - 246, s.v, 99, false);
        assert_eq!(st.push(&again), PushResult::Unchanged);
        assert_eq!(st.push(&again), PushResult::End);
        assert_eq!(st.height() as usize, 30 + 1500 + 24);
        assert_eq!(st.frames(), results.len());
        assert_matches(&st.finish(), &expected(&s));
    }

    #[test]
    fn stitches_with_various_steps_and_jitter() {
        for (step, jitter) in [(90, false), (150, true), (215, true), (37, true)] {
            let s = setup(1100);
            let (st, results) = run(&s, step, jitter, no_trim());
            assert!(
                results
                    .iter()
                    .all(|r| matches!(r, PushResult::Appended { .. })),
                "step {step}: {results:?}"
            );
            assert_matches_tol(&st.finish(), &expected(&s), if jitter { 4 } else { 0 });
        }
    }

    #[test]
    fn identical_frames_end_after_two() {
        let s = setup(600);
        let f = frame(&s.page, &s.header, &s.footer, 0, s.v, 0, false);
        let mut st = Stitcher::new(no_trim());
        assert_eq!(st.push(&f), PushResult::Appended { added_rows: 300 });
        assert_eq!(st.push(&f), PushResult::Unchanged);
        assert_eq!(st.push(&f), PushResult::End);
        assert_eq!(st.height(), 300);
        assert_eq!(st.frames(), 1);
        assert_eq!(st.finish().data, f.data);
    }

    #[test]
    fn nonsense_frame_fails() {
        let s = setup(600);
        let mut st = Stitcher::new(no_trim());
        st.push(&frame(&s.page, &s.header, &s.footer, 0, s.v, 0, false));
        let other = make_rows(31337, 300, W);
        let data: Vec<u8> = other.iter().flatten().copied().collect();
        assert_eq!(
            st.push(&Rgba::from_raw(W as u32, 300, data)),
            PushResult::Failed
        );
        // State is intact: a real next frame still stitches.
        let r = st.push(&frame(&s.page, &s.header, &s.footer, 100, s.v, 1, false));
        assert_eq!(r, PushResult::Appended { added_rows: 100 });
    }

    #[test]
    fn size_mismatch_fails_without_panic() {
        let s = setup(600);
        let mut st = Stitcher::new(no_trim());
        st.push(&frame(&s.page, &s.header, &s.footer, 0, s.v, 0, false));
        assert_eq!(
            st.push(&Rgba::filled(W as u32 + 1, 300, [1, 2, 3, 255])),
            PushResult::Failed
        );
        assert_eq!(
            st.push(&Rgba::filled(W as u32, 299, [1, 2, 3, 255])),
            PushResult::Failed
        );
        assert_eq!(st.push(&Rgba::new(0, 0)), PushResult::Failed);
        let bad = Rgba {
            width: 10,
            height: 10,
            data: vec![0; 5],
        };
        assert_eq!(st.push(&bad), PushResult::Failed);
        assert_eq!(
            Stitcher::new(no_trim()).push(&Rgba::new(0, 5)),
            PushResult::Failed
        );
    }

    #[test]
    fn max_height_cap() {
        let s = setup(1500);
        let opts = StitchOptions {
            max_height: 700,
            tolerance: 6,
            trim_scrollbar: false,
        };
        let (mut st, results) = run(&s, 200, false, opts);
        assert!(results.len() > 2);
        assert_eq!(st.height(), 700);
        // Once capped, further pushes report End.
        let f = frame(&s.page, &s.header, &s.footer, 1254, s.v, 0, false);
        assert_eq!(st.push(&f), PushResult::End);
        let out = st.finish();
        assert_eq!(out.height, 700);
        let mut exp: Vec<&Vec<u8>> = s.header.iter().collect();
        exp.extend(s.page.iter().take(700 - 30 - 24));
        exp.extend(s.footer.iter());
        assert_matches(&out, &exp);
    }

    #[test]
    fn no_fixed_regions_and_trivial_finish() {
        let page = make_page(900);
        let mut st = Stitcher::new(no_trim());
        let mut off = 0;
        while off + 200 <= 900 {
            let data: Vec<u8> = page[off..off + 200].iter().flatten().copied().collect();
            let r = st.push(&Rgba::from_raw(W as u32, 200, data));
            assert!(matches!(r, PushResult::Appended { .. }), "{r:?}");
            off += 150;
        }
        let out = st.finish();
        let top: Vec<&Vec<u8>> = page.iter().take(out.height as usize).collect();
        assert_eq!(out.height as usize, 800);
        assert_matches(&out, &top);

        let mut one = Stitcher::new(no_trim());
        one.push(&Rgba::filled(4, 4, [9, 9, 9, 255]));
        assert_eq!(one.finish().height, 4);
        assert_eq!(Stitcher::new(no_trim()).finish().height, 0);
    }

    #[test]
    fn push_speed_1080p() {
        let (w, h) = (1920usize, 1080usize);
        let rows = make_rows(5, h + 400, w);
        let mk = |off: usize| {
            let data: Vec<u8> = rows[off..off + h].iter().flatten().copied().collect();
            Rgba::from_raw(w as u32, h as u32, data)
        };
        let mut st = Stitcher::new(no_trim());
        st.push(&mk(0));
        let f = mk(300);
        let t = std::time::Instant::now();
        let r = st.push(&f);
        let el = t.elapsed();
        assert_eq!(r, PushResult::Appended { added_rows: 300 });
        eprintln!("1080p push: {el:?}");
        if !cfg!(debug_assertions) {
            assert!(el.as_millis() < 30, "push took {el:?}");
        }
    }

    /// A comment-thread-like page with a 30-row period: 14 unique text rows, 10 blank rows
    /// and 6 identical toolbar rows.
    fn periodic_page(w: usize, rows: usize) -> Vec<Vec<u8>> {
        let mut rng = Lcg(77);
        let toolbar: Vec<u8> = (0..w)
            .flat_map(|x| {
                let v = if x % 16 < 5 { 90 } else { 245 };
                [v, v, v, 255]
            })
            .collect();
        (0..rows)
            .map(|y| match y % 30 {
                0..14 => {
                    let mut r = vec![255u8; w * 4];
                    for x in w / 20..w * 3 / 4 {
                        if rng.next().is_multiple_of(5) {
                            let v = (rng.next() % 120) as u8;
                            r[x * 4..x * 4 + 3].copy_from_slice(&[v, v, v]);
                        }
                    }
                    r
                }
                14..24 => vec![255u8; w * 4],
                _ => toolbar.clone(),
            })
            .collect()
    }

    fn page_frame(page: &[Vec<u8>], w: usize, off: usize, h: usize) -> Rgba {
        let data: Vec<u8> = page[off..off + h].iter().flatten().copied().collect();
        Rgba::from_raw(w as u32, h as u32, data)
    }

    #[test]
    fn periodic_page_never_matches_a_frame_without_overlap() {
        let (w, h) = (400usize, 400usize);
        let page = periodic_page(w, 3000);
        for jump in [1000usize, 1600, 2300] {
            let mut st = Stitcher::new(no_trim());
            assert!(matches!(
                st.push(&page_frame(&page, w, 0, h)),
                PushResult::Appended { .. }
            ));
            assert_eq!(
                st.push(&page_frame(&page, w, 300, h)),
                PushResult::Appended { added_rows: 300 }
            );
            let r = st.push(&page_frame(&page, w, jump, h));
            assert_eq!(r, PushResult::Failed, "jump to {jump}");
            let out = st.finish();
            assert_eq!(out.height, 700);
            let exp: Vec<u8> = page[..700].iter().flatten().copied().collect();
            assert!(
                out.data == exp,
                "jump to {jump}: captured rows were overwritten"
            );
        }
        // First scroll (layout not known yet) too.
        let mut st = Stitcher::new(no_trim());
        st.push(&page_frame(&page, w, 0, h));
        assert_eq!(st.push(&page_frame(&page, w, 1000, h)), PushResult::Failed);
    }

    #[test]
    fn relaxed_search_is_cheap_on_large_frames() {
        let (w, h) = (2560usize, 1400usize);
        let page = periodic_page(w, 4000);
        let mut st = Stitcher::new(no_trim());
        st.push(&page_frame(&page, w, 0, h));
        st.push(&page_frame(&page, w, 600, h));
        let same = page_frame(&page, w, 600, h);
        let t = std::time::Instant::now();
        let _ = st.assess(&same);
        let same_el = t.elapsed();
        let jump = page_frame(&page, w, 2500, h);
        let t = std::time::Instant::now();
        let a = st.assess(&jump);
        let r = st.push(&jump);
        let jump_el = t.elapsed();
        assert_eq!(a.new_rows, None);
        assert_eq!(r, PushResult::Failed);
        eprintln!("2560x1400 assess unchanged {same_el:?}, assess+push no overlap {jump_el:?}");
        if !cfg!(debug_assertions) {
            assert!(same_el.as_millis() < 60, "assess took {same_el:?}");
            assert!(jump_el.as_millis() < 120, "assess+push took {jump_el:?}");
        }
    }

    /// Frame of `page` at offset `s` with header/footer, where band rows in `gray` show a
    /// flat placeholder (lazy content not loaded yet).
    fn frame_with_gray(s: &Setup, off: usize, gray: std::ops::Range<usize>) -> Rgba {
        let mut f = frame(&s.page, &s.header, &s.footer, off, s.v, 0, false);
        for y in gray {
            let r = s.header.len() + y;
            for x in 0..W {
                f.data[(r * W + x) * 4..(r * W + x) * 4 + 4].copy_from_slice(&[214, 214, 218, 255]);
            }
        }
        f
    }

    #[test]
    fn rows_that_load_after_capture_are_replaced() {
        let s = setup(1200);
        let band = s.v - s.header.len() - s.footer.len();
        let last = s.page.len() - band;
        let mut st = Stitcher::new(no_trim());
        let mut off = 0;
        let mut k = 0;
        loop {
            // Every second frame was captured while its lowest 60 rows were still loading;
            // the next frame (150 rows further) shows them loaded.
            let f = if k % 2 == 1 && off != last {
                frame_with_gray(&s, off, band - 60..band)
            } else {
                frame(&s.page, &s.header, &s.footer, off, s.v, 0, false)
            };
            let r = st.push(&f);
            assert!(matches!(r, PushResult::Appended { .. }), "frame {k}: {r:?}");
            if off == last {
                break;
            }
            off = (off + 150).min(last);
            k += 1;
        }
        assert!(st.repairs() > 0);
        assert_matches(&st.finish(), &expected(&s));
    }

    #[test]
    fn header_that_collapses_after_the_first_scroll() {
        // 50-row header at the top of the page, 30 rows (same bar color) once scrolled.
        let page = make_page(1000);
        let bar = |rows: usize| -> Vec<Vec<u8>> {
            (0..rows)
                .map(|y| {
                    let mut r = vec![0u8; W * 4];
                    for x in 0..W {
                        let ink = y % 12 == 5 && x % 9 < (y * 7 + x) % 5;
                        let c: [u8; 4] = if ink {
                            [240, 240, 240, 255]
                        } else {
                            [30, 40, 60, 255]
                        };
                        r[x * 4..x * 4 + 4].copy_from_slice(&c);
                    }
                    r
                })
                .collect()
        };
        let (big, small) = (bar(50), bar(30));
        let footer = make_rows(888, 20, W);
        let v = 260;
        let mk = |hdr: &[Vec<u8>], off: usize| {
            let band = v - hdr.len() - footer.len();
            let mut d = Vec::new();
            for r in hdr.iter().chain(&page[off..off + band]).chain(&footer) {
                d.extend_from_slice(r);
            }
            Rgba::from_raw(W as u32, v as u32, d)
        };
        let mut st = Stitcher::new(no_trim());
        assert!(matches!(st.push(&mk(&big, 0)), PushResult::Appended { .. }));
        // Scrolled: the page moved up 100 rows and the header lost 20 rows.
        let band_small = v - 30 - 20;
        let last = page.len() - band_small;
        let mut off = 100usize;
        loop {
            let r = st.push(&mk(&small, off.min(last)));
            assert!(matches!(r, PushResult::Appended { .. }), "off {off}: {r:?}");
            if off >= last {
                break;
            }
            off += 100;
        }
        let out = st.finish();
        let exp: Vec<&Vec<u8>> = big.iter().chain(page.iter()).chain(footer.iter()).collect();
        assert_matches(&out, &exp);
    }

    #[test]
    fn trims_the_scrollbar_when_asked() {
        let s = setup(900);
        let (st, _) = run(&s, 200, false, StitchOptions::default());
        let out = st.finish();
        assert_eq!(out.width as usize, W - SB);
    }
}
