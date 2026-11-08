//! The capture actions a user can trigger from a hotkey, the tray menu or the command line.
//! One source of truth for their names (Title Case, docs/design/Tray.dc.html), Settings
//! descriptions, config fields and commands, so Settings, the tray, hotkey errors and hints
//! never disagree.

use stinkshot_core::config::Hotkeys;

use crate::state::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Region,
    Window,
    Fullscreen,
    Scrolling,
    Ocr,
    RepeatLast,
    PickColor,
    RecordMp4,
    RecordGif,
}

impl Action {
    /// Menu / Settings order.
    pub const ALL: [Action; 9] = [
        Action::Region,
        Action::Window,
        Action::Fullscreen,
        Action::Scrolling,
        Action::Ocr,
        Action::RepeatLast,
        Action::PickColor,
        Action::RecordMp4,
        Action::RecordGif,
    ];

    /// The one display name, used by Settings rows, tray items, hotkey errors and hints.
    pub fn label(self) -> &'static str {
        match self {
            Action::Region => "Region",
            Action::Window => "Window",
            Action::Fullscreen => "Full Screen",
            Action::Scrolling => "Scrolling",
            Action::Ocr => "Copy Text (OCR)",
            Action::RepeatLast => "Repeat Last Region",
            Action::PickColor => "Pick Color",
            Action::RecordMp4 => "Record MP4",
            Action::RecordGif => "Record GIF",
        }
    }

    /// Settings helper text (sentence case).
    pub fn description(self) -> &'static str {
        match self {
            Action::Region => "Freeze the screen and drag a region.",
            Action::Window => "Capture the active window with rounded corners and alpha.",
            Action::Fullscreen => "Capture the monitor under the cursor.",
            Action::Scrolling => "Select a region, Stinkshot scrolls and stitches it.",
            Action::Ocr => "Select a region and copy the recognized text.",
            Action::RepeatLast => "Capture the previous region again without selecting.",
            Action::PickColor => "Click any pixel to copy its color as hex, RGB or HSL.",
            Action::RecordMp4 => "Select a region and record with hardware encoding.",
            Action::RecordGif => "Select a region and record a size-optimized GIF.",
        }
    }

    /// Stable id for the tray menu item.
    pub fn id(self) -> &'static str {
        match self {
            Action::Region => "region",
            Action::Window => "window",
            Action::Fullscreen => "full",
            Action::Scrolling => "scroll",
            Action::Ocr => "ocr",
            Action::RepeatLast => "repeat",
            Action::PickColor => "pick_color",
            Action::RecordMp4 => "rec_mp4",
            Action::RecordGif => "rec_gif",
        }
    }

    pub fn from_id(id: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|a| a.id() == id)
    }

    pub fn is_recording(self) -> bool {
        matches!(self, Action::RecordMp4 | Action::RecordGif)
    }

    pub fn command(self) -> Command {
        match self {
            Action::Region => Command::CaptureRegion,
            Action::Window => Command::CaptureWindow,
            Action::Fullscreen => Command::CaptureFullscreen,
            Action::Scrolling => Command::CaptureScrolling,
            Action::Ocr => Command::OcrRegion,
            Action::RepeatLast => Command::RepeatLastRegion,
            Action::PickColor => Command::PickColor,
            Action::RecordMp4 => Command::RecordMp4,
            Action::RecordGif => Command::RecordGif,
        }
    }

    pub fn hotkey(self, h: &Hotkeys) -> &String {
        match self {
            Action::Region => &h.region,
            Action::Window => &h.window,
            Action::Fullscreen => &h.fullscreen,
            Action::Scrolling => &h.scrolling,
            Action::Ocr => &h.ocr,
            Action::RepeatLast => &h.repeat_last_region,
            Action::PickColor => &h.pick_color,
            Action::RecordMp4 => &h.record_mp4,
            Action::RecordGif => &h.record_gif,
        }
    }

    pub fn hotkey_mut(self, h: &mut Hotkeys) -> &mut String {
        match self {
            Action::Region => &mut h.region,
            Action::Window => &mut h.window,
            Action::Fullscreen => &mut h.fullscreen,
            Action::Scrolling => &mut h.scrolling,
            Action::Ocr => &mut h.ocr,
            Action::RepeatLast => &mut h.repeat_last_region,
            Action::PickColor => &mut h.pick_color,
            Action::RecordMp4 => &mut h.record_mp4,
            Action::RecordGif => &mut h.record_gif,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_and_are_unique() {
        for a in Action::ALL {
            assert_eq!(Action::from_id(a.id()), Some(a));
        }
        assert_eq!(Action::from_id("nope"), None);
        let mut labels: Vec<_> = Action::ALL.iter().map(|a| a.label()).collect();
        labels.sort();
        labels.dedup();
        assert_eq!(labels.len(), Action::ALL.len());
    }

    #[test]
    fn labels_are_title_case() {
        for a in Action::ALL {
            for word in a.label().split(' ') {
                let first = word.trim_start_matches('(').chars().next().unwrap();
                assert!(first.is_uppercase(), "{} in {}", word, a.label());
            }
            assert!(a.description().ends_with('.'));
        }
    }

    #[test]
    fn hotkey_accessors_agree() {
        let mut h = Hotkeys::default();
        for a in Action::ALL {
            *a.hotkey_mut(&mut h) = format!("F{}", a as usize + 1);
        }
        for a in Action::ALL {
            assert_eq!(a.hotkey(&h), &format!("F{}", a as usize + 1));
        }
    }
}
