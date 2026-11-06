//! Region overlay + inline annotation editor (docs/design/Main.dc.html).
//!
//! One borderless, topmost, tool-window viewport covering `frozen.bounds` exactly in
//! physical px (use `window_util::set_rect` in `on_created`; the egui scale factor of the
//! spanning window may differ from other monitors, so convert physical px <-> points with
//! `ctx.pixels_per_point()` and draw the frozen texture 1:1 in physical pixels).
//!
//! Behavior summary (full spec in docs/ARCHITECTURE.md, "Overlay"):
//! - Frozen screenshot fills the window; outside the selection is dimmed (palette.overlay_dim).
//! - Hover: highlight the UI element (via `uia::ElementSnapper`) or window under the cursor;
//!   wheel cycles element -> parent -> window; click selects it. Drag draws a free region.
//! - Selection has 8 handles; Space+drag moves; arrows nudge 1px (Shift = 10px, Ctrl =
//!   resize). Size label + aspect presets (Free, 16:9, 4:3, 1:1, exact WxH input) above it.
//! - Loupe near the cursor: 11x11 px grid zoomed, coords, scale, hex color; C copies hex.
//! - After a selection exists, the tool bar (design order) appears below it (or above /
//!   inside when no room); action bar on its right: OCR, Pin, Save, Cancel, Copy (Enter).
//! - Tools create `annotate::ShapeKind`s in base-image px; shapes are selectable, movable,
//!   resizable, deletable (Del), recolorable; Ctrl+Z / Ctrl+Y undo/redo; text editing inline.
//! - Committed shapes render through `stinkshot_core::annotate::render` into a texture
//!   (re-rendered on change, off the UI thread if > 16 ms); the shape being dragged is
//!   previewed with the egui painter.
//! - Finish pushes `Command::CaptureDone` / `RegionChosen` / `Cancelled` and returns Close.

use crate::state::{OverlayRequest, Shared, Surface, SurfaceStatus};

pub struct Overlay {
    #[allow(dead_code)]
    req: OverlayRequest,
}

impl Overlay {
    pub fn new(req: OverlayRequest, shared: &Shared) -> Self {
        let _ = shared;
        Self { req }
    }
}

impl Surface for Overlay {
    fn title(&self) -> String {
        "StinkShot.Overlay".into()
    }
    fn viewport_builder(&self, shared: &Shared) -> egui::ViewportBuilder {
        let _ = shared;
        todo!("overlay::viewport_builder")
    }
    fn show(&mut self, ctx: &egui::Context, shared: &mut Shared) -> SurfaceStatus {
        let _ = (ctx, shared);
        todo!("overlay::show")
    }
}
