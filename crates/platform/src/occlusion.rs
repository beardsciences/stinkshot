//! Pure occlusion and hit-test helpers over a Z-ordered window stack (topmost first).
//!
//! The overlay snaps to what is visible under the cursor in the frozen screen. That means:
//! the target is the topmost window at the point (untitled popups, menus and tool windows
//! included, since they cover what is behind them), and a highlighted element rect is
//! clipped to the part of that window no other window covers.

use stinkshot_core::{PxPoint, PxRect};

use crate::Hwnd;

/// One visible top-level window in a Z-ordered snapshot (see `windows_enum::layers`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layer {
    pub hwnd: Hwnd,
    /// Visible frame, physical virtual-desktop px.
    pub rect: PxRect,
    /// Belongs to this process: an occluder and a window-level target, never a UIA target.
    pub own: bool,
}

/// Index of the topmost layer containing `p`.
pub fn top_layer_at(layers: &[Layer], p: PxPoint) -> Option<usize> {
    layers.iter().position(|l| l.rect.contains(p))
}

/// Rects of the layers above `index` (the windows that can cover it).
pub fn occluders_above(layers: &[Layer], index: usize) -> Vec<PxRect> {
    layers[..index.min(layers.len())]
        .iter()
        .map(|l| l.rect)
        .collect()
}

/// Largest-overlap-first greedy cut: the biggest rect inside `rect` that contains `p` and
/// does not overlap any occluder. None when `p` is outside `rect` or covered.
pub fn visible_rect_around(rect: PxRect, p: PxPoint, occluders: &[PxRect]) -> Option<PxRect> {
    if !rect.contains(p) || occluders.iter().any(|o| o.contains(p)) {
        return None;
    }
    let mut hits: Vec<(i64, PxRect)> = occluders
        .iter()
        .filter_map(|o| o.intersect(&rect).map(|i| (i.area(), *o)))
        .collect();
    hits.sort_by_key(|h| std::cmp::Reverse(h.0));
    let mut r = rect;
    for (_, o) in hits {
        if r.intersect(&o).is_none() {
            continue;
        }
        // Every side of `o` that keeps `p` gives a candidate; keep the biggest.
        let mut best: Option<PxRect> = None;
        let mut consider = |c: PxRect| {
            if c.contains(p) && best.is_none_or(|b| c.area() > b.area()) {
                best = Some(c);
            }
        };
        if p.x < o.x {
            consider(PxRect::new(r.x, r.y, o.x - r.x, r.h));
        }
        if p.x >= o.right() {
            consider(PxRect::new(o.right(), r.y, r.right() - o.right(), r.h));
        }
        if p.y < o.y {
            consider(PxRect::new(r.x, r.y, r.w, o.y - r.y));
        }
        if p.y >= o.bottom() {
            consider(PxRect::new(r.x, o.bottom(), r.w, r.bottom() - o.bottom()));
        }
        r = best?;
    }
    Some(r)
}

/// `rect` cut into a grid along every occluder edge inside it: column edges, row edges, and
/// whether each cell (`free[row][col]`) is uncovered. Cells are wholly in or out.
struct Grid {
    xs: Vec<i32>,
    ys: Vec<i32>,
    free: Vec<Vec<bool>>,
}

fn grid(rect: PxRect, occluders: &[PxRect]) -> Grid {
    let clipped: Vec<PxRect> = occluders
        .iter()
        .filter_map(|o| o.intersect(&rect))
        .collect();
    let mut xs: Vec<i32> = vec![rect.x, rect.right()];
    let mut ys: Vec<i32> = vec![rect.y, rect.bottom()];
    for c in &clipped {
        xs.extend([c.x, c.right()]);
        ys.extend([c.y, c.bottom()]);
    }
    xs.sort_unstable();
    xs.dedup();
    ys.sort_unstable();
    ys.dedup();
    let cols = xs.len().saturating_sub(1);
    let rows = ys.len().saturating_sub(1);
    let mut free = vec![vec![true; cols]; rows];
    for c in &clipped {
        // Edges are in the lists, so the searches are exact.
        let (c0, c1) = (
            xs.partition_point(|&x| x < c.x),
            xs.partition_point(|&x| x < c.right()),
        );
        let (r0, r1) = (
            ys.partition_point(|&y| y < c.y),
            ys.partition_point(|&y| y < c.bottom()),
        );
        for row in &mut free[r0..r1] {
            for cell in &mut row[c0..c1] {
                *cell = false;
            }
        }
    }
    Grid { xs, ys, free }
}

