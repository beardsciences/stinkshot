//! Settings window (docs/design/Settings.dc.html). Sections: General, Hotkeys, Capture,
//! Editor, Recording, History, Privacy, About. Edits `shared.config` in place and sets
//! `shared.config_dirty = true`; the shell persists and applies. Hotkey fields capture the
//! next key combo when clicked and show conflicts reported by the shell. The Privacy page
//! states the local-only promise (copy from the design) and holds the update-check toggle.

use crate::state::{Shared, Surface, SurfaceStatus};

pub struct SettingsWindow {}

impl SettingsWindow {
    pub fn new(shared: &Shared) -> Self {
        let _ = shared;
        Self {}
    }
    /// Shell reports hotkey registration failures (e.g. "Ctrl+Shift+KeyS: already in use").
    pub fn set_hotkey_errors(&mut self, errors: Vec<String>) {
        let _ = errors;
    }
}

impl Surface for SettingsWindow {
    fn title(&self) -> String {
        "StinkShot Settings".into()
    }
    fn viewport_builder(&self, shared: &Shared) -> egui::ViewportBuilder {
        let _ = shared;
        todo!("settings_ui::viewport_builder")
    }
    fn show(&mut self, ctx: &egui::Context, shared: &mut Shared) -> SurfaceStatus {
        let _ = (ctx, shared);
        todo!("settings_ui::show")
    }
}
