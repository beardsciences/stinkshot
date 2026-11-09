//! Settings window (docs/design/Settings.dc.html). Sections: General, Hotkeys, Capture,
//! Editor, Recording, History, Privacy, About. Edits `shared.config` in place and sets
//! `shared.config_dirty = true`; the shell persists and applies. Hotkey fields capture the
//! next key combo when clicked and show conflicts reported by the shell. The Privacy page
//! states the local-only promise (copy from the design) and holds the update-check toggle.
//!
//! Dirty tracking: the config is compared with a copy taken at the start of the frame. A
//! change made while the pointer is held (slider drags, colour pickers) is flushed when the
//! pointer is released, so the shell saves once per gesture instead of once per frame. Text
//! fields commit on Enter or focus loss.

#[path = "ui_hotkey.rs"]
pub mod ui_hotkey;

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use egui::{
    Align2, Color32, CornerRadius, RichText, ScrollArea, Sense, Stroke, StrokeKind, Ui, UiBuilder,
    Vec2, pos2, vec2,
};
use stinkshot_core::config::{Config, Hotkeys};
use stinkshot_core::image::ImageFormat;
use stinkshot_history::Stats;
use stinkshot_platform::Hwnd;
use stinkshot_platform::snip::SnipChord;

use self::ui_hotkey::{Captured, display, same_combo};
use crate::hotkeys::SnipStatus;
use crate::l10n::{self, tr};
use crate::settings_search as search;
use crate::state::{Command, Shared, Surface, SurfaceStatus};
use crate::theme::{self, Icon, widgets};
use crate::toast::ui_common as uc;

const NAV_W: f32 = 220.0;
const CONTENT_MAX_W: f32 = 760.0;
const REPO_URL: &str = "https://code.mccullough.dev/christian/stinkshot";
const DEFAULT_ACCENTS: [&str; 4] = ["#7fd36a", "#4cc2ff", "#ffb454", "#c49cff"];

/// A page of the Settings window (`Command::OpenSettingsAt` opens one directly).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    General,
    Hotkeys,
    Capture,
    Editor,
    Recording,
    History,
    Privacy,
    Help,
    About,
}

impl Section {
    /// Page by its English name, any case ("help", "About"), for `stinkshot settings --page`.
    pub fn from_name(name: &str) -> Option<Section> {
        SECTIONS
            .iter()
            .find(|(_, k)| {
                k.strip_prefix("settings.nav.")
                    .is_some_and(|n| n.eq_ignore_ascii_case(name.trim()))
            })
            .map(|(s, _)| *s)
    }
}

/// Pages and their name keys (looked up when drawn; the key doubles as a stable id).
const SECTIONS: [(Section, &str); 9] = [
    (Section::General, "settings.nav.general"),
    (Section::Hotkeys, "settings.nav.hotkeys"),
    (Section::Capture, "settings.nav.capture"),
    (Section::Editor, "settings.nav.editor"),
    (Section::Recording, "settings.nav.recording"),
    (Section::History, "settings.nav.history"),
    (Section::Privacy, "settings.nav.privacy"),
    (Section::Help, "settings.nav.help"),
    (Section::About, "settings.nav.about"),
];

/// Synonym keys searched along with each page's name (Help is not searched: it has no
/// settings).
const SEARCH_KEYWORDS: [(Section, &str); 8] = [
    (Section::General, "settings.search.kw.general"),
    (Section::Hotkeys, "settings.search.kw.hotkeys"),
    (Section::Capture, "settings.search.kw.capture"),
    (Section::Editor, "settings.search.kw.editor"),
    (Section::Recording, "settings.search.kw.recording"),
    (Section::History, "settings.search.kw.history"),
    (Section::Privacy, "settings.search.kw.privacy"),
    (Section::About, "settings.search.kw.about"),
];

/// Displayed name of a page.
fn section_name(section: Section) -> &'static str {
    SECTIONS
        .iter()
        .find(|(s, _)| *s == section)
        .map_or("", |(_, n)| l10n::lookup(n))
}

/// `(config value, label key)` for the Theme setting.
const THEMES: [(&str, &str); 3] = [
    ("system", "settings.theme.system"),
    ("dark", "settings.theme.dark"),
    ("light", "settings.theme.light"),
];

/// Hotkey rows are the shared capture actions (one naming source, `actions.rs`).
use crate::actions::Action as HotkeyField;

const HOTKEY_FIELDS: [HotkeyField; HotkeyField::ALL.len()] = HotkeyField::ALL;

// ---------------------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------------------

/// Fields (other than `me`) bound to the same combination as `me`.
fn conflicts_with(h: &Hotkeys, me: HotkeyField) -> Vec<HotkeyField> {
    let mine = me.hotkey(h);
    if mine.trim().is_empty() {
        return Vec::new();
    }
    HOTKEY_FIELDS
        .iter()
        .copied()
        .filter(|f| *f != me && same_combo(f.hotkey(h), mine))
        .collect()
}

/// Registration error the shell reported for this hotkey string, if any. Matches either the
/// raw string or its friendly form as the whole first word of the message ("Ctrl+Q (Window)
/// ..."), so "Alt+P" does not match an error about "Alt+PrtSc".
fn error_for<'a>(errors: &'a [String], hotkey: &str) -> Option<&'a str> {
    let t = hotkey.trim();
    if t.is_empty() {
        return None;
    }
    let friendly = display(t).to_lowercase();
    let raw = t.to_lowercase();
    let leads_with = |el: &str, key: &str| {
        el.strip_prefix(key)
            .is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '+'))
    };
    errors.iter().map(String::as_str).find(|e| {
        let el = e.to_lowercase();
        leads_with(&el, &raw) || leads_with(&el, &friendly)
    })
}

/// When some problem is a key Windows refused (so Check Again can help), the sentence to show
/// after a check that left it: "Still in use." when another program holds one, else
/// "No change.". None when nothing is retryable.
fn retry_status(errors: &[String]) -> Option<&'static str> {
    use crate::hotkeys::ErrorKind;
    let kinds: Vec<ErrorKind> = errors.iter().filter_map(|e| ErrorKind::of(e)).collect();
    if kinds.contains(&ErrorKind::InUse) {
        Some(tr!("settings.hotkeys.still_in_use"))
    } else if kinds.contains(&ErrorKind::NotRegistered) {
        Some(tr!("settings.hotkeys.no_change"))
    } else {
        None
    }
}

/// Color of a status sentence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tone {
    Good,
    Bad,
    Neutral,
}

/// Whether some action is bound to a plain PrintScreen-based hotkey.
fn printscreen_bound(h: &Hotkeys) -> bool {
    HOTKEY_FIELDS
        .iter()
        .any(|f| f.hotkey(h).to_ascii_lowercase().contains("printscreen"))
}

/// The status sentence under Replace Snipping Tool. `routes` are the actions Win+Shift+S
/// and Win+Shift+R run.
fn snip_status_line(status: &SnipStatus, h: &Hotkeys, routes: [HotkeyField; 2]) -> (String, Tone) {
    let replace = h.replace_snipping_tool;
    match (replace, status) {
        (false, SnipStatus::Active) => (
            tr!("settings.hotkeys.snip_turning_off").into(),
            Tone::Neutral,
        ),
        (false, _) => (String::new(), Tone::Neutral),
        (true, SnipStatus::Off) => (
            tr!("settings.hotkeys.snip_turning_on").into(),
            Tone::Neutral,
        ),
        (true, SnipStatus::Unavailable(why)) => (why.clone(), Tone::Bad),
        (true, SnipStatus::Active) => {
            let _ = routes;
            (tr!("settings.hotkeys.snip_active").into(), Tone::Good)
        }
    }
}

/// A second, neutral line when Replace Snipping Tool is on but no shortcut uses PrtSc, so
/// the key cannot be taken over.
fn snip_prtsc_note(h: &Hotkeys) -> Option<&'static str> {
    (h.replace_snipping_tool && !printscreen_bound(h))
        .then_some(tr!("settings.hotkeys.prtsc_unbound"))
}

/// Title and description of a Snipping Tool chord row in the shortcut list.
fn snip_chord_row(chord: SnipChord, action: HotkeyField, replace: bool) -> (String, String) {
    let title = match chord {
        SnipChord::Region => tr!("settings.hotkeys.chord_snip"),
        SnipChord::Record => tr!("settings.hotkeys.chord_record"),
    };
    let desc = if replace {
        action.label().to_string()
    } else {
        tr!("settings.hotkeys.snipping_tool").to_string()
    };
    (title.to_string(), desc)
}

/// Options for a device dropdown: "" (default device) first, then the detected names, then the
/// configured one if it is no longer present.
fn device_options(detected: &[String], current: &str) -> Vec<(String, String)> {
    let mut v = vec![(
        String::new(),
        tr!("settings.recording.default_device").to_string(),
    )];
    v.extend(detected.iter().map(|d| (d.clone(), d.clone())));
    if !current.is_empty() && !detected.iter().any(|d| d == current) {
        v.push((
            current.to_string(),
            tr!("settings.recording.device_missing", name = current),
        ));
    }
    v
}

/// "30 FPS"-style options for a frame-rate dropdown.
fn fps_labels(values: &[u32]) -> Vec<(u32, String)> {
    values
        .iter()
        .map(|&n| (n, tr!("settings.fps", n = n)))
        .collect()
}

/// Borrowed view of owned dropdown options, for `combo_row`.
fn as_refs<T: Copy>(opts: &[(T, String)]) -> Vec<(T, &str)> {
    opts.iter().map(|(v, l)| (*v, l.as_str())).collect()
}

fn days_label(days: u32) -> String {
    match days {
        0 => tr!("settings.history.forever").into(),
        1 => tr!("settings.history.one_day").into(),
        n => tr!("settings.history.days", n = n),
    }
}

