//! Global hotkeys via `global-hotkey`. Parses `config.hotkeys` strings; PrintScreen
//! variants require `system::take_over_printscreen(true)` when `take_over_printscreen`.
//!
//! The manager owns a hidden window on the thread that created it (the UI thread), so
//! hotkey messages arrive through the winit event loop. A bridge thread forwards them to a
//! channel and wakes the UI.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, OnceLock};

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use crate::actions::Action;
use crate::state::Command;

fn bindings(cfg: &stinkshot_core::config::Hotkeys) -> [(Action, &str); 8] {
    Action::ALL.map(|a| (a, a.hotkey(cfg).as_str()))
}

/// Phrases that identify each hotkey problem in its long text (`ErrorKind::of`).
const INVALID: &str = "is not a valid key combination";
const DUPLICATE: &str = "is set for both";
const IN_USE: &str = "is already used by another program";
const NOT_REGISTERED: &str = "could not be registered with Windows";

/// Hotkey problem texts (long form, for the startup toast and the summary box in Settings).
/// Each starts with the key's display form so Settings can attach it to the matching row
/// (`settings_ui::error_for`), and each says what to do.
fn parse_error_text(key: &str, action: Action) -> String {
    format!(
        "{key} ({}) {INVALID}. Record it again in Settings > Hotkeys.",
        action.label()
    )
}

fn duplicate_text(key: &str, action: Action, other: Action) -> String {
    format!(
        "{key} {DUPLICATE} {} and {}; only {} works. Change one of them in Settings > Hotkeys.",
        other.label(),
        action.label(),
        other.label()
    )
}

fn register_error_text(key: &str, action: Action, in_use: bool) -> String {
    if in_use {
        format!(
            "{key} ({}) {IN_USE}. Pick a different key in Settings > Hotkeys.",
            action.label()
        )
    } else {
        format!(
            "{key} ({}) {NOT_REGISTERED}. Pick a different key in Settings > Hotkeys.",
            action.label()
        )
    }
}

/// Which hotkey problem a long error text describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Invalid,
    Duplicate,
    InUse,
    NotRegistered,
}

impl ErrorKind {
    pub fn of(text: &str) -> Option<ErrorKind> {
        [
            (INVALID, ErrorKind::Invalid),
            (DUPLICATE, ErrorKind::Duplicate),
            (IN_USE, ErrorKind::InUse),
            (NOT_REGISTERED, ErrorKind::NotRegistered),
        ]
        .into_iter()
        .find(|(p, _)| text.contains(p))
        .map(|(_, k)| k)
    }

    /// Short form for the hotkey's own row in Settings > Hotkeys: no key, action name or
    /// Settings path, which the row already shows.
    pub fn row_text(self) -> &'static str {
        match self {
            ErrorKind::Invalid => "Not a valid key combination. Record it again.",
            ErrorKind::Duplicate => "Also set for another action, so only one of them works.",
            ErrorKind::InUse => "Already used by another program. Pick a different key.",
            ErrorKind::NotRegistered => "Windows could not register it. Pick a different key.",
        }
    }
}

pub struct Hotkeys {
    manager: Option<GlobalHotKeyManager>,
    registered: Vec<HotKey>,
    actions: HashMap<u32, Action>,
    suspended: bool,
    took_over_printscreen: bool,
    rx: Receiver<u32>,
}

/// The global-hotkey receiver is process-wide, so exactly one bridge thread exists.
static BRIDGE: OnceLock<Mutex<Option<Receiver<u32>>>> = OnceLock::new();

fn start_bridge() -> Receiver<u32> {
    let (tx, rx): (Sender<u32>, Receiver<u32>) = channel();
    let events = GlobalHotKeyEvent::receiver().clone();
    let spawned = std::thread::Builder::new()
        .name("hotkey-bridge".into())
        .spawn(move || {
            while let Ok(ev) = events.recv() {
                if ev.state() == HotKeyState::Pressed {
                    if tx.send(ev.id()).is_err() {
                        break;
                    }
                    crate::wake::wake();
                }
            }
        });
    if let Err(e) = spawned {
        log::error!("cannot start hotkey bridge thread: {e}");
    }
    rx
}

