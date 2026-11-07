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

/// Area of `rect` not covered by any occluder (exact, coordinate compression).
pub fn uncovered_area(rect: PxRect, occluders: &[PxRect]) -> i64 {
    let clipped: Vec<PxRect> = occluders
        .iter()
        .filter_map(|o| o.intersect(&rect))
        .collect();
    if clipped.is_empty() {
        return rect.area();
    }
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
    let mut free = 0i64;
    for xw in xs.windows(2) {
        for yw in ys.windows(2) {
            let (cx, cy) = (xw[0], yw[0]);
            let covered = clipped
                .iter()
                .any(|c| cx >= c.x && cx < c.right() && cy >= c.y && cy < c.bottom());
            if !covered {
                free += (xw[1] - xw[0]) as i64 * (yw[1] - yw[0]) as i64;
            }
        }
    }
    free
}

/// A level whose visible part around the cursor is less than this share of its full rect
/// is dropped (the next level outward is offered instead).
pub const MIN_VISIBLE_SHARE: f64 = 0.3;

/// Clip a snap chain (innermost element first, top-level window last) to what is visible
/// at `p`. Returns `(index into rects, clipped rect)` pairs in the same order, deduplicated
/// after clipping. Levels that are mostly hidden are dropped, except the outermost one,
/// which is always offered clipped so the window itself stays selectable.
pub fn clip_chain(rects: &[PxRect], p: PxPoint, occluders: &[PxRect]) -> Vec<(usize, PxRect)> {
    let mut out: Vec<(usize, PxRect)> = Vec::new();
    let last = rects.len().saturating_sub(1);
    for (i, r) in rects.iter().enumerate() {
        let Some(v) = visible_rect_around(*r, p, occluders) else {
            continue;
        };
        let share = v.area() as f64 / r.area().max(1) as f64;
        if share < MIN_VISIBLE_SHARE && i != last {
            continue;
        }
        if out.last().is_some_and(|(_, prev)| *prev == v) {
            continue;
        }
        out.push((i, v));
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
        // Window: clipped to the left of the popup.
        assert_eq!(c, vec![(2, PxRect::new(0, 0, 110, 600))]);

        // With the popup further right, the button and toolbar survive (clipped).
        let popup = [PxRect::new(180, 0, 300, 120)];
        let c = clip_chain(&[button, toolbar, window], p, &popup);
        assert_eq!(c[0], (0, PxRect::new(100, 100, 80, 40)));
        assert_eq!(c.last().unwrap().0, 2);
        for (_, r) in &c {
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
        assert_eq!(c, vec![(0, inner)]);
    }

    #[test]
    fn chain_is_empty_when_point_is_covered() {
        let window = PxRect::new(0, 0, 800, 600);
        let occ = [PxRect::new(0, 0, 100, 100)];
        assert!(clip_chain(&[window], pt(10, 10), &occ).is_empty());
    }
}
