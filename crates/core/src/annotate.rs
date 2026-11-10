//! Annotation document model, hit testing, rendering, and re-editable sidecars.
//!
//! All coordinates are float pixels in the BASE IMAGE's space (the cropped capture,
//! physical pixels), never screen or egui points.
//!
//! Re-edit storage for a saved capture `C:\..\Shot.png`:
//!   - `Shot.png`                          flattened result (what the user sees in the folder)
//!   - `<folder>\.stinkshot\edit\<stem>.json` the `Doc`
//!   - `<folder>\.stinkshot\edit\<stem>.base.png` the unannotated original
//!
//! `save_editable` / `load_editable` / `delete_editable` own this layout.

use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::fonts::Fonts;
use crate::geom::{PtF, PxRect, RectF};
use crate::image::Rgba;

mod cursor;
mod render;

pub use cursor::{CursorImage, MAX_CURSOR_SIDE, cursor_from_renders};

pub type ShapeId = u64;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Style {
    pub color: [u8; 4],
    /// Stroke width in image pixels.
    pub width: f32,
    /// Fill for rect/ellipse/callout; None = outline only.
    pub fill: Option<[u8; 4]>,
    /// Subtle drop shadow under strokes/text for legibility.
    pub shadow: bool,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            color: [0xff, 0x8a, 0x3d, 0xff],
            width: 4.0,
            fill: None,
            shadow: true,
        }
    }
}

/// Arrow head look. Older sidecars have no `head` field and load as `Notched`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArrowHead {
    /// Filled head with a swept-back notch (the original look).
    #[default]
    Notched,
    /// Filled plain triangle.
    Triangle,
    /// Two stroked lines (chevron), no fill.
    Open,
}

impl ArrowHead {
    pub const ALL: [ArrowHead; 3] = [ArrowHead::Notched, ArrowHead::Triangle, ArrowHead::Open];

