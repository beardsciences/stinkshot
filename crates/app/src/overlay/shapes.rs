//! Pure annotation-shape math for the editor: bounds, hit testing, translation, resize
//! handles per shape type, creation from a drag, smoothing, recoloring and sizing.
//! All coordinates are float base-image pixels. Text extents come from a `Measure`
//! callback (the app passes `Fonts::measure`; tests pass a fixed-advance fake) so nothing
//! here depends on font files or on core's still-evolving geometry helpers.

use stinkshot_core::annotate::{Doc, Shape, ShapeId, ShapeKind, Style};
use stinkshot_core::{PtF, RectF};

use super::selection::Handle8;

/// Text extent callback: (text, size_px) -> (width, height).
pub type Measure<'a> = &'a dyn Fn(&str, f32) -> (f32, f32);

/// Highlighter alpha the editor writes into `Style::color`.
pub const HIGHLIGHTER_ALPHA: u8 = 0x99;
pub const MIN_SHAPE: f32 = 2.0;

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
}

impl Tool {
    pub fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select (V)",
            Tool::Arrow => "Arrow (A)",
            Tool::Line => "Line (L)",
            Tool::Rect => "Rectangle (R)",
            Tool::Ellipse => "Ellipse (E)",
            Tool::Pen => "Pen (P)",
            Tool::Highlighter => "Highlighter (H)",
            Tool::Text => "Text (T)",
            Tool::Step => "Step Counter (N)",
            Tool::Blur => "Blur (B)",
            Tool::Pixelate => "Pixelate (X)",
            Tool::Redact => "Solid Redact Box (K)",
            Tool::Magnifier => "Magnifier Inset (M)",
            Tool::Callout => "Callout (O)",
            Tool::Spotlight => "Spotlight (S)",
        }
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

fn union(a: RectF, b: RectF) -> RectF {
    let x0 = a.x.min(b.x);
    let y0 = a.y.min(b.y);
    let x1 = (a.x + a.w).max(b.x + b.w);
    let y1 = (a.y + a.h).max(b.y + b.h);
    RectF::new(x0, y0, x1 - x0, y1 - y0)
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

/// Rect covered by a text block anchored at `pos`.
pub fn text_rect(pos: PtF, text: &str, size: f32, measure: Measure) -> RectF {
    let probe = if text.is_empty() { "M" } else { text };
    let (w, h) = measure(probe, size);
    let w = if text.is_empty() { size * 0.6 } else { w };
    RectF::new(pos.x, pos.y, w.max(4.0), h.max(size))
}

/// Axis-aligned visual bounds including stroke (image px).
pub fn bounds(shape: &Shape, measure: Measure) -> RectF {
    let w = shape.style.width;
    match &shape.kind {
        ShapeKind::Arrow { from, to } => RectF::from_points(*from, *to).expand(w * 2.0),
        ShapeKind::Line { from, to } => RectF::from_points(*from, *to).expand(w / 2.0),
        ShapeKind::Rect { rect, .. } | ShapeKind::Ellipse { rect } => rect.expand(w / 2.0),
        ShapeKind::Pen { points } | ShapeKind::Highlighter { points } => {
            points_bbox(points).expand(w / 2.0)
        }
        ShapeKind::Text {
            pos, text, size, ..
        } => text_rect(*pos, text, *size, measure),
        ShapeKind::Step { center, radius, .. } => RectF::new(
            center.x - radius,
            center.y - radius,
            radius * 2.0,
            radius * 2.0,
        ),
        ShapeKind::Blur { rect, .. }
        | ShapeKind::Pixelate { rect, .. }
        | ShapeKind::Redact { rect } => *rect,
        ShapeKind::Magnifier { source, dest, .. } => union(*source, *dest),
        ShapeKind::Callout { rect, tail, .. } => union(*rect, RectF::new(tail.x, tail.y, 0.0, 0.0)),
        ShapeKind::Spotlight { rect, .. } => *rect,
    }
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
        ShapeKind::Arrow { from, to } | ShapeKind::Line { from, to } => {
            dist_to_segment(p, *from, *to) <= w / 2.0 + tol + 1.0
        }
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
            let reach = w / 2.0 + tol;
            match points.len() {
                0 => false,
                1 => dist(p, points[0]) <= reach,
                _ => points
                    .windows(2)
                    .any(|s| dist_to_segment(p, s[0], s[1]) <= reach),
            }
        }
        ShapeKind::Text { .. } => bounds(shape, measure).expand(tol).contains(p),
        ShapeKind::Step { center, radius, .. } => dist(p, *center) <= radius + tol,
        ShapeKind::Blur { rect, .. }
        | ShapeKind::Pixelate { rect, .. }
        | ShapeKind::Redact { rect } => rect.expand(tol).contains(p),
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
            rect.expand(tol).contains(p) || dist(p, *tail) <= 8.0 + tol
        }
        ShapeKind::Spotlight { rect, .. } => near_rect_border(*rect, p, 4.0 + tol),
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

