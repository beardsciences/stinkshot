//! Pure annotation-shape math for the editor: bounds, hit testing, translation, resize
//! handles per shape type, creation from a drag, smoothing, recoloring and sizing, z-order,
//! duplication and the text / arrow option toggles.
//! All coordinates are float base-image pixels. Text extents come from a `Measure`
//! callback (the app passes `Fonts::measure`; tests pass a fixed-advance fake) so nothing
//! here depends on font files.

use stinkshot_core::annotate::{
    self, ArrowHead, Doc, Shape, ShapeId, ShapeKind, Style, arrow_centerline, arrow_mid,
    arrow_parts, ctrl_through, follow_ctrl,
};
use stinkshot_core::{PtF, RectF};

use super::selection::Handle8;
use crate::l10n::tr;

/// Text extent callback: (text, size_px, bold) -> (width, height).
pub type Measure<'a> = &'a dyn Fn(&str, f32, bool) -> (f32, f32);

/// Highlighter alpha the editor writes into `Style::color`.
pub const HIGHLIGHTER_ALPHA: u8 = 0x99;
pub const MIN_SHAPE: f32 = 2.0;
/// Text pill padding as a fraction of the font size (matches core's renderer).
const TEXT_PAD_X: f32 = 0.4;
const TEXT_PAD_Y: f32 = 0.2;
/// A curve handle dragged this close (image px) to the chord's middle straightens the arrow.
pub const STRAIGHTEN_PX: f32 = 4.0;
/// Font sizes the text size buttons step through.
pub const TEXT_SIZES: [f32; 16] = [
    10.0, 12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 28.0, 32.0, 40.0, 48.0, 56.0, 64.0, 72.0, 96.0, 128.0,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    Select,
    Arrow,
    Line,
    Rect,
    Ellipse,
    Pen,
    Highlighter,
    Text,
    Step,
    Blur,
    Pixelate,
    Redact,
    Magnifier,
    Callout,
    Spotlight,
    /// Ruler: measures, adds nothing to the image (`measure.rs`).
    Measure,
    /// Selects OCR words of the capture to copy as text, adds nothing (`select_text.rs`).
    SelectText,
}

impl Tool {
    /// Tool name without its shortcut ("Arrow").
    pub fn name(self) -> &'static str {
        match self {
            Tool::Select => tr!("overlay.tool.select"),
            Tool::Arrow => tr!("overlay.tool.arrow"),
            Tool::Line => tr!("overlay.tool.line"),
            Tool::Rect => tr!("overlay.tool.rect"),
            Tool::Ellipse => tr!("overlay.tool.ellipse"),
            Tool::Pen => tr!("overlay.tool.pen"),
            Tool::Highlighter => tr!("overlay.tool.highlighter"),
            Tool::Text => tr!("overlay.tool.text"),
            Tool::Step => tr!("overlay.tool.step"),
            Tool::Blur => tr!("overlay.tool.blur"),
            Tool::Pixelate => tr!("overlay.tool.pixelate"),
            Tool::Redact => tr!("overlay.tool.redact"),
            Tool::Magnifier => tr!("overlay.tool.magnifier"),
            Tool::Callout => tr!("overlay.tool.callout"),
            Tool::Spotlight => tr!("overlay.tool.spotlight"),
            Tool::Measure => tr!("overlay.tool.measure"),
            Tool::SelectText => tr!("overlay.tool.select_text"),
        }
    }

    /// Tool name with its shortcut key ("Arrow (A)"), from `keys::TOOL_KEYS`.
    pub fn label(self) -> String {
        match super::keys::TOOL_KEYS.iter().find(|(t, _)| *t == self) {
            Some((_, k)) => format!("{} ({})", self.name(), k.name()),
            None => self.name().to_string(),
        }
    }

    /// Key in `[editor.tool_styles]`.
    pub fn config_key(self) -> &'static str {
        match self {
            Tool::Select => "select",
            Tool::Arrow => "arrow",
            Tool::Line => "line",
            Tool::Rect => "rect",
            Tool::Ellipse => "ellipse",
            Tool::Pen => "pen",
            Tool::Highlighter => "highlighter",
            Tool::Text => "text",
            Tool::Step => "step",
            Tool::Blur => "blur",
            Tool::Pixelate => "pixelate",
            Tool::Redact => "redact",
            Tool::Magnifier => "magnifier",
            Tool::Callout => "callout",
            Tool::Spotlight => "spotlight",
            Tool::Measure => "measure",
            Tool::SelectText => "select_text",
        }
    }

    pub const ALL: [Tool; 17] = [
        Tool::Select,
        Tool::Arrow,
        Tool::Line,
        Tool::Rect,
        Tool::Ellipse,
        Tool::Pen,
        Tool::Highlighter,
        Tool::Text,
        Tool::Step,
        Tool::Blur,
        Tool::Pixelate,
        Tool::Redact,
        Tool::Magnifier,
        Tool::Callout,
        Tool::Spotlight,
        Tool::Measure,
        Tool::SelectText,
    ];

    pub fn from_config_key(k: &str) -> Option<Tool> {
        Tool::ALL.into_iter().find(|t| t.config_key() == k)
    }

    /// Tools whose color / width the editor remembers separately.
    pub fn has_style(self) -> bool {
        !matches!(
            self,
            Tool::Select
                | Tool::Blur
                | Tool::Pixelate
                | Tool::Spotlight
                | Tool::Magnifier
                | Tool::Measure
                | Tool::SelectText
        )
    }
}

/// Handle identity on a selected shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShapeHandle {
    /// One of 8 handles on the shape's main rectangle (or bounding box for strokes).
    Rect(Handle8),
    /// Magnifier source rectangle handles.
    Src(Handle8),
    /// Arrow/line start and end points.
    P1,
    P2,
    /// Arrow curve handle (the point the curve passes through at its middle).
    Mid,
    /// Callout tail point.
    Tail,
}

fn pt(x: f32, y: f32) -> PtF {
    PtF::new(x, y)
}

pub fn dist(a: PtF, b: PtF) -> f32 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

/// Distance from `p` to the segment `a`-`b`.
pub fn dist_to_segment(p: PtF, a: PtF, b: PtF) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    if len2 <= f32::EPSILON {
        return dist(p, a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0);
    dist(p, pt(a.x + t * dx, a.y + t * dy))
}

fn dist_to_polyline(p: PtF, pts: &[PtF]) -> f32 {
    match pts.len() {
        0 => f32::INFINITY,
        1 => dist(p, pts[0]),
        _ => pts
            .windows(2)
            .map(|s| dist_to_segment(p, s[0], s[1]))
            .fold(f32::INFINITY, f32::min),
    }
}

pub fn union(a: RectF, b: RectF) -> RectF {
    let x0 = a.x.min(b.x);
    let y0 = a.y.min(b.y);
    let x1 = (a.x + a.w).max(b.x + b.w);
    let y1 = (a.y + a.h).max(b.y + b.h);
    RectF::new(x0, y0, x1 - x0, y1 - y0)
}

pub fn intersects(a: RectF, b: RectF) -> bool {
    a.x <= b.x + b.w && b.x <= a.x + a.w && a.y <= b.y + b.h && b.y <= a.y + a.h
}

