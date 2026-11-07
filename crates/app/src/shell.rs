//! The shell: owns `Shared`, every open `Surface`, the tray, hotkeys, IPC, the active
//! recorder / scrolling job, and background workers (OCR indexing of history, retention,
//! update check). Implements `eframe::App` on the hidden root viewport and shows each
//! surface with `ctx.show_viewport_immediate` every frame, finding each new window's HWND
//! by title and calling `Surface::on_created` once.
//!
//! Command handling (see `state::Command`):
//! - Capture*: freeze screen (`capture::freeze_all`), snapshot windows, open `Overlay`.
//!   CaptureWindow: capture foreground window via WGC + `frame::apply_window_fx` and open
//!   the overlay in Annotate mode. CaptureFullscreen: monitor under cursor, Annotate mode.
//! - CaptureDone: render is already flattened; per FinishAction copy to clipboard / Save As
//!   (native dialog) / pin / OCR-copy; always save into history (`naming::unique_path`,
//!   `annotate::save_editable` if keep_editable), add to index, show toast, queue OCR.
//!   Store `config.capture.last_region`.
//! - RegionChosen(Scrolling): run the scroll loop on a worker (`input::scroll_at`,
//!   `capture::capture_rect`, `Stitcher`), show `ScrollHud`, then Annotate the result.
//! - RegionChosen(Record*): Countdown -> `Recorder::start` + RecordFrame + RecordHud ->
//!   on stop, TrimWindow -> TrimDone -> `record::trim` / `record::to_gif` on a worker ->
//!   history + toast.
//! - RegionChosen(Ocr): OCR the region on a worker, copy text, toast with preview.
//! - Requests to repaint: workers call `ctx.request_repaint()` after posting results.
//!
//! Root window: the root viewport is a 1x1 transparent, click-through, taskbar-less tool
//! window parked at (-32000,-32000). It is technically "visible" to the OS so winit/eframe
//! deliver ordinary redraws with no hidden-window throttling (a truly hidden root would
//! wake at most every 100 ms, which would add that latency to every hotkey press), but it
//! can never be seen or reached. It keeps the event loop alive while no surface is open.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, anyhow};
use egui::Color32;
use stinkshot_core::annotate::Doc;
use stinkshot_core::config::{Config, parse_hex_color};
use stinkshot_core::fonts::Fonts;
use stinkshot_core::{PxRect, Rgba};
use stinkshot_history::{History, ItemId, ItemKind};
use stinkshot_platform::capture::{self, Frozen};
use stinkshot_platform::ocr::{OcrEngine, WindowsOcr};
use stinkshot_platform::system::{self, InstanceGuard};
use stinkshot_platform::{Hwnd, window_util, windows_enum};
use stinkshot_record::{Corner, RecordOptions, Recorder, WebcamOptions};

use crate::capture_ops;
use crate::cli::{self, CliAction};
use crate::history_ui::HistoryWindow;
use crate::hotkeys::Hotkeys;
use crate::overlay::Overlay;
use crate::pin::Pin;
use crate::record_ui::{Countdown, HudToggles, RecordFrame, RecordHud, ScrollHud, TrimWindow};
use crate::settings_ui::SettingsWindow;
use crate::state::{
    CaptureOutput, Command, FinishAction, OverlayMode, OverlayRequest, RegionPurpose, Shared,
    SourceInfo, Surface, SurfaceStatus,
};
use crate::theme;
use crate::toast::{Toast, ToastKind};
use crate::tray::Tray;
use crate::update::Release;
use crate::workers::{
    self, BgConfig, BgKick, Env, SCROLL_CANCEL, SCROLL_RUN, SCROLL_STOP, WorkerMsg,
};
use crate::{logger, sysutil, wake};

const ROOT_TITLE: &str = "StinkShot.Root";
const MAX_TOASTS: usize = 6;

// ---------------------------------------------------------------------------- entry

pub fn run() -> anyhow::Result<()> {
    if std::env::args().len() > 1 {
        sysutil::attach_parent_console();
    }
    stinkshot_platform::dpi::init();
    logger::init();

    let (initial, forward_args) = match cli::parse() {
        CliAction::Headless(job) => {
            if let Err(e) = cli::run_headless(job) {
                log::error!("headless job failed: {e:#}");
                eprintln!("stinkshot: {e:#}");
                std::process::exit(1);
            }
            return Ok(());
        }
        CliAction::RunApp {
            initial,
            forward_args,
        } => (initial, forward_args),
    };

    let Some(guard) = system::acquire_single_instance() else {
        if forward_args.is_empty() {
            return Ok(());
        }
        for _ in 0..30 {
            if system::send_to_running_instance(&forward_args) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        log::error!("another instance owns the mutex but did not answer IPC");
        eprintln!("stinkshot: another instance is running but did not respond");
        std::process::exit(1);
    };

    match run_app(initial, guard) {
        Ok(()) => Ok(()),
        Err(e) => {
            log::error!("fatal: {e:#}");
            sysutil::fatal_box("StinkShot", &format!("StinkShot could not start.\n\n{e:#}"));
            Err(e)
        }
    }
}

struct Boot {
    config: Config,
    fonts: Arc<Fonts>,
    history: Arc<History>,
    ocr: Option<Arc<dyn OcrEngine>>,
    initial: Option<Command>,
    guard: InstanceGuard,
}

fn run_app(initial: Option<Command>, guard: InstanceGuard) -> anyhow::Result<()> {
    let config = Config::load();
    let fonts = Arc::new(Fonts::load_system().context("load system fonts")?);
    let history_dir = config.history_folder();
    let history = Arc::new(
        History::open(&history_dir)
            .with_context(|| format!("open history folder {}", history_dir.display()))?,
    );

    // OCR engine creation touches COM; do it on a scratch thread so the UI thread's apartment
    // stays the one winit sets up.
    let ocr: Option<Arc<dyn OcrEngine>> = std::thread::Builder::new()
        .name("ocr-init".into())
        .spawn(|| WindowsOcr::new().map(|o| Arc::new(o) as Arc<dyn OcrEngine>))
        .ok()
        .and_then(|h| h.join().ok())
        .and_then(|r| match r {
            Ok(o) => Some(o),
            Err(e) => {
                log::warn!("OCR unavailable: {e:#}");
                None
            }
        });

    let boot = Boot {
        config,
        fonts,
        history,
        ocr,
        initial,
        guard,
    };
    let options = eframe::NativeOptions {
        viewport: root_viewport(),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "StinkShot",
        options,
        Box::new(move |cc| {
            Ok(Box::new(
                Shell::new(cc, boot).map_err(|e| format!("{e:#}"))?,
            ))
        }),
    )
    .map_err(|e| anyhow!("{e}"))
}

fn root_viewport() -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_title(ROOT_TITLE)
        .with_inner_size([1.0, 1.0])
        .with_position([-32000.0, -32000.0])
        .with_decorations(false)
        .with_transparent(true)
        .with_taskbar(false)
        .with_active(false)
        .with_resizable(false)
        .with_mouse_passthrough(true)
}