    fn is_default(&self) -> bool {
        *self == ArrowHead::Notched
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ShapeKind {
    Arrow {
        from: PtF,
        to: PtF,
        /// Quadratic control point of a curved arrow; None = straight. The head follows the
        /// curve's tangent at `to`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ctrl: Option<PtF>,
        /// A head at `from` as well.
        #[serde(default, skip_serializing_if = "is_false")]
        double: bool,
        #[serde(default, skip_serializing_if = "ArrowHead::is_default")]
        head: ArrowHead,
    },
    Line {
        from: PtF,
        to: PtF,
    },
    Rect {
        rect: RectF,
        corner_radius: f32,
    },
    Ellipse {
        rect: RectF,
    },
    /// Freehand; points already smoothed by the editor.
    Pen {
        points: Vec<PtF>,
    },
    /// Wide, translucent (multiply-like) stroke; style.width is the marker width.
    Highlighter {
        points: Vec<PtF>,
    },
    /// Axis-aligned highlighter box: filled with the same translucent color as a
    /// `Highlighter` stroke, no outline. style.width is kept only as the marker width.
    HighlightBox {
        rect: RectF,
    },
    /// Top-left anchored text; `size` in image px. Supports '\n'.
    Text {
        pos: PtF,
        text: String,
        size: f32,
        background: Option<[u8; 4]>,
        /// Bold face. Older sidecars load as regular.
        #[serde(default, skip_serializing_if = "is_false")]
        bold: bool,
        /// Fixed box width in image px (pill padding not included): lines also wrap at
        /// spaces to this width. None = the box grows with the text (breaks only at '\n').
        /// Older sidecars have no field and load as None.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<f32>,
    },
    /// Numbered circle; `n` assigned at creation and renumbered by `Doc::renumber_steps`.
    Step {
        center: PtF,
        n: u32,
        radius: f32,
    },
    /// Gaussian-ish blur of the base pixels inside rect. Destructive in output.
    Blur {
        rect: RectF,
        radius: f32,
    },
    /// Mosaic of `block`-px cells inside rect. Destructive in output.
    Pixelate {
        rect: RectF,
        block: u32,
    },
    /// Opaque solid box in style.color. Destructive in output.
    Redact {
        rect: RectF,
    },
    /// Zoomed copy of `source` region drawn into the circle/rounded box at `dest`.
    Magnifier {
        source: RectF,
        dest: RectF,
        circular: bool,
    },
    /// Speech bubble with text; tail points at `tail`.
    Callout {
        rect: RectF,
        tail: PtF,
        text: String,
        size: f32,
    },
    /// Dims everything outside `rect` (multiple spotlights union their clear areas).
    Spotlight {
        rect: RectF,
        dim: f32,
    },
    /// The mouse pointer captured with the screenshot, drawn into `rect` (its bitmap size
    /// unless resized). Not destructive: deleting it shows the screen under it.
    Cursor {
        rect: RectF,
        image: CursorImage,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub id: ShapeId,
    pub kind: ShapeKind,
    pub style: Style,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Doc {
    pub version: u32,
    pub base_width: u32,
    pub base_height: u32,
    /// Back to front.
    pub shapes: Vec<Shape>,
    pub next_id: ShapeId,
    /// Blur, Pixelate and Redact shapes as they were when the base was stored in an edit
    /// sidecar: their effect is already in the base pixels. Empty for a fresh capture.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub baked: Vec<Shape>,
}

// ---------------------------------------------------------------------------------------
// Shared geometry constants and helpers (also used by the renderer).
// ---------------------------------------------------------------------------------------

/// Horizontal / vertical padding of a text background pill, as a fraction of the font size.
pub(crate) const TEXT_PAD_X: f32 = 0.4;
pub(crate) const TEXT_PAD_Y: f32 = 0.2;
/// Callout text padding as a fraction of the font size.
pub(crate) const CALLOUT_PAD: f32 = 0.7;
/// Highlighter strokes are drawn with this fraction of the style color's alpha.
pub(crate) const HIGHLIGHT_ALPHA: f32 = 0.4;

/// The color a highlighter stroke or box actually paints with (style color, alpha scaled by
/// the highlight factor). The editor's live preview uses it so screen matches export.
pub fn highlight_color(c: [u8; 4]) -> [u8; 4] {
    [
        c[0],
        c[1],
        c[2],
        (c[3] as f32 * HIGHLIGHT_ALPHA).round() as u8,
    ]
}

/// The box a freehand highlighter stroke covers: its points' bounds grown by half the marker
/// width (the round-capped stroke's full painted extent). None for other shapes and empty or
/// non-finite strokes.
pub fn highlighter_box_rect(kind: &ShapeKind, width: f32) -> Option<RectF> {
    let ShapeKind::Highlighter { points } = kind else {
        return None;
    };
    let pts: Vec<PtF> = points
        .iter()
        .copied()
        .filter(|p| p.x.is_finite() && p.y.is_finite())
        .collect();
    if pts.is_empty() {
        return None;
    }
    let half = if width.is_finite() {
        width.max(1.0) / 2.0
    } else {
        0.5
    };
    Some(points_bounds(&pts).expand(half))
}

/// Arrow head geometry derived from the stroke width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArrowGeom {
    pub tip: PtF,
    /// Where the shaft ends (the notch at the back of the head).
    pub notch: PtF,
    pub left: PtF,
    pub right: PtF,
    pub head_half_width: f32,
}

/// Head with its tip at `tip`, pointing along the unit vector `dir`; `avail` limits the
/// head length (the shaft length).
fn head_geom(
    tip: PtF,
    dir: (f32, f32),
    avail: f32,
    width: f32,
    style: ArrowHead,
) -> Option<ArrowGeom> {
    if !avail.is_finite() || avail < 0.5 || !dir.0.is_finite() || !dir.1.is_finite() {
        return None;
    }
    let (ux, uy) = dir;
    let (px, py) = (-uy, ux);
    let head_len = (width.max(1.0) * 3.2 + 8.0).min(avail);
    let half = head_len * 0.46;
    let back = |t: f32, s: f32| PtF::new(tip.x - ux * t + px * s, tip.y - uy * t + py * s);
    let notch = match style {
        ArrowHead::Notched => back(head_len * 0.76, 0.0),
        ArrowHead::Triangle => back(head_len, 0.0),
        // The shaft runs into the chevron's point; stop half a stroke short so the round cap
        // does not poke past the tip.
        ArrowHead::Open => back((width * 0.5).min(head_len), 0.0),
    };
    Some(ArrowGeom {
        tip,
        notch,
        left: back(head_len, half),
        right: back(head_len, -half),
        head_half_width: half,
    })
}

/// Samples of the quadratic Bezier `a` -> `c` -> `b` (n segments, n + 1 points).
pub fn quad_points(a: PtF, c: PtF, b: PtF, n: usize) -> Vec<PtF> {
    let n = n.max(1);
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let u = 1.0 - t;
            PtF::new(
                u * u * a.x + 2.0 * u * t * c.x + t * t * b.x,
                u * u * a.y + 2.0 * u * t * c.y + t * t * b.y,
            )
        })
        .collect()
}

/// The point a curved arrow passes through at its middle (t = 0.5), or the chord midpoint.
pub fn arrow_mid(from: PtF, to: PtF, ctrl: Option<PtF>) -> PtF {
    match ctrl {
        Some(c) => PtF::new(
            (from.x + 2.0 * c.x + to.x) / 4.0,
            (from.y + 2.0 * c.y + to.y) / 4.0,
        ),
        None => PtF::new((from.x + to.x) / 2.0, (from.y + to.y) / 2.0),
    }
}

/// The control point that makes the curve pass through `mid` at t = 0.5.
pub fn ctrl_through(from: PtF, to: PtF, mid: PtF) -> PtF {
    PtF::new(
        2.0 * mid.x - (from.x + to.x) / 2.0,
        2.0 * mid.y - (from.y + to.y) / 2.0,
    )
}

/// Centre line of an arrow: two points when straight, a sampled curve when curved.
pub fn arrow_centerline(from: PtF, to: PtF, ctrl: Option<PtF>) -> Vec<PtF> {
    match ctrl {
        Some(c) => quad_points(from, c, to, 48),
        None => vec![from, to],
    }
}

fn polyline_len(pts: &[PtF]) -> f32 {
    pts.windows(2)
        .map(|w| ((w[1].x - w[0].x).powi(2) + (w[1].y - w[0].y).powi(2)).sqrt())
        .sum()
}

/// Cut `d` of arc length off the end of a polyline.
fn trim_end(pts: &mut Vec<PtF>, mut d: f32) {
    while pts.len() >= 2 && d > 0.0 {
        let n = pts.len();
        let (a, b) = (pts[n - 2], pts[n - 1]);
        let seg = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
        if seg > d {
            let t = (seg - d) / seg;
            pts[n - 1] = PtF::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
            return;
        }
        d -= seg;
        pts.pop();
    }
}

fn unit(dx: f32, dy: f32) -> Option<(f32, f32)> {
    let l = (dx * dx + dy * dy).sqrt();
    (l.is_finite() && l > 1e-4).then(|| (dx / l, dy / l))
}

/// Everything needed to draw an arrow: the shaft polyline (already shortened under the
/// heads) and one or two heads. Shared by the renderer, hit testing and the editor preview.
#[derive(Clone, Debug, PartialEq)]
pub struct ArrowParts {
    pub shaft: Vec<PtF>,
    pub heads: Vec<ArrowGeom>,
    pub style: ArrowHead,
}

pub fn arrow_parts(
    from: PtF,
    to: PtF,
    ctrl: Option<PtF>,
    width: f32,
    double: bool,
    style: ArrowHead,
) -> Option<ArrowParts> {
    let line = arrow_centerline(from, to, ctrl);
    let len = polyline_len(&line);
    if !len.is_finite() || len < 0.5 {
        return None;
    }
    let chord = unit(to.x - from.x, to.y - from.y);
    // Tangent at the end of a quadratic is (to - ctrl); fall back to the chord when the
    // control point sits on an end.
    let end_dir = ctrl.and_then(|c| unit(to.x - c.x, to.y - c.y)).or(chord)?;
    let avail = if double { len * 0.5 } else { len };
    let end = head_geom(to, end_dir, avail, width, style)?;
    let mut shaft = line;
    trim_end(&mut shaft, dist(end.tip, end.notch));
    let mut heads = vec![end];
    if double {
        let start_dir = ctrl
            .and_then(|c| unit(from.x - c.x, from.y - c.y))
            .or(chord.map(|(x, y)| (-x, -y)))?;
        if let Some(start) = head_geom(from, start_dir, avail, width, style) {
            shaft.reverse();
            trim_end(&mut shaft, dist(start.tip, start.notch));
            shaft.reverse();
            heads.push(start);
        }
    }
    Some(ArrowParts {
        shaft,
        heads,
        style,
    })
}

fn dist(a: PtF, b: PtF) -> f32 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

/// Keep a curved arrow's bend when one of its ends moves: the control point follows the
/// similarity transform (rotate + scale about the fixed end) that maps the old chord onto
/// the new one. Straight arrows stay straight.
pub fn follow_ctrl(old_from: PtF, old_to: PtF, new_from: PtF, new_to: PtF, ctrl: PtF) -> PtF {
    let (ox, oy) = (old_to.x - old_from.x, old_to.y - old_from.y);
    let (nx, ny) = (new_to.x - new_from.x, new_to.y - new_from.y);
    let ol2 = ox * ox + oy * oy;
    if ol2 < 1e-6 {
        return PtF::new(
            ctrl.x + new_from.x - old_from.x,
            ctrl.y + new_from.y - old_from.y,
        );
    }
    // Complex division n / o gives the rotation + scale.
    let a = (nx * ox + ny * oy) / ol2;
    let b = (ny * ox - nx * oy) / ol2;
    let (rx, ry) = (ctrl.x - old_from.x, ctrl.y - old_from.y);
    PtF::new(new_from.x + a * rx - b * ry, new_from.y + b * rx + a * ry)
}

/// Rounded-corner radius of a callout bubble.
pub fn callout_radius(r: RectF) -> f32 {
    (r.w.min(r.h) * 0.25).clamp(4.0, 16.0)
}

/// Where a callout's tail leaves the bubble: the two base points on the side facing the tail.
/// None when the tail point is inside the bubble.
pub fn callout_tail_base(r: RectF, tail: PtF) -> Option<(PtF, PtF)> {
    if r.w < 1.0 || r.h < 1.0 || r.contains(tail) || !tail.x.is_finite() || !tail.y.is_finite() {
        return None;
    }
    let radius = callout_radius(r);
    let c = r.center();
    let (dx, dy) = (tail.x - c.x, tail.y - c.y);
    let (hw, hh) = (r.w / 2.0, r.h / 2.0);
    let horizontal = (dx / hw).abs() >= (dy / hh).abs();
    let inset = 1.0;
    Some(if horizontal {
        let side_x = if dx > 0.0 { r.x + r.w } else { r.x };
        let t = (side_x - c.x) / dx;
        let bw = (r.h * 0.45).clamp(6.0, 28.0);
        let lo = r.y + radius + bw / 2.0;
        let hi = r.y + r.h - radius - bw / 2.0;
        let ay = if lo < hi {
            (c.y + dy * t).clamp(lo, hi)
        } else {
            c.y
        };
        let ix = if dx > 0.0 { -inset } else { inset };
        (
            PtF::new(side_x + ix, ay - bw / 2.0),
            PtF::new(side_x + ix, ay + bw / 2.0),
        )
    } else {
        let side_y = if dy > 0.0 { r.y + r.h } else { r.y };
        let t = (side_y - c.y) / dy;
        let bw = (r.w * 0.2).clamp(6.0, 28.0);
        let lo = r.x + radius + bw / 2.0;
        let hi = r.x + r.w - radius - bw / 2.0;
        let ax = if lo < hi {
            (c.x + dx * t).clamp(lo, hi)
        } else {
            c.x
        };
        let iy = if dy > 0.0 { -inset } else { inset };
        (
            PtF::new(ax - bw / 2.0, side_y + iy),
            PtF::new(ax + bw / 2.0, side_y + iy),
        )
    })
}

/// One laid-out line of a text or callout shape: the byte range of the shape's text it shows
/// and the top-left of its line box (image px). Lines are `size * LINE_HEIGHT` tall.
#[derive(Clone, Debug, PartialEq)]
pub struct TextLine {
    pub range: std::ops::Range<usize>,
    pub origin: PtF,
}

/// Text layout of a textual shape, exactly as the renderer places it: (size, bold, lines).
/// Text shapes break only at '\n'; callouts also wrap at spaces to the bubble width and
/// centre the block vertically.
pub fn text_layout(fonts: &Fonts, kind: &ShapeKind) -> Option<(f32, bool, Vec<TextLine>)> {
    match kind {
        ShapeKind::Text {
            pos,
            text,
            size,
            bold,
            width,
            ..
        } => {
            let size = size.max(1.0);
            let lh = size * crate::fonts::LINE_HEIGHT;
            let lines = text_line_ranges(text, *width, |s| fonts.line_width(s, size, *bold))
                .into_iter()
                .enumerate()
                .map(|(i, range)| TextLine {
                    range,
                    origin: PtF::new(pos.x, pos.y + i as f32 * lh),
                })
                .collect();
            Some((size, *bold, lines))
        }
        ShapeKind::Callout {
            rect, text, size, ..
        } => {
            let size = size.max(1.0);
            let r = norm(*rect);
            let pad = size * CALLOUT_PAD;
            let ranges = wrap_ranges(fonts, text, size, (r.w - 2.0 * pad).max(size));
            let lh = size * crate::fonts::LINE_HEIGHT;
            let top = r.y + (r.h - ranges.len() as f32 * lh) / 2.0;
            let lines = ranges
                .into_iter()
                .enumerate()
                .map(|(i, range)| TextLine {
                    range,
                    origin: PtF::new(r.x + pad, top + i as f32 * lh),
                })
                .collect();
            Some((size, false, lines))
        }
        _ => None,
    }
}

/// Greedy word wrap at single spaces; every paragraph ('\n') starts a new line. Returns the
/// byte range of each line (a break consumes the space it happens at).
pub(crate) fn wrap_ranges(
    fonts: &Fonts,
    text: &str,
    size: f32,
    max_w: f32,
) -> Vec<std::ops::Range<usize>> {
    wrap_with(text, max_w, |s| fonts.line_width(s, size, false))
}

/// Line byte ranges of a text shape: one per '\n' paragraph, and with a fixed box `width`
/// also wrapped at spaces (`wrap_with`). `line_w` measures one line; the editor passes its
/// own measure so its bounds match the renderer.
pub fn text_line_ranges(
    text: &str,
    width: Option<f32>,
    line_w: impl Fn(&str) -> f32,
) -> Vec<std::ops::Range<usize>> {
    match width {
        Some(w) => wrap_with(text, w, line_w),
        None => {
            let mut out = Vec::new();
            let mut start = 0;
            for line in text.split('\n') {
                out.push(start..start + line.len());
                start += line.len() + 1;
            }
            out
        }
    }
}

/// The text a text shape shows, lines joined with '\n' (wrapped when it has a box width).
pub fn shown_text(text: &str, width: Option<f32>, line_w: impl Fn(&str) -> f32) -> String {
    let ranges = text_line_ranges(text, width, line_w);
    let lines: Vec<&str> = ranges.into_iter().map(|r| &text[r]).collect();
    lines.join("\n")
}

/// Greedy word wrap at single spaces to `max_w` (a word wider than that keeps its own line);
/// every paragraph ('\n') starts a new line. Returns the byte range of each line (a break
/// consumes the space it happens at).
pub fn wrap_with(
    text: &str,
    max_w: f32,
    line_w: impl Fn(&str) -> f32,
) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut para_start = 0;
    for para in text.split('\n') {
        let mut line: Option<std::ops::Range<usize>> = None;
        let mut word_start = para_start;
        for word in para.split(' ') {
            let w = word_start..word_start + word.len();
            line = Some(match line {
                None => w.clone(),
                Some(l) => {
                    let candidate = l.start..w.end;
                    if !text[l.clone()].is_empty() && line_w(&text[candidate.clone()]) > max_w {
                        out.push(l);
                        w.clone()
                    } else {
                        candidate
                    }
                }
            });
            word_start = w.end + 1;
        }
        out.push(line.unwrap_or(para_start..para_start));
        para_start += para.len() + 1;
    }
    out
}