fn points_bbox(points: &[PtF]) -> RectF {
    let mut it = points.iter();
    let Some(f) = it.next() else {
        return RectF::default();
    };
    let (mut x0, mut y0, mut x1, mut y1) = (f.x, f.y, f.x, f.y);
    for p in it {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    RectF::new(x0, y0, x1 - x0, y1 - y0)
}

pub fn step_radius(width: f32) -> f32 {
    9.0 + width * 1.5
}

/// Rect covered by a text block anchored at `pos` (without its pill). With a fixed box
/// `width` the lines wrap to it (`annotate::text_line_ranges`, the renderer's breaks) and
/// the rect is the box: that width (or the widest unbreakable line) by the wrapped lines.
pub fn text_rect(
    pos: PtF,
    text: &str,
    size: f32,
    bold: bool,
    width: Option<f32>,
    measure: Measure,
) -> RectF {
    if let Some(bw) = width {
        let lw = |s: &str| measure(s, size, bold).0;
        let ranges = annotate::text_line_ranges(text, width, lw);
        let widest = ranges
            .iter()
            .map(|r| lw(&text[r.clone()]))
            .fold(0.0, f32::max);
        let line_h = measure("M", size, bold).1.max(size);
        return RectF::new(
            pos.x,
            pos.y,
            bw.max(widest).max(4.0),
            line_h * ranges.len().max(1) as f32,
        );
    }
    let probe = if text.is_empty() { "M" } else { text };
    let (w, h) = measure(probe, size, bold);
    let w = if text.is_empty() { size * 0.6 } else { w };
    RectF::new(pos.x, pos.y, w.max(4.0), h.max(size))
}

/// Narrowest fixed text box, image px (a drag narrower than this is a click).
pub const MIN_TEXT_BOX: f32 = 16.0;

/// A text box from a Text tool drag from `a` to `b` (image px): the box starts at the
/// left of the drag, at the top, and is as wide as the drag. None when the drag is
/// narrower than `min_w` (a click: the text sizes itself).
pub fn text_box_from_drag(a: PtF, b: PtF, min_w: f32) -> Option<(PtF, f32)> {
    let w = (b.x - a.x).abs();
    (w >= min_w.max(MIN_TEXT_BOX)).then(|| (pt(a.x.min(b.x), a.y.min(b.y)), w))
}

/// Axis-aligned visual bounds including stroke (image px).
pub fn bounds(shape: &Shape, measure: Measure) -> RectF {
    let w = shape.style.width;
    match &shape.kind {
        ShapeKind::Arrow {
            from,
            to,
            ctrl,
            double,
            head,
        } => {
            let mut r = points_bbox(&arrow_centerline(*from, *to, *ctrl)).expand(w / 2.0);
            if let Some(parts) = arrow_parts(*from, *to, *ctrl, w, *double, *head) {
                for g in &parts.heads {
                    r = union(r, points_bbox(&[g.tip, g.left, g.right]).expand(w / 4.0));
                }
            }
            r
        }
        ShapeKind::Line { from, to } => RectF::from_points(*from, *to).expand(w / 2.0),
        ShapeKind::Rect { rect, .. } | ShapeKind::Ellipse { rect } => rect.expand(w / 2.0),
        ShapeKind::Pen { points } | ShapeKind::Highlighter { points } => {
            points_bbox(points).expand(w / 2.0)
        }
        ShapeKind::Text {
            pos,
            text,
            size,
            background,
            bold,
            width,
        } => {
            let r = text_rect(*pos, text, *size, *bold, *width, measure);
            if background.is_some() {
                RectF::new(
                    r.x - size * TEXT_PAD_X,
                    r.y - size * TEXT_PAD_Y,
                    r.w + 2.0 * size * TEXT_PAD_X,
                    r.h + 2.0 * size * TEXT_PAD_Y,
                )
            } else {
                r
            }
        }
        ShapeKind::Step { center, radius, .. } => RectF::new(
            center.x - radius,
            center.y - radius,
            radius * 2.0,
            radius * 2.0,
        ),
        ShapeKind::Blur { rect, .. }
        | ShapeKind::Pixelate { rect, .. }
        | ShapeKind::Redact { rect }
        | ShapeKind::HighlightBox { rect } => *rect,
        ShapeKind::Magnifier { source, dest, .. } => union(*source, *dest),
        ShapeKind::Callout { rect, tail, .. } => union(*rect, RectF::new(tail.x, tail.y, 0.0, 0.0)),
        ShapeKind::Spotlight { rect, .. } | ShapeKind::Cursor { rect, .. } => *rect,
    }
}

/// Bounds of several shapes together (None when none of `ids` exists).
pub fn group_bounds(doc: &Doc, ids: &[ShapeId], measure: Measure) -> Option<RectF> {
    doc.shapes
        .iter()
        .filter(|s| ids.contains(&s.id))
        .map(|s| bounds(s, measure))
        .reduce(union)
}

fn near_rect_border(r: RectF, p: PtF, reach: f32) -> bool {
    let outer = r.expand(reach);
    if !outer.contains(p) {
        return false;
    }
    let inner = RectF::new(
        r.x + reach,
        r.y + reach,
        (r.w - 2.0 * reach).max(0.0),
        (r.h - 2.0 * reach).max(0.0),
    );
    !(inner.w > 0.0 && inner.h > 0.0 && inner.contains(p))
}

fn in_ellipse(r: RectF, p: PtF, grow: f32) -> bool {
    let a = r.w / 2.0 + grow;
    let b = r.h / 2.0 + grow;
    if a <= 0.0 || b <= 0.0 {
        return false;
    }
    let c = r.center();
    ((p.x - c.x) / a).powi(2) + ((p.y - c.y) / b).powi(2) <= 1.0
}

/// Does the visual footprint of `shape` contain `p`? `tol` is extra slack in image px.
pub fn hit(shape: &Shape, p: PtF, tol: f32, measure: Measure) -> bool {
    let w = shape.style.width;
    let filled = shape.style.fill.is_some();
    match &shape.kind {
        ShapeKind::Arrow { from, to, ctrl, .. } => {
            dist_to_polyline(p, &arrow_centerline(*from, *to, *ctrl)) <= w / 2.0 + tol + 1.0
                || dist(p, *to) <= w * 1.5 + tol + 4.0
        }
        ShapeKind::Line { from, to } => dist_to_segment(p, *from, *to) <= w / 2.0 + tol + 1.0,
        ShapeKind::Rect { rect, .. } => {
            if filled {
                rect.expand(w / 2.0 + tol).contains(p)
            } else {
                near_rect_border(*rect, p, w / 2.0 + tol)
            }
        }
        ShapeKind::Ellipse { rect } => {
            let reach = w / 2.0 + tol;
            if filled {
                in_ellipse(*rect, p, reach)
            } else {
                in_ellipse(*rect, p, reach) && !in_ellipse(*rect, p, -reach)
            }
        }
        ShapeKind::Pen { points } | ShapeKind::Highlighter { points } => {
            dist_to_polyline(p, points) <= w / 2.0 + tol
        }
        ShapeKind::Text { .. } => bounds(shape, measure).expand(tol).contains(p),
        ShapeKind::Step { center, radius, .. } => dist(p, *center) <= radius + tol,
        ShapeKind::Blur { rect, .. }
        | ShapeKind::Pixelate { rect, .. }
        | ShapeKind::Redact { rect }
        | ShapeKind::HighlightBox { rect } => rect.expand(tol).contains(p),
        ShapeKind::Magnifier {
            source,
            dest,
            circular,
        } => {
            let in_dest = if *circular {
                in_ellipse(*dest, p, tol)
            } else {
                dest.expand(tol).contains(p)
            };
            in_dest || near_rect_border(*source, p, 2.0 + tol)
        }
        ShapeKind::Callout { rect, tail, .. } => {
            let root = pt(
                tail.x.clamp(rect.x, rect.x + rect.w),
                tail.y.clamp(rect.y, rect.y + rect.h),
            );
            rect.expand(tol).contains(p) || dist_to_segment(p, root, *tail) <= 8.0 + tol
        }
        ShapeKind::Spotlight { rect, .. } => near_rect_border(*rect, p, 4.0 + tol),
        ShapeKind::Cursor { rect, .. } => rect.expand(tol).contains(p),
    }
}

/// Topmost shape (last in z-order) hit at `p`.
pub fn hit_test(doc: &Doc, p: PtF, tol: f32, measure: Measure) -> Option<ShapeId> {
    doc.shapes
        .iter()
        .rev()
        .find(|s| hit(s, p, tol, measure))
        .map(|s| s.id)
}

/// Shapes whose bounds touch the marquee `r` (image px), in z-order.
pub fn shapes_in_rect(doc: &Doc, r: RectF, measure: Measure) -> Vec<ShapeId> {
    doc.shapes
        .iter()
        .filter(|s| intersects(bounds(s, measure), r))
        .map(|s| s.id)
        .collect()
}

pub fn translate(kind: &mut ShapeKind, dx: f32, dy: f32) {
    annotate::translate_shape(kind, dx, dy);
}

/// Main rectangle used for the 8 handles (not for arrows/lines/steps).
fn main_rect(shape: &Shape) -> Option<RectF> {
    match &shape.kind {
        ShapeKind::Rect { rect, .. }
        | ShapeKind::Ellipse { rect }
        | ShapeKind::Blur { rect, .. }
        | ShapeKind::Pixelate { rect, .. }
        | ShapeKind::Redact { rect }
        | ShapeKind::HighlightBox { rect }
        | ShapeKind::Spotlight { rect, .. }
        | ShapeKind::Callout { rect, .. } => Some(*rect),
        ShapeKind::Magnifier { dest, .. } => Some(*dest),
        ShapeKind::Pen { points } | ShapeKind::Highlighter { points } => Some(points_bbox(points)),
        _ => None,
    }
}

/// Handle anchor positions for a selected shape.
pub fn handles(shape: &Shape, measure: Measure) -> Vec<(ShapeHandle, PtF)> {
    let mut out = Vec::new();
    let rect_handles =
        |r: RectF, mk: fn(Handle8) -> ShapeHandle, out: &mut Vec<(ShapeHandle, PtF)>| {
            for h in Handle8::ALL {
                let (x, y) = super::selection::handle_pos((r.x, r.y, r.w, r.h), h);
                out.push((mk(h), pt(x, y)));
            }
        };
    match &shape.kind {
        ShapeKind::Arrow { from, to, ctrl, .. } => {
            out.push((ShapeHandle::P1, *from));
            out.push((ShapeHandle::P2, *to));
            out.push((ShapeHandle::Mid, arrow_mid(*from, *to, *ctrl)));
        }
        ShapeKind::Line { from, to } => {
            out.push((ShapeHandle::P1, *from));
            out.push((ShapeHandle::P2, *to));
        }
        ShapeKind::Text { .. } => {
            // Corner: font size. Side edges: box width (a drag fixes it and wraps).
            let b = bounds(shape, measure);
            out.push((
                ShapeHandle::Rect(Handle8::BottomRight),
                pt(b.x + b.w, b.y + b.h),
            ));
            out.push((
                ShapeHandle::Rect(Handle8::Right),
                pt(b.x + b.w, b.y + b.h / 2.0),
            ));
            out.push((ShapeHandle::Rect(Handle8::Left), pt(b.x, b.y + b.h / 2.0)));
        }
        ShapeKind::Step { center, radius, .. } => {
            out.push((
                ShapeHandle::Rect(Handle8::Right),
                pt(center.x + radius, center.y),
            ));
        }
        // Corners only: the pointer keeps its aspect ratio.
        ShapeKind::Cursor { rect, .. } => {
            for h in [
                Handle8::TopLeft,
                Handle8::TopRight,
                Handle8::BottomRight,
                Handle8::BottomLeft,
            ] {
                let (x, y) = super::selection::handle_pos((rect.x, rect.y, rect.w, rect.h), h);
                out.push((ShapeHandle::Rect(h), pt(x, y)));
            }
        }
        ShapeKind::Callout { tail, .. } => {
            if let Some(r) = main_rect(shape) {
                rect_handles(r, ShapeHandle::Rect, &mut out);
            }
            out.push((ShapeHandle::Tail, *tail));
        }
        ShapeKind::Magnifier { source, .. } => {
            if let Some(r) = main_rect(shape) {
                rect_handles(r, ShapeHandle::Rect, &mut out);
            }
            rect_handles(*source, ShapeHandle::Src, &mut out);
        }
        _ => {
            if let Some(r) = main_rect(shape) {
                rect_handles(r, ShapeHandle::Rect, &mut out);
            }
        }
    }
    out
}

/// Handle (if any) of `shape` within `radius` image px of `p`. Later handles win ties so
/// that Src/Tail/Mid handles are reachable over overlapping rect handles.
pub fn hit_handle(shape: &Shape, p: PtF, radius: f32, measure: Measure) -> Option<ShapeHandle> {
    let mut best: Option<(ShapeHandle, f32)> = None;
    for (h, pos) in handles(shape, measure) {
        let d = dist(p, pos);
        if d <= radius && best.is_none_or(|(_, bd)| d <= bd) {
            best = Some((h, d));
        }
    }
    best.map(|(h, _)| h)
}

/// Resize a float rect by dragging `h` to `p`; flips when crossing; `square` forces 1:1
/// (corners: the larger side wins; edges: the other side follows, centred).
pub fn resize_frect(r: RectF, h: Handle8, p: PtF, square: bool) -> RectF {
    let hd = h.h_dir();
    let vd = h.v_dir();
    let fx = if hd < 0 { r.x + r.w } else { r.x };
    let fy = if vd < 0 { r.y + r.h } else { r.y };
    let (mut x0, mut x1) = if hd == 0 {
        (r.x, r.x + r.w)
    } else {
        (fx.min(p.x), fx.max(p.x))
    };
    let (mut y0, mut y1) = if vd == 0 {
        (r.y, r.y + r.h)
    } else {
        (fy.min(p.y), fy.max(p.y))
    };
    if square && hd != 0 && vd != 0 {
        let s = (p.x - fx).abs().max((p.y - fy).abs());
        let sx = if p.x >= fx { 1.0 } else { -1.0 };
        let sy = if p.y >= fy { 1.0 } else { -1.0 };
        x0 = fx.min(fx + sx * s);
        x1 = fx.max(fx + sx * s);
        y0 = fy.min(fy + sy * s);
        y1 = fy.max(fy + sy * s);
    } else if square && hd != 0 {
        let s = x1 - x0;
        let cy = r.y + r.h / 2.0;
        (y0, y1) = (cy - s / 2.0, cy + s / 2.0);
    } else if square && vd != 0 {
        let s = y1 - y0;
        let cx = r.x + r.w / 2.0;
        (x0, x1) = (cx - s / 2.0, cx + s / 2.0);
    }
    RectF::new(x0, y0, x1 - x0, y1 - y0)
}

fn scale_points(points: &mut [PtF], from: RectF, to: RectF) {
    let sx = if from.w.abs() > 1e-3 {
        to.w / from.w
    } else {
        1.0
    };
    let sy = if from.h.abs() > 1e-3 {
        to.h / from.h
    } else {
        1.0
    };
    for p in points {
        p.x = to.x + (p.x - from.x) * sx;
        p.y = to.y + (p.y - from.y) * sy;
    }
}

/// The curve handle constrained to the chord's perpendicular bisector (a symmetric bend).
fn on_bisector(from: PtF, to: PtF, p: PtF) -> PtF {
    let m = pt((from.x + to.x) / 2.0, (from.y + to.y) / 2.0);
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let l = (dx * dx + dy * dy).sqrt();
    if l < 1e-3 {
        return p;
    }
    let (nx, ny) = (-dy / l, dx / l);
    let t = (p.x - m.x) * nx + (p.y - m.y) * ny;
    pt(m.x + nx * t, m.y + ny * t)
}

/// Apply a handle drag to `shape`. `p` is the pointer in image px; `constrain` (Shift)
/// keeps 45 degree angles for lines and arrows, squares and circles for rect-like shapes,
/// and a symmetric bend for the arrow curve handle.
pub fn drag_handle(
    shape: &mut Shape,
    handle: ShapeHandle,
    p: PtF,
    constrain: bool,
    measure: Measure,
) {
    let old_main = main_rect(shape);
    let text_box = bounds(shape, measure);
    let text_h = text_box.h;
    match (&mut shape.kind, handle) {
        (ShapeKind::Arrow { from, to, ctrl, .. }, ShapeHandle::P1 | ShapeHandle::P2) => {
            let (old_from, old_to) = (*from, *to);
            if handle == ShapeHandle::P1 {
                *from = if constrain {
                    constrain_angle(*to, p)
                } else {
                    p
                };
            } else {
                *to = if constrain {
                    constrain_angle(*from, p)
                } else {
                    p
                };
            }
            if let Some(c) = ctrl {
                *c = follow_ctrl(old_from, old_to, *from, *to, *c);
            }
        }
        (ShapeKind::Arrow { from, to, ctrl, .. }, ShapeHandle::Mid) => {
            let p = if constrain {
                on_bisector(*from, *to, p)
            } else {
                p
            };
            let chord_mid = pt((from.x + to.x) / 2.0, (from.y + to.y) / 2.0);
            *ctrl = (dist(p, chord_mid) > STRAIGHTEN_PX).then(|| ctrl_through(*from, *to, p));
        }
        (ShapeKind::Line { from, to }, ShapeHandle::P1) => {
            *from = if constrain {
                constrain_angle(*to, p)
            } else {
                p
            };
        }
        (ShapeKind::Line { from, to }, ShapeHandle::P2) => {
            *to = if constrain {
                constrain_angle(*from, p)
            } else {
                p
            };
        }
        (ShapeKind::Callout { tail, .. }, ShapeHandle::Tail) => *tail = p,
        (ShapeKind::Cursor { rect, image }, ShapeHandle::Rect(h)) => {
            *rect = resize_cursor(*rect, h, p, image.width() as f32, image.height() as f32);
        }
        (ShapeKind::Step { center, radius, .. }, ShapeHandle::Rect(_)) => {
            *radius = dist(*center, p).clamp(6.0, 120.0);
        }
        (
            ShapeKind::Text {
                pos,
                text,
                size,
                bold,
                width,
                background,
            },
            ShapeHandle::Rect(h @ (Handle8::Left | Handle8::Right)),
        ) => {
            let pad = if background.is_some() {
                *size * TEXT_PAD_X
            } else {
                0.0
            };
            let cur = text_rect(*pos, text, *size, *bold, *width, measure);
            let right = cur.x + cur.w;
            if h == Handle8::Right {
                *width = Some((p.x - pad - pos.x).max(MIN_TEXT_BOX));
            } else {
                let x = (p.x + pad).min(right - MIN_TEXT_BOX);
                *width = Some(right - x);
                pos.x = x;
            }
        }
        (ShapeKind::Text { size, .. }, ShapeHandle::Rect(_)) if text_h > 1.0 => {
            // The handle sits on the padded box, so measure from the box's top: a press
            // without movement leaves the size alone.
            let new_h = (p.y - text_box.y).max(4.0);
            *size = (*size * new_h / text_h).clamp(8.0, 400.0);
        }
        (ShapeKind::Magnifier { source, .. }, ShapeHandle::Src(h)) => {
            *source = min_size(resize_frect(*source, h, p, constrain));
        }
        (ShapeKind::Pen { points } | ShapeKind::Highlighter { points }, ShapeHandle::Rect(h)) => {
            if let Some(old) = old_main {
                let new = resize_frect(old, h, p, constrain);
                scale_points(points, old, new);
            }
        }
        (
            ShapeKind::Rect { rect, .. }
            | ShapeKind::Ellipse { rect }
            | ShapeKind::Blur { rect, .. }
            | ShapeKind::Pixelate { rect, .. }
            | ShapeKind::Redact { rect }
            | ShapeKind::HighlightBox { rect }
            | ShapeKind::Spotlight { rect, .. }
            | ShapeKind::Callout { rect, .. }
            | ShapeKind::Magnifier { dest: rect, .. },
            ShapeHandle::Rect(h),
        ) => {
            *rect = min_size(resize_frect(*rect, h, p, constrain));
        }
        _ => {}
    }
}

/// Resize a pointer by dragging corner `h` to `p`: the opposite corner stays, the aspect
/// ratio of its `img_w` x `img_h` bitmap is kept, and the scale stays within 1/4x..8x.
pub fn resize_cursor(r: RectF, h: Handle8, p: PtF, img_w: f32, img_h: f32) -> RectF {
    let (hd, vd) = (h.h_dir(), h.v_dir());
    if hd == 0 || vd == 0 || img_w < 1.0 || img_h < 1.0 {
        return r;
    }
    let ax = if hd < 0 { r.x + r.w } else { r.x };
    let ay = if vd < 0 { r.y + r.h } else { r.y };
    let sx = ((p.x - ax) * hd as f32).max(0.0) / img_w;
    let sy = ((p.y - ay) * vd as f32).max(0.0) / img_h;
    let s = sx.max(sy).clamp(0.25, 8.0);
    let (w, h) = (img_w * s, img_h * s);
    RectF::new(
        if hd < 0 { ax - w } else { ax },
        if vd < 0 { ay - h } else { ay },
        w,
        h,
    )
}

fn min_size(mut r: RectF) -> RectF {
    if r.w < MIN_SHAPE {
        r.w = MIN_SHAPE;
    }
    if r.h < MIN_SHAPE {
        r.h = MIN_SHAPE;
    }
    r
}

/// Snap the segment from `anchor` to `p` to the nearest multiple of 45 degrees.
pub fn constrain_angle(anchor: PtF, p: PtF) -> PtF {
    let (dx, dy) = (p.x - anchor.x, p.y - anchor.y);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-3 {
        return p;
    }
    let step = std::f32::consts::FRAC_PI_4;
    let a = (dy.atan2(dx) / step).round() * step;
    pt(anchor.x + len * a.cos(), anchor.y + len * a.sin())
}

/// Shift while moving: keep the drag on the dominant axis.
pub fn constrain_axis(d: (f32, f32)) -> (f32, f32) {
    if d.0.abs() >= d.1.abs() {
        (d.0, 0.0)
    } else {
        (0.0, d.1)
    }
}

/// Smooth a freehand stroke: drop near-duplicate points then one Chaikin corner-cut pass.
pub fn smooth_points(points: &[PtF]) -> Vec<PtF> {
    let mut dedup: Vec<PtF> = Vec::with_capacity(points.len());
    for p in points {
        if dedup.last().is_none_or(|l| dist(*l, *p) >= 1.0) {
            dedup.push(*p);
        }
    }
    if dedup.len() < 3 {
        return dedup;
    }
    let mut out = Vec::with_capacity(dedup.len() * 2);
    out.push(dedup[0]);
    for w in dedup.windows(2) {
        let (a, b) = (w[0], w[1]);
        out.push(pt(a.x * 0.75 + b.x * 0.25, a.y * 0.75 + b.y * 0.25));
        out.push(pt(a.x * 0.25 + b.x * 0.75, a.y * 0.25 + b.y * 0.75));
    }
    out.push(dedup[dedup.len() - 1]);
    out
}

/// Sticky options for new arrows and text (the last ones picked on a shape).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeOptions {
    pub arrow_double: bool,
    pub arrow_head: ArrowHead,
    pub text_bold: bool,
    pub text_pill: bool,
    /// Highlighter draws boxes (Shift while dragging draws the other mode). Saved in the
    /// config as `editor.highlighter_box`.
    pub highlight_box: bool,
}

