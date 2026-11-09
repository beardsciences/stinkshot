//! Overlay keyboard map: one table for the key handling (`Overlay::read_keys`), the hint
//! bar and the F1 / ? shortcut cheat sheet, so the help can not drift from what the keys do.

use egui::Key;

use super::shapes::Tool;
use crate::l10n::tr;

/// Plain-letter tool shortcuts (no Ctrl / Alt), in toolbar order.
pub const TOOL_KEYS: [(Tool, Key); 16] = [
    (Tool::Select, Key::V),
    (Tool::Arrow, Key::A),
    (Tool::Line, Key::L),
    (Tool::Rect, Key::R),
    (Tool::Ellipse, Key::E),
    (Tool::Pen, Key::P),
    (Tool::Highlighter, Key::H),
    (Tool::Text, Key::T),
    (Tool::Step, Key::N),
    (Tool::Blur, Key::B),
    (Tool::Pixelate, Key::X),
    (Tool::Redact, Key::K),
    (Tool::Magnifier, Key::M),
    (Tool::Callout, Key::O),
    (Tool::Spotlight, Key::S),
    (Tool::Measure, Key::U),
];

/// Palette color keys 1-8.
pub const PALETTE_KEYS: [Key; 8] = [
    Key::Num1,
    Key::Num2,
    Key::Num3,
    Key::Num4,
    Key::Num5,
    Key::Num6,
    Key::Num7,
    Key::Num8,
];

/// A single key, optionally with Ctrl. `label` is how hints and the cheat sheet show it
/// (checked against `key` / `ctrl` by a test).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chord {
    pub key: Key,
    pub ctrl: bool,
    pub label: &'static str,
}

impl Chord {
    const fn ctrl(key: Key, label: &'static str) -> Chord {
        Chord {
            key,
            ctrl: true,
            label,
        }
    }
    const fn plain(key: Key, label: &'static str) -> Chord {
        Chord {
            key,
            ctrl: false,
            label,
        }
    }

    /// Pressed this frame with exactly the right Ctrl state (and no Alt).
    pub fn pressed(self, i: &egui::InputState) -> bool {
        i.modifiers.command == self.ctrl && !i.modifiers.alt && i.key_pressed(self.key)
    }
}

pub const SAVE: Chord = Chord::ctrl(Key::S, "Ctrl+S");
pub const PIN: Chord = Chord::ctrl(Key::P, "Ctrl+P");
pub const COPY_TEXT: Chord = Chord::ctrl(Key::T, "Ctrl+T");
/// Find Sensitive Info (Smart Redact): R for redact.
pub const FIND_SENSITIVE: Chord = Chord::ctrl(Key::R, "Ctrl+R");
pub const UNDO: Chord = Chord::ctrl(Key::Z, "Ctrl+Z");
pub const REDO: Chord = Chord::ctrl(Key::Y, "Ctrl+Y");
pub const COPY_COLOR: Chord = Chord::plain(Key::C, "C");
/// Pick a color from the image (eyedropper).
pub const EYEDROPPER: Chord = Chord::plain(Key::I, "I");
/// Codes and Text panel (QR codes, barcodes, Copy as Table).
pub const SCAN: Chord = Chord::plain(Key::Q, "Q");
/// Beautify panel (frame for sharing).
pub const BEAUTIFY: Chord = Chord::plain(Key::F, "F");
pub const DUPLICATE: Chord = Chord::ctrl(Key::D, "Ctrl+D");
/// Copy / paste shapes inside the editor (Ctrl+C copies the image when no shape is selected).
pub const COPY: Chord = Chord::ctrl(Key::C, "Ctrl+C");
pub const PASTE: Chord = Chord::ctrl(Key::V, "Ctrl+V");
pub const BOLD: Chord = Chord::ctrl(Key::B, "Ctrl+B");
pub const ZOOM_FIT: Chord = Chord::ctrl(Key::Num0, "Ctrl+0");
pub const ZOOM_ACTUAL: Chord = Chord::ctrl(Key::Num1, "Ctrl+1");
/// Z-order chords (read in Overlay::read_keys; brackets with Ctrl, Shift goes all the way).
pub const FORWARD: &str = "Ctrl+]";
pub const BACKWARD: &str = "Ctrl+[";
pub const TO_FRONT: &str = "Ctrl+Shift+]";
pub const TO_BACK: &str = "Ctrl+Shift+[";
pub const HELP: Chord = Chord::plain(Key::F1, "F1");
/// Typed character that also toggles the cheat sheet (layout independent).
pub const HELP_CHAR: &str = "?";