// ------------------------------------------------------------------------ surfaces

#[derive(Default)]
struct HwndState {
    created: bool,
    tries: u32,
}

struct Slot<T> {
    s: T,
    st: HwndState,
}

impl<T> Slot<T> {
    fn new(s: T) -> Self {
        Self {
            s,
            st: HwndState::default(),
        }
    }
}

struct ToastSlot {
    slot: Slot<Toast>,
    index: usize,
}

/// Show one surface as an immediate viewport; resolve its HWND once.
fn drive(
    ctx: &egui::Context,
    shared: &mut Shared,
    surf: &mut dyn Surface,
    st: &mut HwndState,
    focus: bool,
) -> SurfaceStatus {
    let title = surf.title();
    let id = egui::ViewportId::from_hash_of(&title);
    let builder = surf.viewport_builder(shared).with_title(title.clone());
    let status = ctx.show_viewport_immediate(id, builder, |ctx, _class| surf.show(ctx, shared));
    if !st.created && st.tries < 90 {
        st.tries += 1;
        match window_util::find_own_window(&title) {
            Some(hwnd) => {
                st.created = true;
                surf.on_created(hwnd, shared);
                if focus {
                    window_util::force_foreground(hwnd);
                }
            }
            None => ctx.request_repaint(),
        }
    }
    status
}

/// Returns true if the surface closed this frame.
fn drive_opt<T: Surface>(
    ctx: &egui::Context,
    shared: &mut Shared,
    slot: &mut Option<Slot<T>>,
    focus: bool,
) -> bool {
    let Some(sl) = slot else { return false };
    if drive(ctx, shared, &mut sl.s, &mut sl.st, focus) == SurfaceStatus::Close {
        *slot = None;
        return true;
    }
    false
}

// ---------------------------------------------------------------------------- state

#[derive(Clone)]
struct PendingRecord {
    rect: PxRect,
    gif: bool,
}

enum Rec {
    Idle,
    Countdown(PendingRecord),
    /// Recorder is being created on a worker.
    Starting {
        pending: PendingRecord,
        cancel: bool,
    },
    Active {
        rec: Box<Recorder>,
        pending: PendingRecord,
    },
    /// `stop()` is running on a worker.
    Stopping(PendingRecord),
    Trim {
        source: PathBuf,
        done_issued: bool,
    },
}

struct ScrollFlow {
    control: Arc<AtomicU8>,
}

struct Shell {
    ctx: egui::Context,
    shared: Shared,
    applied: Config,
    tray: Option<Tray>,
    hotkeys: Hotkeys,
    hotkey_errors: Vec<String>,
    ipc_rx: Receiver<Vec<String>>,
    worker_tx: Sender<WorkerMsg>,
    worker_rx: Receiver<WorkerMsg>,
    bg_cfg: Arc<Mutex<BgConfig>>,
    bg_kick: Sender<BgKick>,
    pending: VecDeque<Command>,
    next_id: u64,

    overlay: Option<Slot<Overlay>>,
    history_win: Option<Slot<HistoryWindow>>,
    settings: Option<Slot<SettingsWindow>>,
    countdown: Option<Slot<Countdown>>,
    record_frame: Option<Slot<RecordFrame>>,
    record_hud: Option<Slot<RecordHud>>,
    trim: Option<Slot<TrimWindow>>,
    scroll_hud: Option<Slot<ScrollHud>>,
    toasts: Vec<ToastSlot>,
    pins: Vec<Slot<Pin>>,

    /// Snapshot behind an OCR region selection (so the text comes from what the user saw).
    freeze_cache: Option<Frozen>,
    window_capture_pending: bool,
    rec: Rec,
    exporting: bool,
    trim_closed_by_user: bool,
    scroll: Option<ScrollFlow>,

    resolved_theme: (bool, [u8; 4]),
    last_theme_check: Instant,
    update_release: Option<Release>,
    update_inflight: bool,
    update_retry_at: Instant,
    last_housekeeping: Instant,
    root_setup_tries: u32,
    needs_repaint: bool,
    quitting: bool,
    _guard: InstanceGuard,
}

fn resolve_theme(cfg: &Config) -> (bool, [u8; 4]) {
    let dark = match cfg.general.theme.as_str() {
        "dark" => true,
        "light" => false,
        _ => system::system_dark_mode(),
    };
    let accent = if cfg.general.accent.trim().is_empty() {
        system::system_accent()
    } else {
        parse_hex_color(&cfg.general.accent)
    }
    .unwrap_or([0x7f, 0xd3, 0x6a, 0xff]);
    (dark, accent)
}

fn accent_color(a: [u8; 4]) -> Color32 {
    Color32::from_rgb(a[0], a[1], a[2])
}

fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

fn fmt_bytes(b: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    if b as f64 >= MB {
        format!("{:.1} MB", b as f64 / MB)
    } else {
        format!("{} KB", (b / 1024).max(1))
    }
}

