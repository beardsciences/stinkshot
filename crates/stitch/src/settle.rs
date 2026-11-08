//! Pure helpers that decide when a scrolled page has settled: frame differences, loading
//! placeholder rows, background colors, and the scrollbar strip at the right edge.

use stinkshot_core::Rgba;

/// Max per-channel difference for "same color" in run and placeholder checks.
const RUN_TOL: i16 = 10;
/// A run this share of the row (1/N) or longer is a "block" (skeleton bar, flat panel).
const BLOCK_DIV: usize = 10;
/// Rows with more runs than this are content (text, photos, gradients across).
const PLACEHOLDER_MAX_RUNS: usize = 6;

/// Pixels that differ between two frames and their bounding box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Change {
    pub pixels: usize,
    /// (x0, y0, x1, y1), exclusive ends; None when nothing changed.
    pub bbox: Option<(u32, u32, u32, u32)>,
}

impl Change {
    /// Share of the frame covered by the bounding box of the change (0..1).
    pub fn bbox_area_share(&self, w: u32, h: u32) -> f32 {
        match self.bbox {
            Some((x0, y0, x1, y1)) => {
                ((x1 - x0) as f32 * (y1 - y0) as f32) / (w.max(1) as f32 * h.max(1) as f32)
            }
            None => 0.0,
        }
    }
}

/// Pixels differing by more than `tol` in any color channel. None when sizes differ.
pub fn diff(a: &Rgba, b: &Rgba, tol: u8) -> Option<Change> {
    if a.width != b.width || a.height != b.height || a.data.len() != b.data.len() {
        return None;
    }
    let (w, h) = (a.width as usize, a.height as usize);
    let stride = w * 4;
    let tol = i16::from(tol);
    let mut c = Change::default();
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for y in 0..h {
        let ra = &a.data[y * stride..(y + 1) * stride];
        let rb = &b.data[y * stride..(y + 1) * stride];
        if ra == rb {
            continue;
        }
        for (x, (pa, pb)) in ra.chunks_exact(4).zip(rb.chunks_exact(4)).enumerate() {
            let far = (0..3).any(|i| (i16::from(pa[i]) - i16::from(pb[i])).abs() > tol);
            if far {
                c.pixels += 1;
                x0 = x0.min(x);
                x1 = x1.max(x + 1);
                y0 = y0.min(y);
                y1 = y1.max(y + 1);
            }
        }
    }
    if c.pixels > 0 {
        c.bbox = Some((x0 as u32, y0 as u32, x1 as u32, y1 as u32));
    }
    Some(c)
}

#[inline]
fn near(a: [u8; 3], b: [u8; 3], tol: i16) -> bool {
    (0..3).all(|i| (i16::from(a[i]) - i16::from(b[i])).abs() <= tol)
}

#[inline]
fn rgb(px: &[u8]) -> [u8; 3] {
    [px[0], px[1], px[2]]
}

/// Color runs of an RGBA row (a run restarts when a pixel is more than `tol` from the
/// run's first pixel), counted up to `cap`.
pub fn row_runs(row: &[u8], tol: i16, cap: usize) -> usize {
    let mut px = row.chunks_exact(4);
    let Some(first) = px.next() else { return 0 };
    let mut cur = rgb(first);
    let mut runs = 1;
    for p in px {
        let c = rgb(p);
        if !near(c, cur, tol) {
            runs += 1;
            if runs >= cap {
                return runs;
            }
            cur = c;
        }
    }
    runs
}

/// Most common color of a row (5 bits per channel).
pub fn dominant_color(row: &[u8]) -> [u8; 3] {
    let mut counts: Vec<(u16, u32)> = Vec::new();
    for p in row.chunks_exact(4) {
        let k = (u16::from(p[0] >> 3) << 10) | (u16::from(p[1] >> 3) << 5) | u16::from(p[2] >> 3);
        if let Some((_, n)) = counts.iter_mut().find(|(c, _)| *c == k) {
            *n += 1;
        } else if counts.len() < 64 {
            counts.push((k, 1));
        }
    }
    let k = counts.iter().max_by_key(|(_, n)| *n).map_or(0, |(k, _)| *k);
    [
        ((k >> 10) as u8 & 31) << 3,
        ((k >> 5) as u8 & 31) << 3,
        (k as u8 & 31) << 3,
    ]
}

