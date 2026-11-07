//! Hotkey capture for the settings window: turns egui key events into `global-hotkey`
//! format strings ("Control+Shift+KeyS", "Alt+PrintScreen") and back into friendly labels
//! ("Ctrl+Shift+S", "Alt+PrtSc"). Pure logic, unit tested; every generated string is also
//! checked against the real `global_hotkey` parser in the tests.
//!
//! Mounted from `settings_ui.rs` with `#[path]`.

use egui::{Event, Key, Modifiers};

/// `global-hotkey` code name for an egui key, or None for keys that cannot be a hotkey
/// target (modifiers, clipboard pseudo-keys, media keys).
pub fn code_name(key: Key) -> Option<&'static str> {
    use Key::*;
    Some(match key {
        A => "KeyA",
        B => "KeyB",
        C => "KeyC",
        D => "KeyD",
        E => "KeyE",
        F => "KeyF",
        G => "KeyG",
        H => "KeyH",
        I => "KeyI",
        J => "KeyJ",
        K => "KeyK",
        L => "KeyL",
        M => "KeyM",
        N => "KeyN",
        O => "KeyO",
        P => "KeyP",
        Q => "KeyQ",
        R => "KeyR",
        S => "KeyS",
        T => "KeyT",
        U => "KeyU",
        V => "KeyV",
        W => "KeyW",
        X => "KeyX",
        Y => "KeyY",
        Z => "KeyZ",
        Num0 => "Digit0",
        Num1 => "Digit1",
        Num2 => "Digit2",
        Num3 => "Digit3",
        Num4 => "Digit4",
        Num5 => "Digit5",
        Num6 => "Digit6",
        Num7 => "Digit7",
        Num8 => "Digit8",
        Num9 => "Digit9",
        F1 => "F1",
        F2 => "F2",
        F3 => "F3",
        F4 => "F4",
        F5 => "F5",
        F6 => "F6",
        F7 => "F7",
        F8 => "F8",
        F9 => "F9",
        F10 => "F10",
        F11 => "F11",
        F12 => "F12",
        F13 => "F13",
        F14 => "F14",
        F15 => "F15",
        F16 => "F16",
        F17 => "F17",
        F18 => "F18",
        F19 => "F19",
        F20 => "F20",
        F21 => "F21",
        F22 => "F22",
        F23 => "F23",
        F24 => "F24",
        ArrowDown => "ArrowDown",
        ArrowLeft => "ArrowLeft",
        ArrowRight => "ArrowRight",
        ArrowUp => "ArrowUp",
        Escape => "Escape",
        Tab => "Tab",
        Backspace => "Backspace",
        Enter => "Enter",
        Space => "Space",
        Insert => "Insert",
        Delete => "Delete",
        Home => "Home",
        End => "End",
        PageUp => "PageUp",
        PageDown => "PageDown",
        PrintScreen => "PrintScreen",
        ScrollLock => "ScrollLock",
        Pause => "Pause",
        NumLock => "NumLock",
        CapsLock => "CapsLock",
        // Punctuation, by physical position (what global-hotkey registers).
        Comma => "Comma",
        Period => "Period",
        Slash | Questionmark => "Slash",
        Backslash | Pipe => "Backslash",
        Semicolon | Colon => "Semicolon",
        Quote => "Quote",
        Backtick => "Backquote",
        Minus => "Minus",
        Equals | Plus => "Equal",
        OpenBracket | OpenCurlyBracket => "BracketLeft",
        CloseBracket | CloseCurlyBracket => "BracketRight",
        Exclamationmark => "Digit1",
        _ => return None,
    })
}

fn is_modifier_key(key: Key) -> bool {
    use Key::*;
    matches!(
        key,
        ShiftLeft
            | ShiftRight
            | ControlLeft
            | ControlRight
            | AltLeft
            | AltRight
            | SuperLeft
            | SuperRight
    )
}

