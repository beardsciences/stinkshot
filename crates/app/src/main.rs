//! StinkShot entry point. Startup order:
//! 1. `platform::dpi::init()`.
//! 2. Parse CLI (`cli::parse`). Headless jobs (`stinkshot full -o x.png`) run and exit
//!    without UI.
//! 3. Single instance: if another instance runs, forward args over IPC and exit.
//! 4. Load config, fonts, history, OCR engine; take over PrintScreen if configured.
//! 5. Run eframe with an invisible 1x1 root viewport (never shown); every visible window is
//!    a `Surface` shown as an immediate viewport by `shell::Shell`.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod cli;
mod hotkeys;
mod history_ui;
mod overlay;
mod pin;
mod record_ui;
mod settings_ui;
mod shell;
mod state;
mod theme;
mod toast;
mod tray;
mod update;

fn main() -> anyhow::Result<()> {
    shell::run()
}
