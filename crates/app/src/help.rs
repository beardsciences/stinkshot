//! Settings > Help (also the tray's Help item): global hotkeys, editor shortcuts, features
//! and file locations. Offline only. The shortcut lists are generated from the live hotkey
//! config (`actions.rs`) and the overlay key map (`overlay::keys::sheet`), so this page can
//! not drift from what the keys do (tests below assert every binding is listed).

use std::path::{Path, PathBuf};

use egui::{Align2, RichText, Sense, Ui, vec2};
use stinkshot_core::config::{Config, Hotkeys};

use crate::actions::Action;
use crate::state::{Command, Shared};
use crate::theme::{self, widgets};
use crate::toast::ui_common as uc;

/// Every user-visible string on this page (one place, for the localization lookup later).
mod text {
    pub const HOTKEYS: &str = "Global Hotkeys";
    pub const HOTKEYS_DESC: &str = "Work anywhere while Stinkshot runs.";
    pub const CHANGE_HOTKEYS: &str = "Change Hotkeys";
    pub const NOT_SET: &str = "Not Set";
    pub const EDITOR: &str = "Editor Shortcuts";
    pub const EDITOR_DESC: &str = "In the capture overlay. Press F1 or ? there for this list.";
    pub const TRAY: &str = "Tray Icon";
    pub const TRAY_ROWS: [(&str, &str); 3] = [
        ("Click", "Capture a Region"),
        ("Double-Click", "Open History"),
        ("Right-Click", "All Capture Modes and Settings"),
    ];
    pub const FEATURES: &str = "Features";
    pub const FEATURE_ROWS: [(&str, &str); 9] = [
        (
            "Region, Window, Full Screen",
            "Freeze the screen, then select or click.",
        ),
        ("Scrolling", "Scrolls a window and stitches one tall image."),
        (
            "Copy Text (OCR)",
            "Windows' offline OCR; also searchable in History.",
        ),
        (
            "Annotate",
            "Arrows, shapes, text, steps, blur, redact and more.",
        ),
        (
            "Find Sensitive Info",
            "Spots emails, keys and numbers to redact.",
        ),
        ("Beautify", "Frame a capture with padding and a background."),
        ("Pin", "Keep a capture on top of other windows."),
        (
            "Record MP4 or GIF",
            "Hardware-encoded video, trim before saving.",
        ),
        (
            "History",
            "Every capture, searchable. Ctrl+H from Settings.",
        ),
    ];
    pub const FILES: &str = "File Locations";
    pub const CAPTURES: &str = "Captures";
    pub const SETTINGS_FILE: &str = "Settings File";
    pub const LOG: &str = "Log";
    pub const OPEN: &str = "Open";
    pub const SHOW: &str = "Show";
    pub const OPEN_FAILED: &str = "Could Not Open";
    pub const OPEN_FAILED_HINT: &str = "Windows could not open it. Check that the path exists.";
    pub const OFFLINE: &str = "Help is built in and works offline.";
}

/// A group of (keys, what they do) rows.
pub struct Group {
    pub title: &'static str,
    pub rows: Vec<(String, String)>,
}

/// Global hotkeys from the live config, in Settings / tray order.
pub fn hotkey_rows(cfg: &Hotkeys) -> Vec<(String, String)> {
    Action::ALL
        .iter()
        .map(|a| {
            let key = crate::hotkeys::display(a.hotkey(cfg));
            let key = if key.is_empty() {
                text::NOT_SET.to_string()
            } else {
                key
            };
            (key, a.label().to_string())
        })
        .collect()
}

/// Overlay shortcut groups, straight from the overlay's own cheat sheet.
pub fn editor_groups() -> Vec<Group> {
    crate::overlay::keys::sheet(false)
        .into_iter()
        .map(|(title, rows)| Group {
            title,
            rows: rows.into_iter().map(|(k, v)| (k, v.to_string())).collect(),
        })
        .collect()
}

