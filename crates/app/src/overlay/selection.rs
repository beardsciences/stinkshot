//! Pure selection-rectangle math: handle hit-testing, resize with aspect constraint,
//! nudging, clamping, and the image <-> screen mapping. Everything is in physical
//! pixels (overlay-local: origin at the top-left of the frozen image) and has no egui or
//! OS dependencies beyond plain value types, so it is unit tested.

use stinkshot_core::PxRect;

/// The eight resize handles of a rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Handle8 {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

impl Handle8 {
    pub const ALL: [Handle8; 8] = [
        Handle8::TopLeft,
        Handle8::Top,
        Handle8::TopRight,
        Handle8::Right,
        Handle8::BottomRight,
        Handle8::Bottom,
        Handle8::BottomLeft,
        Handle8::Left,
    ];

    /// -1 = moves the left edge, +1 = moves the right edge, 0 = no horizontal movement.
    pub fn h_dir(self) -> i32 {
        match self {
            Handle8::TopLeft | Handle8::Left | Handle8::BottomLeft => -1,
            Handle8::TopRight | Handle8::Right | Handle8::BottomRight => 1,
            _ => 0,
        }
    }
    /// -1 = moves the top edge, +1 = moves the bottom edge, 0 = no vertical movement.
    pub fn v_dir(self) -> i32 {
        match self {
            Handle8::TopLeft | Handle8::Top | Handle8::TopRight => -1,
            Handle8::BottomLeft | Handle8::Bottom | Handle8::BottomRight => 1,
            _ => 0,
        }
    }
}

/// Float rectangle in px used for shape-space handle math. (x, y, w, h)
pub type FRect = (f32, f32, f32, f32);

/// Handle anchor position on a float rect.
pub fn handle_pos(r: FRect, h: Handle8) -> (f32, f32) {
    let (x, y, w, hh) = r;
    let cx = x + w / 2.0;
    let cy = y + hh / 2.0;
    match h {
        Handle8::TopLeft => (x, y),
        Handle8::Top => (cx, y),
        Handle8::TopRight => (x + w, y),
        Handle8::Right => (x + w, cy),
        Handle8::BottomRight => (x + w, y + hh),
        Handle8::Bottom => (cx, y + hh),
        Handle8::BottomLeft => (x, y + hh),
        Handle8::Left => (x, cy),
    }
}

/// Which handle (if any) is within `radius` of `p`; the nearest wins.
pub fn hit_handle(r: FRect, p: (f32, f32), radius: f32) -> Option<Handle8> {
    let mut best: Option<(Handle8, f32)> = None;
    for h in Handle8::ALL {
        let (hx, hy) = handle_pos(r, h);
        let d = ((hx - p.0).powi(2) + (hy - p.1).powi(2)).sqrt();
        if d <= radius && best.is_none_or(|(_, bd)| d < bd - 0.001) {
            best = Some((h, d));
        }
    }
    best.map(|(h, _)| h)
}

pub fn px_to_f(r: PxRect) -> FRect {
    (r.x as f32, r.y as f32, r.w as f32, r.h as f32)
}

/// Aspect ratio as (w, h).
pub type Ratio = (u32, u32);

