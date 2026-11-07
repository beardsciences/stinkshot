//! egui-painter drawing for the overlay: live shape previews (the authoritative pixels come
//! from `annotate::render`), selection chrome, shape handles, the loupe.

use egui::epaint::StrokeKind;
use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Shape as EShape, Stroke, Vec2};
use stinkshot_core::annotate::{Shape, ShapeKind};
use stinkshot_core::{PtF, RectF, Rgba};

use super::hover::contrast_text;
use super::selection::{FRect, Handle8, handle_pos};
use super::shapes::{self, ShapeHandle};

/// Maps image px to egui points: `origin` is the point position of image (0, 0) and `k`
/// is points per image px.
#[derive(Clone, Copy)]
pub struct ImgToPt {
    pub origin: Pos2,
    pub k: f32,
}

impl ImgToPt {
    pub fn p(&self, p: PtF) -> Pos2 {
        Pos2::new(self.origin.x + p.x * self.k, self.origin.y + p.y * self.k)
    }
    pub fn r(&self, r: RectF) -> Rect {
        Rect::from_min_size(
            self.p(PtF::new(r.x, r.y)),
            Vec2::new(r.w * self.k, r.h * self.k),
        )
    }
    pub fn len(&self, l: f32) -> f32 {
        l * self.k
    }
}

pub fn color32(c: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
}

