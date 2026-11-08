//! Measure tool (U): drag a ruler across the capture. Shows width, height, length and angle
//! in physical px (image px of the capture) and in logical px at the capture's scale. Ends snap
//! to the nearest strong edge in the pixels (a UI element's border) when Snap to Elements is on;
//! Alt flips that while held; Shift
//! keeps the ruler horizontal, vertical or at 45 degrees. Nothing is added to the image.

use egui::{Color32, FontId, Pos2, Stroke, Vec2};
use stinkshot_core::{PtF, Rgba};

use super::paint::ImgToPt;
use super::*;

/// Edge search radius around each end, image px.
pub const SNAP_RADIUS: i32 = 6;
/// Minimum luminance step (0-255) that counts as an edge.
const EDGE_MIN: i32 = 28;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measurement {
    pub dx: f32,
    pub dy: f32,
    pub len: f32,
    /// Degrees, counter-clockwise from the positive x axis (screen y points down), 0..360.
    pub angle: f32,
}

pub fn measure(a: PtF, b: PtF) -> Measurement {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let angle = (-dy).atan2(dx).to_degrees().rem_euclid(360.0);
    Measurement {
        dx: dx.abs(),
        dy: dy.abs(),
        len: (dx * dx + dy * dy).sqrt(),
        angle: if angle >= 359.95 { 0.0 } else { angle },
    }
}

/// Shift: the end moves to the nearest 45 degree direction from `a`, keeping its length
/// along that direction.
pub fn constrain(a: PtF, b: PtF) -> PtF {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let ang = dy.atan2(dx);
    let step = std::f32::consts::FRAC_PI_4;
    let snapped = (ang / step).round() * step;
    let (c, s) = (snapped.cos(), snapped.sin());
    let t = dx * c + dy * s;
    let (x, y) = (a.x + t * c, a.y + t * s);
    PtF::new(x.round(), y.round())
}

fn luma(img: &Rgba, x: i32, y: i32) -> i32 {
    let i = ((y as u32 * img.width + x as u32) * 4) as usize;
    let p = &img.data[i..i + 3];
    (p[0] as i32 * 299 + p[1] as i32 * 587 + p[2] as i32 * 114) / 1000
}

/// Strongest boundary in `lo..=hi` (`grad(b)` = step between `b - 1` and `b`), at least
/// `EDGE_MIN`; ties go to the one nearest `at`. Returns (gradient, boundary).
fn best_edge(lo: i32, hi: i32, at: i32, grad: impl Fn(i32) -> i32) -> Option<(i32, i32)> {
    let mut found: Option<(i32, i32)> = None;
    for b in lo.max(1)..=hi {
        let g = grad(b);
        let better = match found {
            None => g >= EDGE_MIN,
            Some((bg, bb)) => g > bg || (g == bg && (b - at).abs() < (bb - at).abs()),
        };
        if better {
            found = Some((g, b));
        }
    }
    found
}

/// The strongest edge within `radius` of `p` on each axis (a pixel boundary: x = 100 is the
/// line between pixels 99 and 100). Axes without an edge keep `p` rounded. Near a corner the
/// second axis is also searched along the first axis' edge, so both ends of a box corner
/// snap even when the pointer is just outside the box.
pub fn snap_edge(img: &Rgba, p: PtF, radius: i32) -> PtF {
    let (w, h) = (img.width as i32, img.height as i32);
    if w < 2 || h < 2 {
        return p;
    }
    let (px, py) = (p.x.round() as i32, p.y.round() as i32);
    let rows = |r: i32| r.clamp(0, h - 1);
    let cols = |c: i32| c.clamp(0, w - 1);
    let x_on = |row: i32| {
        let row = rows(row);
        best_edge(px - radius, (px + radius).min(w - 1), px, |b| {
            (luma(img, b, row) - luma(img, b - 1, row)).abs()
        })
    };
    let y_on = |col: i32| {
        let col = cols(col);
        best_edge(py - radius, (py + radius).min(h - 1), py, |b| {
            (luma(img, col, b) - luma(img, col, b - 1)).abs()
        })
    };
    let pick = |a: Option<(i32, i32)>, b: Option<(i32, i32)>| match (a, b) {
        (Some(x), Some(y)) => Some(if y.0 > x.0 { y } else { x }),
        (x, y) => x.or(y),
    };
    let mut x = x_on(p.y.floor() as i32);
    let mut y = y_on(p.x.floor() as i32);
    if let (Some((_, sx)), None) = (x, y) {
        y = pick(y_on(sx - 1), y_on(sx));
    } else if let (None, Some((_, sy))) = (x, y) {
        x = pick(x_on(sy - 1), x_on(sy));
    }
    PtF::new(
        x.map_or(px, |(_, b)| b) as f32,
        y.map_or(py, |(_, b)| b) as f32,
    )
}