/// Resize `start` by dragging `handle` to cursor `p` (all px). The opposite edge/corner
/// stays fixed; dragging past it flips the rectangle. The result is clamped into `bounds`
/// and optionally constrained to `aspect`. Minimum size is 1x1.
pub fn resize_px_rect(
    start: PxRect,
    handle: Handle8,
    p: (i32, i32),
    bounds: PxRect,
    aspect: Option<Ratio>,
) -> PxRect {
    let hd = handle.h_dir();
    let vd = handle.v_dir();
    let px = p.0.clamp(bounds.x, bounds.right());
    let py = p.1.clamp(bounds.y, bounds.bottom());

    // Fixed anchor edges.
    let fx = if hd < 0 { start.right() } else { start.x };
    let fy = if vd < 0 { start.bottom() } else { start.y };

    let (mut x0, mut x1) = if hd == 0 {
        (start.x, start.right())
    } else {
        (fx.min(px), fx.max(px))
    };
    let (mut y0, mut y1) = if vd == 0 {
        (start.y, start.bottom())
    } else {
        (fy.min(py), fy.max(py))
    };

    if let Some((rw, rh)) = aspect.filter(|(a, b)| *a > 0 && *b > 0) {
        let ratio = rw as f32 / rh as f32;
        if hd != 0 && vd != 0 {
            // Corner: follow the larger implied size, keep the anchor corner fixed.
            let dx = (px - fx).abs() as f32;
            let dy = (py - fy).abs() as f32;
            let sx = if px >= fx { 1 } else { -1 };
            let sy = if py >= fy { 1 } else { -1 };
            let max_w = if sx > 0 {
                (bounds.right() - fx) as f32
            } else {
                (fx - bounds.x) as f32
            };
            let max_h = if sy > 0 {
                (bounds.bottom() - fy) as f32
            } else {
                (fy - bounds.y) as f32
            };
            let w = dx.max(dy * ratio).min(max_w).min(max_h * ratio);
            let h = w / ratio;
            let w = w.round() as i32;
            let h = h.round() as i32;
            x0 = fx.min(fx + sx * w);
            x1 = fx.max(fx + sx * w);
            y0 = fy.min(fy + sy * h);
            y1 = fy.max(fy + sy * h);
        } else if hd != 0 {
            // Horizontal edge handle: height follows width, centered vertically.
            let w = (x1 - x0).max(1);
            let h = ((w as f32 / ratio).round() as i32).max(1);
            if h > bounds.h {
                let hh = bounds.h.max(1);
                let ww = ((hh as f32 * ratio).round() as i32).max(1);
                y0 = bounds.y;
                y1 = bounds.bottom();
                if hd > 0 {
                    x1 = x0 + ww;
                } else {
                    x0 = x1 - ww;
                }
            } else {
                let cy = start.y as f32 + start.h as f32 / 2.0;
                let ny0 =
                    ((cy - h as f32 / 2.0).round() as i32).clamp(bounds.y, bounds.bottom() - h);
                y0 = ny0;
                y1 = ny0 + h;
            }
        } else if vd != 0 {
            let h = (y1 - y0).max(1);
            let w = ((h as f32 * ratio).round() as i32).max(1);
            if w > bounds.w {
                let ww = bounds.w.max(1);
                let hh = ((ww as f32 / ratio).round() as i32).max(1);
                x0 = bounds.x;
                x1 = bounds.right();
                if vd > 0 {
                    y1 = y0 + hh;
                } else {
                    y0 = y1 - hh;
                }
            } else {
                let cx = start.x as f32 + start.w as f32 / 2.0;
                let nx0 =
                    ((cx - w as f32 / 2.0).round() as i32).clamp(bounds.x, bounds.right() - w);
                x0 = nx0;
                x1 = nx0 + w;
            }
        }
    }

    if x1 - x0 < 1 {
        if hd >= 0 {
            x1 = x0 + 1;
        } else {
            x0 = x1 - 1;
        }
    }
    if y1 - y0 < 1 {
        if vd >= 0 {
            y1 = y0 + 1;
        } else {
            y0 = y1 - 1;
        }
    }
    clamp_rect(PxRect::new(x0, y0, x1 - x0, y1 - y0), bounds)
}

/// Shift (never resize) `r` so it lies inside `bounds`; if larger than bounds it is shrunk.
pub fn clamp_rect(r: PxRect, bounds: PxRect) -> PxRect {
    let w = r.w.clamp(1, bounds.w.max(1));
    let h = r.h.clamp(1, bounds.h.max(1));
    let x = r.x.clamp(bounds.x, (bounds.right() - w).max(bounds.x));
    let y = r.y.clamp(bounds.y, (bounds.bottom() - h).max(bounds.y));
    PxRect::new(x, y, w, h)
}

/// Move a rect by (dx, dy) keeping it inside `bounds`.
pub fn move_rect(r: PxRect, dx: i32, dy: i32, bounds: PxRect) -> PxRect {
    clamp_rect(r.translate(dx, dy), bounds)
}

/// Keyboard resize: grow/shrink the right and bottom edges by (dw, dh).
pub fn grow_rect(r: PxRect, dw: i32, dh: i32, bounds: PxRect) -> PxRect {
    let w = (r.w + dw).clamp(1, (bounds.right() - r.x).max(1));
    let h = (r.h + dh).clamp(1, (bounds.bottom() - r.y).max(1));
    PxRect::new(r.x, r.y, w, h)
}

/// Fit `r` to `ratio` keeping its width (height changes), then clamp into `bounds`.
pub fn apply_ratio(r: PxRect, ratio: Ratio, bounds: PxRect) -> PxRect {
    if ratio.0 == 0 || ratio.1 == 0 {
        return r;
    }
    let rt = ratio.0 as f32 / ratio.1 as f32;
    let mut w = r.w.max(1);
    let mut h = ((w as f32 / rt).round() as i32).max(1);
    if h > bounds.h {
        h = bounds.h.max(1);
        w = ((h as f32 * rt).round() as i32).clamp(1, bounds.w.max(1));
    }
    clamp_rect(PxRect::new(r.x, r.y, w, h), bounds)
}