impl Hotkeys {
    /// Returns the manager plus human-readable registration errors (conflicts, bad strings).
    pub fn new(cfg: &stinkshot_core::config::Hotkeys) -> (Hotkeys, Vec<String>) {
        let slot = BRIDGE.get_or_init(|| Mutex::new(Some(start_bridge())));
        let rx = slot
            .lock()
            .ok()
            .and_then(|mut g| g.take())
            .unwrap_or_else(|| {
                // A second Hotkeys in one process never receives events; this is only reachable
                // in tests.
                channel().1
            });
        let mut errors = Vec::new();
        let manager = match GlobalHotKeyManager::new() {
            Ok(m) => Some(m),
            Err(e) => {
                log::error!("global hotkey manager failed: {e}");
                errors.push(
                    "Global hotkeys are unavailable. Use the tray icon, or restart Stinkshot."
                        .to_string(),
                );
                None
            }
        };
        let mut hk = Hotkeys {
            manager,
            registered: Vec::new(),
            actions: HashMap::new(),
            suspended: false,
            took_over_printscreen: false,
            rx,
        };
        errors.extend(hk.reload(cfg));
        (hk, errors)
    }

    /// Unregister all and register from `cfg`. Returns errors.
    pub fn reload(&mut self, cfg: &stinkshot_core::config::Hotkeys) -> Vec<String> {
        let mut errors = Vec::new();
        self.unregister_all();

        let uses_printscreen = bindings(cfg)
            .iter()
            .any(|(_, s)| s.to_ascii_lowercase().contains("printscreen"));
        let want_takeover = cfg.take_over_printscreen && uses_printscreen;
        if want_takeover || self.took_over_printscreen {
            match stinkshot_platform::system::take_over_printscreen(want_takeover) {
                Ok(_) => self.took_over_printscreen = want_takeover,
                Err(e) => {
                    log::warn!("PrintScreen takeover change failed: {e:#}");
                    errors.push(
                        "Windows did not let Stinkshot take over PrtSc, so it may still open the Snipping Tool. Turn off Take Over PrintScreen or try again."
                            .to_string(),
                    );
                }
            }
        }

        let Some(manager) = &self.manager else {
            return errors;
        };
        let mut claimed: HashMap<u32, Action> = HashMap::new();
        for (action, text) in bindings(cfg) {
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            let hotkey: HotKey = match text.parse() {
                Ok(h) => h,
                Err(e) => {
                    log::warn!("hotkey {text} for {} does not parse: {e}", action.label());
                    errors.push(parse_error_text(&display(text), action));
                    continue;
                }
            };
            if let Some(other) = claimed.get(&hotkey.id()) {
                errors.push(duplicate_text(&display(text), action, *other));
                continue;
            }
            match manager.register(hotkey) {
                Ok(()) => {
                    claimed.insert(hotkey.id(), action);
                    self.registered.push(hotkey);
                }
                Err(e) => {
                    log::warn!("hotkey {text} for {} failed: {e}", action.label());
                    let in_use = matches!(e, global_hotkey::Error::AlreadyRegistered(_));
                    errors.push(register_error_text(&display(text), action, in_use));
                }
            }
        }
        self.actions = claimed;
        // Everything registered above is live again, whatever the previous state was.
        self.suspended = false;
        errors
    }

    fn unregister_all(&mut self) {
        if let Some(m) = &self.manager {
            for hk in self.registered.drain(..) {
                if let Err(e) = m.unregister(hk) {
                    log::debug!("unregister {hk}: {e}");
                }
            }
        }
        self.registered.clear();
        self.actions.clear();
    }

