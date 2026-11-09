//! Global hotkeys via `global-hotkey`. Parses `config.hotkeys` strings; PrintScreen
//! variants require `system::take_over_printscreen(true)` when `take_over_printscreen`
//! (or `replace_snipping_tool`) is on.
//!
//! Replace Snipping Tool (`replace_snipping_tool`) also installs
//! `stinkshot_platform::snip::SnipHook`, which swallows Win+Shift+S and Win+Shift+R and
//! routes them here (docs/notes/snipping-tool.md). A binding of one of those chords to a
//! Stinkshot action is not registered with Windows while the hook is on; the hook routes the
//! chord to that action instead, so nothing fires twice.
//!
//! The manager owns a hidden window on the thread that created it (the UI thread), so
//! hotkey messages arrive through the winit event loop. A bridge thread forwards them to a
//! channel and wakes the UI.
//!
//! A key another program holds (Greenshot, say) fails to register. Those keys are retried
//! in the background (`Hotkeys::tick`, `RetryClock`): every few seconds at first, then
//! slower, and at once when Settings opens, Check Again is clicked, or the session unlocks
//! or resumes (`request_retry`). Once the other program lets go, the key works and its
//! warning goes away without a restart.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use stinkshot_platform::snip::{SnipChord, SnipHook};

use crate::actions::Action;
use crate::l10n::tr;
use crate::state::Command;

fn bindings(cfg: &stinkshot_core::config::Hotkeys) -> [(Action, &str); Action::ALL.len()] {
    Action::ALL.map(|a| (a, a.hotkey(cfg).as_str()))
}

/// Phrases that identify each hotkey problem in its long text (`ErrorKind::of`). Looked up so
/// a translated long text still matches its kind.
fn invalid_phrase() -> &'static str {
    tr!("shell.hotkey.phrase.invalid")
}
fn duplicate_phrase() -> &'static str {
    tr!("shell.hotkey.phrase.duplicate")
}
fn in_use_phrase() -> &'static str {
    tr!("shell.hotkey.phrase.in_use")
}
fn not_registered_phrase() -> &'static str {
    tr!("shell.hotkey.phrase.not_registered")
}

/// Hotkey problem texts (long form, for the startup toast and the summary box in Settings).
/// Each starts with the key's display form so Settings can attach it to the matching row
/// (`settings_ui::error_for`), and each says what to do.
fn parse_error_text(key: &str, action: Action) -> String {
    tr!(
        "shell.hotkey.invalid",
        key = key,
        action = action.label(),
        phrase = invalid_phrase()
    )
}

fn duplicate_text(key: &str, action: Action, other: Action) -> String {
    tr!(
        "shell.hotkey.duplicate",
        key = key,
        phrase = duplicate_phrase(),
        other = other.label(),
        action = action.label()
    )
}

fn register_error_text(key: &str, action: Action, in_use: bool) -> String {
    if in_use {
        tr!(
            "shell.hotkey.in_use",
            key = key,
            action = action.label(),
            phrase = in_use_phrase()
        )
    } else {
        tr!(
            "shell.hotkey.not_registered",
            key = key,
            action = action.label(),
            phrase = not_registered_phrase()
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
            (invalid_phrase(), ErrorKind::Invalid),
            (duplicate_phrase(), ErrorKind::Duplicate),
            (in_use_phrase(), ErrorKind::InUse),
            (not_registered_phrase(), ErrorKind::NotRegistered),
        ]
        .into_iter()
        .find(|(p, _)| text.contains(p))
        .map(|(_, k)| k)
    }

    /// Short form for the hotkey's own row in Settings > Hotkeys: no key, action name or
    /// Settings path, which the row already shows.
    pub fn row_text(self) -> &'static str {
        match self {
            ErrorKind::Invalid => tr!("shell.hotkey.row.invalid"),
            ErrorKind::Duplicate => tr!("shell.hotkey.row.duplicate"),
            ErrorKind::InUse => tr!("shell.hotkey.row.in_use"),
            ErrorKind::NotRegistered => tr!("shell.hotkey.row.not_registered"),
        }
    }
}