pub(crate) fn dist_to_segment(p: PtF, a: PtF, b: PtF) -> f32 {
    let (abx, aby) = (b.x - a.x, b.y - a.y);
    let len2 = abx * abx + aby * aby;
    let t = if len2 <= f32::EPSILON {
        0.0
    } else {
        (((p.x - a.x) * abx + (p.y - a.y) * aby) / len2).clamp(0.0, 1.0)
    };
    let (cx, cy) = (a.x + abx * t, a.y + aby * t);
    ((p.x - cx).powi(2) + (p.y - cy).powi(2)).sqrt()
}

fn dist_to_polyline(p: PtF, pts: &[PtF]) -> f32 {
    match pts {
        [] => f32::INFINITY,
        [one] => ((p.x - one.x).powi(2) + (p.y - one.y).powi(2)).sqrt(),
        _ => pts
            .windows(2)
            .map(|w| dist_to_segment(p, w[0], w[1]))
            .fold(f32::INFINITY, f32::min),
    }
}

/// Distance from `p` to the border of `r` (0 on the border, positive both inside and out).
fn dist_to_rect_border(p: PtF, r: RectF) -> f32 {
    let dx = (p.x - r.x).abs().min((p.x - (r.x + r.w)).abs());
    let dy = (p.y - r.y).abs().min((p.y - (r.y + r.h)).abs());
    let inside_x = p.x >= r.x && p.x <= r.x + r.w;
    let inside_y = p.y >= r.y && p.y <= r.y + r.h;
    match (inside_x, inside_y) {
        (true, true) => dx.min(dy),
        (true, false) => dy,
        (false, true) => dx,
        (false, false) => (dx * dx + dy * dy).sqrt(),
    }
}