fn fmt_len(v: f32) -> String {
    if (v - v.round()).abs() < 0.05 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.1}")
    }
}

/// Label lines: physical px, then logical px when the scale is known and not 100 %.
pub fn label(m: &Measurement, scale: Option<f32>) -> (String, Option<String>) {
    let main = format!(
        "{} \u{d7} {} px \u{b7} {} px \u{b7} {}\u{b0}",
        fmt_len(m.dx),
        fmt_len(m.dy),
        fmt_len(m.len),
        fmt_len((m.angle * 10.0).round() / 10.0)
    );
    let logical = scale.filter(|s| (s - 1.0).abs() > 0.01).map(|scale| {
        format!(
            "{} \u{d7} {} \u{b7} {} at {}%",
            fmt_len(m.dx / scale),
            fmt_len(m.dy / scale),
            fmt_len(m.len / scale),
            (scale * 100.0).round() as i32
        )
    });
    (main, logical)
}

impl Overlay {
    /// Where a ruler end goes: clamped to the image, snapped to an edge when snapping is on
    /// (Snap to Elements) and Alt is not held, or off and Alt is held.
    pub(super) fn ruler_point(&mut self, p: (f32, f32), alt: bool) -> PtF {
        let ip = self.clamp_image(self.to_image(p));
        if self.snap_edges == alt {
            return PtF::new(ip.x.round(), ip.y.round());
        }
        let base = self.ensure_base();
        snap_edge(&base, ip, SNAP_RADIUS)
    }

    /// DPI scale for the logical readout: the scale the pixels were captured at. On the
    /// frozen screen that is the monitor under the selection; an opened capture (history
    /// Edit, window, scrolling) carries it in its source, and without it there is no
    /// logical line.
    fn ruler_scale(&self) -> Option<f32> {
        if matches!(self.kind, Kind::Annotate) {
            return self.annotate_meta.as_ref().and_then(|(_, _, s)| s.scale);
        }
        self.sel.map(|s| {
            let r = self.canvas_rect_screen(s);
            self.monitor_at((r.x + r.w / 2, r.y + r.h / 2)).1
        })
    }