fn gb_label(gb: f32) -> String {
    if gb <= 0.0 {
        tr!("settings.history.no_cap").into()
    } else {
        tr!("settings.history.gb", gb = format!("{gb:.1}"))
    }
}

/// "Last checked 2 hours ago" style text from a UTC timestamp.
fn last_check_text(
    last: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    let Some(t) = last else {
        return tr!("settings.privacy.never_checked").into();
    };
    let d = now.signed_duration_since(t);
    let s = d.num_seconds().max(0);
    let ago = if s < 60 {
        tr!("settings.privacy.just_now").to_string()
    } else if s < 3600 {
        tr!("settings.privacy.min_ago", n = s / 60)
    } else if s < 86_400 {
        tr!("settings.privacy.h_ago", n = s / 3600)
    } else {
        tr!("settings.privacy.days_ago", n = s / 86_400)
    };
    tr!("settings.privacy.last_checked", ago = ago)
}

// ---------------------------------------------------------------------------------------
// Window
// ---------------------------------------------------------------------------------------

struct Devices {
    mics: Vec<String>,
    cams: Vec<String>,
    loaded: bool,
}

pub struct SettingsWindow {
    section: Section,
    hotkey_errors: Vec<String>,
    snip_status: SnipStatus,
    capturing: Option<HotkeyField>,
    print_prev: bool,
    pending_dirty: bool,
    devices: Devices,
    device_rx: Option<Receiver<(Vec<String>, Vec<String>)>>,
    folder_rx: Option<Receiver<Option<PathBuf>>>,
    stats: Option<Stats>,
    stats_at: Option<Instant>,
    hwnd: Option<Hwnd>,
    update_requested_at: Option<Instant>,
    copied_url_at: Option<Instant>,
    /// When Check Again was clicked, and when the shell answered.
    hotkey_check_requested: bool,
    hotkey_checked_at: Option<Instant>,
    /// Settings search text; results replace the page while it is not blank.
    search: String,
    /// Best row score per page from the last results frame, to put the best page first.
    search_scores: Vec<(Section, u32)>,
}

impl SettingsWindow {
    pub fn new(shared: &Shared) -> Self {
        let _ = shared;
        Self {
            section: Section::General,
            hotkey_errors: Vec::new(),
            snip_status: SnipStatus::Off,
            capturing: None,
            print_prev: false,
            pending_dirty: false,
            devices: Devices {
                mics: Vec::new(),
                cams: Vec::new(),
                loaded: false,
            },
            device_rx: None,
            folder_rx: None,
            stats: None,
            stats_at: None,
            hwnd: None,
            update_requested_at: None,
            copied_url_at: None,
            hotkey_check_requested: false,
            hotkey_checked_at: None,
            search: String::new(),
            search_scores: Vec::new(),
        }
    }

    /// Switch to `section` (another window asked for a specific page).
    pub fn show_section(&mut self, section: Section) {
        self.search.clear();
        if section != self.section {
            self.section = section;
            self.stats_at = None;
        }
    }

    /// Shell reports hotkey registration failures (e.g. "Ctrl+Shift+KeyS: already in use").
    pub fn set_hotkey_errors(&mut self, errors: Vec<String>) {
        self.hotkey_errors = errors;
    }

    /// Shell finished the retry Check Again asked for (`hotkey_errors` is current).
    pub fn hotkeys_checked(&mut self) {
        if std::mem::take(&mut self.hotkey_check_requested) {
            self.hotkey_checked_at = Some(Instant::now());
        }
    }

    /// Shell reports whether the Replace Snipping Tool hook is running.
    pub fn set_snip_status(&mut self, status: SnipStatus) {
        self.snip_status = status;
    }

    /// True while a hotkey field is waiting for a key press. The shell should suspend global
    /// hotkeys (`Hotkeys::set_suspended`) while this is true, otherwise pressing the combo
    /// being recorded triggers the capture it is currently bound to.
    pub fn is_capturing_hotkey(&self) -> bool {
        self.capturing.is_some()
    }

    fn scan_devices(&mut self, ctx: &egui::Context) {
        if self.device_rx.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let c = ctx.clone();
        if uc::spawn_worker("device-scan", move || {
            let r = (
                stinkshot_record::list_microphones(),
                stinkshot_record::list_webcams(),
            );
            let _ = tx.send(r);
            c.request_repaint();
        }) {
            self.device_rx = Some(rx);
        } else {
            self.devices.loaded = true;
        }
    }

    fn pump(&mut self, ctx: &egui::Context, shared: &mut Shared) {
        if let Some(rx) = &self.device_rx {
            match rx.try_recv() {
                Ok((m, c)) => {
                    self.devices = Devices {
                        mics: m,
                        cams: c,
                        loaded: true,
                    };
                    self.device_rx = None;
                }
                Err(TryRecvError::Disconnected) => {
                    self.devices.loaded = true;
                    self.device_rx = None;
                }
                Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
            }
        }
        if let Some(rx) = &self.folder_rx {
            match rx.try_recv() {
                Ok(picked) => {
                    self.folder_rx = None;
                    if let Some(p) = picked {
                        shared.config.history.folder = p.to_string_lossy().to_string();
                    }
                }
                Err(TryRecvError::Disconnected) => self.folder_rx = None,
                Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(100)),
            }
        }
        // Library stats for the History/Privacy pages (cheap SQL), refreshed every few seconds.
        if matches!(self.section, Section::History | Section::Privacy)
            && self
                .stats_at
                .is_none_or(|t| t.elapsed() > Duration::from_secs(5))
        {
            self.stats = shared.history.stats().ok();
            self.stats_at = Some(Instant::now());
        }
    }

    fn start_folder_pick(&mut self, ctx: &egui::Context, current: PathBuf) {
        if self.folder_rx.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let owner = self.hwnd;
        let c = ctx.clone();
        if uc::spawn_worker("folder-picker", move || {
            let _ = tx.send(uc::pick_folder(owner, Some(&current)));
            c.request_repaint();
        }) {
            self.folder_rx = Some(rx);
        }
    }

    fn apply_capture(&mut self, shared: &mut Shared, ctx: &egui::Context) -> bool {
        let Some(field) = self.capturing else {
            return false;
        };
        let down = uc::async_key_down(uc::VK_SNAPSHOT);
        let polled = if down && !self.print_prev {
            Some("PrintScreen")
        } else {
            None
        };
        self.print_prev = down;
        let win = uc::async_key_down(uc::VK_LWIN) || uc::async_key_down(uc::VK_RWIN);
        let (events, mods) = ctx.input(|i| (i.events.clone(), i.modifiers));
        match ui_hotkey::scan(&events, mods, win, polled) {
            Captured::Nothing => false,
            Captured::Cancel => {
                self.capturing = None;
                true
            }
            Captured::Clear => {
                field.hotkey_mut(&mut shared.config.hotkeys).clear();
                self.capturing = None;
                true
            }
            Captured::Combo(s) => {
                *field.hotkey_mut(&mut shared.config.hotkeys) = s;
                self.capturing = None;
                true
            }
        }
    }
}

impl Surface for SettingsWindow {
    fn title(&self) -> String {
        // Not translated: the shell derives the viewport id from the title.
        "Stinkshot Settings".into() // l10n-ok
    }

    fn viewport_builder(&self, _shared: &Shared) -> egui::ViewportBuilder {
        egui::ViewportBuilder::default()
            .with_title(self.title())
            .with_inner_size(vec2(1040.0, 760.0))
            .with_min_inner_size(vec2(760.0, 520.0))
            .with_resizable(true)
    }

    fn on_created(&mut self, hwnd: Hwnd, shared: &Shared) {
        self.hwnd = Some(hwnd);
        stinkshot_platform::window_util::apply_mica(hwnd, shared.dark);
        stinkshot_platform::window_util::force_foreground(hwnd);
    }

    fn show(&mut self, ctx: &egui::Context, shared: &mut Shared) -> SurfaceStatus {
        // Load (or render) the About page's logo loop in the background so it moves as
        // soon as the page is opened.
        crate::logo_anim::prewarm(ctx);
        let before = shared.config.clone();
        self.pump(ctx, shared);
        let capturing_before = self.capturing.is_some();
        let key_consumed = self.apply_capture(shared, ctx);
        // Esc closes the window unless it belongs to a hotkey field, a text field or a popup.
        // With search text it clears the search instead.
        let esc_free = !capturing_before
            && !key_consumed
            && self.capturing.is_none()
            && !ctx.egui_wants_keyboard_input()
            && !egui::Popup::is_any_open(ctx)
            && ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let esc_close = esc_free && self.search.is_empty();
        if esc_free {
            self.search.clear();
        }
        if self.capturing.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }

        // Ctrl+H opens History, unless a hotkey field is recording (then it is the combo).
        let mut open_history = !capturing_before
            && !key_consumed
            && self.capturing.is_none()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::H));
        // Ctrl+F focuses the search field (same rule).
        let focus_search = !capturing_before
            && !key_consumed
            && self.capturing.is_none()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F));
        let search_before = self.search.clone();

        let p = uc::pal(shared);
        let accent = uc::accent(shared);
        let mut section = self.section;

        uc::root_ui(ctx, p.bg, |ui, rect| {
            let nav = egui::Rect::from_min_size(
                rect.min + vec2(0.0, 8.0),
                vec2(NAV_W, rect.height() - 8.0),
            );
            let content = egui::Rect::from_min_max(
                pos2(rect.left() + NAV_W, rect.top() + 8.0),
                rect.right_bottom(),
            );

            // Content layer (rounded top-left like the History window).
            ui.painter().rect_filled(
                content,
                CornerRadius {
                    nw: 8,
                    ne: 0,
                    sw: 0,
                    se: 0,
                },
                p.layer,
            );
            uc::layer_edges(ui, content, 8.0, p.stroke);

            // Search, above the pages like Windows Settings.
            let search_rect = egui::Rect::from_min_size(
                pos2(nav.left() + 12.0, nav.top() + 8.0),
                vec2(NAV_W - 24.0, 34.0),
            );
            self.search_box(ui, search_rect, shared, accent, focus_search);
            let searching = !search::Query::parse(&self.search).is_empty();

            // Navigation.
            let mut y = search_rect.bottom() + 10.0;
            for (sec, name) in SECTIONS {
                let r =
                    egui::Rect::from_min_size(pos2(nav.left() + 12.0, y), vec2(NAV_W - 24.0, 36.0));
                y += 38.0;
                let resp = ui.interact(r, ui.scope_id().with(("nav", name)), Sense::click());
                let selected = sec == section && !searching;
                crate::a11y::describe(
                    &resp,
                    egui::accesskit::Role::Tab,
                    l10n::lookup(name),
                    Some(selected),
                );
                if selected {
                    ui.painter().rect_filled(
                        r,
                        CornerRadius::same(6),
                        Color32::from_white_alpha(if shared.dark { 18 } else { 0 }),
                    );
                    if !shared.dark {
                        ui.painter().rect_filled(
                            r,
                            CornerRadius::same(6),
                            Color32::from_black_alpha(14),
                        );
                    }
                    ui.painter().rect_filled(
                        egui::Rect::from_min_size(
                            pos2(r.left() + 10.0, r.center().y - 8.0),
                            vec2(3.0, 16.0),
                        ),
                        CornerRadius::same(2),
                        accent,
                    );
                } else if resp.hovered() {
                    ui.painter().rect_filled(
                        r,
                        CornerRadius::same(6),
                        Color32::from_white_alpha(if shared.dark { 10 } else { 0 }),
                    );
                    if !shared.dark {
                        ui.painter().rect_filled(
                            r,
                            CornerRadius::same(6),
                            Color32::from_black_alpha(8),
                        );
                    }
                }
                ui.painter().text(
                    pos2(r.left() + 26.0, r.center().y),
                    Align2::LEFT_CENTER,
                    l10n::lookup(name),
                    uc::font(13.0),
                    p.text,
                );
                if resp.clicked() {
                    section = sec;
                    self.search.clear();
                }
            }

            // Link to History at the very bottom (same place as History's link to Settings).
            if uc::sidebar_link(
                ui,
                nav,
                theme::Icon::History,
                tr!("settings.open_history"),
                "Ctrl+H", // l10n-ok
                tr!("settings.open_history_tip"),
                shared.dark,
            )
            .clicked()
            {
                open_history = true;
            }

            // Page, or search results while the search has words.
            let query = search::Query::parse(&self.search);
            let area = content.shrink2(vec2(28.0, 18.0));
            let w = area.width().min(CONTENT_MAX_W);
            let page_rect = egui::Rect::from_min_size(area.min, vec2(w, area.height()));
            ui.scope_builder(UiBuilder::new().max_rect(page_rect), |ui| {
                let salt = if query.is_empty() {
                    section as u8
                } else {
                    u8::MAX
                };
                ScrollArea::vertical()
                    .id_salt(("settings_page", salt))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(w - 12.0);
                        ui.spacing_mut().item_spacing.y = 6.0;
                        ui.spacing_mut().slider_width = 180.0;
                        if !query.is_empty() {
                            self.search_results(ui, shared, &query, key_consumed);
                            return;
                        }
                        ui.label(
                            RichText::new(
                                SECTIONS
                                    .iter()
                                    .find(|(s, _)| *s == section)
                                    .map_or("", |(_, n)| l10n::lookup(n)),
                            )
                            .font(theme::semibold(28.0))
                            .color(p.text),
                        );
                        ui.add_space(6.0);
                        self.page(section, ui, shared, key_consumed);
                        ui.add_space(16.0);
                    });
            });
        });

        if section != self.section {
            self.section = section;
            self.stats_at = None;
            if section == Section::Recording && !self.devices.loaded {
                self.scan_devices(ctx);
            }
        }
        let searching = !search::Query::parse(&self.search).is_empty();
        // Landing on Recording through other paths (first frame) also needs the scan, and so
        // do search results, which can show its device rows.
        if (self.section == Section::Recording || searching)
            && !self.devices.loaded
            && self.device_rx.is_none()
        {
            self.scan_devices(ctx);
        }
        // A hotkey field records only while it is on screen: on its page, or in results
        // that have not changed since it was clicked.
        if (self.section != Section::Hotkeys && !searching) || self.search != search_before {
            self.capturing = None;
        }

        if open_history {
            shared.outbox.push(Command::OpenHistory);
        }

        // Dirty flag: once per gesture.
        if shared.config != before {
            self.pending_dirty = true;
        }
        let held = ctx.input(|i| i.pointer.any_down());
        let closing = esc_close || ctx.input(|i| i.viewport().close_requested());
        if self.pending_dirty && (!held || closing) {
            shared.config_dirty = true;
            self.pending_dirty = false;
        }
        if closing {
            SurfaceStatus::Close
        } else {
            SurfaceStatus::Open
        }
    }
}

// ---------------------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------------------

impl SettingsWindow {
    fn page(&mut self, section: Section, ui: &mut Ui, shared: &mut Shared, key_consumed: bool) {
        let accent = uc::accent(shared);
        match section {
            Section::General => self.page_general(ui, shared, accent),
            Section::Hotkeys => self.page_hotkeys(ui, shared, key_consumed),
            Section::Capture => self.page_capture(ui, shared, accent),
            Section::Editor => self.page_editor(ui, shared),
            Section::Recording => self.page_recording(ui, shared),
            Section::History => self.page_history(ui, shared),
            Section::Privacy => self.page_privacy(ui, shared, accent),
            Section::Help => crate::help::page(ui, shared),
            Section::About => self.page_about(ui, shared),
        }
    }

    /// The search field at the top of the navigation. Esc in it clears the text.
    fn search_box(
        &mut self,
        ui: &mut Ui,
        rect: egui::Rect,
        shared: &Shared,
        accent: Color32,
        focus: bool,
    ) {
        let p = uc::pal(shared);
        let id = ui.scope_id().with("settings_search");
        let focused = ui.memory(|m| m.has_focus(id));
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), p.layer);
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(6),
            Stroke::new(1.0, p.stroke),
            StrokeKind::Inside,
        );
        ui.painter().rect_filled(
            egui::Rect::from_min_max(
                pos2(rect.left() + 3.0, rect.bottom() - 2.0),
                pos2(rect.right() - 3.0, rect.bottom()),
            ),
            CornerRadius::same(1),
            if focused || !self.search.is_empty() {
                accent
            } else {
                p.stroke
            },
        );
        widgets::paint_icon(
            ui.painter(),
            egui::Rect::from_center_size(
                pos2(rect.left() + 17.0, rect.center().y),
                vec2(15.0, 15.0),
            ),
            Icon::Search,
            p.text_secondary,
        );
        let te_rect = egui::Rect::from_min_max(
            pos2(rect.left() + 32.0, rect.top()),
            pos2(rect.right() - 30.0, rect.bottom()),
        );
        let te = ui.scope_builder(UiBuilder::new().max_rect(te_rect), |ui| {
            ui.add_sized(
                te_rect.size(),
                egui::TextEdit::singleline(&mut self.search)
                    .id(id)
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::ZERO)
                    .align(Align2::LEFT_CENTER)
                    .hint_text(tr!("settings.search.hint"))
                    .font(uc::font(13.0))
                    .min_size(te_rect.size())
                    .desired_width(te_rect.width()),
            )
        });
        crate::a11y::name_from_placeholder(&te.inner);
        if focus {
            te.inner.request_focus();
        }
        if te.inner.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.search.clear();
        }
        if !self.search.is_empty() {
            let cr = egui::Rect::from_center_size(
                pos2(rect.right() - 16.0, rect.center().y),
                vec2(22.0, 22.0),
            );
            let cresp = ui.interact(cr, id.with("clear"), Sense::click());
            crate::a11y::role(&cresp, egui::accesskit::Role::Button);
            if cresp.hovered() {
                ui.painter().rect_filled(
                    cr,
                    CornerRadius::same(5),
                    theme::subtle_fill(shared.dark, 1),
                );
            }
            widgets::paint_icon(
                ui.painter(),
                egui::Rect::from_center_size(cr.center(), vec2(11.0, 11.0)),
                Icon::Close,
                p.text_secondary,
            );
            if cresp.on_hover_text(tr!("settings.search.clear")).clicked() {
                self.search.clear();
                te.inner.request_focus();
            }
        }
    }

    /// Every page's matching rows under its name, best page first, or the empty state.
    fn search_results(
        &mut self,
        ui: &mut Ui,
        shared: &mut Shared,
        query: &search::Query,
        key_consumed: bool,
    ) {
        let p = uc::pal(shared);
        let accent = uc::accent(shared);
        let mark = (p.on_accent, accent);
        let pages: Vec<Section> = SEARCH_KEYWORDS.iter().map(|(s, _)| *s).collect();
        let mut scores = Vec::new();
        let head_h = 30.0;
        for section in search::order(&pages, &self.search_scores) {
            let name = section_name(section);
            let keywords = SEARCH_KEYWORDS
                .iter()
                .find(|(s, _)| *s == section)
                .map_or("", |(_, k)| l10n::lookup(k));
            // The page draws into a child below room for its heading. Only rows draw while
            // the filter is active, so a page with no match leaves nothing behind.
            let top = ui.cursor().min;
            let width = ui.available_width();
            let body =
                egui::Rect::from_min_size(top + vec2(0.0, head_h), vec2(width, f32::INFINITY));
            let mut child = ui.new_child(
                UiBuilder::new()
                    .max_rect(body)
                    .layout(egui::Layout::top_down(egui::Align::Min))
                    .id_salt(("search_page", section as u8)),
            );
            let filter = search::begin(query, name, keywords);
            self.page(section, &mut child, shared, key_consumed);
            let (rows, best) = filter.finish();
            if rows == 0 {
                continue;
            }
            scores.push((section, best));
            let head = egui::Rect::from_min_size(top, vec2(width, head_h));
            ui.scope_builder(UiBuilder::new().max_rect(head), |ui| {
                ui.add(egui::Label::new(search::highlighted(
                    name,
                    &search::highlights(query, name),
                    theme::semibold(18.0),
                    p.text,
                    mark,
                )));
            });
            let used =
                egui::Rect::from_min_max(top, pos2(top.x + width, child.min_rect().bottom()));
            ui.advance_cursor_after_rect(used);
            ui.add_space(10.0);
        }
        if scores.is_empty() {
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 200.0), Sense::hover());
            let c = pos2(r.center().x, r.top() + 70.0);
            ui.painter()
                .circle_filled(c, 30.0, theme::subtle_fill(shared.dark, 1));
            widgets::paint_icon(
                ui.painter(),
                egui::Rect::from_center_size(c, vec2(30.0, 30.0)),
                Icon::Search,
                p.text_secondary,
            );
            ui.painter().text(
                pos2(c.x, c.y + 48.0),
                Align2::CENTER_TOP,
                tr!("settings.search.none"),
                theme::semibold(18.0),
                p.text,
            );
        }
        // A new query can reorder the pages: draw again with the fresh order.
        if search::order(&pages, &scores) != search::order(&pages, &self.search_scores) {
            ui.ctx().request_repaint();
        }
        self.search_scores = scores;
    }
}