fn tr(p: &mut PtF, dx: f32, dy: f32) {
    p.x += dx;
    p.y += dy;
}

fn tr_rect(r: &mut RectF, dx: f32, dy: f32) {
    r.x += dx;
    r.y += dy;
}

pub fn translate(kind: &mut ShapeKind, dx: f32, dy: f32) {
    match kind {
        ShapeKind::Arrow { from, to } | ShapeKind::Line { from, to } => {
            tr(from, dx, dy);
            tr(to, dx, dy);
        }
        ShapeKind::Rect { rect, .. }
        | ShapeKind::Ellipse { rect }
        | ShapeKind::Blur { rect, .. }
        | ShapeKind::Pixelate { rect, .. }
        | ShapeKind::Redact { rect }
        | ShapeKind::Spotlight { rect, .. } => tr_rect(rect, dx, dy),
        ShapeKind::Pen { points } | ShapeKind::Highlighter { points } => {
            points.iter_mut().for_each(|p| tr(p, dx, dy))
        }
        ShapeKind::Text { pos, .. } => tr(pos, dx, dy),
        ShapeKind::Step { center, .. } => tr(center, dx, dy),
        ShapeKind::Magnifier { source, dest, .. } => {
            // Moving the magnifier moves the placed inset; the source stays on the content.
            let _ = source;
            tr_rect(dest, dx, dy);
        }
        ShapeKind::Callout { rect, tail, .. } => {
            tr_rect(rect, dx, dy);
            tr(tail, dx, dy);
        }
    }
}