/// State of Replace Snipping Tool, for the status line in Settings > Hotkeys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnipStatus {
    Off,
    Active,
    /// The hook could not be installed; the text says why and what to do.
    Unavailable(String),
}

/// The action a Snipping Tool chord runs when no Stinkshot hotkey is bound to it.
fn default_snip_action(chord: SnipChord) -> Action {
    match chord {
        SnipChord::Region => Action::Region,
        SnipChord::Record => Action::RecordMp4,
    }
}

/// Which action each Snipping Tool chord runs: the action bound to that exact chord in
/// `cfg`, if any, otherwise the default (Region, Record MP4).
pub fn snip_routes(cfg: &stinkshot_core::config::Hotkeys) -> [(SnipChord, Action); 2] {
    SnipChord::ALL.map(|chord| {
        let bound = bindings(cfg)
            .into_iter()
            .find(|(_, text)| same_hotkey(text, chord.hotkey_string()))
            .map(|(a, _)| a);
        (chord, bound.unwrap_or(default_snip_action(chord)))
    })
}

/// Same combination, compared by the parsed `global-hotkey` id.
fn same_hotkey(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim(), b.trim());
    if a.is_empty() || b.is_empty() {
        return false;
    }
    match (a.parse::<HotKey>(), b.parse::<HotKey>()) {
        (Ok(x), Ok(y)) => x.id() == y.id(),
        _ => false,
    }
}

/// A combination bound to an action, with its display form for error texts.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Binding {
    hotkey: HotKey,
    action: Action,
    key: String,
}

/// A binding Windows refused; retried until it registers or the config changes.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Pending {
    binding: Binding,
    in_use: bool,
}

impl Pending {
    fn text(&self) -> String {
        register_error_text(&self.binding.key, self.binding.action, self.in_use)
    }
}

/// One problem text. `retry` is the hotkey id of a failed registration that is still being
/// retried; the entry goes away when that hotkey registers.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ErrorEntry {
    text: String,
    retry: Option<u32>,
}

/// Registration seam, so the retry logic is testable without Windows.
trait Registrar {
    fn register(&self, hotkey: HotKey) -> Result<(), global_hotkey::Error>;
}

impl Registrar for GlobalHotKeyManager {
    fn register(&self, hotkey: HotKey) -> Result<(), global_hotkey::Error> {
        GlobalHotKeyManager::register(self, hotkey)
    }
}

/// Try every pending binding once. Returns the ones that registered (removed from
/// `pending`) and whether a remaining one changed its error kind. Each hotkey is registered
/// at most once: a success leaves `pending` in the same call.
fn retry_pending(reg: &impl Registrar, pending: &mut Vec<Pending>) -> (Vec<Binding>, bool) {
    let mut done = Vec::new();
    let mut changed = false;
    pending.retain_mut(|p| match reg.register(p.binding.hotkey) {
        Ok(()) => {
            done.push(p.binding.clone());
            false
        }
        Err(e) => {
            let in_use = matches!(e, global_hotkey::Error::AlreadyRegistered(_));
            if in_use != p.in_use {
                p.in_use = in_use;
                changed = true;
            }
            true
        }
    });
    (done, changed)
}

/// When to retry failed registrations: every `FAST` for the first `FAST_TRIES` attempts
/// after something failed (or after a kick), then every `SLOW`. Idle when nothing fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RetryClock {
    next: Option<Instant>,
    tries: u32,
}

impl RetryClock {
    const FAST: Duration = Duration::from_secs(3);
    const FAST_TRIES: u32 = 40;
    const SLOW: Duration = Duration::from_secs(30);

    const fn idle() -> Self {
        RetryClock {
            next: None,
            tries: 0,
        }
    }

    fn interval(&self) -> Duration {
        if self.tries < Self::FAST_TRIES {
            Self::FAST
        } else {
            Self::SLOW
        }
    }

    /// Something failed just now: first retry one fast interval from `now`.
    fn start(&mut self, now: Instant) {
        self.tries = 0;
        self.next = Some(now + Self::FAST);
    }

    /// Nothing fails any more.
    fn stop(&mut self) {
        *self = Self::idle();
    }

    /// Retry at the next tick and go back to the fast pace. No effect when idle.
    fn kick(&mut self, now: Instant) {
        if self.next.is_some() {
            self.tries = 0;
            self.next = Some(now);
        }
    }

