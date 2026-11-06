//! `%APPDATA%\StinkShot\config.toml`. Every field has a serde default so old or partial
//! files always load. Unknown keys are ignored. The settings UI edits this struct and calls
//! `save`; nothing else writes the file.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::image::ImageFormat;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub hotkeys: Hotkeys,
    pub capture: Capture,
    pub editor: Editor,
    pub recording: Recording,
    pub history: HistoryCfg,
    pub privacy: Privacy,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    pub start_with_windows: bool,
    /// "system" | "dark" | "light"
    pub theme: String,
    /// Hex like "#7fd36a".
    pub accent: String,
    pub copy_to_clipboard: bool,
    pub show_toast: bool,
    pub toast_seconds: u32,
    pub play_sound: bool,
}

/// Hotkey strings use the `global-hotkey` crate's format, e.g. "PrintScreen",
/// "Alt+PrintScreen", "Ctrl+Shift+KeyS". Empty string = unbound.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    pub take_over_printscreen: bool,
    pub region: String,
    pub window: String,
    pub fullscreen: String,
    pub scrolling: String,
    pub ocr: String,
    pub repeat_last_region: String,
    pub record_mp4: String,
    pub record_gif: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Capture {
    pub include_cursor: bool,
    pub freeze_screen: bool,
    pub snap_to_elements: bool,
    pub show_loupe: bool,
    pub window_rounded_corners: bool,
    pub window_shadow: bool,
    pub window_padding: bool,
    pub window_padding_px: u32,
    /// Hex color for the padded background frame.
    pub window_padding_color: String,
    pub format: ImageFormat,
    pub jpeg_quality: u8,
    /// See `naming::file_name` for tokens.
    pub filename_pattern: String,
    pub scroll_max_height: u32,
    pub scroll_delay_ms: u32,
    pub last_region: Option<crate::geom::PxRect>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Editor {
    pub color: String,
    pub stroke_width: f32,
    pub font_size: f32,
    pub blur_radius: f32,
    pub pixelate_block: u32,
    pub palette: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Recording {
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub system_audio: bool,
    pub microphone: bool,
    /// Empty = default device.
    pub mic_device: String,
    pub webcam: bool,
    /// Empty = default device.
    pub webcam_device: String,
    pub webcam_diameter: u32,
    pub show_clicks: bool,
    pub show_keys: bool,
    pub countdown_seconds: u32,
    pub gif_fps: u32,
    pub gif_max_width: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryCfg {
    /// Empty = `%USERPROFILE%\Pictures\StinkShot`.
    pub folder: String,
    pub ocr_index: bool,
    /// 0 = keep forever.
    pub keep_days: u32,
    /// 0 = no size cap.
    pub max_gb: f32,
    /// Keep the unannotated original + annotation sidecar so captures stay re-editable.
    /// NOTE: the original contains pixels that were later redacted; it never leaves the PC.
    pub keep_editable: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Privacy {
    /// The one and only network feature. Off by default.
    pub check_for_updates: bool,
    pub last_update_check: Option<chrono::DateTime<chrono::Utc>>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: General::default(),
            hotkeys: Hotkeys::default(),
            capture: Capture::default(),
            editor: Editor::default(),
            recording: Recording::default(),
            history: HistoryCfg::default(),
            privacy: Privacy::default(),
        }
    }
}

impl Default for General {
    fn default() -> Self {
        Self {
            start_with_windows: false,
            theme: "system".into(),
            accent: "#7fd36a".into(),
            copy_to_clipboard: true,
            show_toast: true,
            toast_seconds: 6,
            play_sound: false,
        }
    }
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            take_over_printscreen: true,
            region: "PrintScreen".into(),
            window: "Alt+PrintScreen".into(),
            fullscreen: "Control+PrintScreen".into(),
            scrolling: "Control+Shift+KeyS".into(),
            ocr: "Control+Shift+KeyT".into(),
            repeat_last_region: "Shift+PrintScreen".into(),
            record_mp4: "Control+Shift+KeyR".into(),
            record_gif: "Control+Shift+KeyG".into(),
        }
    }
}

impl Default for Capture {
    fn default() -> Self {
        Self {
            include_cursor: false,
            freeze_screen: true,
            snap_to_elements: true,
            show_loupe: true,
            window_rounded_corners: true,
            window_shadow: true,
            window_padding: false,
            window_padding_px: 48,
            window_padding_color: "#5b7fa8".into(),
            format: ImageFormat::Png,
            jpeg_quality: 90,
            filename_pattern: "StinkShot_{yyyy}-{MM}-{dd}_{HH}-{mm}-{ss}".into(),
            scroll_max_height: 30000,
            scroll_delay_ms: 120,
            last_region: None,
        }
    }
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            color: "#ff8a3d".into(),
            stroke_width: 4.0,
            font_size: 20.0,
            blur_radius: 12.0,
            pixelate_block: 12,
            palette: ["#ff8a3d", "#ff5a52", "#ffd166", "#7fd36a", "#4cc2ff", "#c49cff", "#ffffff", "#111111"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

impl Default for Recording {
    fn default() -> Self {
        Self {
            fps: 60,
            bitrate_kbps: 16000,
            system_audio: true,
            microphone: false,
            mic_device: String::new(),
            webcam: false,
            webcam_device: String::new(),
            webcam_diameter: 180,
            show_clicks: true,
            show_keys: true,
            countdown_seconds: 3,
            gif_fps: 15,
            gif_max_width: 960,
        }
    }
}

impl Default for HistoryCfg {
    fn default() -> Self {
        Self { folder: String::new(), ocr_index: true, keep_days: 90, max_gb: 5.0, keep_editable: true }
    }
}

impl Default for Privacy {
    fn default() -> Self {
        Self { check_for_updates: false, last_update_check: None }
    }
}

impl Config {
    /// `%APPDATA%\StinkShot`.
    pub fn dir() -> PathBuf {
        todo!("core::config::Config::dir")
    }
    pub fn path() -> PathBuf {
        Self::dir().join("config.toml")
    }
    /// Load from `path()`; missing file -> defaults (and writes them). Malformed file ->
    /// back it up as config.toml.bad-<timestamp>, log, return defaults.
    pub fn load() -> Config {
        todo!("core::config::Config::load")
    }
    pub fn load_from(path: &Path) -> anyhow::Result<Config> {
        let _ = path;
        todo!("core::config::Config::load_from")
    }
    /// Atomic write (temp + rename).
    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(&Self::path())
    }
    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        let _ = path;
        todo!("core::config::Config::save_to")
    }
    /// Resolved history folder (applies the empty-string default).
    pub fn history_folder(&self) -> PathBuf {
        todo!("core::config::Config::history_folder")
    }
}

/// Parse "#rrggbb" or "#rrggbbaa" into RGBA. Invalid -> None.
pub fn parse_hex_color(s: &str) -> Option<[u8; 4]> {
    let _ = s;
    todo!("core::config::parse_hex_color")
}

pub fn to_hex_color(c: [u8; 4]) -> String {
    if c[3] == 255 {
        format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
    } else {
        format!("#{:02x}{:02x}{:02x}{:02x}", c[0], c[1], c[2], c[3])
    }
}