/// Paint an approximation of `shape`. `skip_text` hides the text of a textual shape that is
/// being edited by the inline TextEdit.
pub fn paint_shape(painter: &Painter, m: ImgToPt, shape: &Shape, skip_text: bool) {
    let col = color32(shape.style.color);
    let stroke_w = m.len(shape.style.width).max(1.0);
    let stroke = Stroke::new(stroke_w, col);
    match &shape.kind {
        ShapeKind::Arrow { from, to } => {
            let (a, b) = (m.p(*from), m.p(*to));
            painter.add(EShape::line_segment([a, b], stroke));
            let dir = b - a;
            let len = dir.length();
            if len > 0.5 {
                let d = dir / len;
                let head = m.len((shape.style.width * 4.0).max(12.0)).min(len);
                let n = Vec2::new(-d.y, d.x);
                let base = b - d * head;
                let w = head * 0.5;
                painter.add(EShape::convex_polygon(
                    vec![b, base + n * w, base - n * w],
                    col,
                    Stroke::NONE,
                ));
            }
        }
        ShapeKind::Line { from, to } => {
            painter.add(EShape::line_segment([m.p(*from), m.p(*to)], stroke));
        }
        ShapeKind::Rect {
            rect,
            corner_radius,
        } => {
            let r = m.r(*rect);
            let cr = m.len(*corner_radius);
            if let Some(f) = shape.style.fill {
                painter.rect_filled(r, cr, color32(f));
            }
            painter.rect_stroke(r, cr, stroke, StrokeKind::Middle);
        }
        ShapeKind::Ellipse { rect } => {
            let r = m.r(*rect);
            if let Some(f) = shape.style.fill {
                painter.add(EShape::ellipse_filled(
                    r.center(),
                    r.size() / 2.0,
                    color32(f),
                ));
            }
            painter.add(EShape::ellipse_stroke(r.center(), r.size() / 2.0, stroke));
        }
        ShapeKind::Pen { points } | ShapeKind::Highlighter { points } => {
            let pts: Vec<Pos2> = points.iter().map(|p| m.p(*p)).collect();
            let hl = matches!(shape.kind, ShapeKind::Highlighter { .. });
            match pts.len() {
                0 => {}
                1 => {
                    painter.circle_filled(pts[0], stroke_w / 2.0, col);
                }
                _ => {
                    painter.add(EShape::line(pts.clone(), stroke));
                    // Round caps.
                    if !hl {
                        painter.circle_filled(pts[0], stroke_w / 2.0, col);
                        painter.circle_filled(pts[pts.len() - 1], stroke_w / 2.0, col);
                    }
                }
            }
        }
        ShapeKind::Text {
            pos,
            text,
            size,
            background,
        } => {
            let p = m.p(*pos);
            if let Some(bg) = background {
                let r = Rect::from_min_size(
                    p,
                    Vec2::new(
                        m.len(size * 0.55) * text.chars().count().max(1) as f32,
                        m.len(size * 1.25),
                    ),
                );
                painter.rect_filled(r.expand(2.0), 2.0, color32(*bg));
            }
            if !skip_text && !text.is_empty() {
                painter.text(
                    p,
                    Align2::LEFT_TOP,
                    text,
                    FontId::proportional(m.len(*size)),
                    col,
                );
            }
        }
        ShapeKind::Step { center, n, radius } => {
            let c = m.p(*center);
            let r = m.len(*radius);
            painter.circle_filled(c + Vec2::new(0.0, 1.5), r, Color32::from_black_alpha(70));
            painter.circle_filled(c, r, col);
            let t = contrast_text(shape.style.color);
            painter.text(
                c,
                Align2::CENTER_CENTER,
                n.to_string(),
                FontId::proportional(r * 1.1),
                Color32::from_rgb(t[0], t[1], t[2]),
            );
        }
        ShapeKind::Blur { rect, .. } => {
            let r = m.r(*rect);
            painter.rect_filled(r, 0.0, Color32::from_white_alpha(46));
            dashed_rect(painter, r, Color32::from_white_alpha(170));
        }
        ShapeKind::Pixelate { rect, block } => {
            let r = m.r(*rect);
            painter.rect_filled(r, 0.0, Color32::from_white_alpha(30));
            let cell = m.len(*block as f32).max(4.0);
            let mut y = r.top();
            let mut row = 0;
            while y < r.bottom() {
                let mut x = r.left();
                let mut col_i = row % 2;
                while x < r.right() {
                    if col_i % 2 == 0 {
                        let cr = Rect::from_min_max(
                            Pos2::new(x, y),
                            Pos2::new((x + cell).min(r.right()), (y + cell).min(r.bottom())),
                        );
                        painter.rect_filled(cr, 0.0, Color32::from_white_alpha(40));
                    }
                    x += cell;
                    col_i += 1;
                }
                y += cell;
                row += 1;
            }
            dashed_rect(painter, r, Color32::from_white_alpha(170));
        }
        ShapeKind::Redact { rect } => {
            painter.rect_filled(
                m.r(*rect),
                0.0,
                Color32::from_rgb(
                    shape.style.color[0],
                    shape.style.color[1],
                    shape.style.color[2],
                ),
            );
        }
        ShapeKind::Magnifier {
            source,
            dest,
            circular,
        } => {
            let s = m.r(*source);
            dashed_rect(painter, s, Color32::WHITE);
            let d = m.r(*dest);
            if *circular {
                painter.add(EShape::ellipse_filled(
                    d.center(),
                    d.size() / 2.0,
                    Color32::from_black_alpha(90),
                ));
                painter.add(EShape::ellipse_stroke(
                    d.center(),
                    d.size() / 2.0,
                    Stroke::new(2.5, Color32::WHITE),
                ));
            } else {
                painter.rect_filled(d, 6.0, Color32::from_black_alpha(90));
                painter.rect_stroke(d, 6.0, Stroke::new(2.5, Color32::WHITE), StrokeKind::Middle);
            }
            painter.line_segment(
                [s.center(), d.center()],
                Stroke::new(1.0, Color32::from_white_alpha(120)),
            );
        }
        ShapeKind::Callout {
            rect,
            tail,
            text,
            size,
        } => {
            let r = m.r(*rect);
            let fill = color32(shape.style.fill.unwrap_or(shape.style.color));
            let t = m.p(*tail);
            // Tail attaches to the nearest edge.
            let c = r.center();
            let hw = (r.width() / 2.0).max(1.0);
            let hh = (r.height() / 2.0).max(1.0);
            let horizontal = (t.x - c.x).abs() / hw > (t.y - c.y).abs() / hh;
            let pts = if !horizontal && t.y >= c.y {
                let bx = (r.left() + 14.0).clamp(r.left(), r.right());
                vec![
                    Pos2::new(bx, r.bottom() - 1.0),
                    Pos2::new((bx + 18.0).min(r.right()), r.bottom() - 1.0),
                    t,
                ]
            } else if !horizontal {
                let bx = (r.left() + 14.0).clamp(r.left(), r.right());
                vec![
                    Pos2::new(bx, r.top() + 1.0),
                    Pos2::new((bx + 18.0).min(r.right()), r.top() + 1.0),
                    t,
                ]
            } else if t.x < c.x {
                let by = (r.top() + 10.0).clamp(r.top(), r.bottom());
                vec![
                    Pos2::new(r.left() + 1.0, by),
                    Pos2::new(r.left() + 1.0, (by + 16.0).min(r.bottom())),
                    t,
                ]
            } else {
                let by = (r.top() + 10.0).clamp(r.top(), r.bottom());
                vec![
                    Pos2::new(r.right() - 1.0, by),
                    Pos2::new(r.right() - 1.0, (by + 16.0).min(r.bottom())),
                    t,
                ]
            };
            painter.rect_filled(r, m.len(8.0), fill);
            painter.add(EShape::convex_polygon(pts, fill, Stroke::NONE));
            if !skip_text && !text.is_empty() {
                let tc = contrast_text(shape.style.fill.unwrap_or(shape.style.color));
                painter.text(
                    Pos2::new(r.left() + m.len(10.0), r.top() + m.len(6.0)),
                    Align2::LEFT_TOP,
                    text,
                    FontId::proportional(m.len(*size)),
                    Color32::from_rgb(tc[0], tc[1], tc[2]),
                );
            }
        }
        ShapeKind::Spotlight { rect, dim } => {
            // The dimmed surround is drawn by the caller with the selection clip; here just
            // the clear area outline.
            let _ = dim;
            dashed_rect(painter, m.r(*rect), Color32::from_white_alpha(200));
        }
    }
}