/// Long runs of a row: (color, length), only when the row has at most `max_runs` runs.
fn long_runs(row: &[u8], max_runs: usize) -> Option<Vec<([u8; 3], usize)>> {
    let n = row.len() / 4;
    let mut runs: Vec<([u8; 3], usize)> = Vec::new();
    for p in row.chunks_exact(4) {
        let c = rgb(p);
        match runs.last_mut() {
            Some((rc, len)) if near(c, *rc, RUN_TOL) => *len += 1,
            _ => {
                if runs.len() >= max_runs {
                    return None;
                }
                runs.push((c, 1));
            }
        }
    }
    let min = (n / BLOCK_DIV).max(1);
    Some(runs.into_iter().filter(|(_, l)| *l >= min).collect())
}

/// A row that looks like a loading placeholder: few color runs, with a long block in a
/// color that is not one of the page's background colors (skeleton bars, gray panels,
/// stripes). Text, photos and horizontal gradients have many runs and never qualify.
pub fn placeholder_row(row: &[u8], bg: &[[u8; 3]]) -> bool {
    let Some(runs) = long_runs(row, PLACEHOLDER_MAX_RUNS) else {
        return false;
    };
    runs.iter()
        .any(|(c, _)| !bg.iter().any(|b| near(*c, *b, RUN_TOL + 2)))
}

/// Background colors of a frame: colors of fully flat rows (blank lines between content)
/// on at least 1% of the rows, up to 8, most common first; when there are none, the most
/// common long-run color. A skeleton bar never fills a whole row, so a page still loading
/// does not teach its placeholder color as background. `rows` are the compared middle
/// widths of the frame's rows (so a scrollbar beside a blank line does not count).
pub fn background_colors<'a>(rows: impl ExactSizeIterator<Item = &'a [u8]>) -> Vec<[u8; 3]> {
    let h = rows.len();
    let mut flat: Vec<([u8; 3], usize)> = Vec::new();
    let mut runs_hist: Vec<([u8; 3], usize)> = Vec::new();
    let add = |hist: &mut Vec<([u8; 3], usize)>, c: [u8; 3], n: usize| match hist
        .iter_mut()
        .find(|(hc, _)| near(*hc, c, RUN_TOL))
    {
        Some((_, k)) => *k += n,
        None => hist.push((c, n)),
    };
    for row in rows {
        let w = row.len() / 4;
        let Some(runs) = long_runs(row, 32) else {
            continue;
        };
        if let [(c, l)] = runs.as_slice()
            && *l * 100 >= w * 98
        {
            add(&mut flat, *c, 1);
        }
        for (c, l) in runs {
            add(&mut runs_hist, c, l);
        }
    }
    flat.sort_by_key(|b| std::cmp::Reverse(b.1));
    let colors: Vec<[u8; 3]> = flat
        .into_iter()
        .filter(|(_, n)| *n * 100 >= h && *n >= 2)
        .take(8)
        .map(|(c, _)| c)
        .collect();
    if !colors.is_empty() {
        return colors;
    }
    runs_hist
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map(|(c, _)| vec![c])
        .unwrap_or_default()
}

/// Block colors of placeholder-like rows in `rows` (to accept as page colors after a
/// step waited the full time for them to change).
pub fn placeholder_colors<'a>(
    rows: impl Iterator<Item = &'a [u8]>,
    bg: &[[u8; 3]],
) -> Vec<[u8; 3]> {
    let mut out: Vec<[u8; 3]> = Vec::new();
    for row in rows {
        let Some(runs) = long_runs(row, PLACEHOLDER_MAX_RUNS) else {
            continue;
        };
        for (c, _) in runs {
            let known = bg.iter().chain(&out).any(|b| near(c, *b, RUN_TOL + 2));
            if !known && out.len() < 8 {
                out.push(c);
            }
        }
    }
    out
}