impl Shell {
    fn new(cc: &eframe::CreationContext<'_>, boot: Boot) -> anyhow::Result<Shell> {
        let ctx = cc.egui_ctx.clone();
        wake::set_context(ctx.clone());

        let Boot {
            config,
            fonts,
            history,
            ocr,
            initial,
            guard,
        } = boot;
        let resolved_theme = resolve_theme(&config);
        theme::install(
            &ctx,
            &fonts,
            resolved_theme.0,
            accent_color(resolved_theme.1),
        );

        // IPC: args forwarded by later launches.
        let (ipc_tx, ipc_inner_rx) = channel::<Vec<String>>();
        let (ipc_fwd_tx, ipc_rx) = channel::<Vec<String>>();
        match system::start_ipc_listener(ipc_tx) {
            Ok(()) => {
                std::thread::Builder::new()
                    .name("ipc-bridge".into())
                    .spawn(move || {
                        while let Ok(args) = ipc_inner_rx.recv() {
                            if ipc_fwd_tx.send(args).is_err() {
                                break;
                            }
                            wake::wake();
                        }
                    })
                    .context("start ipc bridge")?;
            }
            Err(e) => log::error!("IPC listener failed: {e:#}"),
        }

        let tray = match Tray::new(&config) {
            Ok(t) => Some(t),
            Err(e) => {
                log::error!("tray icon failed: {e:#}");
                None
            }
        };
        let (hotkeys, hotkey_errors) = Hotkeys::new(&config.hotkeys);

        let (worker_tx, worker_rx) = channel();
        let bg_cfg = Arc::new(Mutex::new(BgConfig::from(
            &config,
            history.clone(),
            ocr.clone(),
        )));
        let bg_kick = workers::start_background(bg_cfg.clone(), worker_tx.clone());

        let mut shell = Shell {
            ctx,
            shared: Shared {
                config: config.clone(),
                config_dirty: false,
                history,
                fonts,
                ocr,
                outbox: Vec::new(),
                dark: resolved_theme.0,
                accent: resolved_theme.1,
            },
            applied: config,
            tray,
            hotkeys,
            hotkey_errors,
            ipc_rx,
            worker_tx,
            worker_rx,
            bg_cfg,
            bg_kick,
            pending: VecDeque::new(),
            next_id: 1,
            overlay: None,
            history_win: None,
            settings: None,
            countdown: None,
            record_frame: None,
            record_hud: None,
            trim: None,
            scroll_hud: None,
            toasts: Vec::new(),
            pins: Vec::new(),
            freeze_cache: None,
            window_capture_pending: false,
            rec: Rec::Idle,
            exporting: false,
            trim_closed_by_user: false,
            scroll: None,
            resolved_theme,
            last_theme_check: Instant::now(),
            update_release: None,
            update_inflight: false,
            update_retry_at: Instant::now(),
            last_housekeeping: Instant::now(),
            root_setup_tries: 0,
            needs_repaint: false,
            quitting: false,
            _guard: guard,
        };

        shell.apply_autostart();
        if !shell.hotkey_errors.is_empty() {
            let body = shell.hotkey_errors.join("\n");
            shell.pending.push_back(Command::Notify {
                title: "Some hotkeys are not active".into(),
                body,
                error: true,
            });
        }
        if shell.tray.is_none() {
            shell.pending.push_back(Command::Notify {
                title: "Tray icon unavailable".into(),
                body: "StinkShot is running but could not create its tray icon. Use the hotkeys or the command line.".into(),
                error: true,
            });
        }
        if shell.shared.ocr.is_none() {
            log::info!("no OCR language installed; OCR features will report that");
        }
        if let Some(cmd) = initial {
            shell.pending.push_back(cmd);
        }
        shell.maybe_check_updates(false);
        shell.ctx.request_repaint();
        Ok(shell)
    }

    // --------------------------------------------------------------- helpers

    fn env(&self) -> Env {
        Env {
            tx: self.worker_tx.clone(),
            history: self.shared.history.clone(),
            ocr: self.shared.ocr.clone(),
            cfg: self.shared.config.clone(),
            bg: self.bg_kick.clone(),
        }
    }

    fn touch(&mut self) {
        self.needs_repaint = true;
    }

    fn new_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn notify(&mut self, title: &str, body: impl Into<String>, error: bool) {
        self.pending.push_back(Command::Notify {
            title: title.to_string(),
            body: body.into(),
            error,
        });
    }

    fn push_toast(&mut self, kind: ToastKind) {
        if self.toasts.len() >= MAX_TOASTS {
            self.toasts.remove(0);
        }
        let mut index = 0;
        while self.toasts.iter().any(|t| t.index == index) {
            index += 1;
        }
        let id = self.new_id();
        self.toasts.push(ToastSlot {
            slot: Slot::new(Toast::new(kind, id, index)),
            index,
        });
        self.touch();
    }

    fn add_pin(&mut self, image: Rgba, title: String, item: Option<ItemId>, at: Option<PxRect>) {
        let id = self.new_id();
        self.pins
            .push(Slot::new(Pin::new(image, title, item, at, id)));
        self.touch();
    }

    fn focus_window(title: &str) {
        if let Some(h) = window_util::find_own_window(title) {
            window_util::force_foreground(h);
        }
    }

    fn apply_autostart(&self) {
        let enabled = self.shared.config.general.start_with_windows;
        let exe = match std::env::current_exe() {
            Ok(e) => e,
            Err(e) => {
                log::warn!("cannot resolve exe path for autostart: {e}");
                return;
            }
        };
        let result = if enabled {
            system::set_autostart(true, &exe)
        } else if system::autostart_enabled() {
            system::set_autostart(false, &exe)
        } else {
            Ok(())
        };
        if let Err(e) = result {
            log::warn!("autostart update failed: {e:#}");
        }
    }

    fn busy_with_recording(&self) -> bool {
        !matches!(self.rec, Rec::Idle)
    }