/// Area of `rect` not covered by any occluder (exact, coordinate compression).
pub fn uncovered_area(rect: PxRect, occluders: &[PxRect]) -> i64 {
    let g = grid(rect, occluders);
    let mut free = 0i64;
    for (r, row) in g.free.iter().enumerate() {
        let h = (g.ys[r + 1] - g.ys[r]) as i64;
        for (c, &f) in row.iter().enumerate() {
            if f {
                free += (g.xs[c + 1] - g.xs[c]) as i64 * h;
            }
        }
    }
    free
}

/// The largest rectangle inside `rect` that no occluder overlaps, independent of where the
/// cursor is (exact: histogram scan over the compressed grid). None when fully covered.
pub fn largest_uncovered_rect(rect: PxRect, occluders: &[PxRect]) -> Option<PxRect> {
    if rect.is_empty() {
        return None;
    }
    let g = grid(rect, occluders);
    let cols = g.xs.len() - 1;
    let mut heights = vec![0i32; cols];
    let mut best: Option<PxRect> = None;
    let mut consider = |c: PxRect| {
        if c.w > 0 && c.h > 0 && best.is_none_or(|b| c.area() > b.area()) {
            best = Some(c);
        }
    };
    for (r, row) in g.free.iter().enumerate() {
        let bottom = g.ys[r + 1];
        let row_h = bottom - g.ys[r];
        for (h, &f) in heights.iter_mut().zip(row) {
            *h = if f { *h + row_h } else { 0 };
        }
        // Largest rectangle under a histogram with uneven bar widths.
        let mut stack: Vec<(i32, i32)> = Vec::new(); // (left x, height)
        // A zero-height sentinel at the right edge flushes the stack.
        let bars = heights.iter().copied().chain(std::iter::once(0));
        for (&x, h) in g.xs.iter().zip(bars) {
            let mut left = x;
            while let Some(&(sx, sh)) = stack.last() {
                if sh < h {
                    break;
                }
                stack.pop();
                consider(PxRect::new(sx, bottom - sh, x - sx, sh));
                left = sx;
            }
            if h > 0 {
                stack.push((left, h));
            }
        }
    }
    best
}

/// A level whose uncovered area is less than this share of its full rect is dropped (the
/// next level outward is offered instead). Also the share of a window's uncovered area its
/// largest uncovered rectangle must reach to be offered as the clipped window level.
pub const MIN_VISIBLE_SHARE: f64 = 0.3;