impl Default for ShapeOptions {
    fn default() -> Self {
        Self {
            arrow_double: false,
            arrow_head: ArrowHead::Notched,
            text_bold: false,
            text_pill: false,
            highlight_box: false,
        }
    }
}

/// Editor-wide creation parameters taken from the config.
#[derive(Clone, Debug)]
pub struct ToolParams {
    pub style: Style,
    pub font_size: f32,
    pub blur_radius: f32,
    pub pixelate_block: u32,
    pub options: ShapeOptions,
}

/// Black-ish or white ink for text on `bg` (the threshold core's renderer uses).
pub fn contrast_ink(bg: [u8; 4]) -> [u8; 4] {
    let lum = 0.299 * bg[0] as f32 + 0.587 * bg[1] as f32 + 0.114 * bg[2] as f32;
    if lum > 150.0 {
        [20, 20, 22, 255]
    } else {
        [255, 255, 255, 255]
    }
}

/// Style used for a freshly created shape of `tool` given the current color/width.
pub fn style_for_tool(tool: Tool, base: &Style) -> Style {
    let mut s = base.clone();
    match tool {
        Tool::Highlighter => {
            s.color[3] = HIGHLIGHTER_ALPHA;
            s.width = (base.width * 4.0).max(14.0);
            s.shadow = false;
        }
        Tool::Callout => {
            s.fill = Some([s.color[0], s.color[1], s.color[2], 255]);
            s.color[3] = 255;
        }
        Tool::Blur | Tool::Pixelate | Tool::Redact | Tool::Spotlight | Tool::Magnifier => {
            s.shadow = false;
        }
        _ => {}
    }
    s
}