/// Main rectangle used for the 8 handles (not for arrows/lines/steps).
fn main_rect(shape: &Shape) -> Option<RectF> {
    match &shape.kind {
        ShapeKind::Rect { rect, .. }
        | ShapeKind::Ellipse { rect }
        | ShapeKind::Blur { rect, .. }
        | ShapeKind::Pixelate { rect, .. }
        | ShapeKind::Redact { rect }
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
        ShapeKind::Arrow { from, to } | ShapeKind::Line { from, to } => {
            out.push((ShapeHandle::P1, *from));
            out.push((ShapeHandle::P2, *to));
        }
        ShapeKind::Text { .. } => {
            let b = bounds(shape, measure);
            out.push((
                ShapeHandle::Rect(Handle8::BottomRight),
                pt(b.x + b.w, b.y + b.h),
            ));
        }
        ShapeKind::Step { center, radius, .. } => {
            out.push((
                ShapeHandle::Rect(Handle8::Right),
                pt(center.x + radius, center.y),
            ));
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
/// that Src/Tail handles are reachable over overlapping rect handles.
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

/// Resize a float rect by dragging `h` to `p`; flips when crossing; `square` forces 1:1.
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

/// Apply a handle drag to `shape`. `p` is the pointer in image px; `constrain` (Shift)
/// keeps 45 degree angles for lines and squares for rectangles.
pub fn drag_handle(
    shape: &mut Shape,
    handle: ShapeHandle,
    p: PtF,
    constrain: bool,
    measure: Measure,
) {
    let old_main = main_rect(shape);
    let text_h = bounds(shape, measure).h;
    match (&mut shape.kind, handle) {
        (ShapeKind::Arrow { from, to } | ShapeKind::Line { from, to }, ShapeHandle::P1) => {
            *from = if constrain {
                constrain_angle(*to, p)
            } else {
                p
            };
        }
        (ShapeKind::Arrow { from, to } | ShapeKind::Line { from, to }, ShapeHandle::P2) => {
            *to = if constrain {
                constrain_angle(*from, p)
            } else {
                p
            };
        }
        (ShapeKind::Callout { tail, .. }, ShapeHandle::Tail) => *tail = p,
        (ShapeKind::Step { center, radius, .. }, ShapeHandle::Rect(_)) => {
            *radius = dist(*center, p).clamp(6.0, 120.0);
        }
        (ShapeKind::Text { pos, size, .. }, ShapeHandle::Rect(_)) if text_h > 1.0 => {
            let new_h = (p.y - pos.y).max(4.0);
            *size = (*size * new_h / text_h).clamp(8.0, 400.0);
        }
        (ShapeKind::Magnifier { source, .. }, ShapeHandle::Src(h)) => {
            *source = min_size(resize_frect(*source, h, p, constrain));
        }
        (ShapeKind::Pen { points } | ShapeKind::Highlighter { points }, ShapeHandle::Rect(h)) => {
            if let Some(old) = old_main {
                let new = resize_frect(old, h, p, false);
                scale_points(points, old, new);
            }
        }
        (
            ShapeKind::Rect { rect, .. }
            | ShapeKind::Ellipse { rect }
            | ShapeKind::Blur { rect, .. }
            | ShapeKind::Pixelate { rect, .. }
            | ShapeKind::Redact { rect }
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

/// Editor-wide creation parameters taken from the config.
#[derive(Clone, Debug)]
pub struct ToolParams {
    pub style: Style,
    pub font_size: f32,
    pub blur_radius: f32,
    pub pixelate_block: u32,
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
                ShapeKind::Arrow { from: a, to: b }
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
        | Tool::Magnifier => return None,
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

/// Recolor a shape. Highlighters keep their translucency; callouts recolor their fill too.
pub fn recolor(shape: &mut Shape, color: [u8; 4]) {
    let mut c = color;
    if matches!(shape.kind, ShapeKind::Highlighter { .. }) {
        c[3] = HIGHLIGHTER_ALPHA;
    }
    shape.style.color = c;
    if shape.style.fill.is_some() {
        shape.style.fill = Some([c[0], c[1], c[2], 255]);
    }
}

/// Change the "size" of a shape by `delta` (stroke width, text size, step radius, blur
/// strength...). Returns the new stroke width the toolbar should show, if any.
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
        _ => shape.style.width = (shape.style.width + delta).clamp(1.0, 64.0),
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

/// Where the inline text editor sits for a textual shape: top-left of the text (image px),
/// the font size, and for callouts the inner box width.
pub fn text_anchor(kind: &ShapeKind) -> Option<(PtF, f32, Option<f32>)> {
    match kind {
        ShapeKind::Text { pos, size, .. } => Some((*pos, *size, None)),
        ShapeKind::Callout { rect, size, .. } => Some((
            pt(rect.x + 10.0, rect.y + 6.0),
            *size,
            Some((rect.w - 20.0).max(20.0)),
        )),
        _ => None,
    }
}

/// Copy `doc` keeping only shapes not in `skip` (used so a shape being manipulated is not
/// baked into the cached render while it is painted live).
pub fn doc_without(doc: &Doc, skip: Option<ShapeId>) -> Doc {
    let mut d = doc.clone();
    if let Some(id) = skip {
        d.shapes.retain(|s| s.id != id);
    }
    d
}

/// Translate every shape (selection origin changed while annotations exist).
pub fn translate_all(doc: &mut Doc, dx: f32, dy: f32) {
    for s in &mut doc.shapes {
        translate(&mut s.kind, dx, dy);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measure(text: &str, size: f32) -> (f32, f32) {
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
                to: pt(1.0, 1.0)
            }),
            2
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
                background: None
            }),
            1
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
        assert_eq!(doc_without(&d, Some(1)).shapes.len(), 1);
        assert_eq!(doc_without(&d, None).shapes.len(), 2);
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
        };
        assert!(is_textual(&k));
        set_text(&mut k, "multi\nline");
        assert_eq!(text_of(&k), Some("multi\nline"));
        assert!(text_anchor(&k).is_some());
        let r = text_rect(pt(0.0, 0.0), "", 20.0, &measure);
        assert!(r.w >= 4.0 && r.h >= 20.0);
    }
}