fn rows_card(ui: &mut Ui, shared: &Shared, rows: &[(String, String)]) {
    let p = uc::pal(shared);
    let key_w = 170.0;
    egui::Frame::new()
        .fill(p.card)
        .stroke(egui::Stroke::new(1.0, p.stroke))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(18, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;
            for (key, what) in rows {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(key_w, 20.0), Sense::hover());
                    ui.painter().text(
                        r.left_center(),
                        Align2::LEFT_CENTER,
                        key,
                        theme::semibold(13.0),
                        p.text,
                    );
                    ui.label(
                        RichText::new(what.as_str())
                            .size(13.0)
                            .color(p.text_secondary),
                    );
                });
            }
        });
}

fn group_title(ui: &mut Ui, shared: &Shared, title: &str, desc: &str) {
    let p = uc::pal(shared);
    ui.add_space(14.0);
    ui.label(
        RichText::new(title)
            .font(theme::semibold(14.0))
            .color(p.text),
    );
    if !desc.is_empty() {
        ui.label(RichText::new(desc).size(12.0).color(p.text_tertiary));
    }
    ui.add_space(2.0);
}

fn open(shared: &mut Shared, path: &Path, reveal: bool) {
    let r = if reveal {
        stinkshot_platform::system::reveal_in_explorer(path)
    } else {
        stinkshot_platform::system::open_path(path)
    };
    if let Err(e) = r {
        log::warn!("help: open {}: {e:#}", path.display());
        shared.outbox.push(Command::Notify {
            title: text::OPEN_FAILED.into(),
            body: text::OPEN_FAILED_HINT.into(),
            error: true,
        });
    }
}