    fn due(&self, now: Instant) -> bool {
        self.next.is_some_and(|t| now >= t)
    }

    /// An attempt was made at `now` and something still fails.
    fn attempted(&mut self, now: Instant) {
        self.tries = self.tries.saturating_add(1);
        self.next = Some(now + self.interval());
    }

    /// How long until the next attempt, if one is scheduled.
    fn wait(&self, now: Instant) -> Option<Duration> {
        self.next.map(|t| t.saturating_duration_since(now))
    }
}

/// Set from any thread (session unlock or resume) to retry failed hotkeys on the next tick.
static RETRY_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Ask the UI thread to retry failed hotkeys now, and wake it. Safe from any thread.
pub fn request_retry() {
    RETRY_REQUESTED.store(true, Ordering::Relaxed);
    crate::wake::wake();
}

pub struct Hotkeys {
    manager: Option<GlobalHotKeyManager>,
    registered: Vec<Binding>,
    /// Failed registrations being retried (`tick`).
    pending: Vec<Pending>,
    /// Problems from the last reload, minus retries that have since succeeded.
    errors: Vec<ErrorEntry>,
    /// `errors` changed outside `tick` (a key lost on resume); reported by the next `tick`.
    errors_changed: bool,
    retry: RetryClock,
    actions: HashMap<u32, Action>,
    suspended: bool,
    took_over_printscreen: bool,
    rx: Receiver<u32>,
    snip: Option<SnipHook>,
    snip_status: SnipStatus,
    snip_routes: [(SnipChord, Action); 2],
    snip_tx: Sender<SnipChord>,
    snip_rx: Receiver<SnipChord>,
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
                errors.push(tr!("shell.hotkey.unavailable").to_string());
                None
            }
        };
        let (snip_tx, snip_rx) = channel();
        let mut hk = Hotkeys {
            manager,
            registered: Vec::new(),
            pending: Vec::new(),
            errors: Vec::new(),
            errors_changed: false,
            retry: RetryClock::idle(),
            actions: HashMap::new(),
            suspended: false,
            // Assume a takeover is in place, so the first reload hands PrtSc back to
            // Windows when the takeover is not wanted. That repairs the registry value a
            // crashed or killed instance left at 0; with nothing saved it does nothing.
            took_over_printscreen: true,
            rx,
            snip: None,
            snip_status: SnipStatus::Off,
            snip_routes: snip_routes(cfg),
            snip_tx,
            snip_rx,
        };
        errors.extend(hk.reload(cfg));
        (hk, errors)
    }

    /// Unregister all and register from `cfg`. Returns errors (also kept for `errors`).
    pub fn reload(&mut self, cfg: &stinkshot_core::config::Hotkeys) -> Vec<String> {
        self.errors = self.reload_entries(cfg);
        self.errors_changed = false;
        if self.pending.is_empty() {
            self.retry.stop();
        } else {
            self.retry.start(Instant::now());
        }
        self.errors.iter().map(|e| e.text.clone()).collect()
    }

    fn reload_entries(&mut self, cfg: &stinkshot_core::config::Hotkeys) -> Vec<ErrorEntry> {
        let mut errors: Vec<ErrorEntry> = Vec::new();
        let plain = |text: String| ErrorEntry { text, retry: None };
        self.unregister_all();

        let uses_printscreen = bindings(cfg)
            .iter()
            .any(|(_, s)| s.to_ascii_lowercase().contains("printscreen"));
        let replace = cfg.replace_snipping_tool;
        // Replace Snipping Tool implies the PrintScreen takeover. Without a PrintScreen
        // binding the takeover would only hand the key back to the legacy clipboard copy, so
        // it stays off then (Settings says so).
        let want_takeover = (cfg.take_over_printscreen || replace) && uses_printscreen;
        self.set_snip_hook(replace);
        if let Some(h) = &self.snip {
            // Reload leaves everything live (see the end of this function).
            h.set_pass_through(false);
        }
        self.snip_routes = snip_routes(cfg);
        if want_takeover || self.took_over_printscreen {
            match stinkshot_platform::system::take_over_printscreen(want_takeover) {
                Ok(changed) => {
                    if changed && !want_takeover {
                        log::info!("PrintScreen handed back to Windows");
                    }
                    self.took_over_printscreen = want_takeover;
                }
                Err(e) if !want_takeover => {
                    log::warn!("could not restore PrintScreen setting: {e:#}");
                }
                Err(e) => {
                    log::warn!("PrintScreen takeover change failed: {e:#}");
                    errors.push(plain(tr!("shell.hotkey.prtsc_failed").to_string()));
                }
            }
        }

        let Some(manager) = &self.manager else {
            return errors;
        };
        // Every id a binding took, registered or not, so a second binding of the same
        // combination is reported as a duplicate instead of being registered or retried twice.
        let mut claimed: HashMap<u32, Action> = HashMap::new();
        let mut actions: HashMap<u32, Action> = HashMap::new();
        for (action, text) in bindings(cfg) {
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            let hotkey: HotKey = match text.parse() {
                Ok(h) => h,
                Err(e) => {
                    log::warn!("hotkey {text} for {} does not parse: {e}", action.label());
                    errors.push(plain(parse_error_text(&display(text), action)));
                    continue;
                }
            };
            if let Some(other) = claimed.get(&hotkey.id()) {
                errors.push(plain(duplicate_text(&display(text), action, *other)));
                continue;
            }
            if self.snip.is_some()
                && SnipChord::ALL
                    .iter()
                    .any(|c| same_hotkey(text, c.hotkey_string()))
            {
                // The hook swallows this chord before Windows could deliver it as a hotkey
                // and routes it to `action` itself (`snip_routes`).
                claimed.insert(hotkey.id(), action);
                actions.insert(hotkey.id(), action);
                continue;
            }
            claimed.insert(hotkey.id(), action);
            let binding = Binding {
                hotkey,
                action,
                key: display(text),
            };
            match manager.register(hotkey) {
                Ok(()) => {
                    actions.insert(hotkey.id(), action);
                    self.registered.push(binding);
                }
                Err(e) => {
                    log::warn!("hotkey {text} for {} failed: {e}", action.label());
                    let in_use = matches!(e, global_hotkey::Error::AlreadyRegistered(_));
                    let p = Pending { binding, in_use };
                    errors.push(ErrorEntry {
                        text: p.text(),
                        retry: Some(hotkey.id()),
                    });
                    self.pending.push(p);
                }
            }
        }
        self.actions = actions;
        // Everything registered above is live again, whatever the previous state was.
        self.suspended = false;
        errors
    }

    fn unregister_all(&mut self) {
        if let Some(m) = &self.manager {
            for b in self.registered.drain(..) {
                if let Err(e) = m.unregister(b.hotkey) {
                    log::debug!("unregister {}: {e}", b.hotkey);
                }
            }
        }
        self.registered.clear();
        self.pending.clear();
        self.retry.stop();
        self.actions.clear();
    }

    /// Current problem texts (Settings > Hotkeys). Changes when a retry succeeds; `tick`
    /// says when.
    pub fn errors(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.manager.is_none() {
            out.push(tr!("shell.hotkey.unavailable").to_string());
        }
        out.extend(self.errors.iter().map(|e| e.text.clone()));
        out
    }

    /// Retry failed hotkeys on the next `tick` instead of waiting for the timer.
    pub fn retry_now(&mut self) {
        self.retry.kick(Instant::now());
    }

    /// Retry failed registrations when due. Call on the UI thread (the manager's window
    /// lives there). Returns true when `errors()` changed. Does nothing while suspended:
    /// every hotkey is released on purpose then, and resuming registers them again.
    pub fn tick(&mut self) -> bool {
        let now = Instant::now();
        let flagged = std::mem::take(&mut self.errors_changed);
        if RETRY_REQUESTED.swap(false, Ordering::Relaxed) {
            self.retry.kick(now);
        }
        if self.suspended || !self.retry.due(now) {
            return flagged;
        }
        let Some(m) = &self.manager else {
            self.retry.stop();
            return flagged;
        };
        let (done, kind_changed) = retry_pending(m, &mut self.pending);
        let changed = flagged || kind_changed || !done.is_empty();
        for b in done {
            log::info!(
                "hotkey {} for {} registered on retry",
                b.key,
                b.action.label()
            );
            let id = b.hotkey.id();
            self.errors.retain(|e| e.retry != Some(id));
            self.actions.insert(id, b.action);
            self.registered.push(b);
        }
        if kind_changed {
            for p in &self.pending {
                let id = p.binding.hotkey.id();
                if let Some(e) = self.errors.iter_mut().find(|e| e.retry == Some(id)) {
                    e.text = p.text();
                }
            }
        }
        if self.pending.is_empty() {
            self.retry.stop();
        } else {
            self.retry.attempted(now);
        }
        changed
    }

    /// How long until the next retry, so the caller can wake up for it. None when nothing
    /// is being retried.
    pub fn next_retry_in(&self) -> Option<Duration> {
        self.retry.wait(Instant::now())
    }

    /// Install or remove the Snipping Tool hook.
    fn set_snip_hook(&mut self, on: bool) {
        if !on {
            if self.snip.take().is_some() {
                log::info!("Snipping Tool shortcuts handed back to Windows");
            }
            self.snip_status = SnipStatus::Off;
            return;
        }
        if self.snip.is_some() {
            return;
        }
        // The hook thread sends here; a forwarder wakes the UI. It ends when the hook's
        // sender goes away with the hook thread.
        let (hook_tx, hook_rx) = channel::<SnipChord>();
        let out = self.snip_tx.clone();
        match SnipHook::install(hook_tx) {
            Ok(h) => {
                let spawned = std::thread::Builder::new()
                    .name("snip-bridge".into())
                    .spawn(move || {
                        while let Ok(c) = hook_rx.recv() {
                            if out.send(c).is_err() {
                                break;
                            }
                            crate::wake::wake();
                        }
                    });
                if let Err(e) = spawned {
                    log::error!("cannot start Snipping Tool bridge thread: {e}");
                    self.snip_status =
                        SnipStatus::Unavailable(tr!("shell.hotkey.snip_thread_failed").into());
                    return;
                }
                log::info!("Snipping Tool shortcuts routed to Stinkshot");
                self.snip = Some(h);
                self.snip_status = SnipStatus::Active;
            }
            Err(e) => {
                log::warn!("Snipping Tool hook failed: {e:#}");
                self.snip_status = SnipStatus::Unavailable(tr!("shell.hotkey.snip_failed").into());
            }
        }
    }

    pub fn snip_status(&self) -> SnipStatus {
        self.snip_status.clone()
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
        while let Ok(chord) = self.snip_rx.try_recv() {
            if self.suspended || self.snip.is_none() {
                continue;
            }
            if let Some((_, action)) = self.snip_routes.iter().find(|(c, _)| *c == chord) {
                log::debug!("{} routed to {}", chord.display(), action.label());
                out.push(action.command());
            }
        }
        out
    }

    /// Pause all hotkeys (while the settings window is capturing a new combo). The combos
    /// are unregistered from Windows, not just ignored: RegisterHotKey swallows the
    /// keystroke before any window sees it, so the settings window could never record a
    /// combination that is already bound. Resuming registers them again. The Snipping Tool
    /// hook is told to pass its chords through for the same reason, so Win+Shift+S and
    /// Win+Shift+R reach the recorder (and Windows) instead of being swallowed.
    pub fn set_suspended(&mut self, suspended: bool) {
        if self.suspended == suspended {
            return;
        }
        self.suspended = suspended;
        if let Some(h) = &self.snip {
            h.set_pass_through(suspended);
        }
        let Some(m) = &self.manager else { return };
        if suspended {
            for b in &self.registered {
                if let Err(e) = m.unregister(b.hotkey) {
                    log::debug!("suspend: unregister {}: {e}", b.hotkey);
                }
            }
        } else {
            // Another program can grab a key in the moment it was released; such a key
            // joins the retry list like a failure at load.
            let mut lost = Vec::new();
            self.registered.retain(|b| match m.register(b.hotkey) {
                Ok(()) => true,
                Err(e) => {
                    log::warn!("could not re-register hotkey {}: {e}", b.hotkey);
                    let in_use = matches!(e, global_hotkey::Error::AlreadyRegistered(_));
                    lost.push(Pending {
                        binding: b.clone(),
                        in_use,
                    });
                    false
                }
            });
            for p in lost {
                let id = p.binding.hotkey.id();
                self.actions.remove(&id);
                self.errors.push(ErrorEntry {
                    text: p.text(),
                    retry: Some(id),
                });
                self.pending.push(p);
                self.errors_changed = true;
            }
            if !self.pending.is_empty() {
                // Nothing was retried while suspended; try again soon.
                self.retry.start(Instant::now());
            }
        }
    }

    pub fn is_suspended(&self) -> bool {
        self.suspended
    }

    /// Unregister everything and give PrintScreen back to Windows. Call on quit.
    pub fn shutdown(&mut self) {
        self.unregister_all();
        self.set_snip_hook(false);
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
            // Unbound by default (Pick Color): nothing to register.
            if text.is_empty() {
                continue;
            }
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
    fn snip_chord_strings_parse_and_match_recorded_form() {
        for c in SnipChord::ALL {
            assert!(c.hotkey_string().parse::<HotKey>().is_ok(), "{c:?}");
        }
        // What Settings records for Win+Shift+S (ui_hotkey::build_hotkey order).
        assert!(same_hotkey(
            "Shift+Super+KeyS",
            SnipChord::Region.hotkey_string()
        ));
        assert!(same_hotkey(
            "Meta+Shift+KeyR",
            SnipChord::Record.hotkey_string()
        ));
        assert!(!same_hotkey(
            "Shift+KeyS",
            SnipChord::Region.hotkey_string()
        ));
        assert!(!same_hotkey("", SnipChord::Region.hotkey_string()));
        assert_eq!(display(SnipChord::Region.hotkey_string()), "Shift+Win+S");
    }

    #[test]
    fn snip_routes_default_and_follow_bindings() {
        let mut cfg = stinkshot_core::config::Hotkeys::default();
        assert_eq!(
            snip_routes(&cfg),
            [
                (SnipChord::Region, Action::Region),
                (SnipChord::Record, Action::RecordMp4)
            ]
        );
        // A Stinkshot action bound to the chord gets it (and is not registered twice).
        cfg.ocr = "Shift+Super+KeyS".into();
        cfg.record_gif = "Super+Shift+KeyR".into();
        assert_eq!(
            snip_routes(&cfg),
            [
                (SnipChord::Region, Action::Ocr),
                (SnipChord::Record, Action::RecordGif)
            ]
        );
    }

    fn pending(text: &str, action: Action) -> Pending {
        Pending {
            binding: Binding {
                hotkey: text.parse().expect("test hotkey parses"),
                action,
                key: display(text),
            },
            in_use: true,
        }
    }

    /// Fake Windows: ids in `held` are taken by another program; everything this registrar
    /// registered is recorded, and registering an id twice is a test failure.
    struct FakeRegistrar {
        held: std::cell::RefCell<Vec<u32>>,
        mine: std::cell::RefCell<Vec<u32>>,
        fail_other: std::cell::RefCell<Vec<u32>>,
    }

    impl FakeRegistrar {
        fn new(held: &[u32]) -> Self {
            FakeRegistrar {
                held: std::cell::RefCell::new(held.to_vec()),
                mine: std::cell::RefCell::new(Vec::new()),
                fail_other: std::cell::RefCell::new(Vec::new()),
            }
        }
    }

    impl Registrar for FakeRegistrar {
        fn register(&self, hotkey: HotKey) -> Result<(), global_hotkey::Error> {
            let id = hotkey.id();
            assert!(
                !self.mine.borrow().contains(&id),
                "{hotkey} registered twice"
            );
            if self.held.borrow().contains(&id) {
                return Err(global_hotkey::Error::AlreadyRegistered(hotkey));
            }
            if self.fail_other.borrow().contains(&id) {
                return Err(global_hotkey::Error::FailedToRegister("test".into()));
            }
            self.mine.borrow_mut().push(id);
            Ok(())
        }
    }

    #[test]
    fn retry_registers_once_when_the_other_program_lets_go() {
        let a = pending("Control+Shift+F9", Action::Fullscreen);
        let b = pending("Control+Shift+F10", Action::Window);
        let (ida, idb) = (a.binding.hotkey.id(), b.binding.hotkey.id());
        let reg = FakeRegistrar::new(&[ida, idb]);
        let mut list = vec![a.clone(), b.clone()];

        // Still held: nothing changes.
        let (done, changed) = retry_pending(&reg, &mut list);
        assert!(done.is_empty() && !changed);
        assert_eq!(list.len(), 2);

        // The other program quits and frees one key.
        reg.held.borrow_mut().retain(|id| *id != ida);
        let (done, changed) = retry_pending(&reg, &mut list);
        assert_eq!(done, vec![a.binding.clone()]);
        assert!(!changed);
        assert_eq!(list, vec![b.clone()]);

        // The freed key is not tried again (the fake panics on a double registration).
        let (done, _) = retry_pending(&reg, &mut list);
        assert!(done.is_empty());

        // The other one now fails for a different reason: its kind changes.
        reg.held.borrow_mut().clear();
        reg.fail_other.borrow_mut().push(idb);
        let (done, changed) = retry_pending(&reg, &mut list);
        assert!(done.is_empty() && changed);
        assert!(!list[0].in_use);
        assert!(ErrorKind::of(&list[0].text()) == Some(ErrorKind::NotRegistered));

        reg.fail_other.borrow_mut().clear();
        let (done, _) = retry_pending(&reg, &mut list);
        assert_eq!(done, vec![b.binding]);
        assert!(list.is_empty());
        assert_eq!(*reg.mine.borrow(), vec![ida, idb]);
    }

    #[test]
    fn retry_clock_paces_kicks_and_stops() {
        let t0 = Instant::now();
        let mut c = RetryClock::idle();
        assert!(!c.due(t0) && c.wait(t0).is_none());
        // Idle: a kick (Settings opened, unlock) does not start anything.
        c.kick(t0);
        assert_eq!(c, RetryClock::idle());

        c.start(t0);
        assert!(!c.due(t0));
        assert_eq!(c.wait(t0), Some(RetryClock::FAST));
        let mut now = t0 + RetryClock::FAST;
        assert!(c.due(now));

        // Fast while young, then slow.
        for _ in 0..RetryClock::FAST_TRIES {
            assert!(c.due(now));
            c.attempted(now);
            now += c.wait(now).expect("scheduled");
        }
        assert_eq!(c.interval(), RetryClock::SLOW);
        c.attempted(now);
        assert_eq!(c.wait(now), Some(RetryClock::SLOW));

        // A kick makes it due at once and fast again.
        c.kick(now);
        assert!(c.due(now));
        assert_eq!(c.interval(), RetryClock::FAST);

        // Nothing left to retry: idle, no wake-ups.
        c.stop();
        assert!(!c.due(now + RetryClock::SLOW * 10));
        assert!(c.wait(now).is_none());
    }

    /// Real Windows conflict: a second hotkey window in this process holds an obscure
    /// combination (no other program uses it), so registering it fails as "in use"; once the
    /// holder lets go, a retry registers it. Needs a desktop session.
    #[test]
    #[ignore]
    fn real_conflict_clears_on_retry() {
        let text = "Control+Alt+Shift+F24";
        let hk: HotKey = text.parse().expect("parses");
        let holder = GlobalHotKeyManager::new().expect("holder manager");
        let ours = GlobalHotKeyManager::new().expect("our manager");
        holder.register(hk).expect("holder registers");

        let mut list = vec![pending(text, Action::Fullscreen)];
        let (done, changed) = retry_pending(&ours, &mut list);
        assert!(done.is_empty() && !changed, "should still be in use");
        assert!(list[0].in_use);

        holder.unregister(hk).expect("holder lets go");
        let (done, _) = retry_pending(&ours, &mut list);
        assert_eq!(done.len(), 1);
        assert!(list.is_empty());
        // Ours now: the holder cannot take it back.
        assert!(matches!(
            holder.register(hk),
            Err(global_hotkey::Error::AlreadyRegistered(_))
        ));
        ours.unregister(hk).expect("cleanup");
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