/// First-order distance from `p` to the ellipse inscribed in `r`.
fn dist_to_ellipse_border(p: PtF, r: RectF) -> f32 {
    let (a, b) = ((r.w / 2.0).max(0.5), (r.h / 2.0).max(0.5));
    let c = r.center();
    let (x, y) = (p.x - c.x, p.y - c.y);
    let f = (x / a).powi(2) + (y / b).powi(2) - 1.0;
    let gx = 2.0 * x / (a * a);
    let gy = 2.0 * y / (b * b);
    let g = (gx * gx + gy * gy).sqrt();
    if g < 1e-6 { a.min(b) } else { f.abs() / g }
}

fn inside_ellipse(p: PtF, r: RectF, grow: f32) -> bool {
    let (a, b) = ((r.w / 2.0 + grow).max(0.5), (r.h / 2.0 + grow).max(0.5));
    let c = r.center();
    ((p.x - c.x) / a).powi(2) + ((p.y - c.y) / b).powi(2) <= 1.0
}

/// Normalize a possibly negative-sized rect.
fn norm(r: RectF) -> RectF {
    let x0 = r.x.min(r.x + r.w);
    let y0 = r.y.min(r.y + r.h);
    RectF::new(x0, y0, r.w.abs(), r.h.abs())
}

fn points_bounds(pts: &[PtF]) -> RectF {
    let Some(first) = pts.first() else {
        return RectF::default();
    };
    let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x, first.y);
    for p in pts {
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    RectF::new(x0, y0, x1 - x0, y1 - y0)
}

