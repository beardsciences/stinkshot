//! Smart guides: while a shape is moved, resized or drawn, its edges and centre snap to the
//! edges and centres of the other shapes and of the selection (image), within a small
//! threshold, and thin accent guides show what lined up. Alt turns snapping off. Pure math in
//! image px; the overlay converts the threshold from screen px.

use stinkshot_core::{PtF, RectF};

/// Snap distance in screen points.
pub const SNAP_PT: f32 = 6.0;

/// A guide line to draw: vertical at x = `at` (or horizontal at y = `at`) from `from` to `to`
/// along the other axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Guide {
    pub vertical: bool,
    pub at: f32,
    pub from: f32,
    pub to: f32,
}

fn xs(r: RectF) -> [f32; 3] {
    [r.x, r.x + r.w / 2.0, r.x + r.w]
}

fn ys(r: RectF) -> [f32; 3] {
    [r.y, r.y + r.h / 2.0, r.y + r.h]
}

/// Best (shift, target value) bringing any of `mine` onto any of the targets' values.
fn best(mine: &[f32], targets: &[RectF], of: fn(RectF) -> [f32; 3], tol: f32) -> Option<f32> {
    let mut best: Option<f32> = None;
    for t in targets {
        for tv in of(*t) {
            for m in mine {
                let d = tv - m;
                if d.abs() <= tol && best.is_none_or(|b| d.abs() < b.abs()) {
                    best = Some(d);
                }
            }
        }
    }
    best
}

/// Guides for the values of `r` that coincide with target values (after snapping).
fn guides_for(r: RectF, targets: &[RectF]) -> Vec<Guide> {
    let mut out = Vec::new();
    let eps = 0.01;
    let mut seen: Vec<(bool, f32)> = Vec::new();
    for mx in xs(r) {
        if seen.contains(&(true, mx)) {
            continue;
        }
        seen.push((true, mx));
        let hits: Vec<&RectF> = targets
            .iter()
            .filter(|t| xs(**t).iter().any(|tx| (tx - mx).abs() < eps))
            .collect();
        if !hits.is_empty() {
            let from = hits.iter().map(|t| t.y).fold(r.y, f32::min);
            let to = hits.iter().map(|t| t.y + t.h).fold(r.y + r.h, f32::max);
            out.push(Guide {
                vertical: true,
                at: mx,
                from,
                to,
            });
        }
    }
    for my in ys(r) {
        if seen.contains(&(false, my)) {
            continue;
        }
        seen.push((false, my));
        let hits: Vec<&RectF> = targets
            .iter()
            .filter(|t| ys(**t).iter().any(|ty| (ty - my).abs() < eps))
            .collect();
        if !hits.is_empty() {
            let from = hits.iter().map(|t| t.x).fold(r.x, f32::min);
            let to = hits.iter().map(|t| t.x + t.w).fold(r.x + r.w, f32::max);
            out.push(Guide {
                vertical: false,
                at: my,
                from,
                to,
            });
        }
    }
    out
}

/// Snap a moved rect: returns the extra shift to apply and the guides to draw.
pub fn snap_rect(r: RectF, targets: &[RectF], tol: f32) -> ((f32, f32), Vec<Guide>) {
    let dx = best(&xs(r), targets, xs, tol).unwrap_or(0.0);
    let dy = best(&ys(r), targets, ys, tol).unwrap_or(0.0);
    let snapped = RectF::new(r.x + dx, r.y + dy, r.w, r.h);
    ((dx, dy), guides_for(snapped, targets))
}

/// Snap a dragged point (a resize handle, a line end, a drawing corner) on the axes it
/// moves along. Returns the snapped point and its guides.
pub fn snap_point(
    p: PtF,
    targets: &[RectF],
    tol: f32,
    snap_x: bool,
    snap_y: bool,
) -> (PtF, Vec<Guide>) {
    let dx = if snap_x {
        best(&[p.x], targets, xs, tol).unwrap_or(0.0)
    } else {
        0.0
    };
    let dy = if snap_y {
        best(&[p.y], targets, ys, tol).unwrap_or(0.0)
    } else {
        0.0
    };
    let q = PtF::new(p.x + dx, p.y + dy);
    let dot = RectF::new(q.x, q.y, 0.0, 0.0);
    let guides = guides_for(dot, targets)
        .into_iter()
        .filter(|g| (g.vertical && snap_x && dx != 0.0) || (!g.vertical && snap_y && dy != 0.0))
        .collect();
    (q, guides)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moved_rect_snaps_edges_and_centres() {
        let other = RectF::new(100.0, 100.0, 50.0, 50.0);
        // Left edge 3 px right of the other's right edge: snaps flush.
        let r = RectF::new(153.0, 300.0, 40.0, 20.0);
        let ((dx, dy), g) = snap_rect(r, &[other], 6.0);
        assert_eq!((dx, dy), (-3.0, 0.0));
        assert_eq!(g.len(), 1);
        assert!(g[0].vertical && g[0].at == 150.0);
        // The guide spans both rects.
        assert_eq!((g[0].from, g[0].to), (100.0, 320.0));
        // Centres line up horizontally.
        let r = RectF::new(400.0, 112.0, 20.0, 30.0);
        let ((_, dy), g) = snap_rect(r, &[other], 6.0);
        assert_eq!(dy, -2.0);
        assert!(g.iter().any(|g| !g.vertical && g.at == 125.0));
    }

    #[test]
    fn nothing_snaps_beyond_the_threshold() {
        let other = RectF::new(100.0, 100.0, 50.0, 50.0);
        let r = RectF::new(160.0, 300.0, 40.0, 20.0);
        let ((dx, dy), g) = snap_rect(r, &[other], 6.0);
        assert_eq!((dx, dy), (0.0, 0.0));
        assert!(g.is_empty());
    }

    #[test]
    fn the_closest_candidate_wins() {
        let a = RectF::new(0.0, 0.0, 100.0, 100.0);
        let b = RectF::new(0.0, 0.0, 103.0, 100.0);
        let r = RectF::new(104.0, 300.0, 10.0, 10.0);
        let ((dx, _), _) = snap_rect(r, &[a, b], 6.0);
        assert_eq!(dx, -1.0);
    }

    #[test]
    fn point_snaps_only_on_requested_axes() {
        let img = RectF::new(0.0, 0.0, 800.0, 600.0);
        let (q, g) = snap_point(PtF::new(397.0, 297.0), &[img], 6.0, true, false);
        assert_eq!(q, PtF::new(400.0, 297.0));
        assert_eq!(g.len(), 1);
        assert!(g[0].vertical);
        let (q, g) = snap_point(PtF::new(397.0, 297.0), &[img], 6.0, true, true);
        assert_eq!(q, PtF::new(400.0, 300.0));
        assert_eq!(g.len(), 2);
        let (q, g) = snap_point(PtF::new(397.0, 297.0), &[img], 6.0, false, false);
        assert_eq!(q, PtF::new(397.0, 297.0));
        assert!(g.is_empty());
    }
}