/// Window-level rects to offer at `p`, innermost first: the largest uncovered rectangle of
/// `frame` when it contains `p` and holds a fair share of what is visible, then the whole
/// frame (what the user sees as "that window"). The clipped rect depends only on the window
/// and its occluders, so moving the cursor inside it never reshapes it. Empty when `p` is
/// outside `frame` or covered.
pub fn window_levels(frame: PxRect, p: PxPoint, occluders: &[PxRect]) -> Vec<PxRect> {
    if !frame.contains(p) || occluders.iter().any(|o| o.contains(p)) {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(2);
    if let Some(m) = largest_uncovered_rect(frame, occluders)
        && m != frame
        && m.contains(p)
        && m.area() as f64 >= MIN_VISIBLE_SHARE * uncovered_area(frame, occluders) as f64
    {
        out.push(m);
    }
    out.push(frame);
    out
}

/// Clip a snap chain (innermost element first, top-level window last) to what is visible
/// at `p`. Returns `(index into rects, rect)` pairs in the same order, deduplicated after
/// clipping. Inner levels are cut to the visible band around `p` and dropped when mostly
/// hidden (judged by their real uncovered area, so levels do not come and go as the cursor
/// moves). The outermost level becomes `window_levels`: a stable clipped rect when one fits,
/// then the whole window, so the window itself is always selectable.
pub fn clip_chain(rects: &[PxRect], p: PxPoint, occluders: &[PxRect]) -> Vec<(usize, PxRect)> {
    let mut out: Vec<(usize, PxRect)> = Vec::new();
    let mut push = |i: usize, v: PxRect| {
        if !out.last().is_some_and(|(_, prev)| *prev == v) {
            out.push((i, v));
        }
    };
    let Some(last) = rects.len().checked_sub(1) else {
        return Vec::new();
    };
    for (i, r) in rects[..last].iter().enumerate() {
        let Some(v) = visible_rect_around(*r, p, occluders) else {
            continue;
        };
        let share = uncovered_area(*r, occluders) as f64 / r.area().max(1) as f64;
        if share < MIN_VISIBLE_SHARE {
            continue;
        }
        push(i, v);
    }
    for v in window_levels(rects[last], p, occluders) {
        push(last, v);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: i32, y: i32) -> PxPoint {
        PxPoint { x, y }
    }

    fn layer(hwnd: Hwnd, r: PxRect) -> Layer {
        Layer {
            hwnd,
            rect: r,
            own: false,
        }
    }

    #[test]
    fn top_layer_is_first_hit_in_z_order() {
        // An untitled popup (2) above a big app window (3) above the desktop-sized one (4).
        let ls = [
            layer(1, PxRect::new(900, 900, 50, 50)),
            layer(2, PxRect::new(100, 100, 200, 300)),
            layer(3, PxRect::new(0, 0, 800, 600)),
            layer(4, PxRect::new(0, 0, 1920, 1080)),
        ];
        assert_eq!(top_layer_at(&ls, pt(150, 150)), Some(1));
        assert_eq!(top_layer_at(&ls, pt(500, 500)), Some(2));
        assert_eq!(top_layer_at(&ls, pt(1000, 700)), Some(3));
        assert_eq!(top_layer_at(&ls, pt(5000, 5000)), None);
        assert_eq!(
            occluders_above(&ls, 2),
            vec![
                PxRect::new(900, 900, 50, 50),
                PxRect::new(100, 100, 200, 300)
            ]
        );
        assert!(occluders_above(&ls, 0).is_empty());
    }

    #[test]
    fn unoccluded_rect_is_unchanged() {
        let r = PxRect::new(0, 0, 100, 100);
        let occ = [PxRect::new(200, 200, 50, 50)];
        assert_eq!(visible_rect_around(r, pt(10, 10), &occ), Some(r));
        assert_eq!(visible_rect_around(r, pt(10, 10), &[]), Some(r));
    }

    #[test]
    fn covered_point_or_outside_gives_none() {
        let r = PxRect::new(0, 0, 100, 100);
        let occ = [PxRect::new(40, 40, 20, 20)];
        assert_eq!(visible_rect_around(r, pt(50, 50), &occ), None);
        assert_eq!(visible_rect_around(r, pt(150, 50), &occ), None);
    }

    #[test]
    fn overlap_on_one_side_is_cut_away() {
        // A window covering the right half: keep the left part.
        let r = PxRect::new(0, 0, 200, 100);
        let occ = [PxRect::new(120, -50, 500, 500)];
        assert_eq!(
            visible_rect_around(r, pt(10, 10), &occ),
            Some(PxRect::new(0, 0, 120, 100))
        );
        // Covering the bottom: keep the top.
        let occ = [PxRect::new(-10, 70, 500, 500)];
        assert_eq!(
            visible_rect_around(r, pt(10, 10), &occ),
            Some(PxRect::new(0, 0, 200, 70))
        );
    }

    #[test]
    fn hole_keeps_the_largest_side_containing_the_point() {
        // Occluder in the middle, point left of it: only the left band keeps the point.
        let r = PxRect::new(0, 0, 100, 100);
        let occ = [PxRect::new(40, 40, 20, 20)];
        let v = visible_rect_around(r, pt(5, 50), &occ).unwrap();
        assert_eq!(v, PxRect::new(0, 0, 40, 100));
        assert!(v.contains(pt(5, 50)));
        assert!(v.intersect(&occ[0]).is_none());
        // Point above the hole: the top band.
        let v = visible_rect_around(r, pt(50, 5), &occ).unwrap();
        assert_eq!(v, PxRect::new(0, 0, 100, 40));
    }

    #[test]
    fn multiple_occluders_never_overlap_result() {
        let r = PxRect::new(0, 0, 400, 300);
        let occ = [
            PxRect::new(300, 0, 200, 200),
            PxRect::new(0, 250, 100, 100),
            PxRect::new(150, 120, 30, 30),
        ];
        let p = pt(50, 50);
        let v = visible_rect_around(r, p, &occ).unwrap();
        assert!(v.contains(p));
        for o in &occ {
            assert!(v.intersect(o).is_none(), "{v:?} overlaps {o:?}");
        }
        assert!(r.intersect(&v) == Some(v));
    }

    #[test]
    fn uncovered_area_handles_overlapping_occluders() {
        let r = PxRect::new(0, 0, 100, 100);
        assert_eq!(uncovered_area(r, &[]), 10_000);
        assert_eq!(uncovered_area(r, &[PxRect::new(0, 0, 50, 100)]), 5_000);
        // Two overlapping occluders must not be double-counted.
        assert_eq!(
            uncovered_area(
                r,
                &[PxRect::new(0, 0, 60, 100), PxRect::new(40, 0, 20, 100)]
            ),
            4_000
        );
        assert_eq!(uncovered_area(r, &[PxRect::new(-10, -10, 500, 500)]), 0);
    }

    #[test]
    fn chain_drops_mostly_hidden_levels_but_keeps_window() {
        // Button mostly under a popup, its toolbar half visible, window always offered.
        let button = PxRect::new(100, 100, 100, 40);
        let toolbar = PxRect::new(0, 100, 400, 40);
        let window = PxRect::new(0, 0, 800, 600);
        let popup = [PxRect::new(110, 0, 300, 700)];
        let p = pt(105, 120);
        let c = clip_chain(&[button, toolbar, window], p, &popup);
        // Button: 10/100 wide visible -> dropped. Toolbar: 110/400 visible < 30% -> dropped.
        // Window: the cursor is on the thin strip left of the popup, not in the window's
        // biggest uncovered part (right of the popup), so the whole window is offered.
        assert_eq!(c, vec![(2, window)]);

        // With the popup further right, the button and toolbar survive (clipped), and the
        // window comes clipped (everything below the popup) and then whole.
        let popup = [PxRect::new(180, 0, 300, 120)];
        let c = clip_chain(&[button, toolbar, window], p, &popup);
        assert_eq!(c[0], (0, PxRect::new(100, 100, 80, 40)));
        assert_eq!(
            &c[c.len() - 2..],
            &[(2, PxRect::new(0, 120, 800, 480)), (2, window)]
        );
        for (_, r) in &c[..c.len() - 1] {
            assert!(r.contains(p));
            assert!(r.intersect(&popup[0]).is_none());
        }
    }

    #[test]
    fn chain_dedups_levels_that_clip_to_the_same_rect() {
        let inner = PxRect::new(0, 0, 100, 100);
        let outer = PxRect::new(0, 0, 200, 100);
        let occ = [PxRect::new(100, 0, 200, 200)];
        let c = clip_chain(&[inner, outer], pt(10, 10), &occ);
        // The clipped window equals `inner`; the whole window follows.
        assert_eq!(c, vec![(0, inner), (1, outer)]);
    }

    #[test]
    fn largest_uncovered_rect_is_exact() {
        let r = PxRect::new(0, 0, 100, 100);
        assert_eq!(largest_uncovered_rect(r, &[]), Some(r));
        assert_eq!(
            largest_uncovered_rect(r, &[PxRect::new(-5, -5, 200, 200)]),
            None
        );
        // Hole in the middle: one of the 100x40 bands.
        let m = largest_uncovered_rect(r, &[PxRect::new(40, 40, 20, 20)]).unwrap();
        assert_eq!(m.area(), 4_000);
        // L-shaped free area: the tall left column beats the short bottom row.
        let m = largest_uncovered_rect(r, &[PxRect::new(30, 0, 70, 80)]).unwrap();
        assert_eq!(m, PxRect::new(0, 0, 30, 100));
        // A free block that spans several grid columns and rows.
        let occ = [
            PxRect::new(0, 0, 10, 10),
            PxRect::new(90, 90, 10, 10),
            PxRect::new(50, 0, 5, 5),
        ];
        let m = largest_uncovered_rect(r, &occ).unwrap();
        assert_eq!(m.area(), 8_100);
        for o in &occ {
            assert!(m.intersect(o).is_none());
        }
    }

    #[test]
    fn window_level_is_stable_as_the_cursor_moves() {
        // Live stack: Firefox under Discord (right half) and History (inside the left half).
        let firefox = PxRect::new(0, 0, 3840, 2100);
        let occ = [
            PxRect::new(1920, 0, 1920, 2100),
            PxRect::new(208, 200, 1602, 1064),
        ];
        // On the tab strip, left or right of History's edge: the same, whole window.
        assert_eq!(window_levels(firefox, pt(100, 150), &occ), vec![firefox]);
        assert_eq!(window_levels(firefox, pt(300, 150), &occ), vec![firefox]);
        // Anywhere in the big free block below History: that block, then the window.
        let block = PxRect::new(0, 1264, 1920, 836);
        for p in [pt(10, 1300), pt(1900, 2000), pt(960, 1700)] {
            assert_eq!(window_levels(firefox, p, &occ), vec![block, firefox]);
        }
        // Uncovered window: just the window.
        assert_eq!(window_levels(firefox, pt(10, 10), &[]), vec![firefox]);
    }

    #[test]
    fn chain_is_empty_when_point_is_covered() {
        let window = PxRect::new(0, 0, 800, 600);
        let occ = [PxRect::new(0, 0, 100, 100)];
        assert!(clip_chain(&[window], pt(10, 10), &occ).is_empty());
    }
}