fn union(a: RectF, b: RectF) -> RectF {
    let x0 = a.x.min(b.x);
    let y0 = a.y.min(b.y);
    let x1 = (a.x + a.w).max(b.x + b.w);
    let y1 = (a.y + a.h).max(b.y + b.h);
    RectF::new(x0, y0, x1 - x0, y1 - y0)
}

/// Rectangle covering a text block (and its pill when `background` is set).
pub(crate) fn text_rect(
    fonts: &Fonts,
    pos: PtF,
    text: &str,
    size: f32,
    background: bool,
    bold: bool,
    width: Option<f32>,
) -> RectF {
    let (w, h) = match width {
        None => fonts.measure(text, size, bold),
        Some(bw) => {
            let lw = |s: &str| fonts.line_width(s, size, bold);
            let ranges = text_line_ranges(text, width, lw);
            let widest = ranges
                .iter()
                .map(|r| lw(&text[r.clone()]))
                .fold(0.0, f32::max);
            (
                bw.max(widest),
                ranges.len() as f32 * crate::fonts::LINE_HEIGHT * size,
            )
        }
    };
    let r = RectF::new(pos.x, pos.y, w, h);
    if background {
        r.expand(0.0).pad(size * TEXT_PAD_X, size * TEXT_PAD_Y)
    } else {
        r
    }
}

trait RectPad {
    fn pad(&self, px: f32, py: f32) -> RectF;
}
impl RectPad for RectF {
    fn pad(&self, px: f32, py: f32) -> RectF {
        RectF::new(
            self.x - px,
            self.y - py,
            self.w + 2.0 * px,
            self.h + 2.0 * py,
        )
    }
}

/// A pointer shape for `image` with its top-left at `pos` (image px), drawn 1:1.
pub fn cursor_shape(pos: PtF, image: CursorImage) -> (ShapeKind, Style) {
    let rect = RectF::new(pos.x, pos.y, image.width() as f32, image.height() as f32);
    (
        ShapeKind::Cursor { rect, image },
        Style {
            width: 0.0,
            shadow: false,
            ..Style::default()
        },
    )
}

impl Doc {
    pub const VERSION: u32 = 1;

    pub fn new(base_width: u32, base_height: u32) -> Self {
        Self {
            version: Self::VERSION,
            base_width,
            base_height,
            shapes: Vec::new(),
            next_id: 1,
            baked: Vec::new(),
        }
    }
    /// Append a shape, assigning its id. Returns the id.
    pub fn add(&mut self, kind: ShapeKind, style: Style) -> ShapeId {
        let id = self.next_id;
        self.next_id += 1;
        self.shapes.push(Shape { id, kind, style });
        id
    }
    pub fn get(&self, id: ShapeId) -> Option<&Shape> {
        self.shapes.iter().find(|s| s.id == id)
    }
    pub fn get_mut(&mut self, id: ShapeId) -> Option<&mut Shape> {
        self.shapes.iter_mut().find(|s| s.id == id)
    }
    pub fn remove(&mut self, id: ShapeId) -> Option<Shape> {
        let i = self.shapes.iter().position(|s| s.id == id)?;
        Some(self.shapes.remove(i))
    }
    /// Next step number = max existing + 1.
    pub fn next_step_number(&self) -> u32 {
        self.shapes
            .iter()
            .filter_map(|s| {
                if let ShapeKind::Step { n, .. } = s.kind {
                    Some(n)
                } else {
                    None
                }
            })
            .max()
            .unwrap_or(0)
            + 1
    }
    /// Renumber Step shapes 1..N in z-order (after a deletion).
    pub fn renumber_steps(&mut self) {
        let mut n = 0;
        for s in &mut self.shapes {
            if let ShapeKind::Step { n: sn, .. } = &mut s.kind {
                n += 1;
                *sn = n;
            }
        }
    }
    /// Topmost shape whose visual hit area contains `p` (tolerance in image px).
    ///
    /// Hit areas: strokes (line, arrow, pen, highlighter, unfilled rect/ellipse) hit within
    /// half the stroke width plus the tolerance of the stroke; filled shapes, text, step
    /// circles, redaction rects and magnifier insets hit anywhere inside. A spotlight only
    /// hits near its border so it never swallows clicks on the shapes it frames.
    pub fn hit_test(&self, p: PtF, tolerance: f32) -> Option<ShapeId> {
        let tol = tolerance.max(0.0);
        self.shapes
            .iter()
            .rev()
            .find(|s| shape_hit(s, p, tol))
            .map(|s| s.id)
    }
    pub fn to_json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }
    pub fn from_json(s: &str) -> anyhow::Result<Doc> {
        Ok(serde_json::from_str(s)?)
    }
}

