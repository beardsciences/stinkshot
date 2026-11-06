//! After-capture toast (docs/design/Toast.dc.html): bottom-right of the work area of the
//! monitor with the cursor, above the taskbar; topmost, no activation, excluded from
//! capture, auto-dismiss after config.general.toast_seconds (paused while hovered); click
//! on the thumbnail = Edit. Actions: Edit, Pin, Copy text, Folder (design order).

use stinkshot_core::Rgba;
use stinkshot_history::ItemId;

use crate::state::{Shared, Surface, SurfaceStatus};

pub enum ToastKind {
    Capture { thumb: Rgba, title: String, body: String, item: Option<ItemId>, can_edit: bool },
    Recording { thumb: Option<Rgba>, title: String, body: String, item: Option<ItemId> },
    Message { title: String, body: String, error: bool },
}

pub struct Toast {
    #[allow(dead_code)]
    kind: ToastKind,
    id: u64,
}

impl Toast {
    /// `id` makes the title unique so several toasts can coexist; `stack_index` 0 = lowest.
    pub fn new(kind: ToastKind, id: u64, stack_index: usize) -> Self {
        let _ = stack_index;
        Self { kind, id }
    }
}

impl Surface for Toast {
    fn title(&self) -> String {
        format!("StinkShot.Toast.{}", self.id)
    }
    fn viewport_builder(&self, shared: &Shared) -> egui::ViewportBuilder {
        let _ = shared;
        todo!("toast::viewport_builder")
    }
    fn show(&mut self, ctx: &egui::Context, shared: &mut Shared) -> SurfaceStatus {
        let _ = (ctx, shared);
        todo!("toast::show")
    }
}