/// Dim everything inside `clip` except `hole` (used for spotlight previews).
pub fn paint_spotlight_dim(painter: &Painter, clip: Rect, hole: Rect, dim: f32) {
    let c = Color32::from_black_alpha((dim.clamp(0.0, 1.0) * 255.0) as u8);
    let hole = hole.intersect(clip);
    for r in around(clip, hole) {
        painter.rect_filled(r, 0.0, c);
    }
}

/// The four rects covering `outer` minus `inner` (inner must be inside outer).
pub fn around(outer: Rect, inner: Rect) -> [Rect; 4] {
    [
        Rect::from_min_max(outer.min, Pos2::new(outer.max.x, inner.min.y)),
        Rect::from_min_max(Pos2::new(outer.min.x, inner.max.y), outer.max),
        Rect::from_min_max(
            Pos2::new(outer.min.x, inner.min.y),
            Pos2::new(inner.min.x, inner.max.y),
        ),
        Rect::from_min_max(
            Pos2::new(inner.max.x, inner.min.y),
            Pos2::new(outer.max.x, inner.max.y),
        ),
    ]
}

pub fn dashed_rect(painter: &Painter, r: Rect, color: Color32) {
    let pts = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    for s in EShape::dashed_line(&pts, Stroke::new(1.5, color), 5.0, 4.0) {
        painter.add(s);
    }
}

/// Selection chrome: accent outline plus eight 9x9 handles (design: white fill, accent border).
pub fn paint_selection_chrome(painter: &Painter, r: Rect, accent: Color32, with_handles: bool) {
    painter.rect_stroke(r, 0.0, Stroke::new(1.5, accent), StrokeKind::Outside);
    if with_handles {
        let fr: FRect = (r.min.x, r.min.y, r.width(), r.height());
        for h in Handle8::ALL {
            let (x, y) = handle_pos(fr, h);
            let hr = Rect::from_center_size(Pos2::new(x, y), Vec2::splat(9.0));
            painter.rect_filled(hr, 2.0, Color32::WHITE);
            painter.rect_stroke(hr, 2.0, Stroke::new(1.5, accent), StrokeKind::Inside);
        }
    }
}

/// Handles of a selected annotation shape (round, smaller than selection handles).
pub fn paint_shape_handles(
    painter: &Painter,
    m: ImgToPt,
    shape: &Shape,
    accent: Color32,
    measure: shapes::Measure,
) {
    let b = shapes::bounds(shape, measure);
    let br = m.r(b).expand(2.0);
    for s in EShape::dashed_line(
        &[
            br.left_top(),
            br.right_top(),
            br.right_bottom(),
            br.left_bottom(),
            br.left_top(),
        ],
        Stroke::new(1.0, Color32::from_white_alpha(190)),
        4.0,
        3.0,
    ) {
        painter.add(s);
    }
    for (h, p) in shapes::handles(shape, measure) {
        let c = m.p(p);
        let (fill, r) = match h {
            ShapeHandle::Src(_) => (Color32::from_rgb(0xff, 0xd1, 0x66), 4.0),
            ShapeHandle::Tail | ShapeHandle::P1 | ShapeHandle::P2 => (accent, 5.5),
            ShapeHandle::Rect(_) => (Color32::WHITE, 4.5),
        };
        painter.circle_filled(c, r, fill);
        painter.circle_stroke(c, r, Stroke::new(1.5, accent));
    }
}