/// A new text shape at `pos` with the sticky options: with the pill on, the pill takes the
/// color and the text gets black or white ink on it.
pub fn new_text(pos: PtF, params: &ToolParams) -> (ShapeKind, Style) {
    let mut style = style_for_tool(Tool::Text, &params.style);
    let c = style.color;
    let background = params.options.text_pill.then(|| {
        style.color = contrast_ink(c);
        [c[0], c[1], c[2], 255]
    });
    (
        ShapeKind::Text {
            pos,
            text: String::new(),
            size: params.font_size,
            background,
            bold: params.options.text_bold,
            width: None,
        },
        style,
    )
}

/// Build a shape from a completed drag from `a` to `b` (image px). `None` when the drag is
/// too small to be meaningful for a drag-created tool.
pub fn shape_from_drag(
    tool: Tool,
    a: PtF,
    b: PtF,
    shift: bool,
    params: &ToolParams,
) -> Option<(ShapeKind, Style)> {
    let style = style_for_tool(tool, &params.style);
    let mut b = b;
    let small = (b.x - a.x).abs() < MIN_SHAPE && (b.y - a.y).abs() < MIN_SHAPE;
    let rect = {
        let mut r = RectF::from_points(a, b);
        if shift {
            let s = r.w.max(r.h);
            r = RectF::from_points(
                a,
                pt(
                    a.x + s * (b.x - a.x).signum(),
                    a.y + s * (b.y - a.y).signum(),
                ),
            );
        }
        r
    };
    let kind = match tool {
        Tool::Arrow | Tool::Line => {
            if small {
                return None;
            }
            if shift {
                b = constrain_angle(a, b);
            }
            if tool == Tool::Arrow {
                ShapeKind::Arrow {
                    from: a,
                    to: b,
                    ctrl: None,
                    double: params.options.arrow_double,
                    head: params.options.arrow_head,
                }
            } else {
                ShapeKind::Line { from: a, to: b }
            }
        }
        Tool::Rect => ShapeKind::Rect {
            rect: sized(rect, small)?,
            corner_radius: 3.0,
        },
        Tool::Ellipse => ShapeKind::Ellipse {
            rect: sized(rect, small)?,
        },
        Tool::Blur => ShapeKind::Blur {
            rect: sized(rect, small)?,
            radius: params.blur_radius.max(1.0),
        },
        Tool::Pixelate => ShapeKind::Pixelate {
            rect: sized(rect, small)?,
            block: params.pixelate_block.max(2),
        },
        Tool::Redact => ShapeKind::Redact {
            rect: sized(rect, small)?,
        },
        Tool::Spotlight => ShapeKind::Spotlight {
            rect: sized(rect, small)?,
            dim: 0.6,
        },
        Tool::Callout => {
            let r = if small {
                RectF::new(a.x, a.y, 160.0, 56.0)
            } else {
                rect
            };
            let r = min_size(r);
            let tail = pt(r.x + r.w * 0.25, r.y + r.h + (r.h * 0.6).max(16.0));
            ShapeKind::Callout {
                rect: r,
                tail,
                text: String::new(),
                size: params.font_size,
            }
        }
        Tool::Select
        | Tool::Pen
        | Tool::Highlighter
        | Tool::Text
        | Tool::Step
        | Tool::Magnifier
        | Tool::Measure
        | Tool::SelectText => return None,
    };
    Some((kind, style))
}

fn sized(r: RectF, small: bool) -> Option<RectF> {
    if small || r.w < MIN_SHAPE || r.h < MIN_SHAPE {
        None
    } else {
        Some(r)
    }
}

/// Does a highlighter drag draw a box? The sticky mode, flipped while Shift is held.
pub fn highlight_box_mode(options: &ShapeOptions, shift: bool) -> bool {
    options.highlight_box != shift
}

/// A box highlight from a drag from `a` to `b` (image px), styled like a highlighter stroke.
/// None when the drag is too small.
pub fn highlight_box_from_drag(a: PtF, b: PtF, params: &ToolParams) -> Option<(ShapeKind, Style)> {
    let r = RectF::from_points(a, b);
    if r.w < MIN_SHAPE || r.h < MIN_SHAPE {
        return None;
    }
    Some((
        ShapeKind::HighlightBox { rect: r },
        style_for_tool(Tool::Highlighter, &params.style),
    ))
}

/// To Box: the box highlight covering a freehand highlighter stroke (its painted extent,
/// stroke width included). None for anything else.
pub fn highlighter_to_box(shape: &Shape) -> Option<ShapeKind> {
    annotate::highlighter_box_rect(&shape.kind, shape.style.width)
        .map(|r| ShapeKind::HighlightBox { rect: min_size(r) })
}

/// Freehand highlighter stroke or box highlight.
pub fn is_highlight(kind: &ShapeKind) -> bool {
    matches!(
        kind,
        ShapeKind::Highlighter { .. } | ShapeKind::HighlightBox { .. }
    )
}

/// Recolor a shape. Highlighters keep their translucency; callouts recolor their fill too;
/// text on a pill recolors the pill and picks readable ink.
pub fn recolor(shape: &mut Shape, color: [u8; 4]) {
    if matches!(shape.kind, ShapeKind::Cursor { .. }) {
        return;
    }
    let mut c = color;
    if is_highlight(&shape.kind) {
        c[3] = HIGHLIGHTER_ALPHA;
    }
    if let ShapeKind::Text {
        background: Some(bg),
        ..
    } = &mut shape.kind
    {
        *bg = [c[0], c[1], c[2], 255];
        shape.style.color = contrast_ink(*bg);
        return;
    }
    shape.style.color = c;
    if shape.style.fill.is_some() {
        shape.style.fill = Some([c[0], c[1], c[2], 255]);
    }
}

/// The color the toolbar swatch shows for a shape (the pill of pill text, a callout's fill).
pub fn display_color(shape: &Shape) -> [u8; 4] {
    let c = match (&shape.kind, shape.style.fill) {
        (
            ShapeKind::Text {
                background: Some(bg),
                ..
            },
            _,
        ) => *bg,
        (ShapeKind::Callout { .. }, Some(f)) => f,
        _ => shape.style.color,
    };
    [c[0], c[1], c[2], 255]
}

/// The stroke width the toolbar shows for a shape (highlighters store 4x the setting).
pub fn display_width(shape: &Shape) -> f32 {
    match shape.kind {
        ShapeKind::Highlighter { .. } | ShapeKind::HighlightBox { .. } => {
            (shape.style.width / 4.0).clamp(1.0, 64.0)
        }
        _ => shape.style.width.clamp(1.0, 64.0),
    }
}

/// Change the "size" of a shape by `delta` (stroke width, text size, step radius, blur
/// strength...).
pub fn adjust_size(shape: &mut Shape, delta: f32) {
    match &mut shape.kind {
        ShapeKind::Text { size, .. } | ShapeKind::Callout { size, .. } => {
            *size = (*size + delta * 2.0).clamp(8.0, 400.0);
        }
        ShapeKind::Step { radius, .. } => {
            *radius = (*radius + delta * 1.5).clamp(6.0, 120.0);
            shape.style.width = (shape.style.width + delta).clamp(1.0, 64.0);
        }
        ShapeKind::Blur { radius, .. } => *radius = (*radius + delta * 2.0).clamp(1.0, 200.0),
        ShapeKind::Pixelate { block, .. } => {
            *block = ((*block as f32 + delta * 2.0).round().max(2.0) as u32).min(200);
        }
        ShapeKind::Spotlight { dim, .. } => *dim = (*dim + delta * 0.05).clamp(0.1, 0.95),
        ShapeKind::Cursor { .. } => {}
        _ => shape.style.width = (shape.style.width + delta).clamp(1.0, 64.0),
    }
}

/// Next text size up (`up`) or down from `size` in `TEXT_SIZES`, stopping at the ends.
pub fn step_text_size(size: f32, up: bool) -> f32 {
    if up {
        TEXT_SIZES
            .iter()
            .copied()
            .find(|s| *s > size + 0.5)
            .unwrap_or(size.max(TEXT_SIZES[TEXT_SIZES.len() - 1]))
    } else {
        TEXT_SIZES
            .iter()
            .rev()
            .copied()
            .find(|s| *s < size - 0.5)
            .unwrap_or(size.min(TEXT_SIZES[0]))
    }
}

/// Text size buttons on a text or callout shape.
pub fn bump_text_size(kind: &mut ShapeKind, up: bool) -> bool {
    match kind {
        ShapeKind::Text { size, .. } | ShapeKind::Callout { size, .. } => {
            let n = step_text_size(*size, up);
            let changed = (n - *size).abs() > 0.01;
            *size = n;
            changed
        }
        _ => false,
    }
}

pub fn set_bold(kind: &mut ShapeKind, on: bool) {
    if let ShapeKind::Text { bold, .. } = kind {
        *bold = on;
    }
}

/// Pill on: the pill takes the text's color and the text gets black or white ink. Off: the
/// text takes the pill's color back.
pub fn set_pill(shape: &mut Shape, on: bool) {
    if let ShapeKind::Text { background, .. } = &mut shape.kind {
        match (*background, on) {
            (None, true) => {
                let c = shape.style.color;
                *background = Some([c[0], c[1], c[2], 255]);
                shape.style.color = contrast_ink(c);
            }
            (Some(bg), false) => {
                *background = None;
                shape.style.color = bg;
            }
            _ => {}
        }
    }
}

pub fn set_arrow_double(kind: &mut ShapeKind, on: bool) {
    if let ShapeKind::Arrow { double, .. } = kind {
        *double = on;
    }
}

pub fn set_arrow_head(kind: &mut ShapeKind, style: ArrowHead) {
    if let ShapeKind::Arrow { head, .. } = kind {
        *head = style;
    }
}

pub fn straighten(kind: &mut ShapeKind) {
    if let ShapeKind::Arrow { ctrl, .. } = kind {
        *ctrl = None;
    }
}

/// Is this a shape whose text content can be edited inline?
pub fn is_textual(kind: &ShapeKind) -> bool {
    matches!(kind, ShapeKind::Text { .. } | ShapeKind::Callout { .. })
}

pub fn text_of(kind: &ShapeKind) -> Option<&str> {
    match kind {
        ShapeKind::Text { text, .. } | ShapeKind::Callout { text, .. } => Some(text),
        _ => None,
    }
}

pub fn set_text(kind: &mut ShapeKind, new: &str) {
    if let ShapeKind::Text { text, .. } | ShapeKind::Callout { text, .. } = kind {
        *text = new.to_string();
    }
}

