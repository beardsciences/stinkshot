//! Global hotkeys via `global-hotkey`. Parses `config.hotkeys` strings; PrintScreen
//! variants require `system::take_over_printscreen(true)` when `take_over_printscreen`.

use crate::state::Command;

pub struct Hotkeys {}

impl Hotkeys {
    /// Returns the manager plus human-readable registration errors (conflicts, bad strings).
    pub fn new(cfg: &stinkshot_core::config::Hotkeys) -> (Hotkeys, Vec<String>) {
        let _ = cfg;
        todo!("hotkeys::Hotkeys::new")
    }
    /// Unregister all and register from `cfg`. Returns errors.
    pub fn reload(&mut self, cfg: &stinkshot_core::config::Hotkeys) -> Vec<String> {
        let _ = cfg;
        todo!("hotkeys::Hotkeys::reload")
    }
    pub fn poll(&mut self) -> Vec<Command> {
        todo!("hotkeys::Hotkeys::poll")
    }
    /// Pause all hotkeys (while the settings window is capturing a new combo).
    pub fn set_suspended(&mut self, suspended: bool) {
        let _ = suspended;
    }
}

/// Display form, e.g. "Control+Shift+KeyS" -> "Ctrl+Shift+S", "PrintScreen" -> "PrtSc".
pub fn display(hotkey: &str) -> String {
    let _ = hotkey;
    todo!("hotkeys::display")
}