    pub(super) fn paint_ruler(&self, painter: &egui::Painter, m: ImgToPt, accent: Color32) {
        if self.tool != Tool::Measure {
            return;
        }
        let Some((a, b)) = self.ruler else { return };
        let (pa, pb) = (m.p(a), m.p(b));
        let halo = Stroke::new(4.0, Color32::from_black_alpha(150));
        let line = Stroke::new(2.0, accent);
        let mm = measure(a, b);
        // Legs of the right triangle for a diagonal ruler.
        if mm.dx >= 1.0 && mm.dy >= 1.0 {
            let corner = Pos2::new(pb.x, pa.y);
            let leg = Stroke::new(1.0, Color32::from_white_alpha(170));
            painter.add(egui::Shape::dashed_line(&[pa, corner, pb], leg, 4.0, 3.0));
        }
        painter.line_segment([pa, pb], halo);
        painter.line_segment([pa, pb], line);
        // End ticks across the ruler.
        let d = pb - pa;
        let n = if d.length() > 0.5 {
            Vec2::new(-d.y, d.x).normalized() * 6.0
        } else {
            Vec2::new(0.0, 6.0)
        };
        for p in [pa, pb] {
            painter.line_segment([p - n, p + n], halo);
            painter.line_segment([p - n, p + n], line);
        }
        let (main, logical) = label(&mm, self.ruler_scale());
        let text = match logical {
            Some(l) => format!("{main}\n{l}"),
            None => main,
        };
        let font = FontId::proportional(12.5);
        let galley = painter.layout_no_wrap(text, font, Color32::WHITE);
        let size = galley.size() + Vec2::new(16.0, 8.0);
        // Beside the far end, flipped inside the visible area.
        let clip = painter.clip_rect();
        let mut pos = pb + Vec2::new(12.0, 12.0);
        if pos.x + size.x > clip.right() - 4.0 {
            pos.x = pb.x - 12.0 - size.x;
        }
        if pos.y + size.y > clip.bottom() - 4.0 {
            pos.y = pb.y - 12.0 - size.y;
        }
        let r = egui::Rect::from_min_size(pos, size);
        painter.rect_filled(r, 6.0, Color32::from_black_alpha(205));
        painter.galley(r.center() - galley.size() / 2.0, galley, Color32::WHITE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_and_angle() {
        let m = measure(PtF::new(0.0, 0.0), PtF::new(30.0, -40.0));
        assert_eq!((m.dx, m.dy, m.len), (30.0, 40.0, 50.0));
        assert!((m.angle - 53.13).abs() < 0.01, "{}", m.angle);
        assert_eq!(measure(PtF::new(5.0, 5.0), PtF::new(1.0, 5.0)).angle, 180.0);
        assert_eq!(
            measure(PtF::new(0.0, 0.0), PtF::new(0.0, 10.0)).angle,
            270.0
        );
        assert_eq!(measure(PtF::new(0.0, 0.0), PtF::new(10.0, 0.0)).angle, 0.0);
    }

    #[test]
    fn labels_show_logical_px_off_100_percent() {
        let m = measure(PtF::new(0.0, 0.0), PtF::new(300.0, 0.0));
        let (main, logical) = label(&m, Some(1.0));
        assert_eq!(main, "300 \u{d7} 0 px \u{b7} 300 px \u{b7} 0\u{b0}");
        assert!(logical.is_none());
        let (_, logical) = label(&m, Some(1.5));
        assert_eq!(logical.unwrap(), "200 \u{d7} 0 \u{b7} 200 at 150%");
        let m = measure(PtF::new(0.0, 0.0), PtF::new(10.0, -10.0));
        assert!(label(&m, Some(1.0)).0.contains("14.1 px"));
        // Unknown capture scale (a history item): no logical line.
        assert!(label(&m, None).1.is_none());
        assert!(label(&m, Some(1.0)).0.ends_with("45\u{b0}"));
    }

    #[test]
    fn shift_constrains_to_45_degrees() {
        let a = PtF::new(10.0, 10.0);
        assert_eq!(constrain(a, PtF::new(110.0, 14.0)), PtF::new(110.0, 10.0));
        assert_eq!(constrain(a, PtF::new(13.0, 90.0)), PtF::new(10.0, 90.0));
        let d = constrain(a, PtF::new(60.0, 56.0));
        assert_eq!(d.x - a.x, d.y - a.y);
    }

    /// A dark box from (40, 30) to (80, 60) (exclusive) on white.
    fn boxed() -> Rgba {
        let mut img = Rgba::filled(120, 90, [255, 255, 255, 255]);
        for y in 30..60 {
            for x in 40..80 {
                img.set_pixel(x, y, [30, 30, 30, 255]);
            }
        }
        img
    }

    #[test]
    fn ends_snap_to_box_edges() {
        let img = boxed();
        // Near the left edge, inside the box rows.
        assert_eq!(
            snap_edge(&img, PtF::new(43.0, 45.0), 6),
            PtF::new(40.0, 45.0)
        );
        // Near the right edge (boundary after the last dark pixel).
        assert_eq!(
            snap_edge(&img, PtF::new(77.4, 45.0), 6),
            PtF::new(80.0, 45.0)
        );
        // Near the top-left corner: both axes snap.
        assert_eq!(
            snap_edge(&img, PtF::new(37.0, 33.0), 6),
            PtF::new(40.0, 30.0)
        );
        // Far from any edge: unchanged (rounded).
        assert_eq!(
            snap_edge(&img, PtF::new(10.2, 10.0), 6),
            PtF::new(10.0, 10.0)
        );
        // Measuring edge to edge gives the box size.
        let a = snap_edge(&img, PtF::new(42.0, 45.0), 6);
        let b = snap_edge(&img, PtF::new(78.0, 45.0), 6);
        assert_eq!(measure(a, b).len, 40.0);
    }

    #[test]
    fn faint_steps_are_not_edges() {
        let mut img = Rgba::filled(50, 50, [200, 200, 200, 255]);
        for y in 0..50 {
            for x in 25..50 {
                img.set_pixel(x, y, [210, 210, 210, 255]);
            }
        }
        assert_eq!(
            snap_edge(&img, PtF::new(23.0, 10.0), 6),
            PtF::new(23.0, 10.0)
        );
        // Tiny images do not panic.
        let tiny = Rgba::filled(1, 1, [0, 0, 0, 255]);
        assert_eq!(snap_edge(&tiny, PtF::new(0.4, 0.4), 6), PtF::new(0.4, 0.4));
    }
}