// ---------------------------------------------------------------------------------------
// Row helpers
// ---------------------------------------------------------------------------------------

fn toggle_row(ui: &mut Ui, title: &str, desc: &str, v: &mut bool) {
    widgets::setting_row(ui, title, desc, |ui| {
        widgets::toggle(ui, v);
    });
}

fn combo_row<T: Copy + PartialEq>(
    ui: &mut Ui,
    id: &str,
    title: &str,
    desc: &str,
    v: &mut T,
    opts: &[(T, &str)],
) {
    widgets::setting_row(ui, title, desc, |ui| {
        let cur = opts
            .iter()
            .find(|(o, _)| *o == *v)
            .map(|(_, l)| *l)
            .unwrap_or(tr!("settings.custom"));
        egui::ComboBox::from_id_salt(id)
            .icon(widgets::combo_chevron)
            .selected_text(cur)
            .width(150.0)
            .show_ui(ui, |ui| {
                crate::theme::popup_rows(ui);
                for (o, l) in opts {
                    ui.selectable_value(v, *o, *l);
                }
            });
    });
}

fn slider_row<N: egui::emath::Numeric>(
    ui: &mut Ui,
    title: &str,
    desc: &str,
    v: &mut N,
    range: std::ops::RangeInclusive<N>,
    suffix: &str,
) {
    widgets::setting_row(ui, title, desc, |ui| {
        // egui sizes the round thumb from the row height; 24 pt gives a ~19 pt Fluent-sized
        // thumb instead of a 26 pt disc.
        ui.spacing_mut().interact_size.y = 24.0;
        // Value box the same height so the two share a centre line.
        ui.spacing_mut().button_padding.y = 3.0;
        ui.add(egui::Slider::new(v, range).suffix(suffix.to_string()));
    });
}

fn group_title(ui: &mut Ui, p: &theme::Palette, text: &str) {
    if search::searching() {
        return;
    }
    ui.add_space(14.0);
    ui.label(
        RichText::new(text)
            .font(theme::semibold(14.0))
            .color(p.text),
    );
    ui.add_space(2.0);
}

/// Single-line text field that edits a local buffer and returns the final text on Enter or
/// focus loss. The buffer re-syncs from `current` whenever the field is not focused.
fn commit_text(ui: &mut Ui, id: &str, current: &str, width: f32, hint: &str) -> Option<String> {
    let te_id = ui.scope_id().with(("commit_text", id));
    let focused = ui.memory(|m| m.has_focus(te_id));
    let mut buf: String = if focused {
        ui.data(|d| d.get_temp::<String>(te_id))
            .unwrap_or_else(|| current.to_string())
    } else {
        current.to_string()
    };
    let resp = ui.add(
        egui::TextEdit::singleline(&mut buf)
            .id(te_id)
            .desired_width(width)
            .hint_text(hint.to_string()),
    );
    ui.data_mut(|d| d.insert_temp(te_id, buf.clone()));
    if resp.lost_focus() { Some(buf) } else { None }
}

/// Row of colour swatches; returns the clicked colour.
fn swatch_row(ui: &mut Ui, colors: &[String], selected: &str, ring: Color32) -> Option<String> {
    let mut picked = None;
    for hex in colors {
        let Some(c) = uc::hex_to_color(hex) else {
            continue;
        };
        let (r, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
        let is_sel = hex.eq_ignore_ascii_case(selected);
        crate::a11y::describe(&resp, egui::accesskit::Role::RadioButton, hex, Some(is_sel));
        ui.painter()
            .rect_filled(r.shrink(3.0), CornerRadius::same(13), c);
        if is_sel {
            ui.painter().rect_stroke(
                r.shrink(0.5),
                CornerRadius::same(13),
                Stroke::new(2.0, ring),
                StrokeKind::Inside,
            );
        } else if resp.hovered() {
            ui.painter().rect_stroke(
                r.shrink(1.5),
                CornerRadius::same(13),
                Stroke::new(1.0, Color32::from_white_alpha(120)),
                StrokeKind::Inside,
            );
        }
        if resp
            .on_hover_text(hex.as_str())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            picked = Some(hex.clone());
        }
    }
    picked
}

fn color_button(ui: &mut Ui, hex: &str) -> Option<String> {
    let mut c = uc::hex_to_color(hex).unwrap_or(Color32::WHITE);
    let resp =
        egui::color_picker::color_edit_button_srgba(ui, &mut c, egui::color_picker::Alpha::Opaque);
    if resp.changed() {
        Some(uc::color_to_hex(c))
    } else {
        None
    }
}

fn info_card(
    ui: &mut Ui,
    p: &theme::Palette,
    accent: Color32,
    icon: Icon,
    title: &str,
    body: &str,
) {
    if search::searching() {
        return;
    }
    let w = ui.available_width();
    let galley = ui.painter().layout(
        body.to_string(),
        uc::font(13.0),
        p.text_secondary,
        w - 36.0 - 28.0 - 14.0,
    );
    let h = (16.0 + 20.0 + 6.0 + galley.size().y + 16.0).max(64.0);
    let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    uc::card(ui, r, 8, p.card, p.stroke);
    widgets::paint_icon(
        ui.painter(),
        egui::Rect::from_min_size(r.min + vec2(18.0, 16.0), vec2(28.0, 28.0)),
        icon,
        accent,
    );
    ui.painter().text(
        r.min + vec2(18.0 + 28.0 + 14.0, 16.0),
        Align2::LEFT_TOP,
        title,
        theme::semibold(15.0),
        p.text,
    );
    ui.painter().galley(
        r.min + vec2(18.0 + 28.0 + 14.0, 16.0 + 24.0),
        galley,
        p.text_secondary,
    );
}

// ---------------------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------------------

impl SettingsWindow {
    fn page_general(&mut self, ui: &mut Ui, shared: &mut Shared, accent: Color32) {
        let p = uc::pal(shared);
        let g = &mut shared.config.general;
        toggle_row(
            ui,
            tr!("settings.general.start_with_windows"),
            tr!("settings.general.start_with_windows.desc"),
            &mut g.start_with_windows,
        );
        widgets::setting_row(
            ui,
            tr!("settings.theme"),
            tr!("settings.theme.desc"),
            |ui| {
                let label = THEMES
                    .iter()
                    .find(|(v, _)| *v == g.theme)
                    .map_or(tr!("settings.theme.system"), |(_, k)| l10n::lookup(k));
                egui::ComboBox::from_id_salt("theme")
                    .icon(widgets::combo_chevron)
                    .selected_text(label)
                    .width(170.0)
                    .show_ui(ui, |ui| {
                        crate::theme::popup_rows(ui);
                        for (value, key) in THEMES {
                            ui.selectable_value(&mut g.theme, value.to_string(), l10n::lookup(key));
                        }
                    });
            },
        );
        widgets::setting_row(
            ui,
            tr!("settings.language"),
            tr!("settings.language.desc"),
            |ui| {
                let current = l10n::CHOICES
                    .iter()
                    .find(|(v, _)| *v == g.language)
                    .map_or(tr!("settings.language.system"), |(_, k)| l10n::lookup(k));
                egui::ComboBox::from_id_salt("language")
                    .icon(widgets::combo_chevron)
                    .selected_text(current)
                    .width(170.0)
                    .show_ui(ui, |ui| {
                        crate::theme::popup_rows(ui);
                        for (value, key) in l10n::CHOICES {
                            // Pseudo is a layout test, not a language: debug builds only
                            // (and while selected, so it can be switched back).
                            if *value == "pseudo"
                                && !cfg!(debug_assertions)
                                && g.language != "pseudo"
                            {
                                continue;
                            }
                            let r = ui.selectable_value(
                                &mut g.language,
                                value.to_string(),
                                l10n::lookup(key),
                            );
                            if r.changed() {
                                l10n::apply(&g.language);
                            }
                        }
                    });
            },
        );
        widgets::setting_row(
            ui,
            tr!("settings.general.accent"),
            tr!("settings.general.accent.desc"),
            |ui| {
                ui.horizontal(|ui| {
                    let presets: Vec<String> =
                        DEFAULT_ACCENTS.iter().map(|s| s.to_string()).collect();
                    if let Some(h) = color_button(ui, &g.accent) {
                        g.accent = h;
                    }
                    if let Some(h) = swatch_row(ui, &presets, &g.accent, p.text) {
                        g.accent = h;
                    }
                    if widgets::button(ui, tr!("settings.general.windows_accent"))
                        .on_hover_text(tr!("settings.general.windows_accent.tip"))
                        .clicked()
                        && let Some(c) = stinkshot_platform::system::system_accent()
                    {
                        g.accent = stinkshot_core::config::to_hex_color(c);
                    }
                });
            },
        );
        group_title(ui, p, tr!("settings.general.after_capture"));
        toggle_row(
            ui,
            tr!("settings.general.copy"),
            tr!("settings.general.copy.desc"),
            &mut g.copy_to_clipboard,
        );
        toggle_row(
            ui,
            tr!("settings.general.toast"),
            tr!("settings.general.toast.desc"),
            &mut g.show_toast,
        );
        ui.add_enabled_ui(g.show_toast, |ui| {
            slider_row(
                ui,
                tr!("settings.general.toast_duration"),
                tr!("settings.general.toast_duration.desc"),
                &mut g.toast_seconds,
                2..=30,
                " s",
            );
        });
        toggle_row(
            ui,
            tr!("settings.general.sound"),
            tr!("settings.general.sound.desc"),
            &mut g.play_sound,
        );
        group_title(ui, p, tr!("settings.general.pins"));
        let pins = &mut shared.config.pins;
        toggle_row(
            ui,
            tr!("settings.general.pin_task_view"),
            tr!("settings.general.pin_task_view.desc"),
            &mut pins.show_in_task_view,
        );
        toggle_row(
            ui,
            tr!("settings.general.pin_toolbar"),
            tr!("settings.general.pin_toolbar.desc"),
            &mut pins.always_show_toolbar,
        );
        let _ = accent;
    }