/// Tool name without its shortcut suffix ("Arrow (A)" -> "Arrow").
pub fn tool_name(t: Tool) -> &'static str {
    t.name()
}

#[cfg(test)]
pub fn tool_for_key(k: Key) -> Option<Tool> {
    TOOL_KEYS.iter().find(|(_, key)| *key == k).map(|(t, _)| *t)
}

/// Cheat sheet groups: (title, rows of (keys, what they do)).
pub type Sheet = Vec<(&'static str, Vec<(String, &'static str)>)>;

/// The cheat sheet for an overlay. `select_only`: the overlay only picks a region (Copy Text,
/// recording, scrolling capture), so tools, editing and the finish shortcuts do nothing there.
pub fn sheet(select_only: bool) -> Sheet {
    let select = vec![
        (
            tr!("overlay.keys.k.drag").to_string(),
            tr!("overlay.keys.select_region"),
        ),
        (
            tr!("overlay.keys.k.click").to_string(),
            tr!("overlay.keys.window_or_element"),
        ),
        (
            tr!("overlay.keys.k.wheel").to_string(),
            tr!("overlay.keys.parent_child_scroll"),
        ),
        (
            tr!("overlay.keys.k.shift_wheel").to_string(),
            tr!("overlay.keys.scroll_sideways"),
        ),
        ("Enter".to_string(), tr!("overlay.keys.select_highlight")),
        (
            tr!("overlay.keys.k.shift_drag").to_string(),
            tr!("overlay.keys.square_or_ratio"),
        ),
        (
            tr!("overlay.keys.k.space_drag").to_string(),
            tr!("overlay.keys.pan_or_move"),
        ),
        (
            tr!("overlay.keys.k.arrows").to_string(),
            tr!("overlay.keys.nudge"),
        ),
        (
            tr!("overlay.keys.k.ctrl_arrows").to_string(),
            tr!("overlay.keys.resize_selection"),
        ),
        (COPY_COLOR.label.to_string(), tr!("overlay.keys.copy_color")),
    ];
    let help = (
        tr!("overlay.keys.k.either", a = HELP.label, b = HELP_CHAR),
        tr!("overlay.keys.toggle_help"),
    );
    if select_only {
        let finish = vec![
            ("Enter".to_string(), tr!("overlay.keys.use_region")),
            ("Esc".to_string(), tr!("overlay.keys.cancel")),
            help,
        ];
        return vec![
            (tr!("overlay.keys.group.select"), select),
            (tr!("overlay.keys.group.finish"), finish),
        ];
    }
    let mut select = select;
    select.push((
        tr!("overlay.keys.k.shift_drag_outside").to_string(),
        tr!("overlay.keys.add_region"),
    ));
    let tools = TOOL_KEYS
        .iter()
        .map(|(t, k)| (k.name().to_string(), tool_name(*t)))
        .collect();
    let edit = vec![
        ("1-8".to_string(), tr!("overlay.keys.palette_color")),
        (EYEDROPPER.label.to_string(), tr!("overlay.keys.pick_color")),
        ("[  ]".to_string(), tr!("overlay.keys.thinner_thicker")),
        (UNDO.label.to_string(), tr!("overlay.keys.undo")),
        (REDO.label.to_string(), tr!("overlay.keys.redo")),
        (
            format!("Ctrl+Shift+{}", UNDO.key.name()),
            tr!("overlay.keys.redo_short"),
        ),
        (
            tr!("overlay.keys.k.shift_drag").to_string(),
            tr!("overlay.keys.straight"),
        ),
        (
            tr!("overlay.keys.k.alt_drag").to_string(),
            tr!("overlay.keys.no_snap"),
        ),
        (
            tr!("overlay.keys.k.del").to_string(),
            tr!("overlay.keys.delete_shapes"),
        ),
        (
            tr!("overlay.keys.k.double_click").to_string(),
            tr!("overlay.keys.edit_text"),
        ),
        (BOLD.label.to_string(), tr!("overlay.keys.bold")),
        (
            FIND_SENSITIVE.label.to_string(),
            tr!("overlay.keys.find_sensitive"),
        ),
        (SCAN.label.to_string(), tr!("overlay.keys.scan")),
        (BEAUTIFY.label.to_string(), tr!("overlay.keys.beautify")),
        ("Esc".to_string(), tr!("overlay.keys.deselect_cancel")),
    ];
    let arrange = vec![
        (
            tr!("overlay.keys.k.shift_click").to_string(),
            tr!("overlay.keys.toggle_shape"),
        ),
        (
            tr!("overlay.keys.k.drag_empty").to_string(),
            tr!("overlay.keys.marquee"),
        ),
        (
            tr!("overlay.keys.k.arrows").to_string(),
            tr!("overlay.keys.move_shapes"),
        ),
        (DUPLICATE.label.to_string(), tr!("overlay.keys.duplicate")),
        (COPY.label.to_string(), tr!("overlay.keys.copy_shapes")),
        (PASTE.label.to_string(), tr!("overlay.keys.paste_shapes")),
        (
            format!("{FORWARD}  {BACKWARD}"),
            tr!("overlay.keys.forward_backward"),
        ),
        (
            format!("{TO_FRONT}  {TO_BACK}"),
            tr!("overlay.keys.front_back"),
        ),
    ];
    let view = vec![
        (
            tr!("overlay.keys.k.ctrl_wheel").to_string(),
            tr!("overlay.keys.zoom_cursor"),
        ),
        (
            "Ctrl+=  Ctrl+-".to_string(),
            tr!("overlay.keys.zoom_in_out"),
        ),
        (ZOOM_FIT.label.to_string(), tr!("overlay.keys.fit")),
        (
            ZOOM_ACTUAL.label.to_string(),
            tr!("overlay.keys.actual_size"),
        ),
        (
            tr!("overlay.keys.k.middle_drag").to_string(),
            tr!("overlay.keys.pan"),
        ),
    ];
    let finish = vec![
        ("Enter".to_string(), tr!("overlay.keys.copy_image")),
        (
            "Ctrl+C".to_string(),
            tr!("overlay.keys.copy_image_no_shape"),
        ),
        (SAVE.label.to_string(), tr!("overlay.keys.save_as")),
        (PIN.label.to_string(), tr!("overlay.keys.pin")),
        (COPY_TEXT.label.to_string(), tr!("overlay.keys.copy_text")),
        ("Esc".to_string(), tr!("overlay.keys.cancel")),
        help,
    ];
    vec![
        (tr!("overlay.keys.group.select"), select),
        (tr!("overlay.keys.group.tools"), tools),
        (tr!("overlay.keys.group.edit"), edit),
        (tr!("overlay.keys.group.arrange"), arrange),
        (tr!("overlay.keys.group.view"), view),
        (tr!("overlay.keys.group.finish"), finish),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::ui::TOOL_GROUPS;

    #[test]
    fn every_toolbar_tool_has_one_key_and_its_tooltip_names_it() {
        for group in TOOL_GROUPS {
            for (tool, _) in group.iter() {
                let keys: Vec<Key> = TOOL_KEYS
                    .iter()
                    .filter(|(t, _)| t == tool)
                    .map(|(_, k)| *k)
                    .collect();
                assert_eq!(keys.len(), 1, "{tool:?}");
                assert!(
                    tool.label().ends_with(&format!("({})", keys[0].name())),
                    "{} vs {}",
                    tool.label(),
                    keys[0].name()
                );
                assert_eq!(tool_for_key(keys[0]), Some(*tool));
            }
        }
        let mut ks: Vec<&str> = TOOL_KEYS.iter().map(|(_, k)| k.name()).collect();
        ks.sort_unstable();
        ks.dedup();
        assert_eq!(ks.len(), TOOL_KEYS.len());
    }

    #[test]
    fn chord_labels_match_their_keys() {
        for c in [
            SAVE,
            PIN,
            COPY_TEXT,
            FIND_SENSITIVE,
            UNDO,
            REDO,
            COPY_COLOR,
            HELP,
            EYEDROPPER,
            DUPLICATE,
            COPY,
            PASTE,
            BOLD,
            ZOOM_FIT,
            ZOOM_ACTUAL,
            SCAN,
            BEAUTIFY,
        ] {
            let expect = if c.ctrl {
                format!("Ctrl+{}", c.key.name())
            } else {
                c.key.name().to_string()
            };
            assert_eq!(c.label, expect);
        }
        // Plain-letter chords must not collide with tool letters.
        assert!(tool_for_key(COPY_COLOR.key).is_none());
        assert!(tool_for_key(EYEDROPPER.key).is_none());
        assert!(tool_for_key(SCAN.key).is_none());
        assert!(tool_for_key(BEAUTIFY.key).is_none());
        let mut plain = [EYEDROPPER.key, COPY_COLOR.key, SCAN.key, BEAUTIFY.key].map(|k| k.name());
        plain.sort_unstable();
        assert!(plain.windows(2).all(|w| w[0] != w[1]));
    }

    #[test]
    fn sheet_has_the_four_groups_and_every_tool() {
        let s = sheet(false);
        let titles: Vec<&str> = s.iter().map(|(t, _)| *t).collect();
        assert_eq!(
            titles,
            ["Select", "Tools", "Edit", "Arrange", "View", "Finish"]
        );
        assert_eq!(s[1].1.len(), TOOL_KEYS.len());
        assert!(s[1].1.iter().any(|(k, v)| k == "A" && *v == "Arrow"));
        assert!(s[5].1.iter().any(|(k, _)| k == SAVE.label));
        for (_, what) in &s[1].1 {
            assert!(!what.contains('('), "no shortcut suffix left in {what}");
        }
    }

    #[test]
    fn select_only_sheet_has_no_tools_or_edit() {
        let s = sheet(true);
        let titles: Vec<&str> = s.iter().map(|(t, _)| *t).collect();
        assert_eq!(titles, ["Select", "Finish"]);
        let finish = &s[1].1;
        assert!(
            finish
                .iter()
                .any(|(k, v)| k == "Enter" && *v == "Use This Region")
        );
        assert!(
            !finish
                .iter()
                .any(|(k, _)| k == SAVE.label || k == PIN.label)
        );
        assert!(
            !s.iter()
                .flat_map(|(_, r)| r.iter())
                .any(|(k, _)| k == FIND_SENSITIVE.label)
        );
    }

    #[test]
    fn every_handled_edit_key_is_on_the_sheet() {
        // Each key `read_keys` handles for editing (redo both ways, delete both ways, Shift
        // constraining a shape) has a row.
        let s = sheet(false);
        let rows: Vec<&(String, &str)> = s.iter().flat_map(|(_, r)| r.iter()).collect();
        let has = |k: &str| rows.iter().any(|(key, _)| key.contains(k));
        for k in [
            UNDO.label,
            REDO.label,
            "Ctrl+Shift+Z",
            "Del",
            "Backspace",
            "1-8",
            "[  ]",
            SAVE.label,
            PIN.label,
            COPY_TEXT.label,
            FIND_SENSITIVE.label,
            COPY_COLOR.label,
            HELP.label,
            HELP_CHAR,
            "Esc",
            "Enter",
            "Arrows",
            EYEDROPPER.label,
            SCAN.label,
            BEAUTIFY.label,
            DUPLICATE.label,
            COPY.label,
            PASTE.label,
            BOLD.label,
            ZOOM_FIT.label,
            ZOOM_ACTUAL.label,
            FORWARD,
            BACKWARD,
            TO_FRONT,
            TO_BACK,
            "Ctrl+Wheel",
            "Middle-Drag",
            "Shift+Click",
            "Alt+Drag",
            "Shift+Wheel",
            "Space+Drag",
        ] {
            assert!(has(k), "{k} missing from the cheat sheet");
        }
        assert!(
            s[2].1
                .iter()
                .any(|(k, v)| k == "Shift+Drag" && v.contains("Square"))
        );
    }

    #[test]
    fn view_and_snapping_rows_match_the_behaviour() {
        let s = sheet(false);
        let rows: Vec<&(String, &str)> = s.iter().flat_map(|(_, r)| r.iter()).collect();
        let what = |k: &str| -> Vec<&str> {
            rows.iter()
                .filter(|(key, _)| key == k)
                .map(|(_, v)| *v)
                .collect()
        };
        // One Space+Drag row: it pans in the editor or when zoomed.
        let space = what("Space+Drag");
        assert_eq!(space.len(), 1, "{space:?}");
        assert!(space[0].contains("Pan") && space[0].contains("Move the Selection"));
        assert!(what("Wheel")[0].contains("Scroll When Zoomed"));
        let alt = what("Alt+Drag");
        assert!(alt[0].contains("Draw") && alt[0].contains("Resize"));
        // Undo and redo work while typing on the image too.
        assert!(what(UNDO.label)[0].contains("Typing"));
        assert!(what(REDO.label)[0].contains("Typing"));
    }

    #[test]
    fn ctrl_c_on_the_sheet_says_when_it_copies_shapes() {
        let s = sheet(false);
        let rows: Vec<&(String, &str)> = s.iter().flat_map(|(_, r)| r.iter()).collect();
        let ctrl_c: Vec<&str> = rows
            .iter()
            .filter(|(k, _)| k == COPY.label)
            .map(|(_, v)| *v)
            .collect();
        assert_eq!(ctrl_c.len(), 2, "{ctrl_c:?}");
        assert!(ctrl_c.iter().any(|v| v.contains("Shapes")));
        assert!(ctrl_c.iter().any(|v| v.contains("No Shape Selected")));
    }
}