fn shape_hit(shape: &Shape, p: PtF, tol: f32) -> bool {
    let half = shape.style.width.max(0.0) / 2.0;
    match &shape.kind {
        ShapeKind::Arrow {
            from,
            to,
            ctrl,
            double,
            head,
        } => {
            if dist_to_polyline(p, &arrow_centerline(*from, *to, *ctrl)) <= half + tol {
                return true;
            }
            arrow_parts(*from, *to, *ctrl, shape.style.width, *double, *head).is_some_and(|parts| {
                parts
                    .heads
                    .iter()
                    .any(|g| dist_to_segment(p, g.notch, g.tip) <= g.head_half_width * 0.7 + tol)
            })
        }
        ShapeKind::Line { from, to } => dist_to_segment(p, *from, *to) <= half + tol,
        ShapeKind::Pen { points } | ShapeKind::Highlighter { points } => {
            dist_to_polyline(p, points) <= half + tol
        }
        ShapeKind::Rect { rect, .. } => {
            let r = norm(*rect);
            if shape.style.fill.is_some() {
                r.expand(half + tol).contains(p)
            } else {
                dist_to_rect_border(p, r) <= half + tol
            }
        }
        ShapeKind::Ellipse { rect } => {
            let r = norm(*rect);
            if shape.style.fill.is_some() {
                inside_ellipse(p, r, half + tol)
            } else {
                dist_to_ellipse_border(p, r) <= half + tol
            }
        }
        ShapeKind::Text {
            pos,
            text,
            size,
            background,
            width,
            ..
        } => {
            // Fonts are not available here; approximate with a conservative glyph box.
            let glyph = size * 0.52;
            let (w, lines) = match width {
                Some(bw) => {
                    let per_line = (bw / glyph).max(1.0);
                    let lines: f32 = text
                        .split('\n')
                        .map(|l| (l.chars().count() as f32 / per_line).ceil().max(1.0))
                        .sum();
                    (*bw, lines)
                }
                None => {
                    let longest = text
                        .split('\n')
                        .map(|l| l.chars().count())
                        .max()
                        .unwrap_or(0) as f32;
                    (longest * glyph, text.split('\n').count().max(1) as f32)
                }
            };
            let h = lines * size * crate::fonts::LINE_HEIGHT;
            let mut r = RectF::new(pos.x, pos.y, w, h);
            if background.is_some() {
                r = r.pad(size * TEXT_PAD_X, size * TEXT_PAD_Y);
            }
            r.expand(tol).contains(p)
        }
        ShapeKind::Step { center, radius, .. } => {
            ((p.x - center.x).powi(2) + (p.y - center.y).powi(2)).sqrt() <= radius + tol
        }
        ShapeKind::Blur { rect, .. }
        | ShapeKind::Pixelate { rect, .. }
        | ShapeKind::Redact { rect }
        | ShapeKind::HighlightBox { rect } => norm(*rect).expand(tol).contains(p),
        ShapeKind::Magnifier { dest, .. } => {
            let d = norm(*dest);
            let ring = half.max(2.0);
            d.expand(ring + tol).contains(p)
        }
        ShapeKind::Callout { rect, tail, .. } => {
            let r = norm(*rect);
            if r.expand(half + tol).contains(p) {
                return true;
            }
            // The tail: from where it leaves the bubble out to its tip.
            let root = PtF::new(tail.x.clamp(r.x, r.x + r.w), tail.y.clamp(r.y, r.y + r.h));
            dist_to_segment(p, root, *tail) <= 8.0 + tol
        }
        ShapeKind::Spotlight { rect, .. } => dist_to_rect_border(p, norm(*rect)) <= tol.max(6.0),
        ShapeKind::Cursor { rect, .. } => norm(*rect).expand(tol).contains(p),
    }
}

/// Axis-aligned bounds of a shape including stroke width (image px).
pub fn shape_bounds(shape: &Shape, fonts: &Fonts) -> RectF {
    let half = shape.style.width.max(0.0) / 2.0;
    match &shape.kind {
        ShapeKind::Arrow {
            from,
            to,
            ctrl,
            double,
            head,
        } => {
            let mut r = points_bounds(&arrow_centerline(*from, *to, *ctrl)).expand(half);
            if let Some(parts) = arrow_parts(*from, *to, *ctrl, shape.style.width, *double, *head) {
                for g in &parts.heads {
                    r = union(
                        r,
                        points_bounds(&[g.tip, g.left, g.right]).expand(half * 0.5),
                    );
                }
            }
            r
        }
        ShapeKind::Line { from, to } => RectF::from_points(*from, *to).expand(half),
        ShapeKind::Pen { points } | ShapeKind::Highlighter { points } => {
            points_bounds(points).expand(half)
        }
        ShapeKind::Rect { rect, .. } | ShapeKind::Ellipse { rect } => norm(*rect).expand(half),
        ShapeKind::Text {
            pos,
            text,
            size,
            background,
            bold,
            width,
        } => text_rect(
            fonts,
            *pos,
            text,
            *size,
            background.is_some(),
            *bold,
            *width,
        ),
        ShapeKind::Step { center, radius, .. } => RectF::new(
            center.x - radius,
            center.y - radius,
            radius * 2.0,
            radius * 2.0,
        ),
        ShapeKind::Blur { rect, .. }
        | ShapeKind::Pixelate { rect, .. }
        | ShapeKind::Redact { rect }
        | ShapeKind::HighlightBox { rect } => norm(*rect),
        ShapeKind::Magnifier { dest, .. } => {
            norm(*dest).expand(render::magnifier_ring_width(&shape.style))
        }
        ShapeKind::Callout { rect, tail, .. } => {
            let r = norm(*rect).expand(half);
            union(r, RectF::new(tail.x, tail.y, 0.0, 0.0))
        }
        ShapeKind::Spotlight { rect, .. } | ShapeKind::Cursor { rect, .. } => norm(*rect),
    }
}