/// Rect from two drag points, optionally constrained to a ratio (anchor = `a`), clamped
/// into `bounds`.
pub fn drag_rect(a: (i32, i32), b: (i32, i32), bounds: PxRect, aspect: Option<Ratio>) -> PxRect {
    let ax = a.0.clamp(bounds.x, bounds.right());
    let ay = a.1.clamp(bounds.y, bounds.bottom());
    let bx = b.0.clamp(bounds.x, bounds.right());
    let by = b.1.clamp(bounds.y, bounds.bottom());
    let (mut x0, mut x1, mut y0, mut y1) = (ax.min(bx), ax.max(bx), ay.min(by), ay.max(by));
    if let Some((rw, rh)) = aspect.filter(|(w, h)| *w > 0 && *h > 0) {
        let ratio = rw as f32 / rh as f32;
        let dx = (bx - ax).abs() as f32;
        let dy = (by - ay).abs() as f32;
        let sx = if bx >= ax { 1 } else { -1 };
        let sy = if by >= ay { 1 } else { -1 };
        let max_w = if sx > 0 {
            (bounds.right() - ax) as f32
        } else {
            (ax - bounds.x) as f32
        };
        let max_h = if sy > 0 {
            (bounds.bottom() - ay) as f32
        } else {
            (ay - bounds.y) as f32
        };
        let w = dx.max(dy * ratio).min(max_w).min(max_h * ratio);
        let h = w / ratio;
        let (w, h) = (w.round() as i32, h.round() as i32);
        x0 = ax.min(ax + sx * w);
        x1 = ax.max(ax + sx * w);
        y0 = ay.min(ay + sy * h);
        y1 = ay.max(ay + sy * h);
    }
    PxRect::new(x0, y0, x1 - x0, y1 - y0)
}

/// Parse "W x H" style exact size text ("1280x720", "1280 720", "1280,720", "1280 × 720").
pub fn parse_exact_size(s: &str) -> Option<(i32, i32)> {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_ascii_digit() { c } else { ' ' })
        .collect();
    let mut it = cleaned.split_whitespace();
    let w: i32 = it.next()?.parse().ok()?;
    let h: i32 = it.next()?.parse().ok()?;
    if it.next().is_some() || w < 1 || h < 1 {
        return None;
    }
    Some((w, h))
}

/// Set the selection to an exact size, keeping its top-left where possible, clamped into bounds.
pub fn apply_exact_size(r: PxRect, w: i32, h: i32, bounds: PxRect) -> PxRect {
    clamp_rect(PxRect::new(r.x, r.y, w, h), bounds)
}

/// Image <-> overlay-local mapping. `origin` is the overlay-local px position of the base
/// image's top-left; `scale` is local px per image px (1.0 in Capture mode).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mapping {
    pub origin: (f32, f32),
    pub scale: f32,
}

impl Mapping {
    pub fn to_local(self, p: (f32, f32)) -> (f32, f32) {
        (
            self.origin.0 + p.0 * self.scale,
            self.origin.1 + p.1 * self.scale,
        )
    }
    pub fn to_image(self, p: (f32, f32)) -> (f32, f32) {
        (
            (p.0 - self.origin.0) / self.scale,
            (p.1 - self.origin.1) / self.scale,
        )
    }
    pub fn len_to_local(&self, l: f32) -> f32 {
        l * self.scale
    }
}

/// Scale so a `w` x `h` image fits inside `avail` (w, h) times `fraction`, never above 1.0.
pub fn fit_scale(w: f32, h: f32, avail: (f32, f32), fraction: f32) -> f32 {
    if w <= 0.0 || h <= 0.0 {
        return 1.0;
    }
    let s = (avail.0 * fraction / w).min(avail.1 * fraction / h);
    s.clamp(0.05, 1.0)
}

/// Top-left that centers a `w` x `h` box inside `area`.
pub fn center_in(w: f32, h: f32, area: PxRect) -> (f32, f32) {
    (
        area.x as f32 + (area.w as f32 - w) / 2.0,
        area.y as f32 + (area.h as f32 - h) / 2.0,
    )
}

