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
use crate::geom::{PtF, RectF};
use crate::image::Rgba;

mod render;

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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ShapeKind {
    Arrow {
        from: PtF,
        to: PtF,
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
    /// Top-left anchored text; `size` in image px. Supports '\n'.
    Text {
        pos: PtF,
        text: String,
        size: f32,
        background: Option<[u8; 4]>,
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

/// Arrow head geometry derived from the stroke width.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ArrowGeom {
    pub tip: PtF,
    /// Where the shaft ends (the notch at the back of the head).
    pub notch: PtF,
    pub left: PtF,
    pub right: PtF,
    pub head_half_width: f32,
}

pub(crate) fn arrow_geom(from: PtF, to: PtF, width: f32) -> Option<ArrowGeom> {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let len = (dx * dx + dy * dy).sqrt();
    if !len.is_finite() || len < 0.5 {
        return None;
    }
    let (ux, uy) = (dx / len, dy / len);
    let (px, py) = (-uy, ux);
    let head_len = (width.max(1.0) * 3.2 + 8.0).min(len);
    let half = head_len * 0.46;
    let back = |t: f32, s: f32| PtF::new(to.x - ux * t + px * s, to.y - uy * t + py * s);
    Some(ArrowGeom {
        tip: to,
        notch: back(head_len * 0.76, 0.0),
        left: back(head_len, half),
        right: back(head_len, -half),
        head_half_width: half,
    })
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
pub(crate) fn text_rect(fonts: &Fonts, pos: PtF, text: &str, size: f32, background: bool) -> RectF {
    let (w, h) = fonts.measure(text, size, false);
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

impl Doc {
    pub const VERSION: u32 = 1;

    pub fn new(base_width: u32, base_height: u32) -> Self {
        Self {
            version: Self::VERSION,
            base_width,
            base_height,
            shapes: Vec::new(),
            next_id: 1,
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
        ShapeKind::Arrow { from, to } => {
            if dist_to_segment(p, *from, *to) <= half + tol {
                return true;
            }
            match arrow_geom(*from, *to, shape.style.width) {
                Some(g) => dist_to_segment(p, g.notch, g.tip) <= g.head_half_width * 0.7 + tol,
                None => false,
            }
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
        } => {
            // Fonts are not available here; approximate with a conservative glyph box.
            let lines = text.split('\n').count().max(1) as f32;
            let longest = text
                .split('\n')
                .map(|l| l.chars().count())
                .max()
                .unwrap_or(0) as f32;
            let w = longest * size * 0.52;
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
        | ShapeKind::Redact { rect } => norm(*rect).expand(tol).contains(p),
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
    }
}

/// Axis-aligned bounds of a shape including stroke width (image px).
pub fn shape_bounds(shape: &Shape, fonts: &Fonts) -> RectF {
    let half = shape.style.width.max(0.0) / 2.0;
    match &shape.kind {
        ShapeKind::Arrow { from, to } => {
            let mut r = RectF::from_points(*from, *to).expand(half);
            if let Some(g) = arrow_geom(*from, *to, shape.style.width) {
                r = union(r, points_bounds(&[g.tip, g.left, g.right]));
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
        } => text_rect(fonts, *pos, text, *size, background.is_some()),
        ShapeKind::Step { center, radius, .. } => RectF::new(
            center.x - radius,
            center.y - radius,
            radius * 2.0,
            radius * 2.0,
        ),
        ShapeKind::Blur { rect, .. }
        | ShapeKind::Pixelate { rect, .. }
        | ShapeKind::Redact { rect } => norm(*rect),
        ShapeKind::Magnifier { dest, .. } => {
            norm(*dest).expand(render::magnifier_ring_width(&shape.style))
        }
        ShapeKind::Callout { rect, tail, .. } => {
            let r = norm(*rect).expand(half);
            union(r, RectF::new(tail.x, tail.y, 0.0, 0.0))
        }
        ShapeKind::Spotlight { rect, .. } => norm(*rect),
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
        ShapeKind::Arrow { from, to } | ShapeKind::Line { from, to } => {
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
        | ShapeKind::Spotlight { rect, .. } => mvr(rect),
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
    render::render_shapes(base, &shapes, fonts, false)
}

/// Render only shapes in `ids` order onto a transparent canvas the size of the base
/// (used by the editor to preview a dragged shape cheaply). Destructive shapes render
/// their effect using `base` pixels.
pub fn render_subset(base: &Rgba, doc: &Doc, ids: &[ShapeId], fonts: &Fonts) -> Rgba {
    let shapes: Vec<&Shape> = ids.iter().filter_map(|id| doc.get(*id)).collect();
    render::render_shapes(base, &shapes, fonts, true)
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

/// The base with every destructive shape (Blur, Pixelate, Redact) already applied: what an
/// edit sidecar stores, so the pixels a redaction hides never reach the disk. The shapes stay
/// in the doc as editable objects; moving or deleting one later leaves its old spot redacted.
pub fn bake_destructive(base: &Rgba, doc: &Doc) -> Rgba {
    let mut out = base.clone();
    let shapes: Vec<&Shape> = doc.shapes.iter().collect();
    render::apply_destructive(&mut out, &shapes);
    out
}

/// Write the edit sidecar. The stored base has the destructive shapes baked in (see
/// [`bake_destructive`]), so a redacted secret is not recoverable from the history folder.
pub fn save_editable(flat_path: &Path, base: &Rgba, doc: &Doc) -> anyhow::Result<()> {
    let (json_path, base_path) = sidecar_paths(flat_path)
        .with_context(|| format!("bad capture path {}", flat_path.display()))?;
    let baked = bake_destructive(base, doc);
    let png = baked.to_png().context("encoding base image")?;
    let json = doc.to_json().context("serializing annotations")?;
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
    let has =
        |pair: Option<(PathBuf, PathBuf)>| pair.is_some_and(|(j, b)| j.is_file() && b.is_file());
    has(sidecar_paths(flat_path))
        || (!stem_shared(flat_path) && has(legacy_sidecar_paths(flat_path)))
}

#[cfg(test)]
mod tests;