/// Canonical string: modifiers in the order Control, Shift, Alt, Super, then the key.
pub fn build_hotkey(ctrl: bool, shift: bool, alt: bool, win: bool, code: &str) -> String {
    let mut s = String::new();
    for (on, name) in [
        (ctrl, "Control"),
        (shift, "Shift"),
        (alt, "Alt"),
        (win, "Super"),
    ] {
        if on {
            s.push_str(name);
            s.push('+');
        }
    }
    s.push_str(code);
    s
}

/// Outcome of scanning a frame's events while a hotkey field is waiting for input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Captured {
    Nothing,
    /// Esc with no modifiers: abandon the capture.
    Cancel,
    /// Backspace/Delete with no modifiers: unbind.
    Clear,
    Combo(String),
}

/// Look at this frame's events. `win` is the Windows key state (egui has no modifier flag for
/// it); `polled_main` is a main key detected outside egui (PrintScreen may only produce a key
/// release on Windows, so the widget also polls it).
pub fn scan(
    events: &[Event],
    modifiers: Modifiers,
    win: bool,
    polled_main: Option<&'static str>,
) -> Captured {
    for e in events {
        match e {
            Event::Key {
                key,
                physical_key,
                pressed,
                repeat,
                modifiers: m,
                ..
            } => {
                let main = physical_key.unwrap_or(*key);
                if is_modifier_key(main) || is_modifier_key(*key) {
                    continue;
                }
                // PrintScreen is only reliably reported on release.
                let accept = (*pressed && !*repeat) || (!*pressed && main == Key::PrintScreen);
                if !accept {
                    continue;
                }
                let none = !m.ctrl && !m.shift && !m.alt && !win;
                match main {
                    Key::Escape if none => return Captured::Cancel,
                    Key::Backspace | Key::Delete if none => return Captured::Clear,
                    _ => {}
                }
                if let Some(code) = code_name(main).or_else(|| code_name(*key)) {
                    return Captured::Combo(build_hotkey(m.ctrl, m.shift, m.alt, win, code));
                }
            }
            // egui turns Ctrl+C/X/V into clipboard events and may not report the key itself.
            Event::Copy => {
                return Captured::Combo(build_hotkey(
                    modifiers.ctrl,
                    modifiers.shift,
                    modifiers.alt,
                    win,
                    "KeyC",
                ));
            }
            Event::Cut => {
                return Captured::Combo(build_hotkey(
                    modifiers.ctrl,
                    modifiers.shift,
                    modifiers.alt,
                    win,
                    "KeyX",
                ));
            }
            Event::Paste(_) => {
                return Captured::Combo(build_hotkey(
                    modifiers.ctrl,
                    modifiers.shift,
                    modifiers.alt,
                    win,
                    "KeyV",
                ));
            }
            _ => {}
        }
    }
    if let Some(code) = polled_main {
        return Captured::Combo(build_hotkey(
            modifiers.ctrl,
            modifiers.shift,
            modifiers.alt,
            win,
            code,
        ));
    }
    Captured::Nothing
}

fn friendly_part(part: &str) -> String {
    match part {
        "Control" | "Ctrl" => "Ctrl".into(),
        "Super" | "Meta" | "Cmd" | "Command" => "Win".into(),
        "PrintScreen" => "PrtSc".into(),
        "ArrowUp" => "Up".into(),
        "ArrowDown" => "Down".into(),
        "ArrowLeft" => "Left".into(),
        "ArrowRight" => "Right".into(),
        "PageUp" => "PgUp".into(),
        "PageDown" => "PgDn".into(),
        "Backquote" => "`".into(),
        "Minus" => "-".into(),
        "Equal" => "=".into(),
        "BracketLeft" => "[".into(),
        "BracketRight" => "]".into(),
        "Backslash" => "\\".into(),
        "Semicolon" => ";".into(),
        "Quote" => "'".into(),
        "Comma" => ",".into(),
        "Period" => ".".into(),
        "Slash" => "/".into(),
        "ScrollLock" => "ScrLk".into(),
        "Escape" => "Esc".into(),
        p => {
            if let Some(rest) = p.strip_prefix("Key")
                && rest.len() == 1
            {
                return rest.to_string();
            }
            if let Some(rest) = p.strip_prefix("Digit")
                && rest.len() == 1
            {
                return rest.to_string();
            }
            p.to_string()
        }
    }
}