/// Width of a scrollbar at the right edge, found by comparing the first and the last frame
/// of a scroll capture: its columns are a plain track (few color runs) whose only change
/// is the thumb (one or two runs of rows), side by side. Up to two unchanged border
/// columns at the edge are included. 0 when there is none. `skip` = (header, footer) rows
/// left out of the comparison.
pub fn scrollbar_width(
    first: &[u8],
    last: &[u8],
    w: usize,
    h: usize,
    skip: (usize, usize),
) -> usize {
    if first.len() != w * h * 4 || last.len() != first.len() || w < 64 {
        return 0;
    }
    let (y0, y1) = (skip.0, h.saturating_sub(skip.1));
    if y1 <= y0 + 32 {
        return 0;
    }
    let band = y1 - y0;
    let px = |d: &[u8], x: usize, y: usize| rgb(&d[(y * w + x) * 4..]);
    // Changed-row runs of column x, or None when it is not a track-like column.
    let column = |x: usize| -> Option<Vec<(usize, usize)>> {
        let mut runs: Vec<(usize, usize)> = Vec::new();
        let mut color_runs = [1usize; 2];
        let mut last_c = [px(first, x, y0), px(last, x, y0)];
        for y in y0..y1 {
            let (a, b) = (px(first, x, y), px(last, x, y));
            for (i, c) in [a, b].into_iter().enumerate() {
                if !near(c, last_c[i], RUN_TOL) {
                    color_runs[i] += 1;
                    last_c[i] = c;
                }
            }
            if !near(a, b, 6) {
                match runs.last_mut() {
                    Some((_, e)) if y <= *e + 2 => *e = y,
                    _ => runs.push((y, y)),
                }
            }
        }
        let changed: usize = runs.iter().map(|(s, e)| e - s + 1).sum();
        // Track + thumb + arrow buttons: a handful of color runs top to bottom.
        let plain = color_runs.iter().all(|&n| n <= 7);
        (plain && runs.len() <= 2 && changed * 10 <= band * 6).then_some(runs)
    };
    // Track-like columns from the right edge (up to 32), with their changed-row runs.
    let mut cols: Vec<Vec<(usize, usize)>> = Vec::new();
    while cols.len() < 32 {
        let Some(runs) = column(w - 1 - cols.len()) else {
            break;
        };
        cols.push(runs);
    }
    // The thumb: adjacent columns whose change is the same runs of rows (Windows 11 draws
    // an idle thumb only 2 px wide, a classic thumb fills the track).
    let Some(t0) = cols.iter().position(|r| !r.is_empty()) else {
        return 0;
    };
    let similar = |a: &[(usize, usize)], b: &[(usize, usize)]| {
        a.len() == b.len()
            && a.iter()
                .zip(b)
                .all(|(p, q)| p.0.abs_diff(q.0) <= 3 && p.1.abs_diff(q.1) <= 3)
    };
    let mut t1 = t0;
    while t1 + 1 < cols.len() && similar(&cols[t1 + 1], &cols[t0]) {
        t1 += 1;
    }
    // Track color: the most common color of the thumb column in the last frame.
    let tx = w - 1 - t0;
    let mut hist: Vec<([u8; 3], usize)> = Vec::new();
    for y in y0..y1 {
        let c = px(last, tx, y);
        match hist.iter_mut().find(|(hc, _)| near(*hc, c, 4)) {
            Some((_, n)) => *n += 1,
            None => hist.push((c, 1)),
        }
    }
    let track = hist
        .iter()
        .max_by_key(|(_, n)| *n)
        .map_or([0; 3], |(c, _)| *c);
    let is_track = |i: usize| {
        cols[i].is_empty() && {
            let x = w - 1 - i;
            let on = (y0..y1).filter(|&y| near(px(last, x, y), track, 4)).count();
            on * 10 >= band * 9
        }
    };
    // Plain track columns beside the thumb belong to the scrollbar; anything between the
    // track and the edge (a border) too.
    let mut left = t1;
    while left + 1 < cols.len() && is_track(left + 1) {
        left += 1;
    }
    let width = left + 1;
    if (2..=24).contains(&width) && (t1 - t0) < 24 {
        width
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(colors: &[([u8; 3], usize)]) -> Vec<u8> {
        let mut r = Vec::new();
        for (c, n) in colors {
            for _ in 0..*n {
                r.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        r
    }

    #[test]
    fn placeholder_rows_are_flat_blocks_in_a_new_color() {
        let bg = [[250, 250, 248]];
        let white = [250, 250, 248];
        let gray = [228, 228, 228];
        // skeleton bar on the background
        assert!(placeholder_row(
            &row(&[(white, 40), (gray, 300), (white, 60)]),
            &bg
        ));
        // a fully flat gray row
        assert!(placeholder_row(&row(&[(gray, 400)]), &bg));
        // a blank background row is not a placeholder
        assert!(!placeholder_row(&row(&[(white, 400)]), &bg));
        // text: many runs
        let mut text = Vec::new();
        for i in 0..60 {
            text.push((if i % 2 == 0 { white } else { [40, 40, 40] }, 6));
        }
        text.push((white, 40));
        assert!(!placeholder_row(&row(&text), &bg));
        // short gray bits (a spinner arc) are not blocks
        assert!(!placeholder_row(
            &row(&[(white, 190), (gray, 20), (white, 190)]),
            &bg
        ));
    }

    #[test]
    fn background_is_the_dominant_long_run_color() {
        let w = 100;
        let mut data = Vec::new();
        for y in 0..50 {
            if y < 40 {
                data.extend(row(&[([255, 255, 255], 100)]));
            } else {
                data.extend(row(&[([255, 255, 255], 20), ([0, 0, 255], 80)]));
            }
        }
        let bg = background_colors(data.chunks_exact(w * 4));
        // flat white rows; the blue blocks never fill a row
        assert_eq!(bg, vec![[255, 255, 255]]);
    }

    #[test]
    fn diff_counts_pixels_and_box() {
        let a = Rgba::filled(10, 10, [1, 2, 3, 255]);
        let mut b = a.clone();
        assert_eq!(diff(&a, &b, 0).unwrap().pixels, 0);
        for (x, y) in [(2, 3), (5, 7)] {
            b.data[(y * 10 + x) * 4] = 200;
        }
        let c = diff(&a, &b, 6).unwrap();
        assert_eq!(c.pixels, 2);
        assert_eq!(c.bbox, Some((2, 3, 6, 8)));
        assert!(diff(&a, &Rgba::new(3, 3), 0).is_none());
    }

    /// Frames of a page with a 17 px scrollbar (arrows, track, thumb) plus a 1 px border.
    fn frame(thumb_at: usize, content_shift: usize) -> Vec<u8> {
        let (w, h) = (200usize, 300usize);
        let mut d = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let c: [u8; 3] = if x == w - 1 {
                    [100, 100, 100]
                } else if x >= w - 18 {
                    if y < 17 || y >= h - 17 {
                        [200, 200, 200]
                    } else if y >= thumb_at && y < thumb_at + 60 {
                        [120, 120, 120]
                    } else {
                        [240, 240, 240]
                    }
                } else {
                    let v = ((x * 7 + (y + content_shift) * 13) % 97) as u8;
                    [v, v.wrapping_mul(3), 255 - v]
                };
                d.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        d
    }

    #[test]
    fn finds_the_scrollbar_strip() {
        let a = frame(17, 0);
        let b = frame(150, 400);
        assert_eq!(scrollbar_width(&a, &b, 200, 300, (0, 0)), 18);
        // no thumb movement: nothing to tell it apart from content, keep it
        assert_eq!(scrollbar_width(&a, &frame(17, 400), 200, 300, (0, 0)), 0);
    }

    /// Windows 11 idle scrollbar: a 2 px thumb line in the middle of a 17 px track, with
    /// a 1 px lighter column at the track's left edge (as captured on the dev box).
    #[test]
    fn finds_a_thin_windows_11_thumb() {
        let (w, h) = (300usize, 400usize);
        let mk = |thumb: usize, shift: usize| {
            let mut d = Vec::with_capacity(w * h * 4);
            for y in 0..h {
                for x in 0..w {
                    let c: [u8; 3] = if x >= w - 17 {
                        let tx = x - (w - 17);
                        if tx == 0 {
                            [255, 255, 255]
                        } else if (8..10).contains(&tx) && (thumb..thumb + 50).contains(&y) {
                            [133, 133, 133]
                        } else {
                            [240, 240, 240]
                        }
                    } else {
                        let v = ((x * 5 + (y + shift) * 11) % 89) as u8;
                        [v, 255 - v, v / 2]
                    };
                    d.extend_from_slice(&[c[0], c[1], c[2], 255]);
                }
            }
            d
        };
        // 16 columns: track and thumb; the 1 px white edge stays (it reads as page)
        assert_eq!(scrollbar_width(&mk(17, 0), &mk(300, 500), w, h, (0, 0)), 16);
    }
}