    pub fn poll(&mut self) -> Vec<Command> {
        let mut out = Vec::new();
        while let Ok(id) = self.rx.try_recv() {
            if self.suspended {
                continue;
            }
            if let Some(action) = self.actions.get(&id) {
                out.push(action.command());
            }
        }
        out
    }

    /// Pause all hotkeys (while the settings window is capturing a new combo). The combos
    /// are unregistered from Windows, not just ignored: RegisterHotKey swallows the
    /// keystroke before any window sees it, so the settings window could never record a
    /// combination that is already bound. Resuming registers them again.
    pub fn set_suspended(&mut self, suspended: bool) {
        if self.suspended == suspended {
            return;
        }
        self.suspended = suspended;
        let Some(m) = &self.manager else { return };
        if suspended {
            for hk in &self.registered {
                if let Err(e) = m.unregister(*hk) {
                    log::debug!("suspend: unregister {hk}: {e}");
                }
            }
        } else {
            let mut lost = Vec::new();
            self.registered.retain(|hk| match m.register(*hk) {
                Ok(()) => true,
                Err(e) => {
                    log::warn!("could not re-register hotkey {hk}: {e}");
                    lost.push(hk.id());
                    false
                }
            });
            for id in lost {
                self.actions.remove(&id);
            }
        }
    }

    pub fn is_suspended(&self) -> bool {
        self.suspended
    }

    /// Unregister everything and give PrintScreen back to Windows. Call on quit.
    pub fn shutdown(&mut self) {
        self.unregister_all();
        if self.took_over_printscreen {
            if let Err(e) = stinkshot_platform::system::take_over_printscreen(false) {
                log::warn!("could not restore PrintScreen setting: {e:#}");
            }
            self.took_over_printscreen = false;
        }
    }
}

/// Display form, e.g. "Control+Shift+KeyS" -> "Ctrl+Shift+S", "PrintScreen" -> "PrtSc".
pub fn display(hotkey: &str) -> String {
    let hotkey = hotkey.trim();
    if hotkey.is_empty() {
        return String::new();
    }
    let (mut ctrl, mut alt, mut shift, mut win) = (false, false, false, false);
    let mut key = String::new();
    for token in hotkey.split('+') {
        let t = token.trim();
        match t.to_ascii_uppercase().as_str() {
            "CONTROL" | "CTRL" | "COMMANDORCONTROL" | "COMMANDORCTRL" | "CMDORCTRL"
            | "CMDORCONTROL" => ctrl = true,
            "ALT" | "OPTION" => alt = true,
            "SHIFT" => shift = true,
            "META" | "SUPER" | "WIN" | "CMD" | "COMMAND" => win = true,
            _ => key = key_label(t),
        }
    }
    let mut parts: Vec<&str> = Vec::new();
    if ctrl {
        parts.push("Ctrl");
    }
    if alt {
        parts.push("Alt");
    }
    if shift {
        parts.push("Shift");
    }
    if win {
        parts.push("Win");
    }
    if !key.is_empty() {
        parts.push(&key);
    }
    parts.join("+")
}