/// Copy `doc` keeping only shapes not in `skip` (used so shapes being manipulated are not
/// baked into the cached render while they are painted live).
pub fn doc_without(doc: &Doc, skip: &[ShapeId]) -> Doc {
    let mut d = doc.clone();
    if !skip.is_empty() {
        d.shapes.retain(|s| !skip.contains(&s.id));
    }
    d
}

/// Translate every shape (selection origin changed while annotations exist).
pub fn translate_all(doc: &mut Doc, dx: f32, dy: f32) {
    translate_shapes(&mut doc.shapes, dx, dy);
}

/// Move shapes with the whole picture (the region or crop origin moved): the magnifier's
/// source spot moves too. Used for the document, its history and the shape clipboard.
pub fn translate_shapes(shapes: &mut [Shape], dx: f32, dy: f32) {
    for s in shapes {
        translate(&mut s.kind, dx, dy);
        if let ShapeKind::Magnifier { source, .. } = &mut s.kind {
            // The whole picture moves, so the magnified spot moves with it.
            source.x += dx;
            source.y += dy;
        }
    }
}

// ------------------------------------------------------------------- z-order and copies

/// Ctrl+]: each selected shape swaps with the unselected shape just above it.
pub fn bring_forward(doc: &mut Doc, ids: &[ShapeId]) -> bool {
    let mut changed = false;
    let n = doc.shapes.len();
    for i in (0..n.saturating_sub(1)).rev() {
        if ids.contains(&doc.shapes[i].id) && !ids.contains(&doc.shapes[i + 1].id) {
            doc.shapes.swap(i, i + 1);
            changed = true;
        }
    }
    changed
}

/// Ctrl+[: each selected shape swaps with the unselected shape just below it.
pub fn send_backward(doc: &mut Doc, ids: &[ShapeId]) -> bool {
    let mut changed = false;
    for i in 1..doc.shapes.len() {
        if ids.contains(&doc.shapes[i].id) && !ids.contains(&doc.shapes[i - 1].id) {
            doc.shapes.swap(i, i - 1);
            changed = true;
        }
    }
    changed
}

/// Ctrl+Shift+]: selected shapes on top, keeping their order.
pub fn bring_to_front(doc: &mut Doc, ids: &[ShapeId]) -> bool {
    let before: Vec<ShapeId> = doc.shapes.iter().map(|s| s.id).collect();
    let (sel, rest): (Vec<Shape>, Vec<Shape>) = std::mem::take(&mut doc.shapes)
        .into_iter()
        .partition(|s| ids.contains(&s.id));
    doc.shapes = rest.into_iter().chain(sel).collect();
    doc.shapes.iter().map(|s| s.id).ne(before)
}

/// Ctrl+Shift+[: selected shapes at the bottom, keeping their order.
pub fn send_to_back(doc: &mut Doc, ids: &[ShapeId]) -> bool {
    let before: Vec<ShapeId> = doc.shapes.iter().map(|s| s.id).collect();
    let (sel, rest): (Vec<Shape>, Vec<Shape>) = std::mem::take(&mut doc.shapes)
        .into_iter()
        .partition(|s| ids.contains(&s.id));
    doc.shapes = sel.into_iter().chain(rest).collect();
    doc.shapes.iter().map(|s| s.id).ne(before)
}

/// Add copies of `shapes` on top, moved by `offset`, with fresh ids (and fresh step
/// numbers). Returns the new ids in order.
pub fn add_copies(doc: &mut Doc, shapes: &[Shape], offset: (f32, f32)) -> Vec<ShapeId> {
    let mut out = Vec::with_capacity(shapes.len());
    for s in shapes {
        let mut kind = s.kind.clone();
        translate(&mut kind, offset.0, offset.1);
        if let ShapeKind::Step { n, .. } = &mut kind {
            *n = doc.next_step_number();
        }
        out.push(doc.add(kind, s.style.clone()));
    }
    out
}

/// The selected shapes in z-order (what Ctrl+C / Ctrl+D copy).
pub fn collect(doc: &Doc, ids: &[ShapeId]) -> Vec<Shape> {
    doc.shapes
        .iter()
        .filter(|s| ids.contains(&s.id))
        .cloned()
        .collect()
}

