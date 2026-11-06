//! Lets any thread wake the UI event loop. The shell registers its egui context once;
//! workers, hotkey/tray bridges and the IPC bridge call `wake()` after posting a message.

use std::sync::OnceLock;
use std::time::Duration;

static CTX: OnceLock<egui::Context> = OnceLock::new();

pub fn set_context(ctx: egui::Context) {
    let _ = CTX.set(ctx);
}

pub fn wake() {
    if let Some(ctx) = CTX.get() {
        ctx.request_repaint();
    }
}

/// Request a repaint of the root viewport after `delay` (used for debounces and timers).
pub fn wake_after(delay: Duration) {
    if let Some(ctx) = CTX.get() {
        ctx.request_repaint_after(delay);
    }
}