    // ----------------------------------------------------------- command routing

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::CaptureRegion => self.begin_overlay(OverlayMode::Capture),
            Command::CaptureWindow => self.capture_window(),
            Command::CaptureFullscreen => self.capture_fullscreen(),
            Command::CaptureScrolling => {
                self.begin_overlay(OverlayMode::SelectRegion(RegionPurpose::Scrolling))
            }
            Command::OcrRegion => {
                if self.shared.ocr.is_none() {
                    self.notify(
                        "No OCR language installed",
                        "Add a language in Windows Settings > Time & language > Language & region.",
                        true,
                    );
                } else {
                    self.begin_overlay(OverlayMode::SelectRegion(RegionPurpose::Ocr));
                }
            }
            Command::RepeatLastRegion => self.repeat_last_region(),
            Command::RecordMp4 => self.record_command(false),
            Command::RecordGif => self.record_command(true),
            Command::OpenHistory => self.open_history(),
            Command::OpenSettings => self.open_settings(),
            Command::EditItem(id) => workers::edit_item(&self.env(), id),
            Command::PinItem(id) => workers::pin_item(&self.env(), id),
            Command::PinImage { image, title } => self.add_pin(image, title, None, None),
            Command::CaptureDone(out) => self.capture_done(out),
            Command::RegionChosen {
                purpose,
                rect,
                source,
            } => self.region_chosen(purpose, rect, source),
            Command::Cancelled => {
                self.freeze_cache = None;
            }
            Command::RecordPause => {
                if let Rec::Active { rec, .. } = &self.rec {
                    rec.pause();
                }
            }
            Command::RecordResume => {
                if let Rec::Active { rec, .. } = &self.rec {
                    rec.resume();
                }
            }
            Command::RecordStop => self.record_stop(),
            Command::RecordCancel => self.record_cancel(),
            Command::TrimDone {
                start,
                end,
                as_gif,
                as_mp4,
                ..
            } => self.trim_done(start, end, as_gif, as_mp4),
            Command::ScrollStop => {
                if let Some(s) = &self.scroll {
                    s.control.store(SCROLL_STOP, Ordering::Relaxed);
                }
            }
            Command::ScrollCancel => {
                if let Some(s) = &self.scroll {
                    s.control.store(SCROLL_CANCEL, Ordering::Relaxed);
                }
            }
            Command::CopyItemImage(id) => workers::copy_item_image(&self.env(), id),
            Command::CopyItemText(id) => workers::copy_item_text(&self.env(), id),
            Command::RevealItem(id) => {
                self.with_item(id, |meta| system::reveal_in_explorer(&meta.path))
            }
            Command::OpenItem(id) => self.with_item(id, |meta| system::open_path(&meta.path)),
            Command::DeleteItem(id) => workers::delete_item(&self.env(), id),
            Command::StarItem(id, starred) => {
                if let Err(e) = self.shared.history.set_starred(id, starred) {
                    self.notify("Could not update star", format!("{e:#}"), true);
                }
                self.invalidate_history();
            }
            Command::Notify { title, body, error } => {
                self.push_toast(ToastKind::Message { title, body, error })
            }
            Command::CheckForUpdates => self.maybe_check_updates(true),
            Command::Quit => self.quit(),
            Command::OpenUrl(url) => {
                if !(url.starts_with("https://") || url.starts_with("http://")) {
                    log::warn!("refusing to open non-web URL");
                } else if let Err(e) = system::open_path(std::path::Path::new(&url)) {
                    self.notify("Could not open the page", format!("{e:#}"), true);
                }
            }
            Command::SetHotkeysSuspended(s) => self.hotkeys.set_suspended(s),
        }
    }

    fn with_item(
        &mut self,
        id: ItemId,
        f: impl FnOnce(&stinkshot_history::ItemMeta) -> anyhow::Result<()>,
    ) {
        match self.shared.history.get(id) {
            Ok(Some(meta)) => {
                if let Err(e) = f(&meta) {
                    self.notify("Action failed", format!("{e:#}"), true);
                }
            }
            Ok(None) => self.notify("Item not found", "It may have been deleted.", true),
            Err(e) => self.notify("History error", format!("{e:#}"), true),
        }
    }

    fn invalidate_history(&mut self) {
        if let Some(h) = &mut self.history_win {
            h.s.invalidate();
        }
        self.touch();
    }

    fn capture_blocked(&mut self) -> bool {
        if self.overlay.is_some() || self.window_capture_pending || self.scroll.is_some() {
            return true;
        }
        if self.busy_with_recording() {
            self.notify(
                "Recording in progress",
                "Stop or finish the recording before taking a capture.",
                false,
            );
            return true;
        }
        false
    }

    fn freeze(&mut self) -> Option<(Frozen, Vec<stinkshot_platform::windows_enum::WinInfo>)> {
        let windows = windows_enum::visible_windows();
        match capture::freeze_all(self.shared.config.capture.include_cursor) {
            Ok(f) => Some((f, windows)),
            Err(e) => {
                log::error!("freeze_all failed: {e:#}");
                self.notify("Could not capture the screen", format!("{e:#}"), true);
                None
            }
        }
    }

    fn open_overlay(
        &mut self,
        mode: OverlayMode,
        frozen: Frozen,
        windows: Vec<stinkshot_platform::windows_enum::WinInfo>,
    ) {
        let req = OverlayRequest {
            mode,
            frozen,
            windows,
            last_region: self.shared.config.capture.last_region,
        };
        self.overlay = Some(Slot::new(Overlay::new(req, &self.shared)));
        self.touch();
    }

    fn begin_overlay(&mut self, mode: OverlayMode) {
        if self.capture_blocked() {
            return;
        }
        let Some((frozen, windows)) = self.freeze() else {
            return;
        };
        self.freeze_cache =
            matches!(mode, OverlayMode::SelectRegion(RegionPurpose::Ocr)).then(|| frozen.clone());
        self.open_overlay(mode, frozen, windows);
    }

    fn capture_window(&mut self) {
        if self.capture_blocked() {
            return;
        }
        let Some(win) = capture_ops::target_window() else {
            self.notify(
                "No window to capture",
                "There is no foreground window.",
                true,
            );
            return;
        };
        let Some((frozen, windows)) = self.freeze() else {
            return;
        };
        self.window_capture_pending = true;
        workers::capture_window(&self.env(), frozen, windows, win);
    }

    fn capture_fullscreen(&mut self) {
        if self.capture_blocked() {
            return;
        }
        let source = windows_enum::foreground()
            .map(|w| capture_ops::source_of(&w))
            .unwrap_or_default();
        let Some((frozen, windows)) = self.freeze() else {
            return;
        };
        let mon = capture_ops::monitor_rect_at(frozen.cursor, frozen.bounds);
        let base = capture_ops::crop_frozen(&frozen, mon);
        if base.width == 0 || base.height == 0 {
            self.notify(
                "Could not capture the screen",
                "The monitor is outside the captured area.",
                true,
            );
            return;
        }
        let doc = Doc::new(base.width, base.height);
        self.open_overlay(
            OverlayMode::Annotate {
                base,
                doc,
                mode: "fullscreen",
                editing: None,
                source,
            },
            frozen,
            windows,
        );
    }

    fn repeat_last_region(&mut self) {
        if self.capture_blocked() {
            return;
        }
        let Some(rect) = self.shared.config.capture.last_region else {
            self.notify(
                "No previous region",
                "Select a region first; it will be remembered.",
                false,
            );
            self.begin_overlay(OverlayMode::Capture);
            return;
        };
        let Some((frozen, windows)) = self.freeze() else {
            return;
        };
        let img = capture_ops::crop_frozen(&frozen, rect);
        if img.width == 0 || img.height == 0 {
            self.notify("Last region is off screen", "Select a new region.", false);
            self.open_overlay(OverlayMode::Capture, frozen, windows);
            return;
        }
        let source = capture_ops::source_for_rect(&windows, rect);
        let doc = Doc::new(img.width, img.height);
        let out = CaptureOutput {
            base: img.clone(),
            doc,
            flattened: img,
            rect,
            action: FinishAction::Copy,
            source,
            mode: "region",
            editing: None,
        };
        self.capture_done(out);
    }

    fn capture_done(&mut self, out: CaptureOutput) {
        self.freeze_cache = None;
        if out.mode == "region" && out.editing.is_none() {
            self.shared.config.capture.last_region = Some(out.rect);
            self.shared.config_dirty = true;
        }
        workers::finish_capture(&self.env(), out);
    }

    fn open_history(&mut self) {
        if self.history_win.is_some() {
            Self::focus_window("StinkShot History");
            return;
        }
        self.history_win = Some(Slot::new(HistoryWindow::new(&self.shared)));
        self.touch();
    }

    fn open_settings(&mut self) {
        if self.settings.is_some() {
            Self::focus_window("StinkShot Settings");
            return;
        }
        let mut w = SettingsWindow::new(&self.shared);
        w.set_hotkey_errors(self.hotkey_errors.clone());
        self.settings = Some(Slot::new(w));
        self.touch();
    }

    fn quit(&mut self) {
        if self.quitting {
            return;
        }
        self.quitting = true;
        log::info!("quitting");
        if let Rec::Active { .. } = &self.rec
            && let Rec::Active { rec, .. } = std::mem::replace(&mut self.rec, Rec::Idle)
        {
            // Finish the file so a recording in progress is not lost; the temp file stays.
            std::thread::spawn(move || {
                if let Err(e) = rec.stop() {
                    log::warn!("stop on quit failed: {e:#}");
                }
            });
        }
        if let Some(s) = &self.scroll {
            s.control.store(SCROLL_CANCEL, Ordering::Relaxed);
        }
        self.hotkeys.shutdown();
        self.tray = None;
        self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    // ------------------------------------------------------- region follow-ups

    fn region_chosen(&mut self, purpose: RegionPurpose, rect: PxRect, source: SourceInfo) {
        match purpose {
            RegionPurpose::Ocr => {
                let cached = self.freeze_cache.take();
                match cached {
                    Some(f) => {
                        let img = capture_ops::crop_frozen(&f, rect);
                        if img.width == 0 || img.height == 0 {
                            self.notify("No text found", "The selection was empty.", false);
                        } else {
                            workers::ocr_image(&self.env(), img);
                        }
                    }
                    None => self.notify(
                        "Could not read the selection",
                        "The screen snapshot was lost; try again.",
                        true,
                    ),
                }
            }
            RegionPurpose::Scrolling => {
                if rect.w < 16 || rect.h < 16 {
                    self.notify(
                        "Selection too small",
                        "Select a larger area to scroll-capture.",
                        false,
                    );
                    return;
                }
                let control = Arc::new(AtomicU8::new(SCROLL_RUN));
                self.scroll = Some(ScrollFlow {
                    control: control.clone(),
                });
                self.scroll_hud = Some(Slot::new(ScrollHud::new(rect)));
                workers::scroll_capture(&self.env(), rect, source, control);
                self.touch();
            }
            RegionPurpose::RecordMp4 | RegionPurpose::RecordGif => {
                if rect.w < 16 || rect.h < 16 {
                    self.notify(
                        "Selection too small",
                        "Select a larger area to record.",
                        false,
                    );
                    return;
                }
                let pending = PendingRecord {
                    rect,
                    gif: purpose == RegionPurpose::RecordGif,
                };
                let secs = self.shared.config.recording.countdown_seconds;
                if secs > 0 {
                    self.countdown = Some(Slot::new(Countdown::new(secs, rect)));
                    self.rec = Rec::Countdown(pending);
                    self.touch();
                } else {
                    self.begin_recording(pending);
                }
            }
        }
    }

    // ------------------------------------------------------------------ recording

    /// Hotkey/CLI/tray "record": start selecting, or stop/cancel the running one.
    fn record_command(&mut self, gif: bool) {
        match &self.rec {
            Rec::Idle => {
                if self.overlay.is_some() || self.window_capture_pending || self.scroll.is_some() {
                    return;
                }
                let purpose = if gif {
                    RegionPurpose::RecordGif
                } else {
                    RegionPurpose::RecordMp4
                };
                self.begin_overlay(OverlayMode::SelectRegion(purpose));
            }
            Rec::Countdown(_) => self.record_cancel(),
            Rec::Active { .. } => self.record_stop(),
            _ => self.notify(
                "Recording in progress",
                "Finish the current recording first.",
                false,
            ),
        }
    }

    fn begin_recording(&mut self, pending: PendingRecord) {
        let rc = self.shared.config.recording.clone();
        let output = match workers::temp_recording_path(&self.shared.history) {
            Ok(p) => p,
            Err(e) => {
                self.notify("Could not start recording", format!("{e:#}"), true);
                self.rec = Rec::Idle;
                return;
            }
        };
        let opts = RecordOptions {
            region: pending.rect,
            output,
            fps: rc.fps.clamp(5, 120),
            bitrate_kbps: rc.bitrate_kbps.max(500),
            system_audio: rc.system_audio,
            microphone: rc.microphone,
            mic_device: (!rc.mic_device.is_empty()).then(|| rc.mic_device.clone()),
            webcam: rc.webcam.then(|| WebcamOptions {
                device: (!rc.webcam_device.is_empty()).then(|| rc.webcam_device.clone()),
                diameter: rc.webcam_diameter.clamp(64, 480),
                corner: Corner::BottomRight,
            }),
            show_clicks: rc.show_clicks,
            show_keys: rc.show_keys,
            include_cursor: self.shared.config.capture.include_cursor,
            accent: self.shared.accent,
        };
        self.rec = Rec::Starting {
            pending,
            cancel: false,
        };
        workers::start_recorder(&self.env(), opts);
    }

    fn record_summary(&self, gif: bool) -> String {
        let rc = &self.shared.config.recording;
        let audio = match (rc.microphone, rc.system_audio) {
            (true, true) => "mic + system",
            (true, false) => "mic",
            (false, true) => "system audio",
            (false, false) => "no audio",
        };
        format!(
            "{} \u{b7} H.264 \u{b7} {} fps \u{b7} {}",
            if gif { "GIF" } else { "MP4" },
            rc.fps,
            audio
        )
    }

    fn record_stop(&mut self) {
        if !matches!(self.rec, Rec::Active { .. }) {
            return;
        }
        if let Rec::Active { rec, pending } = std::mem::replace(&mut self.rec, Rec::Idle) {
            self.record_frame = None;
            self.record_hud = None;
            self.rec = Rec::Stopping(pending);
            workers::stop_recorder(&self.env(), *rec);
            self.touch();
        }
    }

    fn record_cancel(&mut self) {
        match std::mem::replace(&mut self.rec, Rec::Idle) {
            Rec::Countdown(_) => {
                self.countdown = None;
                self.touch();
            }
            Rec::Starting { pending, .. } => {
                self.rec = Rec::Starting {
                    pending,
                    cancel: true,
                };
            }
            Rec::Active { rec, .. } => {
                self.record_frame = None;
                self.record_hud = None;
                workers::cancel_recorder(&self.env(), *rec);
                self.touch();
            }
            Rec::Trim { source, .. } => {
                // The Trim window's Discard: drop the raw recording.
                if let Err(e) = std::fs::remove_file(&source) {
                    log::warn!("discard {}: {e}", source.display());
                }
                self.trim = None;
                self.touch();
            }
            other => self.rec = other,
        }
    }

    fn trim_done(&mut self, start: Duration, end: Duration, as_gif: bool, as_mp4: bool) {
        let Rec::Trim {
            source,
            done_issued,
        } = &mut self.rec
        else {
            return;
        };
        let source = source.clone();
        *done_issued = true;
        if !as_gif && !as_mp4 {
            if let Err(e) = std::fs::remove_file(&source) {
                log::warn!("discard {}: {e}", source.display());
            }
            self.rec = Rec::Idle;
            self.trim = None;
            self.touch();
            return;
        }
        self.exporting = true;
        if let Some(t) = &mut self.trim {
            t.s.set_progress(Some(0.0));
        }
        workers::export_recording(&self.env(), source, start, end, as_gif, as_mp4);
    }

    fn tick_recording(&mut self) {
        // Countdown -> start.
        if let Rec::Countdown(p) = &self.rec {
            let done = self
                .countdown
                .as_ref()
                .map(|c| c.s.finished())
                .unwrap_or(true);
            if done {
                let pending = p.clone();
                self.countdown = None;
                self.touch();
                self.begin_recording(pending);
            } else {
                wake::wake_after(Duration::from_millis(100));
            }
        }
        // Live recorder: status, toggles.
        let mut stop_with_error: Option<String> = None;
        if let Rec::Active { rec, .. } = &self.rec {
            let status = rec.status();
            if let Some(err) = status.error.clone() {
                stop_with_error = Some(err);
            }
            if let Some(hud) = &mut self.record_hud {
                hud.s.update_status(status);
                if let Some(t) = hud.s.take_toggle_changes() {
                    rec.set_mic_muted(!t.mic);
                    rec.set_system_muted(!t.system);
                    rec.set_webcam_visible(t.webcam);
                    rec.set_overlays(t.overlays, t.overlays);
                }
            }
            wake::wake_after(Duration::from_millis(200));
        }
        if let Some(err) = stop_with_error {
            log::error!("recorder reported an error: {err}");
            self.notify(
                "Recording problem",
                format!("{err}\nStopping and keeping what was recorded."),
                true,
            );
            self.record_stop();
        }
        // Trim window closed without a choice: keep the untrimmed recording.
        if self.trim_closed_by_user {
            self.trim_closed_by_user = false;
            if let Rec::Trim {
                source,
                done_issued: false,
            } = &self.rec
                && !self.exporting
            {
                let source = source.clone();
                self.exporting = true;
                workers::export_recording(
                    &self.env(),
                    source,
                    Duration::ZERO,
                    Duration::ZERO,
                    false,
                    true,
                );
            }
        }
    }

    // ------------------------------------------------------------------- workers

    fn handle_worker_msg(&mut self, msg: WorkerMsg) {
        match msg {
            WorkerMsg::Cmd(cmd) => self.pending.push_back(cmd),
            WorkerMsg::HistoryChanged => self.invalidate_history(),
            WorkerMsg::CaptureSaved(saved) => self.capture_saved(*saved),
            WorkerMsg::WindowCaptured {
                frozen,
                windows,
                result,
            } => {
                self.window_capture_pending = false;
                match result {
                    Ok((base, source)) => {
                        let doc = Doc::new(base.width, base.height);
                        self.open_overlay(
                            OverlayMode::Annotate {
                                base,
                                doc,
                                mode: "window",
                                editing: None,
                                source,
                            },
                            frozen,
                            windows,
                        );
                    }
                    Err(e) => self.notify("Window capture failed", e, true),
                }
            }
            WorkerMsg::OpenAnnotate {
                base,
                doc,
                mode,
                editing,
                source,
            } => {
                if self.overlay.is_some() {
                    return;
                }
                if let Some((frozen, windows)) = self.freeze() {
                    self.open_overlay(
                        OverlayMode::Annotate {
                            base,
                            doc,
                            mode,
                            editing,
                            source,
                        },
                        frozen,
                        windows,
                    );
                }
            }
            WorkerMsg::PinReady { image, title, item } => {
                self.add_pin(image, title, Some(item), None)
            }
            WorkerMsg::ScrollProgress {
                frames,
                width,
                height,
            } => {
                if let Some(h) = &mut self.scroll_hud {
                    h.s.update(frames, width, height);
                }
                self.touch();
            }
            WorkerMsg::ScrollDone {
                result,
                cancelled,
                partial,
                source,
            } => {
                self.scroll = None;
                self.scroll_hud = None;
                self.touch();
                if cancelled {
                    return;
                }
                match result {
                    Ok(img) => {
                        if partial {
                            self.notify(
                                "Scrolling capture stopped early",
                                "The page could not be matched any further; kept what was captured.",
                                false,
                            );
                        }
                        let doc = Doc::new(img.width, img.height);
                        if let Some((frozen, windows)) = self.freeze() {
                            self.open_overlay(
                                OverlayMode::Annotate {
                                    base: img,
                                    doc,
                                    mode: "scrolling",
                                    editing: None,
                                    source,
                                },
                                frozen,
                                windows,
                            );
                        }
                    }
                    Err(e) => self.notify("Scrolling capture failed", e, true),
                }
            }
            WorkerMsg::RecorderStarted(result) => self.recorder_started(result),
            WorkerMsg::RecordStopped(result) => {
                match (std::mem::replace(&mut self.rec, Rec::Idle), result) {
                    (Rec::Stopping(pending), Ok((res, info))) => {
                        log::info!(
                            "recording stopped: {} ({:?}, {})",
                            res.path.display(),
                            res.duration,
                            res.encoder
                        );
                        self.trim = Some(Slot::new(TrimWindow::new(
                            res.path.clone(),
                            info,
                            pending.gif,
                        )));
                        self.rec = Rec::Trim {
                            source: res.path,
                            done_issued: false,
                        };
                        self.touch();
                    }
                    (_, Err(e)) => self.notify("Could not finish the recording", e, true),
                    (other, Ok(_)) => self.rec = other,
                }
            }
            WorkerMsg::ExportProgress(p) => {
                if let Some(t) = &mut self.trim {
                    t.s.set_progress(Some(p));
                }
                self.touch();
            }
            WorkerMsg::Exported(x) => self.exported(*x),
            WorkerMsg::UpdateChecked(result) => self.update_checked(result),
        }
    }

    fn recorder_started(&mut self, result: Result<Recorder, String>) {
        let Rec::Starting { pending, cancel } = std::mem::replace(&mut self.rec, Rec::Idle) else {
            if let Ok(rec) = result {
                workers::cancel_recorder(&self.env(), rec);
            }
            return;
        };
        match result {
            Ok(rec) if cancel => workers::cancel_recorder(&self.env(), rec),
            Ok(rec) => {
                let rc = &self.shared.config.recording;
                let toggles = HudToggles {
                    mic: rc.microphone,
                    system: rc.system_audio,
                    webcam: rc.webcam,
                    overlays: rc.show_clicks || rc.show_keys,
                };
                let summary = self.record_summary(pending.gif);
                self.record_frame = Some(Slot::new(RecordFrame::new(pending.rect)));
                self.record_hud = Some(Slot::new(RecordHud::new(pending.rect, summary, toggles)));
                self.rec = Rec::Active {
                    rec: Box::new(rec),
                    pending,
                };
                self.touch();
            }
            Err(e) => {
                log::error!("recorder start failed: {e}");
                if !cancel {
                    self.notify("Could not start recording", e, true);
                }
            }
        }
    }

    fn exported(&mut self, x: workers::Exported) {
        self.exporting = false;
        self.trim = None;
        self.rec = Rec::Idle;
        self.touch();
        if let Some(err) = &x.error {
            self.notify("Export problem", err.clone(), true);
        }
        let Some((id, path, kind)) = x.items.first().cloned() else {
            return;
        };
        if !self.shared.config.general.show_toast {
            return;
        }
        let gif_only = x.items.iter().all(|(_, _, k)| *k == ItemKind::Gif);
        let title = if x.items.len() > 1 {
            "MP4 and GIF saved".to_string()
        } else if gif_only {
            "GIF saved".to_string()
        } else {
            "Recording saved".to_string()
        };
        let body = format!(
            "{} \u{b7} {} x {} \u{b7} {}",
            fmt_duration(x.duration),
            x.width,
            x.height,
            fmt_bytes(x.bytes)
        );
        let _ = (path, kind);
        self.push_toast(ToastKind::Recording {
            thumb: x.thumb.map(|t| t.thumbnail(360, 220)),
            title,
            body,
            item: Some(id),
        });
    }

    fn capture_saved(&mut self, s: workers::CaptureSaved) {
        if s.action == FinishAction::Pin
            && let Some(img) = s.pin_image
        {
            let title = s
                .path
                .as_ref()
                .and_then(|p| p.file_stem())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Pinned capture".into());
            self.add_pin(img, title, s.item, Some(s.rect));
        }
        if !self.shared.config.general.show_toast {
            return;
        }
        let detail = if !s.source.app.is_empty() {
            s.source.app.clone()
        } else {
            s.mode.to_string()
        };
        let (title, body) = if let Some(text) = &s.copied_text {
            ("Text copied".to_string(), workers::preview(text, 120))
        } else if s.action == FinishAction::CopyText {
            return; // the worker already explained why nothing was copied
        } else {
            let title = if let Some(p) = &s.saved_as {
                format!(
                    "Saved {}",
                    p.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                )
            } else if s.editing {
                "Capture updated".to_string()
            } else if s.action == FinishAction::Pin {
                "Pinned to screen".to_string()
            } else if s.copied_image {
                "Copied to clipboard".to_string()
            } else {
                "Saved to history".to_string()
            };
            (
                title,
                format!("{} x {} \u{b7} {}", s.width, s.height, detail),
            )
        };
        self.push_toast(ToastKind::Capture {
            thumb: s.thumb,
            title,
            body,
            item: s.item,
            can_edit: s.item.is_some(),
        });
    }

    // -------------------------------------------------------------------- updates

    fn maybe_check_updates(&mut self, force: bool) {
        if !self.shared.config.privacy.check_for_updates {
            if force {
                self.notify(
                    "Update checks are off",
                    "Turn them on in Settings > Privacy. They are the only network feature.",
                    false,
                );
            }
            return;
        }
        if self.update_inflight || (!force && Instant::now() < self.update_retry_at) {
            return;
        }
        let due = match self.shared.config.privacy.last_update_check {
            Some(t) => chrono::Utc::now().signed_duration_since(t) >= chrono::Duration::hours(24),
            None => true,
        };
        if !due && !force {
            return;
        }
        self.update_inflight = true;
        workers::check_updates(self.worker_tx.clone());
    }

    fn update_checked(&mut self, result: Result<Option<Release>, String>) {
        self.update_inflight = false;
        match result {
            Ok(release) => {
                self.shared.config.privacy.last_update_check = Some(chrono::Utc::now());
                self.shared.config_dirty = true;
                if let Some(r) = release {
                    let first = self
                        .update_release
                        .as_ref()
                        .map(|u| u.tag != r.tag)
                        .unwrap_or(true);
                    if first {
                        self.notify(
                            &format!("StinkShot {} is available", r.tag),
                            "Open the tray menu to see the release page.",
                            false,
                        );
                    }
                    if let Some(t) = &mut self.tray {
                        t.set_update(Some(&r));
                    }
                    self.update_release = Some(r);
                }
            }
            Err(e) => {
                log::info!("update check failed: {e}");
                self.update_retry_at = Instant::now() + Duration::from_secs(3600);
            }
        }
    }

    // ------------------------------------------------------------- config apply

    fn apply_config(&mut self) {
        if !self.shared.config_dirty {
            return;
        }
        self.shared.config_dirty = false;
        let new = self.shared.config.clone();
        if let Err(e) = new.save() {
            log::error!("saving config failed: {e:#}");
            self.notify("Could not save settings", format!("{e:#}"), true);
        }
        let old = std::mem::replace(&mut self.applied, new.clone());

        if old.general.theme != new.general.theme || old.general.accent != new.general.accent {
            self.refresh_theme(true);
        }
        if old.general.start_with_windows != new.general.start_with_windows {
            self.apply_autostart();
        }
        if old.hotkeys != new.hotkeys {
            self.hotkey_errors = self.hotkeys.reload(&new.hotkeys);
            if let Some(s) = &mut self.settings {
                s.s.set_hotkey_errors(self.hotkey_errors.clone());
            }
            if let Some(t) = &mut self.tray {
                t.update_labels(&new);
            }
        } else if (old.general.accent != new.general.accent
            || old.privacy.check_for_updates != new.privacy.check_for_updates)
            && let Some(t) = &mut self.tray
        {
            t.update_labels(&new);
        }
        if old.history.folder != new.history.folder {
            let dir = new.history_folder();
            match History::open(&dir) {
                Ok(h) => {
                    self.shared.history = Arc::new(h);
                    let _ = self.bg_kick.send(BgKick::Full);
                    self.invalidate_history();
                }
                Err(e) => {
                    log::error!("cannot open history folder {}: {e:#}", dir.display());
                    self.notify("Could not open that history folder", format!("{e:#}"), true);
                    self.shared.config.history.folder = old.history.folder.clone();
                    self.applied.history.folder = old.history.folder.clone();
                    self.shared.config_dirty = true;
                }
            }
        }
        if let Ok(mut g) = self.bg_cfg.lock() {
            *g = BgConfig::from(&new, self.shared.history.clone(), self.shared.ocr.clone());
        }
        if old.history.ocr_index != new.history.ocr_index
            || old.history.keep_days != new.history.keep_days
            || old.history.max_gb != new.history.max_gb
        {
            let _ = self.bg_kick.send(BgKick::Full);
        }
        if !old.privacy.check_for_updates && new.privacy.check_for_updates {
            self.maybe_check_updates(false);
        }
        if old.privacy.check_for_updates && !new.privacy.check_for_updates {
            self.update_release = None;
            if let Some(t) = &mut self.tray {
                t.set_update(None);
            }
        }
    }

    fn refresh_theme(&mut self, force: bool) {
        let resolved = resolve_theme(&self.shared.config);
        if force || resolved != self.resolved_theme {
            self.resolved_theme = resolved;
            theme::install(
                &self.ctx,
                &self.shared.fonts,
                resolved.0,
                accent_color(resolved.1),
            );
            self.touch();
        }
        self.shared.dark = resolved.0;
        self.shared.accent = resolved.1;
    }

    // ----------------------------------------------------------------- frame steps

    fn setup_root_window(&mut self) {
        if self.root_setup_tries >= 200 {
            return;
        }
        self.root_setup_tries += 1;
        if let Some(h) = window_util::find_own_window(ROOT_TITLE) {
            let h: Hwnd = h;
            window_util::set_tool_window(h, true);
            window_util::exclude_from_capture(h, true);
            self.root_setup_tries = 200;
        }
    }

    fn poll_inputs(&mut self) {
        if let Some(t) = &mut self.tray {
            self.pending.extend(t.poll());
        }
        self.pending.extend(self.hotkeys.poll());
        while let Ok(args) = self.ipc_rx.try_recv() {
            match cli::command_from_args(&args) {
                Some(cmd) => self.pending.push_back(cmd),
                None => log::debug!("ignored forwarded args {args:?}"),
            }
        }
        while let Ok(msg) = self.worker_rx.try_recv() {
            self.handle_worker_msg(msg);
        }
    }

    fn process_pending(&mut self) {
        let mut guard = 0;
        while let Some(cmd) = self.pending.pop_front() {
            self.handle(cmd);
            guard += 1;
            if guard > 200 {
                log::error!(
                    "command loop did not settle; dropping {} commands",
                    self.pending.len()
                );
                self.pending.clear();
                break;
            }
        }
    }

    fn housekeeping(&mut self) {
        if self.last_theme_check.elapsed() >= Duration::from_secs(3)
            && self.shared.config.general.theme == "system"
        {
            self.last_theme_check = Instant::now();
            self.refresh_theme(false);
        }
        if self.last_housekeeping.elapsed() >= Duration::from_secs(3600) {
            self.last_housekeeping = Instant::now();
            self.maybe_check_updates(false);
        }
        self.ctx.request_repaint_after(Duration::from_secs(60));
    }

    fn show_all(&mut self, ctx: &egui::Context) {
        let shared = &mut self.shared;
        let mut closed = false;
        closed |= drive_opt(ctx, shared, &mut self.overlay, true);
        closed |= drive_opt(ctx, shared, &mut self.history_win, true);
        closed |= drive_opt(ctx, shared, &mut self.settings, true);
        closed |= drive_opt(ctx, shared, &mut self.countdown, false);
        closed |= drive_opt(ctx, shared, &mut self.record_frame, false);
        closed |= drive_opt(ctx, shared, &mut self.record_hud, false);
        closed |= drive_opt(ctx, shared, &mut self.scroll_hud, false);
        if drive_opt(ctx, shared, &mut self.trim, true) {
            self.trim_closed_by_user = true;
            closed = true;
        }
        let before = self.toasts.len();
        self.toasts.retain_mut(|t| {
            drive(ctx, shared, &mut t.slot.s, &mut t.slot.st, false) == SurfaceStatus::Open
        });
        closed |= self.toasts.len() != before;
        let before = self.pins.len();
        self.pins
            .retain_mut(|p| drive(ctx, shared, &mut p.s, &mut p.st, false) == SurfaceStatus::Open);
        closed |= self.pins.len() != before;
        if closed {
            // One more pass so egui destroys the windows of surfaces that just closed.
            self.touch();
        }
    }

    fn drain_outbox(&mut self) {
        if !self.shared.outbox.is_empty() {
            let drained: Vec<Command> = self.shared.outbox.drain(..).collect();
            self.pending.extend(drained);
        }
    }
}

impl eframe::App for Shell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.setup_root_window();
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        self.refresh_theme(false);
        self.poll_inputs();
        self.process_pending();
        self.tick_recording();
        self.process_pending();
        self.apply_config();
        self.housekeeping();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.show_all(&ctx);
        self.drain_outbox();
        self.process_pending();
        self.tick_recording();
        self.process_pending();
        self.apply_config();
        if self.needs_repaint || !self.pending.is_empty() {
            self.needs_repaint = false;
            ctx.request_repaint();
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn on_exit(&mut self) {
        self.hotkeys.shutdown();
        self.tray = None;
        log::info!("StinkShot exited");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_and_bytes_format() {
        assert_eq!(fmt_duration(Duration::from_secs(5)), "0:05");
        assert_eq!(fmt_duration(Duration::from_secs(125)), "2:05");
        assert_eq!(fmt_duration(Duration::from_secs(3725)), "1:02:05");
        assert_eq!(fmt_bytes(512), "1 KB");
        assert_eq!(fmt_bytes(5 * 1024 * 1024), "5.0 MB");
    }
}
