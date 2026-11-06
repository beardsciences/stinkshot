//! Tray icon + menu (docs/design/Tray.dc.html): header "StinkShot <version> · offline",
//! capture modes with their hotkey labels, record MP4/GIF, History, Settings, Quit.
//! Left click = region capture; double click = History. Icon generated in code (rounded
//! accent square with a small white shutter mark) at 16/20/24/32 px for DPI.

use crate::state::Command;

pub struct Tray {}

impl Tray {
    pub fn new(config: &stinkshot_core::config::Config) -> anyhow::Result<Tray> {
        let _ = config;
        todo!("tray::Tray::new")
    }
    /// Refresh hotkey labels after settings change.
    pub fn update_labels(&mut self, config: &stinkshot_core::config::Config) {
        let _ = config;
    }
    /// Drain menu/icon events into commands. Call every frame.
    pub fn poll(&mut self) -> Vec<Command> {
        todo!("tray::Tray::poll")
    }
}