    /// Check Again under the hotkey problems, while one of them is a key Windows refused
    /// (the shell also retries those on its own). Says "Still in use." for a moment after a
    /// check that did not clear everything.
    fn check_again_row(&mut self, ui: &mut Ui, shared: &mut Shared) {
        let Some(still) = retry_status(&self.hotkey_errors) else {
            self.hotkey_checked_at = None;
            return;
        };
        let p = uc::pal(shared);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let checking = self.hotkey_check_requested;
            let clicked = ui
                .add_enabled_ui(!checking, |ui| {
                    widgets::button(ui, tr!("settings.hotkeys.check_again"))
                })
                .inner
                .clicked();
            if clicked {
                shared.outbox.push(Command::RetryHotkeys);
                self.hotkey_check_requested = true;
                self.hotkey_checked_at = None;
            }
            const SHOW: Duration = Duration::from_secs(4);
            if let Some(at) = self.hotkey_checked_at {
                let age = at.elapsed();
                if age < SHOW {
                    ui.add_space(6.0);
                    ui.label(RichText::new(still).size(12.0).color(p.text_tertiary));
                    ui.ctx().request_repaint_after(SHOW - age);
                } else {
                    self.hotkey_checked_at = None;
                }
            }
        });
    }

    fn page_hotkeys(&mut self, ui: &mut Ui, shared: &mut Shared, key_consumed: bool) {
        let p = uc::pal(shared);
        let accent = uc::accent(shared);
        group_title(ui, p, tr!("settings.hotkeys.integration"));
        toggle_row(
            ui,
            tr!("settings.hotkeys.replace_snip"),
            tr!("settings.hotkeys.replace_snip.desc"),
            &mut shared.config.hotkeys.replace_snipping_tool,
        );
        let routes = crate::hotkeys::snip_routes(&shared.config.hotkeys);
        let (status, tone) = snip_status_line(
            &self.snip_status,
            &shared.config.hotkeys,
            [routes[0].1, routes[1].1],
        );
        let color = match tone {
            Tone::Good => theme::accent_fg(accent, shared.dark),
            Tone::Bad => p.danger,
            Tone::Neutral => p.text_tertiary,
        };
        let browsing = !search::searching();
        let mut lines = Vec::new();
        if !status.is_empty() {
            lines.push((status, color));
        }
        if let Some(note) = snip_prtsc_note(&shared.config.hotkeys) {
            lines.push((note.to_string(), p.text_tertiary));
        }
        if browsing {
            ui.add_space(2.0);
            // Indented to line up with the text inside the card above.
            ui.horizontal(|ui| {
                ui.add_space(19.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for (text, color) in lines {
                        ui.add(
                            egui::Label::new(RichText::new(text).size(12.0).color(color)).wrap(),
                        );
                    }
                });
            });
            ui.add_space(4.0);
        }
        if shared.config.hotkeys.replace_snipping_tool {
            // Included in Replace Snipping Tool: shown on and locked.
            let mut on = true;
            ui.add_enabled_ui(false, |ui| {
                toggle_row(
                    ui,
                    tr!("settings.hotkeys.printscreen"),
                    tr!("settings.hotkeys.printscreen.locked"),
                    &mut on,
                );
            });
        } else {
            toggle_row(
                ui,
                tr!("settings.hotkeys.printscreen"),
                tr!("settings.hotkeys.printscreen.desc"),
                &mut shared.config.hotkeys.take_over_printscreen,
            );
        }
        if browsing && !self.hotkey_errors.is_empty() {
            ui.add_space(4.0);
            let text = self.hotkey_errors.join("\n");
            let w = ui.available_width();
            let galley = ui
                .painter()
                .layout(text, uc::font(12.0), p.danger, w - 28.0);
            let (r, _) = ui.allocate_exact_size(vec2(w, galley.size().y + 24.0), Sense::hover());
            ui.painter()
                .rect_filled(r, CornerRadius::same(8), uc::with_alpha(p.danger, 28));
            ui.painter().rect_stroke(
                r,
                CornerRadius::same(8),
                Stroke::new(1.0, uc::with_alpha(p.danger, 110)),
                StrokeKind::Inside,
            );
            ui.painter()
                .galley(r.min + vec2(14.0, 12.0), galley, p.danger);
            self.check_again_row(ui, shared);
        }
        group_title(ui, p, tr!("settings.hotkeys.shortcuts"));
        let hk_snapshot = shared.config.hotkeys.clone();
        for field in HOTKEY_FIELDS {
            let current = field.hotkey(&hk_snapshot).clone();
            let mut desc = field.description().to_string();
            let dupes = conflicts_with(&hk_snapshot, field);
            if !dupes.is_empty() {
                let list = dupes
                    .iter()
                    .map(|f| f.label())
                    .collect::<Vec<_>>()
                    .join(", ");
                desc.push_str("  ");
                desc.push_str(&tr!("settings.hotkeys.also_used", list = list));
            }
            // Short form: the row already names the action, shows the key and sits in
            // Settings > Hotkeys; "Also used by" above already covers a duplicate.
            let alert = error_for(&self.hotkey_errors, &current)
                .and_then(crate::hotkeys::ErrorKind::of)
                .filter(|kind| *kind != crate::hotkeys::ErrorKind::Duplicate || dupes.is_empty())
                .map(crate::hotkeys::ErrorKind::row_text);
            let capturing = self.capturing == Some(field);
            let mut clear = false;
            let mut toggle_capture = false;
            widgets::setting_row_alert(ui, field.label(), &desc, alert, |ui| {
                ui.horizontal(|ui| {
                    if !current.is_empty()
                        && !capturing
                        && widgets::button(ui, tr!("settings.hotkeys.clear")).clicked()
                    {
                        clear = true;
                    }
                    let label = if capturing {
                        tr!("settings.hotkeys.press_keys").to_string()
                    } else {
                        display(&current)
                    };
                    let resp = if capturing {
                        widgets::accent_button(ui, &label)
                    } else {
                        widgets::button(ui, &label)
                    };
                    if capturing {
                        // Keep Tab / arrows / Esc from moving focus while recording.
                        resp.request_focus();
                        ui.memory_mut(|m| {
                            m.set_focus_lock_filter(
                                resp.id,
                                egui::EventFilter {
                                    tab: true,
                                    horizontal_arrows: true,
                                    vertical_arrows: true,
                                    escape: true,
                                },
                            )
                        });
                    }
                    if resp.clicked() && !key_consumed {
                        toggle_capture = true;
                    } else if capturing
                        && !key_consumed
                        && ui.input(|i| i.pointer.any_click())
                        && !resp.hovered()
                    {
                        // Clicked elsewhere: abandon.
                        toggle_capture = true;
                    }
                });
            });
            if clear {
                field.hotkey_mut(&mut shared.config.hotkeys).clear();
            }
            if toggle_capture {
                if capturing {
                    self.capturing = None;
                } else {
                    self.capturing = Some(field);
                    self.print_prev = uc::async_key_down(uc::VK_SNAPSHOT);
                }
            }
        }
        // The Snipping Tool chords: fixed keys, routed by Replace Snipping Tool.
        let replace = shared.config.hotkeys.replace_snipping_tool;
        for (chord, action) in routes {
            let (title, desc) = snip_chord_row(chord, action, replace);
            widgets::setting_row(ui, &title, &desc, |ui| {
                ui.add_enabled_ui(false, |ui| {
                    widgets::button(ui, chord.display())
                        .on_disabled_hover_text(tr!("settings.hotkeys.chord_fixed"));
                });
            });
        }
        if !browsing {
            return;
        }
        ui.add_space(8.0);
        ui.label(
            RichText::new(tr!("settings.hotkeys.help"))
                .size(12.0)
                .color(p.text_tertiary),
        );
        ui.add_space(4.0);
        if widgets::button(ui, tr!("settings.hotkeys.reset")).clicked() {
            let keep = shared.config.hotkeys.take_over_printscreen;
            let replace = shared.config.hotkeys.replace_snipping_tool;
            shared.config.hotkeys = Hotkeys {
                take_over_printscreen: keep,
                replace_snipping_tool: replace,
                ..Hotkeys::default()
            };
            self.capturing = None;
        }
        let _ = accent;
    }

    fn page_capture(&mut self, ui: &mut Ui, shared: &mut Shared, accent: Color32) {
        let p = uc::pal(shared);
        let c = &mut shared.config.capture;
        toggle_row(
            ui,
            tr!("settings.capture.cursor"),
            tr!("settings.capture.cursor.desc"),
            &mut c.include_cursor,
        );
        toggle_row(
            ui,
            tr!("settings.capture.cursor_object"),
            tr!("settings.capture.cursor_object.desc"),
            &mut c.cursor_object,
        );
        toggle_row(
            ui,
            tr!("settings.capture.snap"),
            tr!("settings.capture.snap.desc"),
            &mut c.snap_to_elements,
        );
        toggle_row(
            ui,
            tr!("settings.capture.loupe"),
            tr!("settings.capture.loupe.desc"),
            &mut c.show_loupe,
        );
        {
            use stinkshot_core::config::AfterRegion;
            combo_row(
                ui,
                "after_region",
                tr!("settings.capture.after_region"),
                tr!("settings.capture.after_region.desc"),
                &mut c.after_region,
                &[
                    (
                        AfterRegion::Annotate,
                        tr!("settings.capture.after_region.annotate"),
                    ),
                    (
                        AfterRegion::Window,
                        tr!("settings.capture.after_region.window"),
                    ),
                ],
            );
        }
        toggle_row(
            ui,
            tr!("settings.capture.open_editor"),
            tr!("settings.capture.open_editor.desc"),
            &mut c.open_editor_after_capture,
        );

        group_title(ui, p, tr!("settings.capture.window"));
        toggle_row(
            ui,
            tr!("settings.capture.corners"),
            tr!("settings.capture.corners.desc"),
            &mut c.window_rounded_corners,
        );
        toggle_row(
            ui,
            tr!("settings.capture.shadow"),
            tr!("settings.capture.shadow.desc"),
            &mut c.window_shadow,
        );
        toggle_row(
            ui,
            tr!("settings.capture.frame"),
            tr!("settings.capture.frame.desc"),
            &mut c.window_padding,
        );
        ui.add_enabled_ui(c.window_padding, |ui| {
            slider_row(
                ui,
                tr!("settings.capture.padding"),
                tr!("settings.capture.padding.desc"),
                &mut c.window_padding_px,
                0..=200,
                " px",
            );
            widgets::setting_row(
                ui,
                tr!("settings.capture.padding_color"),
                tr!("settings.capture.padding_color.desc"),
                |ui| {
                    ui.horizontal(|ui| {
                        if let Some(h) = color_button(ui, &c.window_padding_color) {
                            c.window_padding_color = h;
                        }
                        if let Some(t) =
                            commit_text(ui, "pad_hex", &c.window_padding_color, 90.0, "#5b7fa8")
                            && let Some(rgba) = uc::parse_hex(&t)
                        {
                            c.window_padding_color = stinkshot_core::config::to_hex_color(rgba);
                        }
                    });
                },
            );
        });

        group_title(ui, p, tr!("settings.capture.saving"));
        combo_row(
            ui,
            "format",
            tr!("settings.capture.format"),
            tr!("settings.capture.format.desc"),
            &mut c.format,
            &[
                (ImageFormat::Png, "PNG"),
                (ImageFormat::Jpeg, "JPEG"),
                (ImageFormat::Webp, "WebP"),
            ],
        );
        if c.format == ImageFormat::Jpeg {
            slider_row(
                ui,
                tr!("settings.capture.jpeg_quality"),
                tr!("settings.capture.jpeg_quality.desc"),
                &mut c.jpeg_quality,
                40..=100,
                "",
            );
        }
        let preview = {
            let parts = stinkshot_core::naming::NameParts {
                now: chrono::Local::now(),
                app: "firefox",
                title: tr!("settings.capture.example_title"),
                width: 1280,
                height: 720,
            };
            format!(
                "{}.{}",
                stinkshot_core::naming::expand_pattern(&c.filename_pattern, &parts),
                c.format.extension()
            )
        };
        widgets::setting_row(
            ui,
            tr!("settings.capture.pattern"),
            &tr!("settings.capture.pattern.desc", preview = preview),
            |ui| {
                if let Some(t) = commit_text(
                    ui,
                    "pattern",
                    &c.filename_pattern,
                    260.0,
                    "Stinkshot_{yyyy}-{MM}-{dd}", // l10n-ok
                ) {
                    let t = t.trim().to_string();
                    c.filename_pattern = if t.is_empty() {
                        Config::default().capture.filename_pattern
                    } else {
                        t
                    };
                }
            },
        );

        group_title(ui, p, tr!("settings.capture.scrolling"));
        slider_row(
            ui,
            tr!("settings.capture.max_height"),
            tr!("settings.capture.max_height.desc"),
            &mut c.scroll_max_height,
            2000..=60000,
            " px",
        );
        slider_row(
            ui,
            tr!("settings.capture.scroll_delay"),
            tr!("settings.capture.scroll_delay.desc"),
            &mut c.scroll_delay_ms,
            30..=1000,
            " ms",
        );
        let _ = accent;
    }

    fn page_editor(&mut self, ui: &mut Ui, shared: &mut Shared) {
        let p = uc::pal(shared);
        let accent = uc::accent(shared);
        let e = &mut shared.config.editor;
        group_title(ui, p, tr!("settings.editor.defaults"));
        widgets::setting_row(
            ui,
            tr!("settings.editor.color"),
            tr!("settings.editor.color.desc"),
            |ui| {
                ui.horizontal(|ui| {
                    if let Some(h) = swatch_row(ui, &e.palette, &e.color, p.text) {
                        e.color = h;
                    }
                });
            },
        );
        slider_row(
            ui,
            tr!("settings.editor.stroke"),
            tr!("settings.editor.stroke.desc"),
            &mut e.stroke_width,
            1.0..=24.0,
            " px",
        );
        slider_row(
            ui,
            tr!("settings.editor.font_size"),
            tr!("settings.editor.font_size.desc"),
            &mut e.font_size,
            8.0..=96.0,
            " px",
        );
        slider_row(
            ui,
            tr!("settings.editor.blur"),
            tr!("settings.editor.blur.desc"),
            &mut e.blur_radius,
            2.0..=48.0,
            " px",
        );
        slider_row(
            ui,
            tr!("settings.editor.pixelate"),
            tr!("settings.editor.pixelate.desc"),
            &mut e.pixelate_block,
            4..=64,
            " px",
        );

        group_title(ui, p, tr!("settings.editor.palette"));
        let mut remove: Option<usize> = None;
        let mut add = false;
        let mut reset = false;
        widgets::setting_row(
            ui,
            tr!("settings.editor.palette_colors"),
            tr!("settings.editor.palette_colors.desc"),
            |ui| {
                ui.horizontal(|ui| {
                    for (i, hex) in e.palette.clone().iter().enumerate() {
                        if let Some(h) = color_button(ui, hex) {
                            // Keep the default color in sync when its swatch is edited.
                            if e.color.eq_ignore_ascii_case(hex) {
                                e.color = h.clone();
                            }
                            e.palette[i] = h;
                        }
                    }
                });
            },
        );
        widgets::setting_row(
            ui,
            tr!("settings.editor.manage"),
            tr!("settings.editor.manage.desc"),
            |ui| {
                ui.horizontal(|ui| {
                    if e.palette.len() < 12
                        && widgets::button(ui, tr!("settings.editor.add")).clicked()
                    {
                        add = true;
                    }
                    if e.palette.len() > 1
                        && widgets::button(ui, tr!("settings.editor.remove_last")).clicked()
                    {
                        remove = Some(e.palette.len() - 1);
                    }
                    if widgets::button(ui, tr!("settings.editor.reset")).clicked() {
                        reset = true;
                    }
                });
            },
        );
        if add {
            e.palette.push(uc::color_to_hex(accent));
        }
        if let Some(i) = remove {
            e.palette.remove(i);
        }
        if reset {
            e.palette = Config::default().editor.palette;
            if !e.palette.iter().any(|h| h.eq_ignore_ascii_case(&e.color)) {
                e.color = e
                    .palette
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "#ff8a3d".into());
            }
        }
    }

    fn page_recording(&mut self, ui: &mut Ui, shared: &mut Shared) {
        let p = uc::pal(shared);
        let ctx = ui.ctx().clone();
        let mut rescan = false;
        let loaded = self.devices.loaded;
        let mic_opts = device_options(&self.devices.mics, &shared.config.recording.mic_device);
        let cam_opts = device_options(&self.devices.cams, &shared.config.recording.webcam_device);
        let r = &mut shared.config.recording;
        group_title(ui, p, tr!("settings.recording.video"));
        combo_row(
            ui,
            "fps",
            tr!("settings.recording.fps"),
            tr!("settings.recording.fps.desc"),
            &mut r.fps,
            &as_refs(&fps_labels(&[30, 60, 120])),
        );
        slider_row(
            ui,
            tr!("settings.recording.bitrate"),
            tr!("settings.recording.bitrate.desc"),
            &mut r.bitrate_kbps,
            1000..=80000,
            " kbps",
        );
        slider_row(
            ui,
            tr!("settings.recording.countdown"),
            tr!("settings.recording.countdown.desc"),
            &mut r.countdown_seconds,
            0..=10,
            " s",
        );
        toggle_row(
            ui,
            tr!("settings.recording.clicks"),
            tr!("settings.recording.clicks.desc"),
            &mut r.show_clicks,
        );
        toggle_row(
            ui,
            tr!("settings.recording.keys"),
            tr!("settings.recording.keys.desc"),
            &mut r.show_keys,
        );

        group_title(ui, p, tr!("settings.recording.audio_webcam"));
        toggle_row(
            ui,
            tr!("settings.recording.system_audio"),
            tr!("settings.recording.system_audio.desc"),
            &mut r.system_audio,
        );
        toggle_row(
            ui,
            tr!("settings.recording.mic"),
            tr!("settings.recording.mic.desc"),
            &mut r.microphone,
        );
        let dev_row = |ui: &mut Ui,
                       id: &str,
                       title: &str,
                       desc: &str,
                       value: &mut String,
                       opts: &[(String, String)],
                       enabled: bool| {
            widgets::setting_row(ui, title, desc, |ui| {
                ui.add_enabled_ui(enabled, |ui| {
                    let cur = opts
                        .iter()
                        .find(|(v, _)| v == value)
                        .map(|(_, l)| l.as_str())
                        .unwrap_or(tr!("settings.recording.default_device"));
                    egui::ComboBox::from_id_salt(id)
                        .icon(widgets::combo_chevron)
                        .selected_text(cur.to_string())
                        .width(240.0)
                        .show_ui(ui, |ui| {
                            crate::theme::popup_rows(ui);
                            for (v, l) in opts {
                                ui.selectable_value(value, v.clone(), l.as_str());
                            }
                        });
                });
            });
        };
        dev_row(
            ui,
            "mic_dev",
            tr!("settings.recording.mic_device"),
            if loaded {
                tr!("settings.recording.mic_device.desc")
            } else {
                tr!("settings.recording.scanning")
            },
            &mut r.mic_device,
            &mic_opts,
            r.microphone,
        );
        toggle_row(
            ui,
            tr!("settings.recording.webcam"),
            tr!("settings.recording.webcam.desc"),
            &mut r.webcam,
        );
        dev_row(
            ui,
            "cam_dev",
            tr!("settings.recording.webcam_device"),
            if loaded {
                tr!("settings.recording.webcam_device.desc")
            } else {
                tr!("settings.recording.scanning")
            },
            &mut r.webcam_device,
            &cam_opts,
            r.webcam,
        );
        ui.add_enabled_ui(r.webcam, |ui| {
            slider_row(
                ui,
                tr!("settings.recording.webcam_size"),
                tr!("settings.recording.webcam_size.desc"),
                &mut r.webcam_diameter,
                96..=360,
                " px",
            );
        });
        widgets::setting_row(
            ui,
            tr!("settings.recording.devices"),
            tr!("settings.recording.devices.desc"),
            |ui| {
                if widgets::button(ui, tr!("settings.recording.refresh")).clicked() {
                    rescan = true;
                }
            },
        );

        group_title(ui, p, tr!("settings.recording.gif"));
        combo_row(
            ui,
            "gif_fps",
            tr!("settings.recording.gif_fps"),
            tr!("settings.recording.gif_fps.desc"),
            &mut r.gif_fps,
            &as_refs(&fps_labels(&[10, 12, 15, 20, 25, 30])),
        );
        slider_row(
            ui,
            tr!("settings.recording.gif_width"),
            tr!("settings.recording.gif_width.desc"),
            &mut r.gif_max_width,
            240..=1920,
            " px",
        );
        if rescan {
            self.devices.loaded = false;
            self.scan_devices(&ctx);
        }
    }

    fn history_rows(&mut self, ui: &mut Ui, shared: &mut Shared) {
        let ctx = ui.ctx().clone();
        let p = uc::pal(shared);
        let default_folder = Config::default().history_folder();
        let shown_folder = shared.config.history_folder();
        let has_ocr = shared.ocr.is_some();
        let picking = self.folder_rx.is_some();
        let mut pick = false;
        let mut open = false;
        let mut reset = false;
        let custom = !shared.config.history.folder.is_empty();
        widgets::setting_row(
            ui,
            tr!("settings.history.folder"),
            &tr!(
                "settings.history.folder.desc",
                path = shown_folder.display()
            ),
            |ui| {
                ui.horizontal(|ui| {
                    if custom && widgets::button(ui, tr!("settings.history.use_default")).clicked()
                    {
                        reset = true;
                    }
                    if widgets::button(ui, tr!("settings.open")).clicked() {
                        open = true;
                    }
                    if widgets::button(
                        ui,
                        if picking {
                            tr!("settings.history.choosing")
                        } else {
                            tr!("settings.history.change")
                        },
                    )
                    .clicked()
                        && !picking
                    {
                        pick = true;
                    }
                });
            },
        );
        if pick {
            self.start_folder_pick(&ctx, shown_folder.clone());
        }
        if open && let Err(e) = stinkshot_platform::system::open_path(&shown_folder) {
            shared.outbox.push(Command::Notify {
                title: tr!("settings.history.open_failed").into(),
                body: crate::errors::report(
                    "open folder",
                    &e,
                    tr!("settings.history.open_failed.hint"),
                ),
                error: true,
            });
        }
        if reset {
            shared.config.history.folder.clear();
        }
        let _ = default_folder;
        let ocr_desc = if has_ocr {
            tr!("settings.history.ocr.desc").to_string()
        } else {
            tr!(
                "settings.history.ocr.desc_missing",
                help = crate::l10n::tr!("shell.ocr.none.body")
            )
        };
        toggle_row(
            ui,
            tr!("settings.history.ocr"),
            &ocr_desc,
            &mut shared.config.history.ocr_index,
        );
        let h = &mut shared.config.history;
        widgets::setting_row(
            ui,
            tr!("settings.history.keep"),
            tr!("settings.history.keep.desc"),
            |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::DragValue::new(&mut h.keep_days)
                            .range(0..=3650)
                            .speed(1.0)
                            .custom_formatter(|n, _| days_label(n as u32))
                            .custom_parser(|s| {
                                let t = s.trim().to_lowercase();
                                if t.starts_with("forever") || t.starts_with("never") {
                                    Some(0.0)
                                } else {
                                    t.split_whitespace()
                                        .next()
                                        .and_then(|n| n.parse::<f64>().ok())
                                }
                            }),
                    );
                    ui.add(
                        egui::DragValue::new(&mut h.max_gb)
                            .range(0.0..=2000.0)
                            .speed(0.1)
                            .custom_formatter(|n, _| gb_label(n as f32))
                            .custom_parser(|s| {
                                let t = s.trim().to_lowercase();
                                if t.starts_with("no") {
                                    Some(0.0)
                                } else {
                                    t.split_whitespace()
                                        .next()
                                        .and_then(|n| n.parse::<f64>().ok())
                                }
                            }),
                    );
                });
            },
        );
        toggle_row(
            ui,
            tr!("settings.history.originals"),
            tr!("settings.history.originals.desc"),
            &mut h.keep_editable,
        );
        if let Some(s) = &self.stats
            && !search::searching()
        {
            ui.add_space(2.0);
            ui.label(
                RichText::new(tr!(
                    "settings.history.stats",
                    count = uc::fmt_thousands(s.count),
                    size = uc::fmt_bytes(s.bytes)
                ))
                .size(12.0)
                .color(p.text_tertiary),
            );
        }
    }

    fn page_history(&mut self, ui: &mut Ui, shared: &mut Shared) {
        self.history_rows(ui, shared);
    }

    fn page_privacy(&mut self, ui: &mut Ui, shared: &mut Shared, accent: Color32) {
        let p = uc::pal(shared);
        info_card(
            ui,
            p,
            accent,
            Icon::Shield,
            tr!("settings.privacy.card"),
            tr!("settings.privacy.card.body"),
        );
        ui.add_space(2.0);
        let enabled = shared.config.privacy.check_for_updates;
        toggle_row(
            ui,
            tr!("settings.privacy.updates"),
            tr!("settings.privacy.updates.desc"),
            &mut shared.config.privacy.check_for_updates,
        );
        let last = last_check_text(shared.config.privacy.last_update_check, chrono::Utc::now());
        let requested = self
            .update_requested_at
            .is_some_and(|t| t.elapsed() < Duration::from_secs(4));
        let mut check = false;
        widgets::setting_row(
            ui,
            tr!("settings.privacy.check_now"),
            &if requested {
                tr!("settings.privacy.check_requested").to_string()
            } else {
                last
            },
            |ui| {
                ui.add_enabled_ui(enabled, |ui| {
                    if widgets::button(ui, tr!("settings.privacy.check_now")).clicked() {
                        check = true;
                    }
                });
                if !enabled {
                    ui.label(
                        RichText::new(tr!("settings.privacy.updates_off"))
                            .size(12.0)
                            .color(p.text_tertiary),
                    );
                }
            },
        );
        if check {
            shared.outbox.push(Command::CheckForUpdates);
            self.update_requested_at = Some(Instant::now());
        }
        if requested {
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }
        // The history rows are History's own; search shows them there only.
        if search::searching() {
            return;
        }
        group_title(ui, p, tr!("settings.privacy.history"));
        self.history_rows(ui, shared);
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(tr!(
                    "settings.privacy.stored_in",
                    path = Config::path().display()
                ))
                .size(12.0)
                .color(p.text_tertiary),
            );
            if ui
                .link(RichText::new(tr!("settings.privacy.open_file")).size(12.0))
                .clicked()
                && let Err(e) = stinkshot_platform::system::open_path(&Config::path())
            {
                shared.outbox.push(Command::Notify {
                    title: tr!("settings.privacy.open_file_failed").into(),
                    body: crate::errors::report(
                        "open config file",
                        &e,
                        tr!("settings.privacy.open_file_failed.hint"),
                    ),
                    error: true,
                });
            }
        });
    }

    fn page_about(&mut self, ui: &mut Ui, shared: &mut Shared) {
        let p = uc::pal(shared);
        if !search::searching() {
            crate::logo_anim::header(
                ui,
                theme::semibold(22.0),
                uc::font(13.0),
                p.text,
                p.text_secondary,
            );
        }
        widgets::setting_row(
            ui,
            tr!("settings.about.license"),
            tr!("settings.about.license.desc"),
            |ui| {
                ui.label(
                    RichText::new("GPL-3.0-or-later") // l10n-ok
                        .size(13.0)
                        .color(p.text_secondary),
                );
            },
        );
        let copied = self
            .copied_url_at
            .is_some_and(|t| t.elapsed() < Duration::from_secs(2));
        let mut open = false;
        let mut copy = false;
        widgets::setting_row(ui, tr!("settings.about.source"), REPO_URL, |ui| {
            ui.horizontal(|ui| {
                if widgets::button(
                    ui,
                    if copied {
                        tr!("settings.about.copied")
                    } else {
                        tr!("settings.about.copy_link")
                    },
                )
                .clicked()
                {
                    copy = true;
                }
                if widgets::button(ui, tr!("settings.open")).clicked() {
                    open = true;
                }
            });
        });
        if open
            && let Err(e) = stinkshot_platform::system::open_path(std::path::Path::new(REPO_URL))
        {
            shared.outbox.push(Command::Notify {
                title: tr!("settings.about.link_failed").into(),
                body: crate::errors::report(
                    "open link",
                    &e,
                    tr!("settings.about.link_failed.hint"),
                ),
                error: true,
            });
        }
        if copy {
            match stinkshot_platform::clipboard::set_text(REPO_URL) {
                Ok(()) => self.copied_url_at = Some(Instant::now()),
                Err(e) => shared.outbox.push(Command::Notify {
                    title: tr!("settings.about.copy_failed").into(),
                    body: crate::workers::clipboard_text_error(&e),
                    error: true,
                }),
            }
        }
        if copied {
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }
        let mut show_tour = false;
        widgets::setting_row(
            ui,
            tr!("settings.about.tour"),
            tr!("settings.about.tour.desc"),
            |ui| {
                if widgets::button(ui, tr!("settings.about.show_tour")).clicked() {
                    show_tour = true;
                }
            },
        );
        if show_tour {
            shared.outbox.push(Command::OpenTour);
        }
        let mut open_log = false;
        widgets::setting_row(
            ui,
            tr!("settings.about.log"),
            tr!("settings.about.log.desc"),
            |ui| {
                if widgets::button(ui, tr!("settings.about.open_log")).clicked() {
                    open_log = true;
                }
            },
        );
        if open_log
            && let Err(e) = stinkshot_platform::system::reveal_in_explorer(&crate::logger::path())
        {
            shared.outbox.push(Command::Notify {
                title: tr!("settings.about.log_failed").into(),
                // No LOG_HINT here: it would point back at this button.
                body: {
                    log::warn!("open log: {e:#}");
                    tr!("settings.about.log_failed.body").into()
                },
                error: true,
            });
        }
        widgets::setting_row(
            ui,
            tr!("settings.about.privacy"),
            tr!("settings.about.privacy.desc"),
            |ui| {
                ui.label(
                    RichText::new(tr!("settings.about.offline"))
                        .size(13.0)
                        .color(p.text_secondary),
                );
            },
        );
        let _ = Vec2::ZERO;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_detection_is_order_and_case_insensitive() {
        // Window shares its combination with scrolling ("Control+Alt+KeyS").
        let h = Hotkeys {
            window: "alt+control+keys".into(),
            ..Hotkeys::default()
        };
        assert_eq!(
            conflicts_with(&h, HotkeyField::Scrolling),
            vec![HotkeyField::Window]
        );
        assert_eq!(
            conflicts_with(&h, HotkeyField::Window),
            vec![HotkeyField::Scrolling]
        );
        assert!(conflicts_with(&h, HotkeyField::Region).is_empty());
    }

    #[test]
    fn unbound_fields_never_conflict() {
        let mut h = Hotkeys::default();
        h.window.clear();
        h.ocr.clear();
        assert!(conflicts_with(&h, HotkeyField::Window).is_empty());
        assert!(conflicts_with(&h, HotkeyField::Ocr).is_empty());
    }

    #[test]
    fn defaults_have_no_conflicts() {
        let h = Hotkeys::default();
        for f in HOTKEY_FIELDS {
            assert!(conflicts_with(&h, f).is_empty(), "{f:?}");
        }
    }

    #[test]
    fn shell_errors_match_raw_or_friendly_prefix() {
        let errors = vec![
            "Control+Shift+KeyS: already in use".to_string(),
            "Alt+PrtSc: invalid".to_string(),
        ];
        assert_eq!(
            error_for(&errors, "Control+Shift+KeyS"),
            Some("Control+Shift+KeyS: already in use")
        );
        assert_eq!(
            error_for(&errors, "Alt+PrintScreen"),
            Some("Alt+PrtSc: invalid")
        );
        assert_eq!(error_for(&errors, "Control+KeyT"), None);
        assert_eq!(error_for(&errors, ""), None);
        // A shorter key is not a prefix match of a longer one.
        let errors = vec!["Alt+PrtSc (Window) is already used by another program.".to_string()];
        assert_eq!(error_for(&errors, "Alt+P"), None);
        assert!(error_for(&errors, "Alt+PrintScreen").is_some());
    }

    #[test]
    fn check_again_only_for_keys_windows_refused() {
        let in_use = tr!(
            "shell.hotkey.in_use",
            key = "Ctrl+F9",
            action = "Full Screen",
            phrase = tr!("shell.hotkey.phrase.in_use")
        );
        let refused = tr!(
            "shell.hotkey.not_registered",
            key = "Ctrl+F9",
            action = "Full Screen",
            phrase = tr!("shell.hotkey.phrase.not_registered")
        );
        let invalid = tr!(
            "shell.hotkey.invalid",
            key = "Ctrl+F9",
            action = "Full Screen",
            phrase = tr!("shell.hotkey.phrase.invalid")
        );
        assert_eq!(retry_status(&[]), None);
        assert_eq!(retry_status(std::slice::from_ref(&invalid)), None);
        assert_eq!(
            retry_status(&[invalid.clone(), refused.clone()]),
            Some(tr!("settings.hotkeys.no_change"))
        );
        assert_eq!(
            retry_status(&[refused, in_use]),
            Some(tr!("settings.hotkeys.still_in_use"))
        );
    }

    #[test]
    fn device_options_keep_missing_selection() {
        let detected = vec!["Mic A".to_string(), "Mic B".to_string()];
        let o = device_options(&detected, "Mic B");
        assert_eq!(o.len(), 3);
        assert_eq!(o[0].0, "");
        let o = device_options(&detected, "USB Mic");
        assert_eq!(o.last().map(|x| x.1.as_str()), Some("USB Mic (not found)"));
        assert_eq!(device_options(&[], "").len(), 1);
    }

    #[test]
    fn retention_labels() {
        assert_eq!(days_label(0), "Forever");
        assert_eq!(days_label(1), "1 day");
        assert_eq!(days_label(90), "90 days");
        assert_eq!(gb_label(0.0), "No Cap");
        assert_eq!(gb_label(5.0), "5.0 GB");
    }

    #[test]
    fn last_check_text_buckets() {
        let now = chrono::Utc::now();
        assert_eq!(last_check_text(None, now), "Never checked");
        assert_eq!(last_check_text(Some(now), now), "Last checked just now");
        assert_eq!(
            last_check_text(Some(now - chrono::Duration::minutes(5)), now),
            "Last checked 5 min ago"
        );
        assert_eq!(
            last_check_text(Some(now - chrono::Duration::hours(3)), now),
            "Last checked 3 h ago"
        );
        assert_eq!(
            last_check_text(Some(now - chrono::Duration::days(2)), now),
            "Last checked 2 days ago"
        );
    }

    #[test]
    fn every_field_round_trips_through_accessors() {
        let mut h = Hotkeys::default();
        for (i, f) in HOTKEY_FIELDS.iter().enumerate() {
            *f.hotkey_mut(&mut h) = format!("KeyF{i}");
        }
        for (i, f) in HOTKEY_FIELDS.iter().enumerate() {
            assert_eq!(f.hotkey(&h), &format!("KeyF{i}"));
        }
    }

    #[test]
    fn snip_status_lines() {
        let routes = [HotkeyField::Region, HotkeyField::RecordMp4];
        let mut h = Hotkeys::default();
        let (s, t) = snip_status_line(&SnipStatus::Off, &h, routes);
        assert!(s.is_empty() && t == Tone::Neutral, "{s}");
        h.replace_snipping_tool = true;
        let (s, t) = snip_status_line(&SnipStatus::Active, &h, routes);
        assert_eq!(t, Tone::Good);
        assert_eq!(s, "Active");
        let why = SnipStatus::Unavailable("Nope. Try again.".into());
        assert_eq!(
            snip_status_line(&why, &h, routes),
            ("Nope. Try again.".to_string(), Tone::Bad)
        );
        assert_eq!(
            snip_status_line(&SnipStatus::Off, &h, routes).1,
            Tone::Neutral
        );
        // No PrintScreen binding: a note says PrtSc still opens the Snipping Tool.
        assert_eq!(snip_prtsc_note(&h), None);
        for f in HOTKEY_FIELDS {
            f.hotkey_mut(&mut h).clear();
        }
        assert!(snip_prtsc_note(&h).is_some_and(|n| n.contains("PrtSc isn't bound")));
        h.replace_snipping_tool = false;
        assert_eq!(snip_prtsc_note(&h), None);
        assert_eq!(
            snip_status_line(&SnipStatus::Active, &h, routes).0,
            "Turning off\u{2026}"
        );
    }

    #[test]
    fn snip_chord_rows() {
        let (t, d) = snip_chord_row(SnipChord::Region, HotkeyField::Region, true);
        assert_eq!(t, "Snipping Tool Snip");
        assert_eq!(d, "Region");
        let (t, d) = snip_chord_row(SnipChord::Record, HotkeyField::RecordMp4, false);
        assert_eq!(t, "Snipping Tool Recording");
        assert_eq!(d, "Snipping Tool");
    }
}
