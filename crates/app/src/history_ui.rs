//! History window (docs/design/History.dc.html): sidebar filters (All, Starred,
//! Recordings, by source app with counts, retention summary), search box (FTS over OCR
//! text/title/app, debounced 150 ms), date filter, virtualized thumbnail grid (thumbnails
//! loaded lazily on a worker via `History::thumbnail`), per-item actions: Edit, Pin, Copy
//! image, Copy text, Open, Show in folder, Star, Delete (with confirm). Enter = Edit.

use crate::state::{Shared, Surface, SurfaceStatus};

pub struct HistoryWindow {}

impl HistoryWindow {
    pub fn new(shared: &Shared) -> Self {
        let _ = shared;
        Self {}
    }
    /// Shell calls this after history changes (new capture, OCR finished) to refresh.
    pub fn invalidate(&mut self) {}
}

impl Surface for HistoryWindow {
    fn title(&self) -> String {
        "StinkShot History".into()
    }
    fn viewport_builder(&self, shared: &Shared) -> egui::ViewportBuilder {
        let _ = shared;
        todo!("history_ui::viewport_builder")
    }
    fn show(&mut self, ctx: &egui::Context, shared: &mut Shared) -> SurfaceStatus {
        let _ = (ctx, shared);
        todo!("history_ui::show")
    }
}
