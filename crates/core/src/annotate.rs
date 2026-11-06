//! Annotation document model, hit testing, rendering, and re-editable sidecars.
//!
//! All coordinates are float pixels in the BASE IMAGE's space (the cropped capture,
//! physical pixels), never screen or egui points.
//!
//! Re-edit storage for a saved capture `C:\..\Shot.png`:
//!   - `Shot.png`                          flattened result (what the user sees in the folder)
//!   - `<folder>\.stinkshot\edit\<stem>.json` the `Doc`
//!   - `<folder>\.stinkshot\edit\<stem>.base.png` the unannotated original
//! `save_editable` / `load_editable` / `delete_editable` own this layout.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::fonts::Fonts;
use crate::geom::{PtF, RectF};
use crate::image::Rgba;

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
        Self { color: [0xff, 0x8a, 0x3d, 0xff], width: 4.0, fill: None, shadow: true }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ShapeKind {
    Arrow { from: PtF, to: PtF },
    Line { from: PtF, to: PtF },
    Rect { rect: RectF, corner_radius: f32 },
    Ellipse { rect: RectF },
    /// Freehand; points already smoothed by the editor.
    Pen { points: Vec<PtF> },
    /// Wide, translucent (multiply-like) stroke; style.width is the marker width.
    Highlighter { points: Vec<PtF> },
    /// Top-left anchored text; `size` in image px. Supports '\n'.
    Text { pos: PtF, text: String, size: f32, background: Option<[u8; 4]> },
    /// Numbered circle; `n` assigned at creation and renumbered by `Doc::renumber_steps`.
    Step { center: PtF, n: u32, radius: f32 },
    /// Gaussian-ish blur of the base pixels inside rect. Destructive in output.
    Blur { rect: RectF, radius: f32 },
    /// Mosaic of `block`-px cells inside rect. Destructive in output.
    Pixelate { rect: RectF, block: u32 },
    /// Opaque solid box in style.color. Destructive in output.
    Redact { rect: RectF },
    /// Zoomed copy of `source` region drawn into the circle/rounded box at `dest`.
    Magnifier { source: RectF, dest: RectF, circular: bool },
    /// Speech bubble with text; tail points at `tail`.
    Callout { rect: RectF, tail: PtF, text: String, size: f32 },
    /// Dims everything outside `rect` (multiple spotlights union their clear areas).
    Spotlight { rect: RectF, dim: f32 },
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

impl Doc {
    pub const VERSION: u32 = 1;

    pub fn new(base_width: u32, base_height: u32) -> Self {
        Self { version: Self::VERSION, base_width, base_height, shapes: Vec::new(), next_id: 1 }
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
            .filter_map(|s| if let ShapeKind::Step { n, .. } = s.kind { Some(n) } else { None })
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
    pub fn hit_test(&self, p: PtF, tolerance: f32) -> Option<ShapeId> {
        let _ = (p, tolerance);
        todo!("core::annotate::Doc::hit_test")
    }
    pub fn to_json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }
    pub fn from_json(s: &str) -> anyhow::Result<Doc> {
        Ok(serde_json::from_str(s)?)
    }
}

/// Axis-aligned bounds of a shape including stroke width (image px).
pub fn shape_bounds(shape: &Shape, fonts: &Fonts) -> RectF {
    let _ = (shape, fonts);
    todo!("core::annotate::shape_bounds")
}

/// Move a shape by (dx, dy) image px.
pub fn translate_shape(kind: &mut ShapeKind, dx: f32, dy: f32) {
    let _ = (kind, dx, dy);
    todo!("core::annotate::translate_shape")
}

/// Flatten: base + all shapes -> new image of the same size.
/// Order of operations (so redaction is real and can't be undone from the output):
///   1. Copy base.
///   2. Apply destructive shapes (Blur, Pixelate, Redact) in z-order directly to pixels.
///   3. Draw Spotlight dimming.
///   4. Draw remaining vector shapes in z-order (anti-aliased, tiny-skia), text with ab_glyph.
///   Magnifier samples from the result of step 2 (so redactions stay redacted when magnified).
pub fn render(base: &Rgba, doc: &Doc, fonts: &Fonts) -> Rgba {
    let _ = (base, doc, fonts);
    todo!("core::annotate::render")
}

/// Render only shapes in `ids` order onto a transparent canvas the size of the base
/// (used by the editor to preview a dragged shape cheaply). Destructive shapes render
/// their effect using `base` pixels.
pub fn render_subset(base: &Rgba, doc: &Doc, ids: &[ShapeId], fonts: &Fonts) -> Rgba {
    let _ = (base, doc, ids, fonts);
    todo!("core::annotate::render_subset")
}

pub fn save_editable(flat_path: &Path, base: &Rgba, doc: &Doc) -> anyhow::Result<()> {
    let _ = (flat_path, base, doc);
    todo!("core::annotate::save_editable")
}

/// None if no sidecar exists for this file.
pub fn load_editable(flat_path: &Path) -> anyhow::Result<Option<(Rgba, Doc)>> {
    let _ = flat_path;
    todo!("core::annotate::load_editable")
}

pub fn delete_editable(flat_path: &Path) -> anyhow::Result<()> {
    let _ = flat_path;
    todo!("core::annotate::delete_editable")
}

pub fn has_editable(flat_path: &Path) -> bool {
    let _ = flat_path;
    todo!("core::annotate::has_editable")
}