fn key_label(code: &str) -> String {
    let up = code.to_ascii_uppercase();
    match up.as_str() {
        "PRINTSCREEN" | "SNAPSHOT" => return "PrtSc".into(),
        "ESCAPE" => return "Esc".into(),
        "BACKQUOTE" => return "`".into(),
        "MINUS" => return "-".into(),
        "EQUAL" => return "=".into(),
        "COMMA" => return ",".into(),
        "PERIOD" => return ".".into(),
        "SLASH" => return "/".into(),
        "BACKSLASH" => return "\\".into(),
        "SEMICOLON" => return ";".into(),
        "QUOTE" => return "'".into(),
        "BRACKETLEFT" => return "[".into(),
        "BRACKETRIGHT" => return "]".into(),
        "PAGEUP" => return "PgUp".into(),
        "PAGEDOWN" => return "PgDn".into(),
        "DELETE" => return "Del".into(),
        "INSERT" => return "Ins".into(),
        "ARROWUP" => return "Up".into(),
        "ARROWDOWN" => return "Down".into(),
        "ARROWLEFT" => return "Left".into(),
        "ARROWRIGHT" => return "Right".into(),
        _ => {}
    }
    if let Some(rest) = up.strip_prefix("KEY")
        && rest.len() == 1
    {
        return rest.to_string();
    }
    if let Some(rest) = up.strip_prefix("DIGIT")
        && rest.len() == 1
    {
        return rest.to_string();
    }
    if up.starts_with("NUMPAD") && code.len() > 6 {
        return format!("Num {}", &code[6..]);
    }
    // F1..F24, Space, Enter, Tab, Home, End ... keep the given spelling.
    code.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_forms() {
        assert_eq!(display("Control+Shift+KeyS"), "Ctrl+Shift+S");
        assert_eq!(display("PrintScreen"), "PrtSc");
        assert_eq!(display("Alt+PrintScreen"), "Alt+PrtSc");
        assert_eq!(display("Control+PrintScreen"), "Ctrl+PrtSc");
        assert_eq!(display("Shift+PrintScreen"), "Shift+PrtSc");
        assert_eq!(display("Shift+Control+KeyR"), "Ctrl+Shift+R");
        assert_eq!(display("Ctrl+Alt+Digit5"), "Ctrl+Alt+5");
        assert_eq!(display("Control+F9"), "Ctrl+F9");
        assert_eq!(display("Meta+KeyV"), "Win+V");
        assert_eq!(display(""), "");
    }

    #[test]
    fn default_hotkeys_parse() {
        let cfg = stinkshot_core::config::Hotkeys::default();
        for (action, text) in bindings(&cfg) {
            let parsed: Result<HotKey, _> = text.parse();
            assert!(parsed.is_ok(), "{} ({text}) should parse", action.label());
        }
    }

    #[test]
    fn error_texts_lead_with_the_key_and_say_what_to_do() {
        for t in [
            parse_error_text("Ctrl+Q", Action::Window),
            duplicate_text("Ctrl+Q", Action::Window, Action::Region),
            register_error_text("Ctrl+Q", Action::Window, true),
            register_error_text("Ctrl+Q", Action::Window, false),
        ] {
            assert!(t.starts_with("Ctrl+Q"), "{t}");
            assert!(t.contains("Settings > Hotkeys"), "{t}");
            assert!(t.contains("Window") || t.contains("Region"), "{t}");
        }
    }

    #[test]
    fn row_texts_drop_the_key_action_and_path() {
        let cases = [
            (
                parse_error_text("Ctrl+Q", Action::Window),
                ErrorKind::Invalid,
            ),
            (
                duplicate_text("Ctrl+Q", Action::Window, Action::Region),
                ErrorKind::Duplicate,
            ),
            (
                register_error_text("Ctrl+Q", Action::Window, true),
                ErrorKind::InUse,
            ),
            (
                register_error_text("Ctrl+Q", Action::Window, false),
                ErrorKind::NotRegistered,
            ),
        ];
        for (long, kind) in cases {
            assert_eq!(ErrorKind::of(&long), Some(kind), "{long}");
            let row = kind.row_text();
            assert!(
                !row.contains("Ctrl+Q") && !row.contains("Settings"),
                "{row}"
            );
            let names_action = row
                .split_whitespace()
                .any(|w| w == "Window" || w == "Region");
            assert!(!names_action, "{row}");
        }
    }

    #[test]
    fn default_hotkeys_are_distinct() {
        let cfg = stinkshot_core::config::Hotkeys::default();
        let mut ids: Vec<u32> = bindings(&cfg)
            .iter()
            .filter_map(|(_, t)| t.parse::<HotKey>().ok().map(|h| h.id()))
            .collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n);
    }
}