/// Move a shape by (dx, dy) image px.
pub fn translate_shape(kind: &mut ShapeKind, dx: f32, dy: f32) {
    let mv = |p: &mut PtF| {
        p.x += dx;
        p.y += dy;
    };
    let mvr = |r: &mut RectF| {
        r.x += dx;
        r.y += dy;
    };
    match kind {
        ShapeKind::Arrow { from, to, ctrl, .. } => {
            mv(from);
            mv(to);
            if let Some(c) = ctrl {
                mv(c);
            }
        }
        ShapeKind::Line { from, to } => {
            mv(from);
            mv(to);
        }
        ShapeKind::Rect { rect, .. } | ShapeKind::Ellipse { rect } => mvr(rect),
        ShapeKind::Pen { points } | ShapeKind::Highlighter { points } => {
            points.iter_mut().for_each(mv)
        }
        ShapeKind::Text { pos, .. } => mv(pos),
        ShapeKind::Step { center, .. } => mv(center),
        ShapeKind::Blur { rect, .. }
        | ShapeKind::Pixelate { rect, .. }
        | ShapeKind::Redact { rect }
        | ShapeKind::HighlightBox { rect }
        | ShapeKind::Spotlight { rect, .. }
        | ShapeKind::Cursor { rect, .. } => mvr(rect),
        // The magnifier's source stays put: moving the inset must not change what it magnifies.
        ShapeKind::Magnifier { dest, .. } => mvr(dest),
        ShapeKind::Callout { rect, tail, .. } => {
            mvr(rect);
            mv(tail);
        }
    }
}

/// Flatten: base + all shapes -> new image of the same size.
/// Order of operations (so redaction is real and can't be undone from the output):
///   1. Copy base.
///   2. Apply destructive shapes (Blur, Pixelate, Redact) in z-order directly to pixels.
///   3. Draw Spotlight dimming.
///   4. Draw remaining vector shapes in z-order (anti-aliased, tiny-skia), text with ab_glyph.
///      Magnifier samples from the result of step 2 (so redactions stay redacted when magnified).
pub fn render(base: &Rgba, doc: &Doc, fonts: &Fonts) -> Rgba {
    let shapes: Vec<&Shape> = doc.shapes.iter().collect();
    render::render_shapes(base, &shapes, &doc.baked, fonts, false)
}

/// The part `region` (image px, clipped to the image) of `render(base, doc)`, drawn over
/// `work`, which must be `bake_destructive(base, doc)` for the same doc. Vector shapes,
/// spotlights and redact boxes are drawn; the destructive pixels come from `work`. The editor
/// keeps `work` while only vector shapes change and re-renders just the area an edit touched.
pub fn render_region(work: &Rgba, doc: &Doc, fonts: &Fonts, region: PxRect) -> Rgba {
    let shapes: Vec<&Shape> = doc.shapes.iter().collect();
    render::render_over_region(work, &shapes, fonts, region)
}

/// One non-destructive shape (text, callout, arrow...) alone on a transparent canvas just
/// big enough for it, at integer image px `origin`. The editor shows a text shape being typed
/// with this, so it looks exactly like the final render. None for destructive shapes,
/// magnifiers and spotlights (they need the image) or degenerate shapes.
pub fn render_shape_patch(shape: &Shape, fonts: &Fonts) -> Option<(PxRect, Rgba)> {
    render::render_patch(shape, fonts)
}

/// Render only shapes in `ids` order onto a transparent canvas the size of the base
/// (used by the editor to preview a dragged shape cheaply). Destructive shapes render
/// their effect using `base` pixels.
pub fn render_subset(base: &Rgba, doc: &Doc, ids: &[ShapeId], fonts: &Fonts) -> Rgba {
    let shapes: Vec<&Shape> = ids.iter().filter_map(|id| doc.get(*id)).collect();
    render::render_shapes(base, &shapes, &doc.baked, fonts, true)
}

// ---------------------------------------------------------------------------------------
// Sidecars
// ---------------------------------------------------------------------------------------

/// Sidecar locations keyed by the full file name (`Foo.png.json`), so two captures that
/// share a stem but differ in extension never share a sidecar.
fn sidecar_paths(flat_path: &Path) -> Option<(PathBuf, PathBuf)> {
    let name = flat_path.file_name()?.to_string_lossy().to_string();
    let dir = sidecar_dir(flat_path);
    Some((
        dir.join(format!("{name}.json")),
        dir.join(format!("{name}.base.png")),
    ))
}

fn sidecar_dir(flat_path: &Path) -> PathBuf {
    flat_path
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(".stinkshot")
        .join("edit")
}