/// Place the tool bar and action bar around a selection. Returns top-left positions in
/// overlay-local px. Prefers below the selection (tool bar left-aligned with it, action bar
/// to its right), then above, then inside at the bottom. If both bars do not fit side by
/// side within `bounds` they are stacked.
pub fn layout_bars(
    sel: PxRect,
    bounds: PxRect,
    tool: (f32, f32),
    action: (f32, f32),
    gap: f32,
    pad: f32,
) -> ((f32, f32), (f32, f32)) {
    let bx0 = bounds.x as f32 + pad;
    let bx1 = bounds.right() as f32 - pad;
    let total_w = tool.0 + gap + action.0;
    let side_by_side = total_w <= (bx1 - bx0);
    let block_h = if side_by_side {
        tool.1.max(action.1)
    } else {
        tool.1 + gap + action.1
    };
    let block_w = if side_by_side {
        total_w
    } else {
        tool.0.max(action.0)
    };

    let below_y = sel.bottom() as f32 + 14.0;
    let above_y = sel.y as f32 - 14.0 - block_h - 34.0; // leave room for the size label
    let y = if below_y + block_h <= bounds.bottom() as f32 - pad {
        below_y
    } else if above_y >= bounds.y as f32 + pad {
        above_y
    } else {
        (sel.bottom() as f32 - block_h - 12.0).max(bounds.y as f32 + pad)
    };
    let mut x = sel.x as f32;
    if x + block_w > bx1 {
        x = bx1 - block_w;
    }
    x = x.max(bx0);
    if side_by_side {
        ((x, y), (x + tool.0 + gap, y))
    } else {
        let mut ax = sel.x as f32;
        if ax + action.0 > bx1 {
            ax = bx1 - action.0;
        }
        ax = ax.max(bx0);
        ((x, y), (ax, y + tool.1 + gap))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: PxRect = PxRect::new(0, 0, 1000, 800);

    #[test]
    fn hit_handles_on_corners_and_edges() {
        let r = (100.0, 100.0, 200.0, 100.0);
        assert_eq!(hit_handle(r, (101.0, 99.0), 8.0), Some(Handle8::TopLeft));
        assert_eq!(hit_handle(r, (200.0, 100.0), 8.0), Some(Handle8::Top));
        assert_eq!(hit_handle(r, (300.0, 150.0), 8.0), Some(Handle8::Right));
        assert_eq!(hit_handle(r, (200.0, 150.0), 8.0), None);
    }

    #[test]
    fn resize_free_and_flip() {
        let s = PxRect::new(100, 100, 200, 100);
        let r = resize_px_rect(s, Handle8::BottomRight, (400, 300), B, None);
        assert_eq!(r, PxRect::new(100, 100, 300, 200));
        // Drag the right edge left past the left edge: flips.
        let r = resize_px_rect(s, Handle8::Right, (50, 150), B, None);
        assert_eq!(r, PxRect::new(50, 100, 50, 100));
        // Edge handle does not change the other axis.
        let r = resize_px_rect(s, Handle8::Top, (999, 50), B, None);
        assert_eq!(r, PxRect::new(100, 50, 200, 150));
    }

    #[test]
    fn resize_clamps_to_bounds() {
        let s = PxRect::new(900, 700, 50, 50);
        let r = resize_px_rect(s, Handle8::BottomRight, (5000, 5000), B, None);
        assert_eq!(r, PxRect::new(900, 700, 100, 100));
        let r = resize_px_rect(s, Handle8::TopLeft, (-50, -50), B, None);
        assert_eq!(r, PxRect::new(0, 0, 950, 750));
    }

    #[test]
    fn resize_with_aspect_corner() {
        let s = PxRect::new(100, 100, 160, 90);
        let r = resize_px_rect(s, Handle8::BottomRight, (420, 200), B, Some((16, 9)));
        assert_eq!((r.x, r.y, r.w, r.h), (100, 100, 320, 180));
        // Top-left handle keeps the bottom-right corner fixed.
        let r = resize_px_rect(s, Handle8::TopLeft, (0, 90), B, Some((1, 1)));
        assert_eq!(r.right(), 260);
        assert_eq!(r.bottom(), 190);
        assert_eq!(r.w, r.h);
    }

    #[test]
    fn resize_with_aspect_edge_adjusts_other_axis() {
        let s = PxRect::new(100, 100, 160, 90);
        let r = resize_px_rect(s, Handle8::Right, (420, 0), B, Some((16, 9)));
        assert_eq!((r.x, r.w, r.h), (100, 320, 180));
        let r = resize_px_rect(s, Handle8::Bottom, (0, 280), B, Some((1, 1)));
        assert_eq!((r.h, r.w), (180, 180));
    }

    #[test]
    fn min_size_is_one() {
        let s = PxRect::new(100, 100, 50, 50);
        let r = resize_px_rect(s, Handle8::Right, (100, 120), B, None);
        assert!(r.w >= 1 && r.h >= 1);
    }

    #[test]
    fn nudge_and_grow_stay_in_bounds() {
        let r = PxRect::new(0, 0, 100, 100);
        assert_eq!(move_rect(r, -5, -5, B), PxRect::new(0, 0, 100, 100));
        assert_eq!(move_rect(r, 10, 1, B), PxRect::new(10, 1, 100, 100));
        let r = PxRect::new(950, 750, 50, 50);
        assert_eq!(grow_rect(r, 10, 10, B), r);
        assert_eq!(grow_rect(r, -10, -60, B), PxRect::new(950, 750, 40, 1));
    }

    #[test]
    fn ratio_presets() {
        let r = PxRect::new(10, 10, 320, 400);
        assert_eq!(apply_ratio(r, (16, 9), B), PxRect::new(10, 10, 320, 180));
        assert_eq!(apply_ratio(r, (1, 1), B), PxRect::new(10, 10, 320, 320));
        let r = PxRect::new(0, 0, 1000, 800);
        let a = apply_ratio(r, (1, 1), B);
        assert_eq!((a.w, a.h), (800, 800));
    }

    #[test]
    fn drag_rect_aspect() {
        let r = drag_rect((100, 100), (300, 120), B, Some((1, 1)));
        assert_eq!((r.w, r.h), (200, 200));
        let r = drag_rect((100, 100), (50, 40), B, None);
        assert_eq!(r, PxRect::new(50, 40, 50, 60));
        let r = drag_rect((900, 700), (2000, 2000), B, None);
        assert_eq!(r, PxRect::new(900, 700, 100, 100));
    }

    #[test]
    fn exact_size_parsing() {
        assert_eq!(parse_exact_size("1280x720"), Some((1280, 720)));
        assert_eq!(parse_exact_size(" 640 \u{d7} 480 "), Some((640, 480)));
        assert_eq!(parse_exact_size("640"), None);
        assert_eq!(parse_exact_size("0x10"), None);
        assert_eq!(parse_exact_size("1 2 3"), None);
        assert_eq!(
            apply_exact_size(PxRect::new(900, 700, 10, 10), 500, 500, B),
            PxRect::new(500, 300, 500, 500)
        );
    }

    #[test]
    fn mapping_round_trip() {
        let m = Mapping {
            origin: (100.0, 50.0),
            scale: 0.5,
        };
        let l = m.to_local((200.0, 100.0));
        assert_eq!(l, (200.0, 100.0));
        assert_eq!(m.to_image(l), (200.0, 100.0));
        assert_eq!(m.len_to_local(10.0), 5.0);
    }

    #[test]
    fn fit_scale_never_upscales() {
        assert_eq!(fit_scale(100.0, 100.0, (1000.0, 1000.0), 0.9), 1.0);
        let s = fit_scale(2000.0, 1000.0, (1000.0, 1000.0), 0.9);
        assert!((s - 0.45).abs() < 1e-5);
        assert_eq!(
            center_in(100.0, 50.0, PxRect::new(0, 0, 200, 100)),
            (50.0, 25.0)
        );
    }

    #[test]
    fn bars_layout_prefers_below_then_above() {
        let sel = PxRect::new(100, 100, 300, 200);
        let ((tx, ty), (ax, ay)) = layout_bars(sel, B, (500.0, 44.0), (250.0, 44.0), 8.0, 8.0);
        assert_eq!((tx, ty), (100.0, 314.0));
        assert_eq!(ay, ty);
        assert_eq!(ax, 100.0 + 500.0 + 8.0);
        let sel = PxRect::new(100, 600, 300, 190);
        let ((_, ty), _) = layout_bars(sel, B, (500.0, 44.0), (250.0, 44.0), 8.0, 8.0);
        assert!(ty < 600.0);
        let narrow = PxRect::new(0, 0, 600, 800);
        let ((_, ty), (_, ay)) = layout_bars(
            PxRect::new(10, 10, 100, 100),
            narrow,
            (500.0, 44.0),
            (250.0, 44.0),
            8.0,
            8.0,
        );
        assert!(ay > ty);
    }
}
