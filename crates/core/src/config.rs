//! `%APPDATA%\Stinkshot\config.toml`. Every field has a serde default so old or partial
//! files always load. Unknown keys are ignored. The settings UI edits this struct and calls
//! `save`; nothing else writes the file.

use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::image::ImageFormat;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub hotkeys: Hotkeys,
    pub capture: Capture,
    pub editor: Editor,
    pub recording: Recording,
    pub history: HistoryCfg,
    pub privacy: Privacy,
    pub pins: Pins,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    pub start_with_windows: bool,
    /// "system" | "dark" | "light"
    pub theme: String,
    /// UI language: "system" (Windows locale), a code like "en", or "pseudo".
    pub language: String,
    /// Hex like "#7fd36a".
    pub accent: String,
    pub copy_to_clipboard: bool,
    pub show_toast: bool,
    pub toast_seconds: u32,
    pub play_sound: bool,
    /// The first-run tour was shown (or skipped). Settings > About > Show Tour reopens it.
    pub tour_seen: bool,
    /// The version that last ran with this config, for the "Updated to" toast. Empty before
    /// 0.1.3 wrote it.
    pub last_version: String,
}

/// Hotkey strings use the `global-hotkey` crate's format, e.g. "PrintScreen",
/// "Alt+PrintScreen", "Ctrl+Shift+KeyS". Empty string = unbound.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    pub take_over_printscreen: bool,
    /// Route the Snipping Tool shortcuts (Win+Shift+S, Win+Shift+R, and PrintScreen through
    /// the takeover) to Stinkshot with a low-level keyboard hook. Off by default.
    pub replace_snipping_tool: bool,
    pub region: String,
    pub window: String,
    pub fullscreen: String,
    pub scrolling: String,
    pub ocr: String,
    pub repeat_last_region: String,
    pub record_mp4: String,
    pub record_gif: String,
    /// Standalone color picker (empty = no hotkey; also in the tray menu).
    pub pick_color: String,
    /// Capture the text selected in the focused app (UI Automation). Empty = no hotkey.
    pub selected_text: String,
    /// Same, grown to whole paragraphs. Empty = no hotkey.
    pub selected_paragraph: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Capture {
    /// Draw the pointer into the pixels of captures that skip the editor (quick window and
    /// full-screen captures, scrolling, CLI), and of editor captures when `cursor_object`
    /// is off.
    pub include_cursor: bool,
    /// In the editor (region overlay, full screen with the editor on), keep the pointer as a
    /// movable object, selected at first so Del removes it. Overrides `include_cursor` there.
    pub cursor_object: bool,
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
    /// Window and full-screen captures open in the editor first. Off: they are finished
    /// right away (clipboard, history, toast) and the toast's Edit opens the editor.
    pub open_editor_after_capture: bool,
    /// What confirming a selected region (Enter) does in the region overlay.
    pub after_region: AfterRegion,
}

/// Region overlay: what Enter does once a region is selected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AfterRegion {
    /// Annotate on the frozen screen; Enter copies.
    #[default]
    Annotate,
    /// Enter unfreezes the screen and opens the capture in an Editor Window.
    Window,
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
    /// Last style picked in Find Sensitive Info: "box", "blur" or "pixelate".
    pub smart_redact_style: String,
    /// Last colors used in the editor, newest first (at most 6), as hex.
    pub recent_colors: Vec<String>,
    /// Color and stroke width each editor tool last used, keyed by tool ("arrow", "rect",
    /// "redact", ...). Tools without an entry start from `color` / `stroke_width`.
    pub tool_styles: std::collections::BTreeMap<String, ToolStyle>,
    /// Highlighter draws boxes instead of freehand strokes (Shift while dragging draws the
    /// other mode).
    pub highlighter_box: bool,
    /// Color picker copy format: "hex", "rgb" or "hsl".
    pub picker_format: String,
    /// Colors picked with the color picker (C in the overlay, Pick Color), newest first, hex.
    pub picker_recent: Vec<String>,
    /// Beautify frame for sharing (applied at export when enabled).
    pub beautify: crate::beautify::Beautify,
}