/// Pre-extension-keyed layout (`Foo.json`), still read for captures made by older builds.
fn legacy_sidecar_paths(flat_path: &Path) -> Option<(PathBuf, PathBuf)> {
    let stem = flat_path.file_stem()?.to_string_lossy().to_string();
    let dir = sidecar_dir(flat_path);
    Some((
        dir.join(format!("{stem}.json")),
        dir.join(format!("{stem}.base.png")),
    ))
}

/// True when another capture with a different extension shares this file's stem, in which
/// case a legacy stem-keyed sidecar cannot be attributed to this file.
fn stem_shared(flat_path: &Path) -> bool {
    let (Some(stem), Some(own), Some(dir)) = (
        flat_path.file_stem(),
        flat_path.file_name(),
        flat_path.parent(),
    ) else {
        return false;
    };
    ["png", "jpg", "jpeg", "webp"].iter().any(|ext| {
        let mut name = stem.to_os_string();
        name.push(".");
        name.push(ext);
        name != own && dir.join(&name).exists()
    })
}

/// Sidecar paths that currently exist for this file (current layout, plus a legacy one
/// that is unambiguously this file's). Used for size accounting and cleanup.
pub fn editable_files(flat_path: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut take = |pair: Option<(PathBuf, PathBuf)>| {
        if let Some((j, b)) = pair {
            out.extend([j, b].into_iter().filter(|p| p.is_file()));
        }
    };
    take(sidecar_paths(flat_path));
    if !stem_shared(flat_path) {
        take(legacy_sidecar_paths(flat_path));
    }
    out
}

/// True when the flat file was written after its sidecar: the name was reused by another
/// picture (the capture was deleted or replaced outside the app), so the sidecar holds some
/// other image's edits. The app always writes the flat file first.
fn sidecar_is_stale(flat_path: &Path, json_path: &Path) -> bool {
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let (Some(flat), Some(side)) = (modified(flat_path), modified(json_path)) else {
        return false;
    };
    flat.duration_since(side)
        .is_ok_and(|d| d > std::time::Duration::from_secs(2))
}

/// The base with every destructive shape (Blur, Pixelate, Redact) already applied: what an
/// edit sidecar stores, so the pixels a redaction hides never reach the disk. The shapes stay
/// in the doc as editable objects; moving or deleting one later leaves its old spot redacted.
pub fn bake_destructive(base: &Rgba, doc: &Doc) -> Rgba {
    let mut out = base.clone();
    let shapes: Vec<&Shape> = doc.shapes.iter().collect();
    render::apply_destructive(&mut out, &shapes, &doc.baked);
    out
}

/// Write the edit sidecar. The stored base has the destructive shapes baked in (see
/// [`bake_destructive`]), so a redacted secret is not recoverable from the history folder.
pub fn save_editable(flat_path: &Path, base: &Rgba, doc: &Doc) -> anyhow::Result<()> {
    let (json_path, base_path) = sidecar_paths(flat_path)
        .with_context(|| format!("bad capture path {}", flat_path.display()))?;
    let baked = bake_destructive(base, doc);
    let png = baked.to_png().context("encoding base image")?;
    // The stored base now holds every destructive shape; say so, or the next save would
    // apply them a second time.
    let mut stored = doc.clone();
    stored.baked = doc
        .shapes
        .iter()
        .filter(|s| render::is_destructive(&s.kind))
        .cloned()
        .collect();
    let json = stored.to_json().context("serializing annotations")?;
    // Base first: a doc without its base is useless, a base without a doc is ignored.
    crate::image::write_atomic(&base_path, &png)?;
    crate::image::write_atomic(&json_path, json.as_bytes())?;
    Ok(())
}

/// None if no sidecar exists for this file.
pub fn load_editable(flat_path: &Path) -> anyhow::Result<Option<(Rgba, Doc)>> {
    let Some((mut json_path, mut base_path)) = sidecar_paths(flat_path) else {
        return Ok(None);
    };
    if !json_path.is_file()
        && !base_path.is_file()
        && !stem_shared(flat_path)
        && let Some((j, b)) = legacy_sidecar_paths(flat_path)
    {
        (json_path, base_path) = (j, b);
    }
    if !json_path.is_file() || !base_path.is_file() {
        if json_path.is_file() != base_path.is_file() {
            log::warn!("incomplete edit sidecar for {}", flat_path.display());
        }
        return Ok(None);
    }
    if sidecar_is_stale(flat_path, &json_path) {
        log::warn!(
            "edit sidecar for {} is older than the file, ignoring it",
            flat_path.display()
        );
        return Ok(None);
    }
    let json = std::fs::read_to_string(&json_path)
        .with_context(|| format!("reading {}", json_path.display()))?;
    let doc = Doc::from_json(&json).with_context(|| format!("parsing {}", json_path.display()))?;
    if doc.version > Doc::VERSION {
        log::warn!(
            "annotation doc version {} is newer than supported {}",
            doc.version,
            Doc::VERSION
        );
    }
    let base = Rgba::load(&base_path)?;
    Ok(Some((base, doc)))
}

pub fn delete_editable(flat_path: &Path) -> anyhow::Result<()> {
    for p in editable_files(flat_path) {
        match std::fs::remove_file(&p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("deleting {}", p.display())),
        }
    }
    Ok(())
}

pub fn has_editable(flat_path: &Path) -> bool {
    let has = |pair: Option<(PathBuf, PathBuf)>| {
        pair.is_some_and(|(j, b)| j.is_file() && b.is_file() && !sidecar_is_stale(flat_path, &j))
    };
    has(sidecar_paths(flat_path))
        || (!stem_shared(flat_path) && has(legacy_sidecar_paths(flat_path)))
}

#[cfg(test)]
mod tests;
