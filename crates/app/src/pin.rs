//! Pinned capture (docs/design/Pin.dc.html): borderless topmost window showing an image
//! 1:1 at its physical size. Drag moves; wheel zooms (10%..800%); Ctrl+wheel opacity
//! (20%..100%); double-click opens in the editor (if from history); Ctrl+C copies; Esc or
//! the close button closes; hover shows the mini toolbar (opacity, copy, close) and the
//! accent outline.

use stinkshot_core::{PxRect, Rgba};
use stinkshot_history::ItemId;

use crate::state::{Shared, Surface, SurfaceStatus};

pub struct Pin {
    #[allow(dead_code)]
    image: Rgba,
    id: u64,
}

impl Pin {
    /// `at`: initial physical rect (e.g. where the region was captured); None = centered
    /// on the monitor with the cursor.
    pub fn new(image: Rgba, title: String, item: Option<ItemId>, at: Option<PxRect>, id: u64) -> Self {
        let _ = (title, item, at);
        Self { image, id }
    }
}

impl Surface for Pin {
    fn title(&self) -> String {
        format!("StinkShot.Pin.{}", self.id)
    }
    fn viewport_builder(&self, shared: &Shared) -> egui::ViewportBuilder {
        let _ = shared;
        todo!("pin::viewport_builder")
    }
    fn show(&mut self, ctx: &egui::Context, shared: &mut Shared) -> SurfaceStatus {
        let _ = (ctx, shared);
        todo!("pin::show")
    }
}