/// "Control+Shift+KeyS" -> "Ctrl+Shift+S"; "" -> "Not Set".
pub fn display(hotkey: &str) -> String {
    let t = hotkey.trim();
    if t.is_empty() {
        return "Not Set".into();
    }
    t.split('+')
        .map(|p| friendly_part(p.trim()))
        .collect::<Vec<_>>()
        .join("+")
}

/// Two hotkey strings that bind the same combination (case-insensitive, modifier-order
/// independent, Ctrl/Control aliases equal).
pub fn same_combo(a: &str, b: &str) -> bool {
    fn key(s: &str) -> Option<(Vec<String>, String)> {
        let t = s.trim();
        if t.is_empty() {
            return None;
        }
        let mut mods = Vec::new();
        let mut main = String::new();
        for part in t.split('+') {
            let p = part.trim();
            match p.to_ascii_uppercase().as_str() {
                "CONTROL" | "CTRL" => mods.push("C".to_string()),
                "SHIFT" => mods.push("S".to_string()),
                "ALT" | "OPTION" => mods.push("A".to_string()),
                "SUPER" | "META" | "CMD" | "COMMAND" => mods.push("W".to_string()),
                other => main = other.to_string(),
            }
        }
        mods.sort();
        Some((mods, main))
    }
    match (key(a), key(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key_event(key: Key, physical: Option<Key>, pressed: bool, m: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: physical,
            pressed,
            repeat: false,
            modifiers: m,
        }
    }

    fn ctrl_shift() -> Modifiers {
        Modifiers {
            ctrl: true,
            shift: true,
            command: true,
            ..Default::default()
        }
    }

    #[test]
    fn builds_default_style_strings() {
        assert_eq!(
            build_hotkey(true, true, false, false, "KeyS"),
            "Control+Shift+KeyS"
        );
        assert_eq!(
            build_hotkey(false, false, true, false, "PrintScreen"),
            "Alt+PrintScreen"
        );
        assert_eq!(
            build_hotkey(false, false, false, false, "PrintScreen"),
            "PrintScreen"
        );
        assert_eq!(
            build_hotkey(true, true, true, true, "F5"),
            "Control+Shift+Alt+Super+F5"
        );
    }

    #[test]
    fn captures_combo_from_key_event() {
        let ev = [key_event(Key::S, Some(Key::S), true, ctrl_shift())];
        assert_eq!(
            scan(&ev, ctrl_shift(), false, None),
            Captured::Combo("Control+Shift+KeyS".into())
        );
    }

    #[test]
    fn prefers_the_physical_key() {
        // Dvorak: logical 'Semicolon' sits on the physical 'Z' key.
        let ev = [key_event(
            Key::Semicolon,
            Some(Key::Z),
            true,
            Modifiers::default(),
        )];
        assert_eq!(
            scan(&ev, Modifiers::default(), false, None),
            Captured::Combo("KeyZ".into())
        );
    }

    #[test]
    fn modifier_only_presses_wait_for_a_main_key() {
        let ev = [key_event(
            Key::ControlLeft,
            Some(Key::ControlLeft),
            true,
            Modifiers {
                ctrl: true,
                command: true,
                ..Default::default()
            },
        )];
        assert_eq!(
            scan(&ev, Modifiers::default(), false, None),
            Captured::Nothing
        );
    }

    #[test]
    fn escape_cancels_and_backspace_clears_only_bare() {
        let none = Modifiers::default();
        assert_eq!(
            scan(
                &[key_event(Key::Escape, None, true, none)],
                none,
                false,
                None
            ),
            Captured::Cancel
        );
        assert_eq!(
            scan(
                &[key_event(Key::Backspace, None, true, none)],
                none,
                false,
                None
            ),
            Captured::Clear
        );
        assert_eq!(
            scan(
                &[key_event(Key::Delete, None, true, none)],
                none,
                false,
                None
            ),
            Captured::Clear
        );
        let ctrl = Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        };
        assert_eq!(
            scan(
                &[key_event(Key::Escape, None, true, ctrl)],
                ctrl,
                false,
                None
            ),
            Captured::Combo("Control+Escape".into())
        );
    }

    #[test]
    fn key_release_and_repeat_are_ignored_except_print_screen() {
        let none = Modifiers::default();
        assert_eq!(
            scan(&[key_event(Key::A, None, false, none)], none, false, None),
            Captured::Nothing
        );
        let repeat = Event::Key {
            key: Key::A,
            physical_key: None,
            pressed: true,
            repeat: true,
            modifiers: none,
        };
        assert_eq!(scan(&[repeat], none, false, None), Captured::Nothing);
        assert_eq!(
            scan(
                &[key_event(Key::PrintScreen, None, false, none)],
                none,
                false,
                None
            ),
            Captured::Combo("PrintScreen".into())
        );
    }

    #[test]
    fn clipboard_events_become_ctrl_combos() {
        let ctrl = Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        };
        assert_eq!(
            scan(&[Event::Copy], ctrl, false, None),
            Captured::Combo("Control+KeyC".into())
        );
        assert_eq!(
            scan(&[Event::Cut], ctrl, false, None),
            Captured::Combo("Control+KeyX".into())
        );
        assert_eq!(
            scan(&[Event::Paste(String::new())], ctrl, false, None),
            Captured::Combo("Control+KeyV".into())
        );
    }

    #[test]
    fn windows_key_and_polled_main_key() {
        let none = Modifiers::default();
        let ev = [key_event(Key::E, Some(Key::E), true, none)];
        assert_eq!(
            scan(&ev, none, true, None),
            Captured::Combo("Super+KeyE".into())
        );
        assert_eq!(
            scan(
                &[],
                Modifiers {
                    alt: true,
                    ..Default::default()
                },
                false,
                Some("PrintScreen")
            ),
            Captured::Combo("Alt+PrintScreen".into())
        );
    }

    #[test]
    fn display_is_friendly() {
        assert_eq!(display("Control+Shift+KeyS"), "Ctrl+Shift+S");
        assert_eq!(display("PrintScreen"), "PrtSc");
        assert_eq!(display("Alt+PrintScreen"), "Alt+PrtSc");
        assert_eq!(display("Shift+PrintScreen"), "Shift+PrtSc");
        assert_eq!(display("Control+Alt+Digit5"), "Ctrl+Alt+5");
        assert_eq!(display("Super+ArrowUp"), "Win+Up");
        assert_eq!(display("F12"), "F12");
        assert_eq!(display(""), "Not Set");
    }

    #[test]
    fn combo_comparison_ignores_order_and_aliases() {
        assert!(same_combo("Control+Shift+KeyS", "shift+ctrl+keys"));
        assert!(!same_combo("Control+KeyS", "Control+Shift+KeyS"));
        assert!(!same_combo("", ""));
        assert!(same_combo("PrintScreen", "PrintScreen"));
    }

    #[test]
    fn every_capturable_key_parses_with_global_hotkey() {
        use global_hotkey::hotkey::HotKey;
        for key in Key::ALL {
            if let Some(code) = code_name(*key) {
                for s in [code.to_string(), build_hotkey(true, true, true, true, code)] {
                    assert!(
                        HotKey::try_from(s.as_str()).is_ok(),
                        "{key:?} -> {s} does not parse"
                    );
                }
            }
        }
        // The shipped defaults also round-trip through display().
        let d = stinkshot_core::config::Hotkeys::default();
        for s in [
            &d.region,
            &d.window,
            &d.fullscreen,
            &d.scrolling,
            &d.ocr,
            &d.repeat_last_region,
            &d.record_mp4,
            &d.record_gif,
        ] {
            assert!(HotKey::try_from(s.as_str()).is_ok());
            assert_ne!(display(s), "Not Set");
        }
    }
}