/// A tool's remembered color (hex) and stroke width.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolStyle {
    pub color: String,
    pub width: f32,
}

impl Default for ToolStyle {
    fn default() -> Self {
        Self {
            color: "#ff8a3d".into(),
            width: 4.0,
        }
    }
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
    /// Empty = `%USERPROFILE%\Pictures\Stinkshot`.
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

/// Pinned captures (always-on-top image windows).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pins {
    /// Pins are normal app windows: listed in Alt+Tab, Task View and the taskbar (still
    /// always on top). Off: tool windows that stay out of those.
    pub show_in_task_view: bool,
    /// The pin toolbar is always shown in a strip above the image. Off: it appears on hover
    /// over the image.
    pub always_show_toolbar: bool,
}

impl Default for Pins {
    fn default() -> Self {
        Self {
            show_in_task_view: true,
            always_show_toolbar: true,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Privacy {
    /// The one and only network feature. Off by default.
    pub check_for_updates: bool,
    pub last_update_check: Option<chrono::DateTime<chrono::Utc>>,
}

impl Default for General {
    fn default() -> Self {
        Self {
            start_with_windows: false,
            theme: "system".into(),
            language: "system".into(),
            accent: "#7fd36a".into(),
            copy_to_clipboard: true,
            show_toast: true,
            toast_seconds: 6,
            play_sound: false,
            tour_seen: false,
            last_version: String::new(),
        }
    }
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self {
            take_over_printscreen: true,
            replace_snipping_tool: false,
            region: "PrintScreen".into(),
            window: "Alt+PrintScreen".into(),
            fullscreen: "Control+PrintScreen".into(),
            scrolling: "Control+Alt+KeyS".into(),
            ocr: "Control+Alt+KeyT".into(),
            repeat_last_region: "Shift+PrintScreen".into(),
            record_mp4: "Control+Alt+KeyR".into(),
            record_gif: "Control+Alt+KeyG".into(),
            pick_color: String::new(),
            selected_text: String::new(),
            selected_paragraph: String::new(),
        }
    }
}

impl Default for Capture {
    fn default() -> Self {
        Self {
            include_cursor: false,
            cursor_object: true,
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
            filename_pattern: "Stinkshot_{yyyy}-{MM}-{dd}_{HH}-{mm}-{ss}".into(),
            scroll_max_height: 30000,
            scroll_delay_ms: 120,
            last_region: None,
            open_editor_after_capture: false,
            after_region: AfterRegion::Annotate,
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
            palette: [
                "#ff8a3d", "#ff5a52", "#ffd166", "#7fd36a", "#4cc2ff", "#c49cff", "#ffffff",
                "#111111",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            smart_redact_style: "box".into(),
            recent_colors: Vec::new(),
            tool_styles: Default::default(),
            highlighter_box: false,
            picker_format: "hex".into(),
            picker_recent: Vec::new(),
            beautify: Default::default(),
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
        Self {
            folder: String::new(),
            ocr_index: true,
            keep_days: 0,
            max_gb: 0.0,
            keep_editable: true,
        }
    }
}

/// Development profile: when `STINKSHOT_PROFILE=<name>` is set, the app uses
/// `%APPDATA%\Stinkshot-<name>`, `Pictures\Stinkshot-<name>`, and its own single-instance
/// mutex and IPC window, so a dev build can run beside the user's real instance without
/// touching its config or history. Returns "" (normal) or "-<name>" (ASCII alnum, '-', '_').
pub fn profile_suffix() -> String {
    std::env::var("STINKSHOT_PROFILE")
        .ok()
        .map(|p| {
            p.chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .take(32)
                .collect::<String>()
        })
        .filter(|p| !p.is_empty())
        .map(|p| format!("-{p}"))
        .unwrap_or_default()
}

/// The default `capture.filename_pattern` written by builds that spelled the name "StinkShot".
const LEGACY_FILENAME_PATTERN: &str = "StinkShot_{yyyy}-{MM}-{dd}_{HH}-{mm}-{ss}";

impl Config {
    /// `%APPDATA%\Stinkshot` (plus the dev profile suffix, see `profile_suffix`).
    pub fn dir() -> PathBuf {
        let base = std::env::var_os("APPDATA")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| directories::BaseDirs::new().map(|d| d.config_dir().to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join(format!("Stinkshot{}", profile_suffix()))
    }
    pub fn path() -> PathBuf {
        Self::dir().join("config.toml")
    }
    /// Load from `path()`; missing file -> defaults (and writes them). Malformed file ->
    /// back it up as config.toml.bad-<timestamp>, log, return defaults.
    pub fn load() -> Config {
        let path = Self::path();
        if !path.exists() {
            let cfg = Config::default();
            if let Err(e) = cfg.save_to(&path) {
                log::warn!(
                    "could not write default config to {}: {e:#}",
                    path.display()
                );
            }
            return cfg;
        }
        match Self::load_from(&path) {
            Ok(cfg) => cfg,
            Err(e) => {
                log::error!("config {} is unreadable: {e:#}", path.display());
                let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
                let mut backup_name = path
                    .file_name()
                    .map(|n| n.to_os_string())
                    .unwrap_or_default();
                backup_name.push(format!(".bad-{stamp}"));
                let backup = path.with_file_name(backup_name);
                match std::fs::rename(&path, &backup) {
                    Ok(()) => {
                        log::warn!("backed up malformed config to {}", backup.display());
                        let cfg = Config::default();
                        if let Err(e) = cfg.save_to(&path) {
                            log::warn!("could not write fresh config: {e:#}");
                        }
                        cfg
                    }
                    Err(e) => {
                        log::warn!("could not back up malformed config: {e}");
                        Config::default()
                    }
                }
            }
        }
    }
    pub fn load_from(path: &Path) -> anyhow::Result<Config> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let mut cfg: Config =
            toml::from_str(text).with_context(|| format!("parsing {}", path.display()))?;
        cfg.normalize();
        Ok(cfg)
    }
    /// Clamp values that would break callers (hand-edited files).
    fn normalize(&mut self) {
        self.capture.jpeg_quality = self.capture.jpeg_quality.clamp(1, 100);
        // Empty, or the untouched default from before the "Stinkshot" spelling (Nov 2026):
        // both become the current default. A customized pattern is left alone.
        if self.capture.filename_pattern.trim().is_empty()
            || self.capture.filename_pattern == LEGACY_FILENAME_PATTERN
        {
            self.capture.filename_pattern = Capture::default().filename_pattern;
        }
        if !(self.editor.stroke_width.is_finite() && self.editor.stroke_width > 0.0) {
            self.editor.stroke_width = Editor::default().stroke_width;
        }
        if !(self.editor.font_size.is_finite() && self.editor.font_size > 0.0) {
            self.editor.font_size = Editor::default().font_size;
        }
        if self.editor.palette.is_empty() {
            self.editor.palette = Editor::default().palette;
        }
        if self.recording.fps == 0 {
            self.recording.fps = Recording::default().fps;
        }
        if self.recording.gif_fps == 0 {
            self.recording.gif_fps = Recording::default().gif_fps;
        }
        if !(self.history.max_gb.is_finite() && self.history.max_gb >= 0.0) {
            self.history.max_gb = 0.0;
        }
    }
    /// Atomic write (temp + rename).
    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(&Self::path())
    }
    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        let text = toml::to_string_pretty(self).context("serializing config")?;
        crate::image::write_atomic(path, text.as_bytes())
    }
    /// Resolved history folder (applies the empty-string default). A custom folder may use
    /// `%VAR%` environment references.
    pub fn history_folder(&self) -> PathBuf {
        let custom = self.history.folder.trim();
        if !custom.is_empty() {
            return PathBuf::from(expand_env(custom));
        }
        let home = std::env::var_os("USERPROFILE")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| directories::UserDirs::new().map(|d| d.home_dir().to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));
        home.join("Pictures")
            .join(format!("Stinkshot{}", profile_suffix()))
    }
}

/// Expand `%NAME%` references; unknown variables are left as written.
fn expand_env(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('%') {
            Some(j) if j > 0 => {
                let name = &after[..j];
                match std::env::var(name) {
                    Ok(v) => out.push_str(&v),
                    Err(_) => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[j + 1..];
            }
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Parse "#rrggbb" or "#rrggbbaa" into RGBA. Invalid -> None.
pub fn parse_hex_color(s: &str) -> Option<[u8; 4]> {
    let s = s.trim().strip_prefix('#')?;
    if !s.is_ascii() || !(s.len() == 6 || s.len() == 8) || !s.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    let a = if s.len() == 8 { byte(6)? } else { 255 };
    Some([byte(0)?, byte(2)?, byte(4)?, a])
}

pub fn to_hex_color(c: [u8; 4]) -> String {
    if c[3] == 255 {
        format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
    } else {
        format!("#{:02x}{:02x}{:02x}{:02x}", c[0], c[1], c[2], c[3])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("stinkshot-core-cfg-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn hex_colors() {
        assert_eq!(parse_hex_color("#ff8a3d"), Some([0xff, 0x8a, 0x3d, 255]));
        assert_eq!(parse_hex_color("#FF8A3D80"), Some([0xff, 0x8a, 0x3d, 0x80]));
        assert_eq!(parse_hex_color("  #000000 "), Some([0, 0, 0, 255]));
        for bad in [
            "",
            "#",
            "ff8a3d",
            "#ff8a3",
            "#ff8a3dd",
            "#gg0000",
            "#12345678a",
            "#é12345",
            "#+12345",
        ] {
            assert_eq!(parse_hex_color(bad), None, "{bad:?}");
        }
        assert_eq!(to_hex_color([1, 2, 3, 255]), "#010203");
        assert_eq!(
            parse_hex_color(&to_hex_color([9, 8, 7, 6])),
            Some([9, 8, 7, 6])
        );
    }

    #[test]
    fn default_round_trip() {
        let d = tmp("rt");
        let p = d.join("config.toml");
        let cfg = Config::default();
        cfg.save_to(&p).unwrap();
        assert_eq!(Config::load_from(&p).unwrap(), cfg);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn modified_round_trip_including_options() {
        let d = tmp("rt2");
        let p = d.join("sub").join("config.toml");
        let mut cfg = Config::default();
        cfg.general.theme = "dark".into();
        cfg.hotkeys.region = String::new();
        cfg.capture.format = ImageFormat::Webp;
        cfg.capture.last_region = Some(crate::geom::PxRect::new(-1920, 10, 800, 600));
        cfg.editor.stroke_width = 0.1;
        cfg.editor.palette = vec!["#010203".into()];
        cfg.editor.recent_colors = vec!["#112233".into(), "#445566".into()];
        cfg.editor.tool_styles.insert(
            "arrow".into(),
            ToolStyle {
                color: "#ff0000".into(),
                width: 6.0,
            },
        );
        cfg.editor.highlighter_box = true;
        cfg.editor.picker_format = "hsl".into();
        cfg.editor.picker_recent = vec!["#abcdef".into()];
        cfg.editor.beautify.enabled = true;
        cfg.editor.beautify.aspect = crate::beautify::Aspect::Portrait;
        cfg.editor.beautify.background = crate::beautify::BackgroundKind::Blurred;
        cfg.hotkeys.pick_color = "Control+Alt+KeyC".into();
        cfg.hotkeys.selected_text = "Control+Alt+KeyP".into();
        cfg.hotkeys.selected_paragraph = "Control+Alt+Shift+KeyP".into();
        cfg.history.max_gb = 2.5;
        cfg.privacy.check_for_updates = true;
        cfg.privacy.last_update_check = Some(chrono::Utc::now());
        cfg.save_to(&p).unwrap();
        let back = Config::load_from(&p).unwrap();
        assert_eq!(back, cfg);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn partial_file_fills_defaults_and_ignores_unknown() {
        let d = tmp("partial");
        let p = d.join("config.toml");
        std::fs::write(
            &p,
            "mystery = 1\n[general]\ntheme = \"light\"\nnew_future_key = true\n[capture]\njpeg_quality = 70\n[hotkeys]\nregion = \"F9\"\n",
        )
        .unwrap();
        let cfg = Config::load_from(&p).unwrap();
        assert_eq!(cfg.general.theme, "light");
        assert_eq!(cfg.general.accent, General::default().accent);
        assert_eq!(cfg.capture.jpeg_quality, 70);
        assert_eq!(
            cfg.capture.filename_pattern,
            Capture::default().filename_pattern
        );
        assert_eq!(cfg.hotkeys.region, "F9");
        assert_eq!(cfg.hotkeys.window, Hotkeys::default().window);
        assert_eq!(cfg.recording, Recording::default());
        // Editor memory added later: absent in old files, empty when loaded.
        assert!(cfg.editor.recent_colors.is_empty() && cfg.editor.tool_styles.is_empty());
        assert_eq!(cfg.editor.picker_format, "hex");
        assert!(cfg.editor.picker_recent.is_empty());
        assert!(!cfg.editor.highlighter_box);
        assert_eq!(cfg.editor.beautify, crate::beautify::Beautify::default());
        assert!(cfg.hotkeys.pick_color.is_empty());
        // Pin options added later: both on for old files.
        assert!(cfg.pins.show_in_task_view && cfg.pins.always_show_toolbar);
        assert!(cfg.hotkeys.selected_text.is_empty() && cfg.hotkeys.selected_paragraph.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn old_config_without_editor_flag_loads_with_it_off() {
        let d = tmp("editor_flag");
        let p = d.join("config.toml");
        // A config written before the flag existed: the [capture] table lacks it.
        std::fs::write(
            &p,
            "[capture]
include_cursor = true
",
        )
        .unwrap();
        let cfg = Config::load_from(&p).unwrap();
        assert!(cfg.capture.include_cursor);
        assert!(!cfg.capture.open_editor_after_capture);
        // Configs from before the cursor object existed get it on.
        assert!(cfg.capture.cursor_object);
        std::fs::write(
            &p,
            "[capture]
open_editor_after_capture = true
",
        )
        .unwrap();
        assert!(
            Config::load_from(&p)
                .unwrap()
                .capture
                .open_editor_after_capture
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn after_region_defaults_to_annotate_and_round_trips() {
        let d = tmp("after_region");
        let p = d.join("config.toml");
        std::fs::write(
            &p,
            "[capture]
include_cursor = true
",
        )
        .unwrap();
        assert_eq!(
            Config::load_from(&p).unwrap().capture.after_region,
            AfterRegion::Annotate
        );
        std::fs::write(
            &p,
            "[capture]
after_region = \"window\"
",
        )
        .unwrap();
        assert_eq!(
            Config::load_from(&p).unwrap().capture.after_region,
            AfterRegion::Window
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn empty_file_is_all_defaults() {
        let d = tmp("empty");
        let p = d.join("config.toml");
        std::fs::write(&p, "").unwrap();
        assert_eq!(Config::load_from(&p).unwrap(), Config::default());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn malformed_and_missing_are_errors_in_load_from() {
        let d = tmp("bad");
        let p = d.join("config.toml");
        assert!(Config::load_from(&p).is_err());
        std::fs::write(&p, "this is [not toml").unwrap();
        assert!(Config::load_from(&p).is_err());
        std::fs::write(&p, "[capture]\njpeg_quality = \"high\"\n").unwrap();
        assert!(Config::load_from(&p).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn out_of_range_values_are_normalized() {
        let d = tmp("norm");
        let p = d.join("config.toml");
        std::fs::write(
            &p,
            "[capture]\njpeg_quality = 0\nfilename_pattern = \"  \"\n[editor]\npalette = []\nstroke_width = -3.0\n",
        )
        .unwrap();
        let cfg = Config::load_from(&p).unwrap();
        assert_eq!(cfg.capture.jpeg_quality, 1);
        assert_eq!(
            cfg.capture.filename_pattern,
            Capture::default().filename_pattern
        );
        assert_eq!(cfg.editor.palette, Editor::default().palette);
        assert_eq!(cfg.editor.stroke_width, 4.0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn utf8_bom_is_tolerated() {
        let d = tmp("bom");
        let p = d.join("config.toml");
        std::fs::write(&p, "\u{feff}[general]\ntheme = \"dark\"\n").unwrap();
        assert_eq!(Config::load_from(&p).unwrap().general.theme, "dark");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn legacy_default_filename_pattern_is_respelled() {
        let d = tmp("legacy_pattern");
        let p = d.join("config.toml");
        let text = format!("[capture]\nfilename_pattern = \"{LEGACY_FILENAME_PATTERN}\"\n");
        std::fs::write(&p, text).unwrap();
        let cfg = Config::load_from(&p).unwrap();
        assert_eq!(
            cfg.capture.filename_pattern,
            Capture::default().filename_pattern
        );
        assert!(cfg.capture.filename_pattern.starts_with("Stinkshot_"));
        // A pattern the user changed keeps its spelling.
        let text = "[capture]\nfilename_pattern = \"StinkShot_{title}\"\n";
        std::fs::write(&p, text).unwrap();
        assert_eq!(
            Config::load_from(&p).unwrap().capture.filename_pattern,
            "StinkShot_{title}"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn history_folder_default_and_custom() {
        let mut cfg = Config::default();
        let def = cfg.history_folder();
        assert!(
            def.ends_with("Pictures/Stinkshot") || def.ends_with("Pictures\\Stinkshot"),
            "{def:?}"
        );
        cfg.history.folder = "D:\\Shots".into();
        assert_eq!(cfg.history_folder(), PathBuf::from("D:\\Shots"));
        cfg.history.folder = "   ".into();
        assert_eq!(cfg.history_folder(), def);
    }

    #[test]
    fn env_expansion() {
        let path = std::env::var("PATH").unwrap();
        assert_eq!(expand_env("%PATH%\\x"), format!("{path}\\x"));
        assert_eq!(
            expand_env("%STINKSHOT_NOPE_XYZ%\\x"),
            "%STINKSHOT_NOPE_XYZ%\\x"
        );
        assert_eq!(expand_env("100%"), "100%");
        assert_eq!(expand_env("a%%b"), "a%%b");
    }

    #[test]
    fn config_dir_ends_with_app_name() {
        assert!(Config::dir().ends_with("Stinkshot"));
        assert!(Config::path().ends_with("config.toml"));
    }

    #[test]
    fn load_backs_up_malformed_file() {
        // Config::load() is bound to %APPDATA%, so exercise the same flow by pointing APPDATA
        // at a temp dir. Tests in this module that read APPDATA only compare suffixes.
        let d = tmp("load");
        // SAFETY: tests that touch APPDATA run only here and set it before reading.
        unsafe { std::env::set_var("APPDATA", &d) };
        let first = Config::load();
        assert_eq!(first, Config::default());
        assert!(
            Config::path().exists(),
            "defaults are written on first load"
        );
        std::fs::write(Config::path(), "garbage [[[").unwrap();
        let second = Config::load();
        assert_eq!(second, Config::default());
        let backups: Vec<_> = std::fs::read_dir(Config::dir())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("config.toml.bad-")
            })
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(
            std::fs::read_to_string(backups[0].path()).unwrap(),
            "garbage [[["
        );
        assert!(Config::load_from(&Config::path()).is_ok());
        let _ = std::fs::remove_dir_all(&d);
    }
}