/// (label, path, reveal-in-Explorer instead of open) for the File Locations rows.
fn locations(cfg: &Config) -> [(&'static str, PathBuf, bool); 3] {
    [
        (text::CAPTURES, cfg.history_folder(), false),
        (text::SETTINGS_FILE, Config::path(), true),
        (text::LOG, crate::logger::path(), true),
    ]
}

/// The Help page body (Settings draws the page title).
pub fn page(ui: &mut Ui, shared: &mut Shared) {
    let p = uc::pal(shared);
    ui.label(
        RichText::new(text::OFFLINE)
            .size(13.0)
            .color(p.text_secondary),
    );

    group_title(ui, shared, text::HOTKEYS, text::HOTKEYS_DESC);
    rows_card(ui, shared, &hotkey_rows(&shared.config.hotkeys));
    ui.add_space(4.0);
    if widgets::button(ui, text::CHANGE_HOTKEYS).clicked() {
        shared.outbox.push(Command::OpenSettingsAt(
            crate::settings_ui::Section::Hotkeys,
        ));
    }

    group_title(ui, shared, text::TRAY, "");
    let tray: Vec<(String, String)> = text::TRAY_ROWS
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    rows_card(ui, shared, &tray);

    group_title(ui, shared, text::EDITOR, text::EDITOR_DESC);
    for g in editor_groups() {
        ui.add_space(4.0);
        ui.label(RichText::new(g.title).size(12.0).color(p.text_tertiary));
        rows_card(ui, shared, &g.rows);
    }

    group_title(ui, shared, text::FEATURES, "");
    for (name, desc) in text::FEATURE_ROWS {
        widgets::setting_row(ui, name, desc, |_| {});
    }

    group_title(ui, shared, text::FILES, "");
    let mut clicked: Option<(PathBuf, bool)> = None;
    for (label, path, reveal) in locations(&shared.config) {
        let shown = path.display().to_string();
        widgets::setting_row(ui, label, &shown, |ui| {
            let b = if reveal { text::SHOW } else { text::OPEN };
            if widgets::button(ui, b).clicked() {
                clicked = Some((path.clone(), reveal));
            }
        });
    }
    if let Some((path, reveal)) = clicked {
        if !reveal {
            // The captures folder may not exist before the first capture.
            let _ = std::fs::create_dir_all(&path);
        }
        open(shared, &path, reveal);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::keys;

    fn all_editor_rows() -> Vec<(String, String)> {
        editor_groups().into_iter().flat_map(|g| g.rows).collect()
    }

    #[test]
    fn every_global_hotkey_is_listed_with_its_key() {
        let cfg = Hotkeys::default();
        let rows = hotkey_rows(&cfg);
        assert_eq!(rows.len(), Action::ALL.len());
        for a in Action::ALL {
            let key = crate::hotkeys::display(a.hotkey(&cfg));
            let want = if key.is_empty() {
                text::NOT_SET.to_string()
            } else {
                key
            };
            assert!(
                rows.iter().any(|(k, v)| *k == want && v == a.label()),
                "{} missing",
                a.label()
            );
        }
    }

    #[test]
    fn hotkey_rows_follow_the_config() {
        let mut cfg = Hotkeys::default();
        *Action::Region.hotkey_mut(&mut cfg) = "Control+Shift+KeyQ".into();
        *Action::Window.hotkey_mut(&mut cfg) = String::new();
        let rows = hotkey_rows(&cfg);
        assert!(rows.contains(&("Ctrl+Shift+Q".into(), Action::Region.label().into())));
        assert!(rows.contains(&(text::NOT_SET.into(), Action::Window.label().into())));
    }

    #[test]
    fn every_overlay_binding_is_listed() {
        let rows = all_editor_rows();
        // The whole cheat sheet, row for row.
        let sheet: Vec<(String, String)> = keys::sheet(false)
            .into_iter()
            .flat_map(|(_, r)| r)
            .map(|(k, v)| (k, v.to_string()))
            .collect();
        assert_eq!(rows, sheet);
        // Every tool letter and every chord.
        let has = |k: &str| rows.iter().any(|(key, _)| key.contains(k));
        for (_, k) in keys::TOOL_KEYS {
            assert!(
                rows.iter().any(|(key, _)| key == k.name()),
                "tool key {} missing",
                k.name()
            );
        }
        for c in [
            keys::SAVE,
            keys::PIN,
            keys::COPY_TEXT,
            keys::FIND_SENSITIVE,
            keys::UNDO,
            keys::REDO,
            keys::COPY_COLOR,
            keys::EYEDROPPER,
            keys::SCAN,
            keys::BEAUTIFY,
            keys::DUPLICATE,
            keys::COPY,
            keys::PASTE,
            keys::BOLD,
            keys::ZOOM_FIT,
            keys::ZOOM_ACTUAL,
            keys::HELP,
        ] {
            assert!(has(c.label), "{} missing", c.label);
        }
        for k in [
            keys::FORWARD,
            keys::BACKWARD,
            keys::TO_FRONT,
            keys::TO_BACK,
            keys::HELP_CHAR,
        ] {
            assert!(has(k), "{k} missing");
        }
        // The select-only sheet is a subset in its keys.
        for (_, r) in keys::sheet(true) {
            for (k, _) in r {
                assert!(rows.iter().any(|(key, _)| *key == k), "{k} missing");
            }
        }
    }

    #[test]
    fn copy_is_concise_and_title_case() {
        let title_case = |s: &str| {
            s.split([' ', '-', ',']).filter(|w| !w.is_empty()).all(|w| {
                let small = ["a", "an", "and", "or", "of", "the", "to", "in", "on", "for"];
                small.contains(&w) || !w.starts_with(|c: char| c.is_lowercase())
            })
        };
        for t in [
            text::HOTKEYS,
            text::CHANGE_HOTKEYS,
            text::NOT_SET,
            text::EDITOR,
            text::TRAY,
            text::FEATURES,
            text::FILES,
            text::CAPTURES,
            text::SETTINGS_FILE,
            text::OPEN_FAILED,
        ] {
            assert!(title_case(t), "{t}");
        }
        for (name, desc) in text::FEATURE_ROWS {
            assert!(title_case(name), "{name}");
            assert!(desc.len() <= 60, "{desc}");
        }
        for (k, v) in text::TRAY_ROWS {
            assert!(title_case(k) && title_case(v), "{k} {v}");
        }
    }

    #[test]
    fn locations_point_at_the_profile_paths() {
        let cfg = Config::default();
        let l = locations(&cfg);
        assert_eq!(l[0].1, cfg.history_folder());
        assert!(l[1].1.ends_with("config.toml"));
        assert!(l[2].1.to_string_lossy().ends_with(".log"));
    }
}