/// Size of one loupe cell in points and cell count (design: 11 x 11 at 12 pt = 132 pt).
pub const LOUPE_CELLS: i32 = 11;
pub const LOUPE_CELL: f32 = 12.0;

/// Pixel color at local-px (x, y) of the frozen image, black outside.
pub fn sample(img: &Rgba, x: i32, y: i32) -> [u8; 3] {
    if x < 0 || y < 0 || x >= img.width as i32 || y >= img.height as i32 {
        return [0, 0, 0];
    }
    let p = img.pixel(x as u32, y as u32);
    [p[0], p[1], p[2]]
}

/// Paint the 11x11 zoomed grid with a crosshair row/column and a highlighted center cell.
/// Returns the center pixel color.
pub fn paint_loupe(
    painter: &Painter,
    top_left: Pos2,
    img: &Rgba,
    cx: i32,
    cy: i32,
    accent: Color32,
) -> [u8; 3] {
    let size = LOUPE_CELLS as f32 * LOUPE_CELL;
    let frame = Rect::from_min_size(top_left, Vec2::splat(size));
    painter.rect_filled(frame.expand(2.0), 10.0, Color32::WHITE);
    painter.rect_filled(frame, 8.0, Color32::from_rgb(0x1e, 0x1e, 0x1e));
    let half = LOUPE_CELLS / 2;
    for j in 0..LOUPE_CELLS {
        for i in 0..LOUPE_CELLS {
            let c = sample(img, cx + i - half, cy + j - half);
            let cell = Rect::from_min_size(
                top_left + Vec2::new(i as f32 * LOUPE_CELL, j as f32 * LOUPE_CELL),
                Vec2::splat(LOUPE_CELL),
            );
            // Square cells; the rounded frame is only painted behind, so corners overdraw
            // slightly, which reads fine at this size.
            painter.rect_filled(cell, 0.0, Color32::from_rgb(c[0], c[1], c[2]));
            if i == half || j == half {
                painter.rect_filled(cell, 0.0, accent.gamma_multiply(0.35));
            }
        }
    }
    let center = Rect::from_min_size(
        top_left + Vec2::new(half as f32 * LOUPE_CELL, half as f32 * LOUPE_CELL),
        Vec2::splat(LOUPE_CELL),
    );
    painter.rect_stroke(
        center,
        0.0,
        Stroke::new(2.0, Color32::WHITE),
        StrokeKind::Inside,
    );
    painter.rect_stroke(
        frame.expand(2.0),
        10.0,
        Stroke::new(2.0, Color32::WHITE),
        StrokeKind::Inside,
    );
    sample(img, cx, cy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_clamps_outside_to_black() {
        let img = Rgba::filled(4, 4, [10, 20, 30, 255]);
        assert_eq!(sample(&img, 1, 1), [10, 20, 30]);
        assert_eq!(sample(&img, -1, 0), [0, 0, 0]);
        assert_eq!(sample(&img, 4, 0), [0, 0, 0]);
    }

    #[test]
    fn around_covers_without_overlap() {
        let outer = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(100.0, 80.0));
        let inner = Rect::from_min_max(Pos2::new(20.0, 10.0), Pos2::new(60.0, 50.0));
        let area: f32 = around(outer, inner)
            .iter()
            .map(|r| r.width() * r.height())
            .sum();
        assert!((area - (100.0 * 80.0 - 40.0 * 40.0)).abs() < 1e-3);
    }

    #[test]
    fn img_to_pt_scales() {
        let m = ImgToPt {
            origin: Pos2::new(10.0, 20.0),
            k: 0.5,
        };
        assert_eq!(m.p(PtF::new(100.0, 100.0)), Pos2::new(60.0, 70.0));
        assert_eq!(m.len(8.0), 4.0);
        assert_eq!(
            m.r(RectF::new(0.0, 0.0, 10.0, 10.0)).size(),
            Vec2::new(5.0, 5.0)
        );
    }
}