/// Paste offset: shift copies so they stay inside a `w` x `h` image (at least partly).
pub fn paste_offset(group: RectF, offset: (f32, f32), w: f32, h: f32) -> (f32, f32) {
    let mut dx = offset.0;
    let mut dy = offset.1;
    if group.x + dx > w - 8.0 {
        dx = w - 8.0 - group.x - group.w.min(w / 2.0);
    }
    if group.y + dy > h - 8.0 {
        dy = h - 8.0 - group.y - group.h.min(h / 2.0);
    }
    if group.x + group.w + dx < 8.0 {
        dx = 8.0 - group.x;
    }
    if group.y + group.h + dy < 8.0 {
        dy = 8.0 - group.y;
    }
    (dx, dy)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measure(text: &str, size: f32, _bold: bool) -> (f32, f32) {
        let lines: Vec<&str> = text.split('\n').collect();
        let w = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as f32 * size * 0.5;
        (w, lines.len() as f32 * size * 1.25)
    }

    fn style() -> Style {
        Style {
            color: [255, 0, 0, 255],
            width: 4.0,
            fill: None,
            shadow: true,
        }
    }

    fn doc_with(kinds: Vec<ShapeKind>) -> Doc {
        let mut d = Doc::new(400, 300);
        for k in kinds {
            d.add(k, style());
        }
        d
    }

    #[test]
    fn hit_arrow_line_by_distance() {
        let d = doc_with(vec![ShapeKind::Line {
            from: pt(10.0, 10.0),
            to: pt(110.0, 10.0),
        }]);
        assert!(hit_test(&d, pt(60.0, 13.0), 2.0, &measure).is_some());
        assert!(hit_test(&d, pt(60.0, 30.0), 2.0, &measure).is_none());
        assert!(hit_test(&d, pt(130.0, 10.0), 2.0, &measure).is_none());
    }

    #[test]
    fn hit_rect_outline_not_inside_unless_filled() {
        let r = RectF::new(50.0, 50.0, 100.0, 60.0);
        let d = doc_with(vec![ShapeKind::Rect {
            rect: r,
            corner_radius: 0.0,
        }]);
        assert!(hit_test(&d, pt(50.0, 80.0), 2.0, &measure).is_some());
        assert!(hit_test(&d, pt(100.0, 80.0), 2.0, &measure).is_none());
        let mut d = d;
        d.shapes[0].style.fill = Some([0, 0, 0, 255]);
        assert!(hit_test(&d, pt(100.0, 80.0), 2.0, &measure).is_some());
    }

    #[test]
    fn hit_ellipse_ring() {
        let d = doc_with(vec![ShapeKind::Ellipse {
            rect: RectF::new(0.0, 0.0, 100.0, 100.0),
        }]);
        assert!(hit_test(&d, pt(100.0, 50.0), 2.0, &measure).is_some());
        assert!(hit_test(&d, pt(50.0, 50.0), 2.0, &measure).is_none());
        assert!(hit_test(&d, pt(2.0, 2.0), 1.0, &measure).is_none());
    }

    #[test]
    fn topmost_wins() {
        let d = doc_with(vec![
            ShapeKind::Redact {
                rect: RectF::new(0.0, 0.0, 100.0, 100.0),
            },
            ShapeKind::Redact {
                rect: RectF::new(50.0, 50.0, 100.0, 100.0),
            },
        ]);
        assert_eq!(hit_test(&d, pt(60.0, 60.0), 1.0, &measure), Some(2));
        assert_eq!(hit_test(&d, pt(10.0, 10.0), 1.0, &measure), Some(1));
        assert_eq!(hit_test(&d, pt(300.0, 10.0), 1.0, &measure), None);
    }

    #[test]
    fn hit_text_step_pen() {
        let d = doc_with(vec![
            ShapeKind::Text {
                pos: pt(10.0, 10.0),
                text: "Hello".into(),
                size: 20.0,
                background: None,
                bold: false,
                width: None,
            },
            ShapeKind::Step {
                center: pt(200.0, 200.0),
                n: 1,
                radius: 15.0,
            },
            ShapeKind::Pen {
                points: vec![pt(300.0, 10.0), pt(300.0, 100.0)],
            },
        ]);
        assert_eq!(hit_test(&d, pt(30.0, 20.0), 1.0, &measure), Some(1));
        assert_eq!(hit_test(&d, pt(210.0, 205.0), 1.0, &measure), Some(2));
        assert_eq!(hit_test(&d, pt(302.0, 50.0), 1.0, &measure), Some(3));
    }

    #[test]
    fn translate_moves_everything() {
        let mut k = ShapeKind::Callout {
            rect: RectF::new(0.0, 0.0, 10.0, 10.0),
            tail: pt(5.0, 20.0),
            text: String::new(),
            size: 14.0,
        };
        translate(&mut k, 3.0, -2.0);
        match k {
            ShapeKind::Callout { rect, tail, .. } => {
                assert_eq!((rect.x, rect.y), (3.0, -2.0));
                assert_eq!((tail.x, tail.y), (8.0, 18.0));
            }
            _ => unreachable!(),
        }
        let mut k = ShapeKind::Magnifier {
            source: RectF::new(0.0, 0.0, 10.0, 10.0),
            dest: RectF::new(50.0, 50.0, 20.0, 20.0),
            circular: true,
        };
        translate(&mut k, 1.0, 1.0);
        match k {
            ShapeKind::Magnifier { source, dest, .. } => {
                assert_eq!(source.x, 0.0);
                assert_eq!(dest.x, 51.0);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn handles_per_kind() {
        let mk = |kind| Shape {
            id: 1,
            kind,
            style: style(),
        };
        let n = |k| handles(&mk(k), &measure).len();
        assert_eq!(
            n(ShapeKind::Arrow {
                from: pt(0.0, 0.0),
                to: pt(1.0, 1.0),
                ctrl: None,
                double: false,
                head: ArrowHead::Notched,
            }),
            3
        );
        assert_eq!(
            n(ShapeKind::Rect {
                rect: RectF::new(0.0, 0.0, 9.0, 9.0),
                corner_radius: 0.0
            }),
            8
        );
        assert_eq!(
            n(ShapeKind::Callout {
                rect: RectF::new(0.0, 0.0, 9.0, 9.0),
                tail: pt(0.0, 20.0),
                text: String::new(),
                size: 12.0
            }),
            9
        );
        assert_eq!(
            n(ShapeKind::Magnifier {
                source: RectF::new(0.0, 0.0, 9.0, 9.0),
                dest: RectF::new(20.0, 0.0, 9.0, 9.0),
                circular: true
            }),
            16
        );
        assert_eq!(
            n(ShapeKind::Step {
                center: pt(5.0, 5.0),
                n: 1,
                radius: 10.0
            }),
            1
        );
        assert_eq!(
            n(ShapeKind::Text {
                pos: pt(0.0, 0.0),
                text: "a".into(),
                size: 12.0,
                background: None,
                bold: false,
                width: None,
            }),
            3
        );
        assert_eq!(
            n(ShapeKind::Pen {
                points: vec![pt(0.0, 0.0), pt(5.0, 5.0)]
            }),
            8
        );
    }

    #[test]
    fn drag_handle_resizes_rect_and_flips() {
        let mut s = Shape {
            id: 1,
            kind: ShapeKind::Redact {
                rect: RectF::new(10.0, 10.0, 40.0, 40.0),
            },
            style: style(),
        };
        drag_handle(
            &mut s,
            ShapeHandle::Rect(Handle8::BottomRight),
            pt(100.0, 80.0),
            false,
            &measure,
        );
        assert!(
            matches!(s.kind, ShapeKind::Redact { rect } if rect == RectF::new(10.0, 10.0, 90.0, 70.0))
        );
        drag_handle(
            &mut s,
            ShapeHandle::Rect(Handle8::Left),
            pt(200.0, 0.0),
            false,
            &measure,
        );
        match s.kind {
            ShapeKind::Redact { rect } => {
                assert_eq!(rect.x, 100.0);
                assert_eq!(rect.w, 100.0);
                assert_eq!(rect.h, 70.0);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn drag_handle_arrow_points_constrain() {
        let mut s = Shape {
            id: 1,
            kind: ShapeKind::Arrow {
                from: pt(0.0, 0.0),
                to: pt(10.0, 0.0),
                ctrl: None,
                double: false,
                head: ArrowHead::Notched,
            },
            style: style(),
        };
        drag_handle(&mut s, ShapeHandle::P2, pt(100.0, 8.0), true, &measure);
        match s.kind {
            ShapeKind::Arrow { to, .. } => assert!(to.y.abs() < 1e-3 && to.x > 90.0),
            _ => unreachable!(),
        }
        drag_handle(&mut s, ShapeHandle::P1, pt(-5.0, 7.0), false, &measure);
        assert!(matches!(s.kind, ShapeKind::Arrow { from, .. } if from == pt(-5.0, 7.0)));
    }

    #[test]
    fn drag_handle_scales_pen_and_text() {
        let mut s = Shape {
            id: 1,
            kind: ShapeKind::Pen {
                points: vec![pt(10.0, 10.0), pt(20.0, 20.0)],
            },
            style: style(),
        };
        drag_handle(
            &mut s,
            ShapeHandle::Rect(Handle8::BottomRight),
            pt(30.0, 30.0),
            false,
            &measure,
        );
        match &s.kind {
            ShapeKind::Pen { points } => assert_eq!(points[1], pt(30.0, 30.0)),
            _ => unreachable!(),
        }
        let mut t = Shape {
            id: 2,
            kind: ShapeKind::Text {
                pos: pt(0.0, 0.0),
                text: "ab".into(),
                size: 20.0,
                background: None,
                bold: false,
                width: None,
            },
            style: style(),
        };
        let h = bounds(&t, &measure).h;
        drag_handle(
            &mut t,
            ShapeHandle::Rect(Handle8::BottomRight),
            pt(10.0, h * 2.0),
            false,
            &measure,
        );
        assert!(matches!(t.kind, ShapeKind::Text { size, .. } if (size - 40.0).abs() < 0.01));
    }

    #[test]
    fn step_radius_handle() {
        let mut s = Shape {
            id: 1,
            kind: ShapeKind::Step {
                center: pt(50.0, 50.0),
                n: 1,
                radius: 10.0,
            },
            style: style(),
        };
        drag_handle(
            &mut s,
            ShapeHandle::Rect(Handle8::Right),
            pt(80.0, 50.0),
            false,
            &measure,
        );
        assert!(matches!(s.kind, ShapeKind::Step { radius, .. } if (radius - 30.0).abs() < 1e-3));
    }

    #[test]
    fn angle_constraint_snaps_to_45() {
        let p = constrain_angle(pt(0.0, 0.0), pt(100.0, 30.0));
        assert!(p.y.abs() < 1e-3);
        let p = constrain_angle(pt(0.0, 0.0), pt(100.0, 90.0));
        assert!((p.x - p.y).abs() < 1e-2);
    }

    #[test]
    fn smoothing_keeps_endpoints_and_removes_dupes() {
        let pts = vec![
            pt(0.0, 0.0),
            pt(0.2, 0.0),
            pt(10.0, 0.0),
            pt(10.0, 10.0),
            pt(20.0, 10.0),
        ];
        let s = smooth_points(&pts);
        assert_eq!(s.first().copied(), Some(pt(0.0, 0.0)));
        assert_eq!(s.last().copied(), Some(pt(20.0, 10.0)));
        assert!(s.len() > 4);
        assert_eq!(smooth_points(&[pt(1.0, 1.0)]).len(), 1);
    }

    fn params() -> ToolParams {
        ToolParams {
            style: style(),
            font_size: 20.0,
            blur_radius: 12.0,
            pixelate_block: 10,
            options: ShapeOptions::default(),
        }
    }

    #[test]
    fn creation_from_drag() {
        let (k, _) =
            shape_from_drag(Tool::Rect, pt(10.0, 10.0), pt(60.0, 40.0), false, &params()).unwrap();
        assert!(
            matches!(k, ShapeKind::Rect { rect, .. } if rect == RectF::new(10.0, 10.0, 50.0, 30.0))
        );
        let (k, _) = shape_from_drag(
            Tool::Ellipse,
            pt(10.0, 10.0),
            pt(60.0, 40.0),
            true,
            &params(),
        )
        .unwrap();
        assert!(matches!(k, ShapeKind::Ellipse { rect } if rect.w == rect.h));
        assert!(
            shape_from_drag(Tool::Rect, pt(10.0, 10.0), pt(10.5, 10.5), false, &params()).is_none()
        );
        assert!(
            shape_from_drag(
                Tool::Arrow,
                pt(10.0, 10.0),
                pt(10.0, 10.0),
                false,
                &params()
            )
            .is_none()
        );
        let (k, s) = shape_from_drag(
            Tool::Callout,
            pt(10.0, 10.0),
            pt(10.0, 10.0),
            false,
            &params(),
        )
        .unwrap();
        assert!(matches!(k, ShapeKind::Callout { .. }));
        assert!(s.fill.is_some());
        let (k, _) =
            shape_from_drag(Tool::Blur, pt(0.0, 0.0), pt(50.0, 50.0), false, &params()).unwrap();
        assert!(matches!(k, ShapeKind::Blur { radius, .. } if radius == 12.0));
        let (k, _) = shape_from_drag(
            Tool::Pixelate,
            pt(0.0, 0.0),
            pt(50.0, 50.0),
            false,
            &params(),
        )
        .unwrap();
        assert!(matches!(k, ShapeKind::Pixelate { block: 10, .. }));
    }

    #[test]
    fn highlighter_style_is_translucent_and_wide() {
        let s = style_for_tool(Tool::Highlighter, &style());
        assert_eq!(s.color[3], HIGHLIGHTER_ALPHA);
        assert!(s.width >= 14.0);
    }

    #[test]
    fn recolor_and_resize() {
        let mut s = Shape {
            id: 1,
            kind: ShapeKind::Highlighter {
                points: vec![pt(0.0, 0.0)],
            },
            style: style(),
        };
        recolor(&mut s, [1, 2, 3, 255]);
        assert_eq!(s.style.color, [1, 2, 3, HIGHLIGHTER_ALPHA]);
        let mut c = Shape {
            id: 2,
            kind: ShapeKind::Callout {
                rect: RectF::new(0.0, 0.0, 9.0, 9.0),
                tail: pt(0.0, 0.0),
                text: String::new(),
                size: 14.0,
            },
            style: Style {
                fill: Some([9, 9, 9, 255]),
                ..style()
            },
        };
        recolor(&mut c, [10, 20, 30, 255]);
        assert_eq!(c.style.fill, Some([10, 20, 30, 255]));
        adjust_size(&mut c, 2.0);
        assert!(matches!(c.kind, ShapeKind::Callout { size, .. } if size == 18.0));
        let mut r = Shape {
            id: 3,
            kind: ShapeKind::Line {
                from: pt(0.0, 0.0),
                to: pt(1.0, 1.0),
            },
            style: style(),
        };
        adjust_size(&mut r, 2.0);
        assert_eq!(r.style.width, 6.0);
        adjust_size(&mut r, -100.0);
        assert_eq!(r.style.width, 1.0);
    }

    #[test]
    fn doc_without_and_translate_all() {
        let mut d = doc_with(vec![
            ShapeKind::Redact {
                rect: RectF::new(0.0, 0.0, 5.0, 5.0),
            },
            ShapeKind::Redact {
                rect: RectF::new(10.0, 10.0, 5.0, 5.0),
            },
        ]);
        assert_eq!(doc_without(&d, &[1]).shapes.len(), 1);
        assert_eq!(doc_without(&d, &[]).shapes.len(), 2);
        translate_all(&mut d, -3.0, 4.0);
        assert!(
            matches!(d.shapes[1].kind, ShapeKind::Redact { rect } if rect.x == 7.0 && rect.y == 14.0)
        );
    }

    #[test]
    fn text_helpers() {
        let mut k = ShapeKind::Text {
            pos: pt(1.0, 2.0),
            text: "x".into(),
            size: 16.0,
            background: None,
            bold: false,
            width: None,
        };
        assert!(is_textual(&k));
        set_text(&mut k, "multi\nline");
        assert_eq!(text_of(&k), Some("multi\nline"));
        let r = text_rect(pt(0.0, 0.0), "", 20.0, false, None, &measure);
        assert!(r.w >= 4.0 && r.h >= 20.0);
    }

    fn text_shape(text: &str, width: Option<f32>, pill: bool) -> Shape {
        Shape {
            id: 1,
            kind: ShapeKind::Text {
                pos: pt(10.0, 20.0),
                text: text.into(),
                size: 20.0,
                background: pill.then_some([0, 0, 0, 255]),
                bold: false,
                width,
            },
            style: style(),
        }
    }

    #[test]
    fn fixed_text_box_wraps_to_its_width() {
        // Fake measure: 10 px per char at size 20, 25 px lines.
        let auto = text_shape("aaaa bbbb cccc", None, false);
        assert_eq!(bounds(&auto, &measure), RectF::new(10.0, 20.0, 140.0, 25.0));
        // 100 px box: "aaaa bbbb" (90) fits, "cccc" wraps.
        let boxed = text_shape("aaaa bbbb cccc", Some(100.0), false);
        assert_eq!(
            bounds(&boxed, &measure),
            RectF::new(10.0, 20.0, 100.0, 50.0)
        );
        // Narrower than a word: the word overflows the box on its own line.
        let tight = text_shape("aaaa bbbb", Some(30.0), false);
        assert_eq!(bounds(&tight, &measure), RectF::new(10.0, 20.0, 40.0, 50.0));
        // Empty boxed text keeps its box and one line.
        let empty = text_shape("", Some(80.0), false);
        assert_eq!(bounds(&empty, &measure), RectF::new(10.0, 20.0, 80.0, 25.0));
        // The pill pads the box like auto text.
        let pill = text_shape("aaaa", Some(100.0), true);
        assert_eq!(bounds(&pill, &measure), RectF::new(2.0, 16.0, 116.0, 33.0));
    }

    #[test]
    fn text_drag_draws_a_box_and_a_click_does_not() {
        assert_eq!(text_box_from_drag(pt(5.0, 5.0), pt(9.0, 30.0), 12.0), None);
        assert_eq!(
            text_box_from_drag(pt(200.0, 50.0), pt(80.0, 10.0), 12.0),
            Some((pt(80.0, 10.0), 120.0))
        );
        // The minimum never drops below MIN_TEXT_BOX, even zoomed in.
        assert_eq!(text_box_from_drag(pt(0.0, 0.0), pt(10.0, 0.0), 1.0), None);
    }

    #[test]
    fn text_side_handles_set_the_box_width() {
        let mut s = text_shape("aaaa bbbb cccc", None, false);
        let hs: Vec<ShapeHandle> = handles(&s, &measure).into_iter().map(|(h, _)| h).collect();
        assert!(hs.contains(&ShapeHandle::Rect(Handle8::Right)));
        assert!(hs.contains(&ShapeHandle::Rect(Handle8::Left)));
        // Right edge: auto text becomes a fixed box and wraps.
        drag_handle(
            &mut s,
            ShapeHandle::Rect(Handle8::Right),
            pt(110.0, 30.0),
            false,
            &measure,
        );
        assert!(matches!(s.kind, ShapeKind::Text { width: Some(w), .. } if w == 100.0));
        assert_eq!(bounds(&s, &measure).h, 50.0);
        // Left edge: the right edge stays, the box grows to the left.
        drag_handle(
            &mut s,
            ShapeHandle::Rect(Handle8::Left),
            pt(0.0, 30.0),
            false,
            &measure,
        );
        assert!(
            matches!(s.kind, ShapeKind::Text { pos, width: Some(w), .. } if pos.x == 0.0 && w == 110.0)
        );
        // Never narrower than the minimum box.
        drag_handle(
            &mut s,
            ShapeHandle::Rect(Handle8::Right),
            pt(-50.0, 30.0),
            false,
            &measure,
        );
        assert!(matches!(s.kind, ShapeKind::Text { width: Some(w), .. } if w == MIN_TEXT_BOX));
        // The corner still scales the font.
        let before = bounds(&s, &measure);
        drag_handle(
            &mut s,
            ShapeHandle::Rect(Handle8::BottomRight),
            pt(before.x + before.w, before.y + before.h * 2.0),
            false,
            &measure,
        );
        assert!(matches!(s.kind, ShapeKind::Text { size, .. } if size > 20.0));
    }

    fn redact_doc(n: usize) -> Doc {
        let mut d = Doc::new(400, 300);
        for i in 0..n {
            d.add(
                ShapeKind::Redact {
                    rect: RectF::new(i as f32 * 10.0, 0.0, 5.0, 5.0),
                },
                style(),
            );
        }
        d
    }

    fn order(d: &Doc) -> Vec<ShapeId> {
        d.shapes.iter().map(|s| s.id).collect()
    }

    #[test]
    fn z_order_steps_and_extremes() {
        let mut d = redact_doc(5); // ids 1..=5, bottom to top
        assert!(bring_forward(&mut d, &[2, 3]));
        assert_eq!(order(&d), [1, 4, 2, 3, 5]);
        assert!(send_backward(&mut d, &[3]));
        assert_eq!(order(&d), [1, 4, 3, 2, 5]);
        assert!(bring_to_front(&mut d, &[1, 3]));
        assert_eq!(order(&d), [4, 2, 5, 1, 3]);
        assert!(send_to_back(&mut d, &[5, 3]));
        assert_eq!(order(&d), [5, 3, 4, 2, 1]);
        // Already at the edge: nothing changes.
        assert!(!send_to_back(&mut d, &[5]));
        assert!(!bring_forward(&mut d, &[1]));
        assert!(!send_backward(&mut d, &[5, 3]));
        assert_eq!(order(&d), [5, 3, 4, 2, 1]);
    }

    #[test]
    fn copies_get_new_ids_offsets_and_step_numbers() {
        let mut d = Doc::new(400, 300);
        let a = d.add(
            ShapeKind::Step {
                center: pt(50.0, 50.0),
                n: 1,
                radius: 10.0,
            },
            style(),
        );
        let b = d.add(
            ShapeKind::Rect {
                rect: RectF::new(100.0, 100.0, 20.0, 20.0),
                corner_radius: 0.0,
            },
            style(),
        );
        let src = collect(&d, &[b, a]);
        assert_eq!(src.len(), 2);
        assert_eq!(src[0].id, a, "copies keep z-order, not pick order");
        let ids = add_copies(&mut d, &src, (16.0, 16.0));
        assert_eq!(ids.len(), 2);
        assert!(!ids.contains(&a) && !ids.contains(&b));
        assert!(
            matches!(d.get(ids[0]).unwrap().kind, ShapeKind::Step { n: 2, center, .. } if center == pt(66.0, 66.0))
        );
        assert!(
            matches!(d.get(ids[1]).unwrap().kind, ShapeKind::Rect { rect, .. } if rect.x == 116.0)
        );
    }

    #[test]
    fn pasted_copies_stay_on_the_image() {
        let g = RectF::new(380.0, 280.0, 40.0, 30.0);
        let (dx, dy) = paste_offset(g, (64.0, 64.0), 400.0, 300.0);
        assert!(g.x + dx <= 400.0 - 8.0 && g.y + dy <= 300.0 - 8.0);
        let inside = RectF::new(10.0, 10.0, 20.0, 20.0);
        assert_eq!(
            paste_offset(inside, (16.0, 16.0), 400.0, 300.0),
            (16.0, 16.0)
        );
    }

    #[test]
    fn marquee_selects_touching_shapes() {
        let d = redact_doc(4); // x = 0, 10, 20, 30
        let ids = shapes_in_rect(&d, RectF::new(12.0, 2.0, 10.0, 1.0), &measure);
        assert_eq!(ids, [2, 3]);
        assert!(shapes_in_rect(&d, RectF::new(100.0, 100.0, 5.0, 5.0), &measure).is_empty());
        assert_eq!(
            group_bounds(&d, &[1, 4], &measure),
            Some(RectF::new(0.0, 0.0, 35.0, 5.0))
        );
    }

    #[test]
    fn shift_resize_keeps_squares_on_edge_handles_too() {
        let r = RectF::new(10.0, 10.0, 40.0, 20.0);
        let s = resize_frect(r, Handle8::Right, pt(90.0, 0.0), true);
        assert_eq!((s.w, s.h), (80.0, 80.0));
        assert_eq!(s.y + s.h / 2.0, 20.0, "centred on the old middle");
        let s = resize_frect(r, Handle8::Bottom, pt(0.0, 60.0), true);
        assert_eq!((s.w, s.h), (50.0, 50.0));
        let s = resize_frect(r, Handle8::BottomRight, pt(70.0, 40.0), true);
        assert_eq!(s.w, s.h);
        // Ellipses resize the same way (circle).
        let mut e = Shape {
            id: 1,
            kind: ShapeKind::Ellipse { rect: r },
            style: style(),
        };
        drag_handle(
            &mut e,
            ShapeHandle::Rect(Handle8::Left),
            pt(-30.0, 0.0),
            true,
            &measure,
        );
        assert!(matches!(e.kind, ShapeKind::Ellipse { rect } if (rect.w - rect.h).abs() < 1e-4));
        assert_eq!(constrain_axis((5.0, -2.0)), (5.0, 0.0));
        assert_eq!(constrain_axis((1.0, -9.0)), (0.0, -9.0));
    }

    fn arrow_shape() -> Shape {
        Shape {
            id: 1,
            kind: ShapeKind::Arrow {
                from: pt(0.0, 0.0),
                to: pt(100.0, 0.0),
                ctrl: None,
                double: false,
                head: ArrowHead::Notched,
            },
            style: style(),
        }
    }

    #[test]
    fn curve_handle_bends_and_straightens() {
        let mut s = arrow_shape();
        let hs = handles(&s, &measure);
        assert!(hs.contains(&(ShapeHandle::Mid, pt(50.0, 0.0))));
        drag_handle(&mut s, ShapeHandle::Mid, pt(50.0, 40.0), false, &measure);
        let ShapeKind::Arrow { ctrl, .. } = s.kind else {
            unreachable!()
        };
        assert_eq!(ctrl, Some(pt(50.0, 80.0)));
        // The handle now sits on the curve where it was dropped.
        assert!(handles(&s, &measure).contains(&(ShapeHandle::Mid, pt(50.0, 40.0))));
        // The curve is hit where it runs, the chord no longer.
        let d = Doc {
            shapes: vec![s.clone()],
            ..Doc::new(200, 200)
        };
        assert!(hit_test(&d, pt(50.0, 40.0), 2.0, &measure).is_some());
        assert!(hit_test(&d, pt(50.0, 0.0), 2.0, &measure).is_none());
        // Shift keeps the bend symmetric (on the perpendicular bisector).
        drag_handle(&mut s, ShapeHandle::Mid, pt(70.0, 30.0), true, &measure);
        assert!(
            matches!(s.kind, ShapeKind::Arrow { ctrl: Some(c), .. } if (c.x - 50.0).abs() < 1e-3)
        );
        // Moving an end keeps the bend; dropping the handle on the chord straightens.
        drag_handle(&mut s, ShapeHandle::P2, pt(200.0, 0.0), false, &measure);
        assert!(
            matches!(s.kind, ShapeKind::Arrow { ctrl: Some(c), .. } if (c.x - 100.0).abs() < 1e-3)
        );
        drag_handle(&mut s, ShapeHandle::Mid, pt(101.0, 2.0), false, &measure);
        assert!(matches!(s.kind, ShapeKind::Arrow { ctrl: None, .. }));
    }

    #[test]
    fn arrow_options_and_bounds() {
        let mut s = arrow_shape();
        set_arrow_double(&mut s.kind, true);
        set_arrow_head(&mut s.kind, ArrowHead::Open);
        assert!(matches!(
            s.kind,
            ShapeKind::Arrow {
                double: true,
                head: ArrowHead::Open,
                ..
            }
        ));
        let b = bounds(&s, &measure);
        assert!(b.x <= 0.0 && b.x + b.w >= 100.0 && b.h > 4.0);
        drag_handle(&mut s, ShapeHandle::Mid, pt(50.0, -60.0), false, &measure);
        assert!(bounds(&s, &measure).y <= -59.0);
        straighten(&mut s.kind);
        assert!(matches!(s.kind, ShapeKind::Arrow { ctrl: None, .. }));
        // New arrows take the sticky options.
        let mut p = params();
        p.options.arrow_double = true;
        p.options.arrow_head = ArrowHead::Triangle;
        let (k, _) = shape_from_drag(Tool::Arrow, pt(0.0, 0.0), pt(50.0, 0.0), false, &p).unwrap();
        assert!(matches!(
            k,
            ShapeKind::Arrow {
                double: true,
                head: ArrowHead::Triangle,
                ..
            }
        ));
    }

    #[test]
    fn text_pill_takes_the_color_and_gets_readable_ink() {
        let mut p = params();
        p.style.color = [255, 220, 0, 255];
        p.options.text_pill = true;
        p.options.text_bold = true;
        let (kind, st) = new_text(pt(5.0, 5.0), &p);
        assert!(matches!(
            kind,
            ShapeKind::Text {
                background: Some([255, 220, 0, 255]),
                bold: true,
                ..
            }
        ));
        assert_eq!(st.color, [20, 20, 22, 255], "dark ink on yellow");
        let mut s = Shape {
            id: 1,
            kind,
            style: st,
        };
        // Recoloring pill text recolors the pill; ink follows.
        recolor(&mut s, [10, 20, 160, 255]);
        assert!(matches!(
            s.kind,
            ShapeKind::Text {
                background: Some([10, 20, 160, 255]),
                ..
            }
        ));
        assert_eq!(s.style.color, [255, 255, 255, 255]);
        assert_eq!(display_color(&s), [10, 20, 160, 255]);
        set_pill(&mut s, false);
        assert!(matches!(
            s.kind,
            ShapeKind::Text {
                background: None,
                ..
            }
        ));
        assert_eq!(s.style.color, [10, 20, 160, 255]);
        set_pill(&mut s, true);
        assert_eq!(s.style.color, [255, 255, 255, 255]);
        // Bold text measures (and so hits) wider.
        let mut bold = s.clone();
        set_bold(&mut bold.kind, true);
        set_text(&mut bold.kind, "Hello");
        assert!(matches!(bold.kind, ShapeKind::Text { bold: true, .. }));
    }

    #[test]
    fn text_size_steps() {
        assert_eq!(step_text_size(20.0, true), 24.0);
        assert_eq!(step_text_size(20.0, false), 18.0);
        assert_eq!(step_text_size(21.0, false), 20.0);
        assert_eq!(step_text_size(128.0, true), 128.0);
        assert_eq!(step_text_size(10.0, false), 10.0);
        let mut k = ShapeKind::Text {
            pos: pt(0.0, 0.0),
            text: "x".into(),
            size: 20.0,
            background: None,
            bold: false,
            width: None,
        };
        assert!(bump_text_size(&mut k, true));
        assert!(matches!(k, ShapeKind::Text { size, .. } if size == 24.0));
    }

    #[test]
    fn tool_config_keys_round_trip() {
        for t in Tool::ALL {
            assert_eq!(Tool::from_config_key(t.config_key()), Some(t));
        }
        assert_eq!(Tool::from_config_key("nope"), None);
        assert!(Tool::Arrow.has_style() && Tool::Redact.has_style());
        assert!(!Tool::Select.has_style() && !Tool::Blur.has_style());
    }

    #[test]
    fn translate_all_moves_the_magnifier_source_too() {
        let mut d = Doc::new(100, 100);
        d.add(
            ShapeKind::Magnifier {
                source: RectF::new(10.0, 10.0, 5.0, 5.0),
                dest: RectF::new(50.0, 50.0, 20.0, 20.0),
                circular: true,
            },
            style(),
        );
        translate_all(&mut d, -5.0, 3.0);
        assert!(matches!(
            d.shapes[0].kind,
            ShapeKind::Magnifier { source, dest, .. } if source.x == 5.0 && dest.y == 53.0
        ));
        // The shape clipboard (a plain list) moves the same way.
        let mut clip = d.shapes.clone();
        translate_shapes(&mut clip, 5.0, -3.0);
        assert!(matches!(
            clip[0].kind,
            ShapeKind::Magnifier { source, dest, .. } if source.x == 10.0 && dest.y == 50.0
        ));
    }

    #[test]
    fn highlight_box_mode_flips_with_shift() {
        let mut o = ShapeOptions::default();
        assert!(!highlight_box_mode(&o, false));
        assert!(highlight_box_mode(&o, true));
        o.highlight_box = true;
        assert!(highlight_box_mode(&o, false));
        assert!(!highlight_box_mode(&o, true));
    }

    #[test]
    fn highlight_box_from_drag_is_styled_like_a_stroke() {
        let p = params();
        let (k, s) = highlight_box_from_drag(pt(80.0, 40.0), pt(20.0, 60.0), &p).unwrap();
        assert_eq!(
            k,
            ShapeKind::HighlightBox {
                rect: RectF::new(20.0, 40.0, 60.0, 20.0)
            }
        );
        assert_eq!(s, style_for_tool(Tool::Highlighter, &p.style));
        assert_eq!(s.fill, None, "no outline or extra fill");
        assert!(highlight_box_from_drag(pt(5.0, 5.0), pt(6.0, 40.0), &p).is_none());
    }

    #[test]
    fn to_box_covers_the_stroke_and_keeps_the_style() {
        let hl_style = style_for_tool(Tool::Highlighter, &style());
        let stroke = Shape {
            id: 7,
            kind: ShapeKind::Highlighter {
                points: vec![pt(10.0, 30.0), pt(60.0, 34.0), pt(110.0, 28.0)],
            },
            style: hl_style.clone(),
        };
        let w = hl_style.width;
        let k = highlighter_to_box(&stroke).unwrap();
        assert_eq!(
            k,
            ShapeKind::HighlightBox {
                rect: RectF::new(10.0 - w / 2.0, 28.0 - w / 2.0, 100.0 + w, 6.0 + w)
            }
        );
        // The box is exactly the selection bounds the stroke had.
        let boxed = Shape {
            kind: k,
            ..stroke.clone()
        };
        assert_eq!(bounds(&boxed, &measure), bounds(&stroke, &measure));
        let pen = Shape {
            kind: ShapeKind::Pen {
                points: vec![pt(0.0, 0.0), pt(9.0, 9.0)],
            },
            ..stroke
        };
        assert!(highlighter_to_box(&pen).is_none());
    }

    #[test]
    fn highlight_box_edits_like_a_rect() {
        let mut s = Shape {
            id: 1,
            kind: ShapeKind::HighlightBox {
                rect: RectF::new(20.0, 20.0, 100.0, 30.0),
            },
            style: style_for_tool(Tool::Highlighter, &style()),
        };
        let d = Doc {
            shapes: vec![s.clone()],
            ..Doc::new(400, 300)
        };
        assert_eq!(hit_test(&d, pt(70.0, 35.0), 0.0, &measure), Some(1));
        assert_eq!(handles(&s, &measure).len(), 8);
        drag_handle(
            &mut s,
            ShapeHandle::Rect(Handle8::BottomRight),
            pt(150.0, 80.0),
            false,
            &measure,
        );
        assert_eq!(
            s.kind,
            ShapeKind::HighlightBox {
                rect: RectF::new(20.0, 20.0, 130.0, 60.0)
            }
        );
        translate(&mut s.kind, 10.0, 5.0);
        assert_eq!(bounds(&s, &measure), RectF::new(30.0, 25.0, 130.0, 60.0));
        recolor(&mut s, [1, 2, 3, 255]);
        assert_eq!(s.style.color, [1, 2, 3, HIGHLIGHTER_ALPHA]);
        assert_eq!(display_width(&s), (s.style.width / 4.0).clamp(1.0, 64.0));
    }

    fn cursor_doc() -> (Doc, ShapeId) {
        let img = stinkshot_core::annotate::CursorImage::new(stinkshot_core::Rgba::filled(
            10,
            20,
            [0, 0, 0, 255],
        ));
        let (k, st) = stinkshot_core::annotate::cursor_shape(pt(50.0, 50.0), img);
        let mut d = Doc::new(400, 300);
        let id = d.add(k, st);
        (d, id)
    }

    #[test]
    fn cursor_hits_bounds_and_corner_handles() {
        let (d, id) = cursor_doc();
        let s = d.get(id).unwrap();
        assert_eq!(bounds(s, &measure), RectF::new(50.0, 50.0, 10.0, 20.0));
        assert_eq!(hit_test(&d, pt(55.0, 60.0), 0.0, &measure), Some(id));
        assert_eq!(hit_test(&d, pt(70.0, 60.0), 0.0, &measure), None);
        let hs = handles(s, &measure);
        assert_eq!(hs.len(), 4);
        assert!(hs.contains(&(ShapeHandle::Rect(Handle8::BottomRight), pt(60.0, 70.0))));
    }

    #[test]
    fn cursor_resizes_with_aspect_and_ignores_color_and_size_keys() {
        let (mut d, id) = cursor_doc();
        let s = d.get_mut(id).unwrap();
        // Drag the bottom-right corner to double width: height follows.
        drag_handle(
            s,
            ShapeHandle::Rect(Handle8::BottomRight),
            pt(70.0, 60.0),
            false,
            &measure,
        );
        assert!(
            matches!(&s.kind, ShapeKind::Cursor { rect, .. } if *rect == RectF::new(50.0, 50.0, 20.0, 40.0))
        );
        // Top-left keeps the bottom-right corner fixed.
        drag_handle(
            s,
            ShapeHandle::Rect(Handle8::TopLeft),
            pt(65.0, 80.0),
            false,
            &measure,
        );
        assert!(
            matches!(&s.kind, ShapeKind::Cursor { rect, .. } if *rect == RectF::new(65.0, 80.0, 5.0, 10.0))
        );
        // Never smaller than a quarter.
        drag_handle(
            s,
            ShapeHandle::Rect(Handle8::TopLeft),
            pt(69.5, 89.5),
            false,
            &measure,
        );
        assert!(
            matches!(&s.kind, ShapeKind::Cursor { rect, .. } if (rect.w - 2.5).abs() < 1e-4 && (rect.h - 5.0).abs() < 1e-4)
        );
        let before = s.clone();
        recolor(s, [1, 2, 3, 255]);
        adjust_size(s, 3.0);
        assert_eq!(*s, before);
    }

    #[test]
    fn resize_cursor_ignores_edge_handles() {
        let r = RectF::new(0.0, 0.0, 10.0, 20.0);
        assert_eq!(
            resize_cursor(r, Handle8::Right, pt(50.0, 50.0), 10.0, 20.0),
            r
        );
    }
}
