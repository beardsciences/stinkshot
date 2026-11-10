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
use stinkshot_platform::quick_overlay::{self, QuickOverlay};
use stinkshot_platform::system::{self, InstanceGuard};
use stinkshot_platform::{Hwnd, window_util, windows_enum};
use stinkshot_record::{Corner, RecordOptions, Recorder, WebcamOptions};

use crate::capture_ops;
use crate::cli::{self, CliAction};
use crate::errors::{self, report, report_str, report_str_with, report_with};
use crate::history_ui::HistoryWindow;
use crate::hotkeys::Hotkeys;
use crate::l10n::tr;
use crate::overlay::{EditorWindow, Overlay};
use crate::pin::{Pin, PinOpts};
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
use crate::{logger, sysutil, timing, wake};

#[cfg(feature = "soak")]
pub(crate) mod soak;

const ROOT_TITLE: &str = "StinkShot.Root";
const MAX_TOASTS: usize = 6;
/// How long Quit waits for recordings, exports and saves in flight before closing anyway.
const QUIT_GRACE: Duration = Duration::from_secs(30);
/// How long after a tray click a double click may still cancel the overlay it opened.
const TRAY_DOUBLE_CLICK_GRACE: Duration = Duration::from_millis(900);
/// How long after startup (or after a capture or recording) the About logo's loop may be
/// rendered into its disk cache.
const LOGO_PREWARM_DELAY: Duration = Duration::from_secs(30);

/// The system display scale (the primary monitor's at sign-in), for work done before any
/// window is on screen.
fn system_scale() -> f32 {
    // SAFETY: no arguments; returns the system DPI.
    let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForSystem() };
    if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 }
}

/// After a double click is handled, tray single clicks are ignored this long: the button-up of
/// the second click can still reach the tray icon once the overlay under it is gone.
const TRAY_CLICK_QUIET: Duration = Duration::from_millis(700);
/// How long the capture session keeps its shadow textures after the last capture.
const SHADOW_IDLE: Duration = Duration::from_secs(30);
/// Hand the working set back after this long with nothing open (`trim_when_idle`).
/// `STINKSHOT_TRIM_IDLE_SECS` overrides it (0 turns trimming off), for measurements.
const TRIM_IDLE: Duration = Duration::from_secs(60);
/// How long a restart after a GPU device loss waits for jobs in flight (`wait_for_jobs`).
const GPU_RESTART_GRACE: Duration = Duration::from_secs(15);

/// Block until worker jobs in flight (saves, exports, a recording's stop) have finished or
/// `max` has passed. For exits that bypass `quit` (the GPU-loss restart).
fn wait_for_jobs(max: Duration) {
    let deadline = Instant::now() + max;
    loop {
        let busy = workers::jobs_in_flight();
        if busy == 0 {
            return;
        }
        if Instant::now() >= deadline {
            log::warn!("restarting with {busy} job(s) still running");
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

// ---------------------------------------------------------------------------- entry

pub fn run() -> anyhow::Result<()> {
    timing::mark_start();
    let pre_main = timing::since_process_creation_ms();
    if std::env::args().len() > 1 {
        sysutil::attach_parent_console();
    }
    stinkshot_platform::dpi::init();
    // Before any worker touches WinRT: cached factories must never outlive the MTA.
    stinkshot_platform::keep_mta_alive();
    logger::init();
    timing::point(format_args!(
        "logger ready (process created {pre_main:.1} ms before main)"
    ));

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
        CliAction::QuitRunning => return quit_running(),
    };

    // A restart after a GPU device loss waits for the old process to let go of the mutex.
    crate::gpu::wait_for_predecessor();
    let Some(guard) = system::acquire_single_instance() else {
        timing::point(format_args!("another instance holds the mutex; forwarding"));
        if forward_args.is_empty() {
            return Ok(());
        }
        for attempt in 0..30 {
            if system::send_to_running_instance(&forward_args) {
                timing::point(format_args!(
                    "forwarded to the running instance (attempt {attempt})"
                ));
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        log::error!("another instance owns the mutex but did not answer IPC");
        eprintln!("stinkshot: another instance is running but did not respond");
        std::process::exit(1);
    };

    // A plain launch (not `--tray`, no subcommand) that started a fresh instance: the user
    // clicked the exe and would otherwise see nothing at all.
    timing::point(format_args!("single-instance mutex acquired"));
    // Answer forwarded commands from now on: a launch right after this one must not wait for
    // fonts, history, OCR and wgpu (about 1 s, more on a cold start) and give up after its 3 s
    // of retries. Commands queue in the channel until the shell's first frame drains them.
    let ipc_rx = start_ipc();
    // The GPU device (driver load, about 0.5 s) is made on a worker while the tray, hotkeys,
    // fonts and history come up on this thread.
    let gpu = crate::gpu::start();
    // The first freeze would otherwise create the DXGI duplication session itself (device,
    // duplication and a GDI seed per output: 0.3 to 2 s). Do it now, beside eframe start-up.
    let _ = std::thread::Builder::new()
        .name("capture-warmup".into())
        .spawn(|| {
            capture::warm_up();
            timing::point(format_args!("capture session warmed up"));
        });
    start_capture_rewarm();
    let welcome = initial.is_none() && !forward_args.is_empty();
    match run_app(initial, welcome, guard, ipc_rx, gpu) {
        Ok(()) => Ok(()),
        Err(e) => {
            log::error!("fatal: {e:#}");
            let why = errors::explain(&e, tr!("shell.start.retry"));
            sysutil::fatal_box("Stinkshot", &tr!("shell.start.failed", why = why));
            Err(e)
        }
    }
}

/// `stinkshot quit`: ask the instance of this profile to exit, then wait until its mutex is
/// free (it finishes recordings and saves first, up to `QUIT_GRACE`). Exit code 0 when no
/// instance is left, 1 when it did not answer or did not exit. The installer runs this before
/// replacing or removing the exe, so a quit restores PrintScreen like a normal exit does.
fn quit_running() -> anyhow::Result<()> {
    if let Some(guard) = system::acquire_single_instance() {
        drop(guard);
        return Ok(());
    }
    let process = sysutil::running_instance();
    let args = vec!["quit".to_string()];
    if !(0..30).any(|_| {
        system::send_to_running_instance(&args) || {
            std::thread::sleep(Duration::from_millis(100));
            false
        }
    }) {
        eprintln!("stinkshot: the running instance did not respond");
        std::process::exit(1);
    }
    let deadline = Instant::now() + QUIT_GRACE + Duration::from_secs(5);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
        if let Some(guard) = system::acquire_single_instance() {
            drop(guard);
            // The mutex goes before the process has unloaded (GPU and COM teardown), and the
            // exe stays locked until it has.
            let left = deadline.saturating_duration_since(Instant::now());
            if process.as_ref().is_some_and(|p| !p.wait(left)) {
                break;
            }
            log::info!("quit: the running instance exited");
            return Ok(());
        }
    }
    eprintln!("stinkshot: the running instance did not exit");
    std::process::exit(1);
}

/// Warm the capture session up again after events that invalidate it (unlock, display change,
/// resume), and retry hotkeys that failed to register on unlock and resume. Without the
/// warm-up the first PrtSc after any of them pays the cold session build (about 0.8 s) that
/// the start-up warm-up removed. Bursts (a display change fires several
/// messages, resume is followed by unlock) are coalesced, and the warm-up waits a moment so
/// the desktop has settled.
fn start_capture_rewarm() {
    const SETTLE: Duration = Duration::from_millis(1500);
    let _ = std::thread::Builder::new()
        .name("capture-rewarm".into())
        .spawn(move || {
            // Created here so start-up does not wait for the watcher window.
            let (tx, rx) = channel::<system::SessionEvent>();
            if let Err(e) = system::start_session_watcher(tx) {
                log::warn!("capture re-warm-up unavailable: {e:#}");
                return;
            }
            while let Ok(first) = rx.recv() {
                // A program that held one of our hotkeys may have quit meanwhile.
                if matches!(
                    first,
                    system::SessionEvent::Unlock | system::SessionEvent::Resume
                ) {
                    crate::hotkeys::request_retry();
                }
                std::thread::sleep(SETTLE);
                // Anything that arrived meanwhile is covered by this warm-up.
                while rx.try_recv().is_ok() {}
                log::debug!("capture re-warm-up after {first:?}");
                capture::warm_up();
            }
        });
}

/// Start the IPC listener (message-only window on its own thread) and a bridge that wakes the
/// UI once it exists. Before the shell registers its context `wake()` is a no-op; the shell
/// drains anything queued on its first frame.
fn start_ipc() -> Receiver<Vec<String>> {
    let (ipc_tx, ipc_inner_rx) = channel::<Vec<String>>();
    let (ipc_fwd_tx, ipc_rx) = channel::<Vec<String>>();
    match system::start_ipc_listener(ipc_tx) {
        Ok(()) => {
            let bridge = std::thread::Builder::new()
                .name("ipc-bridge".into())
                .spawn(move || {
                    while let Ok(args) = ipc_inner_rx.recv() {
                        timing::point(format_args!("ipc: received {args:?}"));
                        if ipc_fwd_tx.send(args).is_err() {
                            break;
                        }
                        wake::wake();
                    }
                });
            if let Err(e) = bridge {
                log::error!("IPC bridge failed to start: {e}");
            }
        }
        Err(e) => log::error!("IPC listener failed: {e:#}"),
    }
    timing::point(format_args!("ipc listener ready"));
    ipc_rx
}

/// A screen frozen while the app was still starting (`prefreeze_while_gpu_starts`).
struct EarlyFreeze {
    at: Instant,
    frozen: Frozen,
    windows: Vec<windows_enum::WinInfo>,
    /// The frozen screen already on display while eframe starts (region, scrolling, OCR).
    quick: Option<QuickOverlay>,
}

/// How long an early freeze stays usable for the command it was taken for.
const EARLY_FREEZE_MAX_AGE: Duration = Duration::from_secs(3);
/// The quick overlay never stays longer than this, even if the region overlay never shows.
const QUICK_OVERLAY_MAX: Duration = Duration::from_secs(10);

/// Commands whose handler starts with `Shell::freeze`.
fn freezes_first(cmd: &Command) -> bool {
    matches!(
        cmd,
        Command::CaptureRegion
            | Command::CaptureScrolling
            | Command::OcrRegion
            | Command::PickColor
            | Command::CaptureWindow
    )
}

/// Commands that open the region overlay on the frozen screen.
fn opens_region_overlay(cmd: &Command) -> bool {
    matches!(
        cmd,
        Command::CaptureRegion | Command::CaptureScrolling | Command::OcrRegion
    )
}

/// Whether a freeze draws the pointer into its pixels: per Include Mouse Cursor, except for
/// the editor when Cursor as Object keeps the pointer as a movable object instead.
fn freeze_draws_cursor(editor: bool, cap: &stinkshot_core::config::Capture) -> bool {
    cap.include_cursor && !(editor && cap.cursor_object)
}

/// Freeze the screen for `cmd` while the app is still starting, and for the region overlay
/// put the frozen screen on display at once (`quick_overlay`).
fn early_freeze_for(
    cmd: &Command,
    cap: &stinkshot_core::config::Capture,
    accent: [u8; 3],
) -> Option<EarlyFreeze> {
    let windows = windows_enum::visible_windows();
    let editor = matches!(cmd, Command::CaptureRegion);
    let frozen = match capture::freeze_all(freeze_draws_cursor(editor, cap)) {
        Ok(f) => f,
        Err(e) => {
            log::debug!("early freeze failed: {e:#}");
            return None;
        }
    };
    timing::point(format_args!("early freeze done (GPU still starting)"));
    let quick = opens_region_overlay(cmd)
        .then(|| {
            let rects = windows.iter().map(|w| w.rect).collect();
            quick_overlay::show(&frozen.image, frozen.bounds, rects, accent)
        })
        .and_then(|r| r.map_err(|e| log::warn!("quick overlay: {e:#}")).ok());
    if quick.is_some() {
        timing::point(format_args!("quick overlay shown"));
    }
    Some(EarlyFreeze {
        at: Instant::now(),
        frozen,
        windows,
        quick,
    })
}

/// While the GPU device is still being made the main thread only waits for it. When the
/// launch's own command (`initial`, e.g. `stinkshot region`) or a command forwarded meanwhile
/// starts with a freeze, freeze the screen now: the overlay then opens as soon as eframe is up
/// instead of freezing after it. For the region overlay the frozen screen goes on display at
/// once in a GDI window (`quick_overlay`), which takes the first input until the overlay
/// replaces it. Only the first such command gets the early freeze.
fn prefreeze_while_gpu_starts(
    initial: Option<&Command>,
    ipc_rx: &Receiver<Vec<String>>,
    gpu: Option<&std::thread::JoinHandle<Option<eframe::egui_wgpu::WgpuSetupExisting>>>,
    cap: &stinkshot_core::config::Capture,
    accent: [u8; 3],
) -> (Vec<Vec<String>>, Option<EarlyFreeze>) {
    let mut queued = Vec::new();
    let mut early = None;
    let Some(gpu) = gpu else {
        return (queued, early);
    };
    if let Some(cmd) = initial.filter(|c| freezes_first(c))
        && !gpu.is_finished()
    {
        early = early_freeze_for(cmd, cap, accent);
    }
    while !gpu.is_finished() {
        match ipc_rx.recv_timeout(Duration::from_millis(5)) {
            Ok(args) => {
                // A launch command that freezes owns the early freeze even if that failed: a
                // forwarded one must not take it and leave the launch command unfrozen first.
                let taken = early.is_some() || initial.is_some_and(freezes_first);
                if !taken && let Some(cmd) = cli::command_from_args(&args).filter(freezes_first) {
                    early = early_freeze_for(&cmd, cap, accent);
                }
                queued.push(args);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    (queued, early)
}

struct Boot {
    ipc_rx: Receiver<Vec<String>>,
    /// Forwarded commands taken off `ipc_rx` before eframe started, in order.
    early_args: Vec<Vec<String>>,
    early_freeze: Option<EarlyFreeze>,
    config: Config,
    fonts: Arc<Fonts>,
    history: Arc<History>,
    ocr: Option<Arc<dyn OcrEngine>>,
    initial: Option<Command>,
    welcome: bool,
    guard: InstanceGuard,
    tray: Option<Tray>,
    hotkeys: Hotkeys,
    hotkey_errors: Vec<String>,
}

/// Body of the tr!("shell.welcome.title") toast shown when the user starts the app by hand.
fn welcome_body(region_hotkey: &str) -> String {
    let key = crate::hotkeys::display(region_hotkey);
    if key.is_empty() {
        tr!("shell.welcome.no_key").into()
    } else {
        tr!("shell.welcome.key", key = key)
    }
}

fn run_app(
    initial: Option<Command>,
    welcome: bool,
    guard: InstanceGuard,
    ipc_rx: Receiver<Vec<String>>,
    gpu: Option<std::thread::JoinHandle<Option<eframe::egui_wgpu::WgpuSetupExisting>>>,
) -> anyhow::Result<()> {
    let config = Config::load();
    crate::l10n::apply(&config.general.language);
    timing::point(format_args!("config loaded"));

    // OCR engine creation touches COM; do it on a scratch thread so the UI thread's apartment
    // stays the one winit sets up. Joined after the GPU device.
    let ocr_init = std::thread::Builder::new()
        .name("ocr-init".into())
        .spawn(|| WindowsOcr::new().map(|o| Arc::new(o) as Arc<dyn OcrEngine>))
        .ok();

    // Tray icon and global hotkeys before anything slow: the icon is what tells the user
    // Stinkshot is up, and both only need the config. Their hidden windows live on this thread,
    // so their messages (a click, a hotkey) wait in its queue until the event loop runs and
    // are handled on the first frame, like IPC commands.
    let tray = match Tray::new(&config) {
        Ok(t) => Some(t),
        Err(e) => {
            log::error!("tray icon failed: {e:#}");
            None
        }
    };
    timing::point(format_args!("tray created"));
    let (hotkeys, hotkey_errors) = Hotkeys::new(&config.hotkeys);
    timing::point(format_args!("hotkeys registered"));

    let fonts = Arc::new(Fonts::load_system().context("load system fonts")?);
    timing::point(format_args!("fonts loaded"));
    let history_dir = config.history_folder();
    let history = Arc::new(
        History::open(&history_dir)
            .with_context(|| format!("open history folder {}", history_dir.display()))?,
    );
    timing::point(format_args!("history opened"));
    crate::recompress::clean_up(&history_dir);

    let accent = resolve_theme(&config).1;
    let (early_args, early_freeze) = prefreeze_while_gpu_starts(
        initial.as_ref(),
        &ipc_rx,
        gpu.as_ref(),
        &config.capture,
        [accent[0], accent[1], accent[2]],
    );
    let wgpu_options = crate::gpu::options(gpu);
    timing::point(format_args!("gpu device joined"));
    let ocr: Option<Arc<dyn OcrEngine>> =
        ocr_init.and_then(|h| h.join().ok()).and_then(|r| match r {
            Ok(o) => Some(o),
            Err(e) => {
                log::warn!("OCR unavailable: {e:#}");
                None
            }
        });
    timing::point(format_args!("OCR engine ready"));

    let boot = Boot {
        ipc_rx,
        early_args,
        early_freeze,
        config,
        fonts,
        history,
        ocr,
        initial,
        welcome,
        guard,
        tray,
        hotkeys,
        hotkey_errors,
    };
    let options = eframe::NativeOptions {
        viewport: root_viewport(),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options,
        // `clipboard_in` / `clipboard_out` do this with the app's own clipboard code.
        clipboard_shortcuts: false,
        ..Default::default()
    };
    // A device loss inside a paint ends in a panic in egui-wgpu (`gpu.rs`). winit hands panics
    // back out of its event loop, so it lands here: start a fresh instance instead of dying.
    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        eframe::run_native(
            "Stinkshot",
            options,
            Box::new(move |cc| {
                Ok(Box::new(
                    Shell::new(cc, boot).map_err(|e| format!("{e:#}"))?,
                ))
            }),
        )
    }));
    match run {
        Ok(r) => r.map_err(|e| anyhow!("{e}")),
        Err(payload) if crate::gpu::lost() || crate::gpu::is_device_panic(payload.as_ref()) => {
            // The shell is gone (dropped while unwinding; `Drop for Shell` kept a recording),
            // but saves and exports run on worker threads: let them finish first.
            wait_for_jobs(GPU_RESTART_GRACE);
            crate::gpu::relaunch();
            log::logger().flush();
            std::process::exit(0);
        }
        Err(payload) => std::panic::resume_unwind(payload),
    }
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
        // eframe stamps the root icon onto the first active window; without one it uses
        // the egui logo.
        .with_icon(crate::app_icon::window_icon())
}

// ------------------------------------------------------------------------ surfaces

#[derive(Default)]
struct HwndState {
    created: bool,
    tries: u32,
    /// The window and the DPI its native icon was picked for (re-applied on DPI change).
    icon: Option<(stinkshot_platform::Hwnd, u32)>,
    /// UI Automation provider for screen readers (`a11y.rs`).
    a11y: Option<crate::a11y::Attached>,
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

/// Ctrl+C / Ctrl+X / Ctrl+V as egui's clipboard events, with the text read from the system
/// clipboard. egui-winit's own translation only reaches the system clipboard with its
/// `clipboard` feature (arboard); without it a paste is dropped. Ctrl+V with no text on the
/// clipboard stays a key press (the editor pastes shapes on it).
fn clipboard_in(ctx: &egui::Context) {
    use egui::{Event, Key};
    ctx.input_mut(|i| {
        for ev in &mut i.events {
            let Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = ev
            else {
                continue;
            };
            if !modifiers.command || modifiers.alt {
                continue;
            }
            match key {
                Key::C => *ev = Event::Copy,
                Key::X => *ev = Event::Cut,
                Key::V => {
                    let text = stinkshot_platform::clipboard::get_text()
                        .map(|t| t.replace("\r\n", "\n"))
                        .filter(|t| !t.is_empty());
                    if let Some(text) = text {
                        *ev = Event::Paste(text);
                    }
                }
                _ => {}
            }
        }
    });
}

/// Text that egui widgets copied this frame goes to the system clipboard.
fn clipboard_out(ctx: &egui::Context) {
    let copied: Vec<String> = ctx.output(|o| {
        o.commands
            .iter()
            .filter_map(|c| match c {
                egui::OutputCommand::CopyText(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    });
    if let Some(text) = copied.last()
        && let Err(e) = stinkshot_platform::clipboard::set_text(text)
    {
        log::warn!("copy text: {e:#}");
    }
}

/// One frame of a surface, with the clipboard wired to its text fields.
fn show_surface(ctx: &egui::Context, surf: &mut dyn Surface, shared: &mut Shared) -> SurfaceStatus {
    clipboard_in(ctx);
    let status = surf.show(ctx, shared);
    clipboard_out(ctx);
    status
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
    let builder = surf
        .viewport_builder(shared)
        .with_title(title.clone())
        .with_icon(crate::app_icon::window_icon());
    let mut status = ctx.show_viewport_immediate(id, builder.clone(), |ctx, _class| {
        show_surface(ctx, surf, shared)
    });
    // The frame is presented by now (immediate viewports paint inside the call).
    surf.after_paint();
    if let Some((hwnd, dpi)) = &mut st.icon {
        crate::app_icon::refresh_native(*hwnd, dpi);
    }
    if !st.created && st.tries < 90 {
        st.tries += 1;
        match sysutil::find_own_window(&title) {
            Some(hwnd) => {
                timing::point(format_args!(
                    "{title}: window created ({} frames)",
                    st.tries
                ));
                st.created = true;
                st.icon = Some((hwnd, crate::app_icon::apply_native(hwnd)));
                st.a11y = crate::a11y::attach(ctx, id, hwnd);
                surf.on_created(hwnd, shared);
                if focus {
                    window_util::force_foreground(hwnd);
                }
            }
            None => ctx.request_repaint(),
        }
    }
    let mut extra = 0;
    while status == SurfaceStatus::Open && extra < 3 && surf.wants_frame_now() {
        extra += 1;
        status = ctx.show_viewport_immediate(id, builder.clone(), |ctx, _class| {
            show_surface(ctx, surf, shared)
        });
        surf.after_paint();
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
        /// Started as a GIF recording: the untrimmed fallback saves a GIF.
        gif: bool,
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
    tour: Option<Slot<crate::tour::TourWindow>>,
    countdown: Option<Slot<Countdown>>,
    record_frame: Option<Slot<RecordFrame>>,
    record_hud: Option<Slot<RecordHud>>,
    trim: Option<Slot<TrimWindow>>,
    scroll_hud: Option<Slot<ScrollHud>>,
    toasts: Vec<ToastSlot>,
    pins: Vec<Slot<Pin>>,
    /// Editor Windows (`overlay::EditorWindow`), at most one per history item.
    editors: Vec<Slot<EditorWindow>>,

    /// Snapshot behind an OCR region selection (so the text comes from what the user saw).
    freeze_cache: Option<Frozen>,
    /// A finished capture waiting for the overlay slot to free up (opened by
    /// `open_queued_annotate`, never dropped).
    queued_annotate: Option<OverlayMode>,
    window_capture_pending: bool,
    /// The overlay was opened by a tray click at this time (double-click detection).
    tray_overlay_at: Option<Instant>,
    /// Tray single clicks are ignored until then (`TRAY_CLICK_QUIET`).
    tray_click_quiet_until: Option<Instant>,
    /// Hint-bar notice for the next overlay that opens.
    next_overlay_notice: Option<&'static str>,
    next_overlay_tray: bool,
    rec: Rec,
    exporting: bool,
    trim_closed_by_user: bool,
    scroll: Option<ScrollFlow>,

    resolved_theme: (bool, [u8; 4]),
    /// Windows contrast theme and Text size the theme was last installed with.
    a11y_prefs: crate::a11y::Prefs,
    last_theme_check: Instant,
    update_release: Option<Release>,
    /// Update Now is downloading or the installer was started: ignore further clicks.
    update_installing: bool,
    /// `stinkshot update` is waiting for its check to install what it finds.
    install_after_check: bool,
    update_inflight: bool,
    /// The check in flight was asked for by the user (Check Now): always report the outcome.
    update_forced: bool,
    update_retry_at: Instant,
    last_housekeeping: Instant,
    /// Since when nothing has been open or running (`trim_when_idle`), and whether the
    /// working set was trimmed in this quiet spell.
    quiet_since: Instant,
    trimmed: bool,
    /// When to render the About logo's loop into its disk cache if it has no copy (once,
    /// a while after startup, so the first visit to the About page is instant).
    logo_prewarm_at: Option<Instant>,
    root_setup_tries: u32,
    first_frame_seen: bool,
    needs_repaint: bool,
    quitting: bool,
    /// Set by `quit()`; the window closes when in-flight jobs finish or this passes.
    quit_deadline: Option<Instant>,
    /// Diagnostics (`STINKSHOT_TEST_GPU_LOSS`): the device, and the overlay number whose paint
    /// destroys it.
    test_gpu_loss: Option<(eframe::wgpu::Device, u32)>,
    overlays_opened: u32,
    /// Screen frozen during start-up for the first capture command (`freeze` uses it once).
    early_freeze: Option<EarlyFreeze>,
    /// The start-up freeze's GDI window, shown until the region overlay is up
    /// (`settle_quick_overlay`).
    quick_overlay: Option<(QuickOverlay, Instant)>,
    #[cfg(feature = "soak")]
    soak: soak::SoakState,
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

/// HUD summary line. A GIF has no audio and is written at the GIF frame rate, so it must not
/// claim H.264, the MP4 frame rate or audio sources.
fn record_summary(rc: &stinkshot_core::config::Recording, gif: bool, fps: u32) -> String {
    if gif {
        return tr!(
            "shell.hud.gif",
            fps = rc.gif_fps.max(1),
            w = rc.gif_max_width
        );
    }
    let audio = match (rc.microphone, rc.system_audio) {
        (true, true) => tr!("shell.hud.mic_system"),
        (true, false) => tr!("shell.hud.mic"),
        (false, true) => tr!("shell.hud.system_audio"),
        (false, false) => tr!("shell.hud.no_audio"),
    };
    tr!("shell.hud.mp4", fps = fps, audio = audio)
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
        timing::point(format_args!("eframe up (wgpu initialized), Shell::new"));
        let ctx = cc.egui_ctx.clone();
        wake::set_context(ctx.clone());
        crate::a11y::install(&ctx);

        let Boot {
            ipc_rx,
            early_args,
            early_freeze,
            config,
            fonts,
            history,
            ocr,
            initial,
            welcome,
            guard,
            tray,
            hotkeys,
            hotkey_errors,
        } = boot;
        let resolved_theme = resolve_theme(&config);
        theme::install(
            &ctx,
            &fonts,
            resolved_theme.0,
            accent_color(resolved_theme.1),
        );
        timing::point(format_args!("theme installed"));

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
                update: Default::default(),
                update_check_failed: false,
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
            tour: None,
            countdown: None,
            record_frame: None,
            record_hud: None,
            trim: None,
            scroll_hud: None,
            toasts: Vec::new(),
            pins: Vec::new(),
            editors: Vec::new(),
            freeze_cache: None,
            queued_annotate: None,
            window_capture_pending: false,
            tray_overlay_at: None,
            tray_click_quiet_until: None,
            next_overlay_notice: None,
            next_overlay_tray: false,
            rec: Rec::Idle,
            exporting: false,
            trim_closed_by_user: false,
            scroll: None,
            resolved_theme,
            a11y_prefs: crate::a11y::prefs(),
            last_theme_check: Instant::now(),
            update_release: None,
            update_installing: false,
            install_after_check: false,
            update_inflight: false,
            update_forced: false,
            update_retry_at: Instant::now(),
            last_housekeeping: Instant::now(),
            quiet_since: Instant::now(),
            trimmed: false,
            logo_prewarm_at: Some(Instant::now() + LOGO_PREWARM_DELAY),
            root_setup_tries: 0,
            first_frame_seen: false,
            needs_repaint: false,
            quitting: false,
            quit_deadline: None,
            test_gpu_loss: None,
            overlays_opened: 0,
            early_freeze,
            quick_overlay: None,
            #[cfg(feature = "soak")]
            soak: Default::default(),
            _guard: guard,
        };
        if let Some(rs) = cc.wgpu_render_state.as_ref() {
            crate::gpu::install_handlers(&rs.device);
            shell.test_gpu_loss =
                crate::gpu::test_loss_plan(&rs.device).map(|n| (rs.device.clone(), n));
        }

        // The launch's own command first: it came before anything forwarded, and when it
        // freezes, the start-up freeze was taken for it.
        let launched_with_command = initial.is_some();
        if let Some(cmd) = initial {
            shell.pending.push_back(cmd);
        }
        for args in early_args {
            match cli::command_from_args(&args) {
                Some(cmd) => shell.pending.push_back(cmd),
                None => log::debug!("ignored forwarded args {args:?}"),
            }
        }
        shell.apply_autostart();
        if !shell.hotkey_errors.is_empty() {
            let body = shell.hotkey_errors.join("\n");
            shell.pending.push_back(Command::Notify {
                title: tr!("shell.hotkeys.inactive").into(),
                body,
                error: true,
            });
        }
        if shell.tray.is_none() {
            shell.pending.push_back(Command::Notify {
                title: tr!("shell.tray.unavailable").into(),
                body: tr!("shell.tray.unavailable.body").into(),
                error: true,
            });
        }
        if shell.shared.ocr.is_none() {
            log::info!("no OCR language installed; OCR features will report that");
        }
        if !launched_with_command && welcome {
            let body = welcome_body(&shell.shared.config.hotkeys.region);
            shell.notify(tr!("shell.welcome.title"), body, false);
        }
        // First run: the short tour, once (it sets `general.tour_seen` when closed). Only on a
        // plain launch: not over a capture the launch asked for, and not from `--tray`
        // (autostart, silent upgrades, scripts), which stays silent.
        if welcome && !shell.shared.config.general.tour_seen {
            shell.pending.push_back(Command::OpenTour);
        }
        // Also after `--tray`: a silent upgrade from Update Now relaunches that way.
        shell.note_version();
        shell.maybe_check_updates(false);
        shell.ctx.request_repaint();
        timing::point(format_args!("Shell::new done"));
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
        let pins = &self.shared.config.pins;
        let opts = PinOpts {
            task_view: pins.show_in_task_view,
            always_toolbar: pins.always_show_toolbar,
        };
        self.pins
            .push(Slot::new(Pin::new(image, title, item, at, id, opts)));
        self.touch();
    }

    fn focus_window(title: &str) {
        if let Some(h) = sysutil::find_own_window(title) {
            window_util::force_foreground(h);
        }
    }

    /// Title of the Editor Window editing history item `id`, if one is open.
    fn editor_title(&self, id: ItemId) -> Option<String> {
        self.editors
            .iter()
            .find(|e| e.s.item() == Some(id))
            .map(|e| e.s.title())
    }

    /// Open a capture in a new Editor Window (or focus the one already editing that item).
    /// No screen freeze: the window is a normal one beside the others.
    fn open_editor(&mut self, req: crate::state::EditorRequest) {
        if let Some(id) = req.editing
            && let Some(t) = self.editor_title(id)
        {
            Self::focus_window(&t);
            return;
        }
        let taken: Vec<String> = self.editors.iter().map(|e| e.s.title()).collect();
        let title = crate::overlay::window_title(req.name.as_deref(), &taken);
        self.editors
            .push(Slot::new(EditorWindow::new(req, title, &self.shared)));
        self.touch();
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
        if matches!(
            cmd,
            Command::CaptureRegion
                | Command::CaptureWindow
                | Command::CaptureFullscreen
                | Command::CaptureScrolling
                | Command::OcrRegion
                | Command::RepeatLastRegion
        ) {
            timing::point(format_args!("dispatch {cmd:?}"));
        }
        match cmd {
            Command::CaptureRegion => self.begin_overlay(OverlayMode::Capture),
            Command::CaptureWindow => self.capture_window(),
            Command::CaptureFullscreen => self.capture_fullscreen(),
            Command::CaptureScrolling => {
                self.begin_overlay(OverlayMode::SelectRegion(RegionPurpose::Scrolling))
            }
            Command::OcrRegion => {
                if self.shared.ocr.is_none() {
                    self.notify(tr!("shell.ocr.none"), tr!("shell.ocr.none.body"), true);
                } else {
                    self.begin_overlay(OverlayMode::SelectRegion(RegionPurpose::Ocr));
                }
            }
            Command::RepeatLastRegion => self.repeat_last_region(),
            Command::CaptureSelectedText { paragraph } => self.capture_selected_text(paragraph),
            Command::PickColor => {
                self.begin_overlay(OverlayMode::SelectRegion(RegionPurpose::PickColor))
            }
            Command::RecordMp4 => self.record_command(false),
            Command::RecordGif => self.record_command(true),
            Command::OpenHistory => self.open_history(),
            Command::OpenSettings => self.open_settings(None),
            Command::OpenSettingsAt(section) => self.open_settings(Some(section)),
            Command::OpenTour => self.open_tour(),
            Command::RetryHotkeys => {
                self.hotkeys.retry_now();
                self.tick_hotkeys(true);
            }
            Command::EditItem(id) => {
                // Already open: bring that window up instead of loading it again.
                if let Some(t) = self.editor_title(id) {
                    Self::focus_window(&t);
                } else {
                    workers::edit_item(&self.env(), id)
                }
            }
            Command::OpenEditor(req) => self.open_editor(*req),
            Command::PinItem(id) => workers::pin_item(&self.env(), id),
            Command::CaptureDone(out) => self.capture_done(*out),
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
            Command::RevealItem(id) => workers::reveal_item(&self.env(), id),
            Command::OpenItem(id) => workers::open_item(&self.env(), id),
            Command::DeleteItem(id) => workers::delete_item(&self.env(), id),
            Command::StarItem(id, starred) => {
                if let Err(e) = self.shared.history.set_starred(id, starred) {
                    let body = report("set starred", &e, tr!("shell.star.failed.body"));
                    self.notify(tr!("shell.star.failed"), body, true);
                }
                self.invalidate_history();
            }
            Command::Notify { title, body, error } => {
                self.push_toast(ToastKind::Message { title, body, error })
            }
            Command::CheckForUpdates => self.maybe_check_updates(true),
            Command::InstallUpdate => self.install_update(),
            Command::UpdateNow => {
                if self.update_release.is_some() {
                    self.install_update();
                } else if self.shared.config.privacy.check_for_updates {
                    self.install_after_check = true;
                    self.maybe_check_updates(true);
                } else {
                    self.maybe_check_updates(true);
                }
            }
            Command::OpenReleasePage => {
                let url = self
                    .update_release
                    .as_ref()
                    .map(|r| r.url.clone())
                    .unwrap_or_else(|| crate::update::DEFAULT_PAGE.to_string());
                self.pending.push_back(Command::OpenUrl(url));
            }
            Command::Quit => self.quit(),
            Command::TrayClick
                if self
                    .tray_click_quiet_until
                    .is_some_and(|t| Instant::now() < t) =>
            {
                log::debug!("tray click ignored: end of a double click");
            }
            Command::TrayClick => {
                let had_overlay = self.overlay.is_some();
                self.next_overlay_tray = true;
                self.begin_overlay(OverlayMode::Capture);
                self.next_overlay_tray = false;
                if !had_overlay && self.overlay.is_some() {
                    self.tray_overlay_at = Some(Instant::now());
                }
            }
            Command::TrayDoubleClick => self.tray_double_click(),
            #[cfg(feature = "soak")]
            Command::Soak(args) => self.soak(args),
            Command::OpenUrl(url) => {
                if !(url.starts_with("https://") || url.starts_with("http://")) {
                    log::warn!("refusing to open non-web URL");
                } else {
                    workers::open_url(&self.env(), url);
                }
            }
        }
    }

    fn invalidate_history(&mut self) {
        if let Some(h) = &mut self.history_win {
            h.s.invalidate();
        }
        self.touch();
    }

    fn capture_blocked(&mut self) -> bool {
        if self.quitting {
            return true;
        }
        if self.overlay.is_some() || self.window_capture_pending || self.scroll.is_some() {
            return true;
        }
        if self.busy_with_recording() {
            self.notify(
                tr!("shell.busy.recording"),
                tr!("shell.busy.recording.capture"),
                false,
            );
            return true;
        }
        false
    }

    /// Why the editor cannot open right now, if it cannot.
    fn annotate_block_reason(&self) -> Option<&'static str> {
        if self.overlay.is_some() {
            Some(tr!("shell.busy.capture_open"))
        } else if self.scroll.is_some() || self.window_capture_pending {
            Some(tr!("shell.busy.capture"))
        } else if self.busy_with_recording() {
            Some(tr!("shell.busy.recording.edit"))
        } else {
            None
        }
    }

    /// Opens a capture that finished while the overlay slot was taken.
    fn open_queued_annotate(&mut self) {
        if self.queued_annotate.is_none() || self.annotate_block_reason().is_some() {
            return;
        }
        let Some(mode) = self.queued_annotate.take() else {
            return;
        };
        if let Some((frozen, windows)) = self.freeze() {
            self.open_overlay(mode, frozen, windows);
        }
    }

    fn freeze(&mut self) -> Option<(Frozen, Vec<stinkshot_platform::windows_enum::WinInfo>)> {
        self.freeze_for(false)
    }

    /// `freeze` for a capture that opens in the editor (`editor`), where Cursor as Object
    /// keeps the pointer out of the pixels.
    fn freeze_for(
        &mut self,
        editor: bool,
    ) -> Option<(Frozen, Vec<stinkshot_platform::windows_enum::WinInfo>)> {
        if let Some(e) = self.early_freeze.take()
            && e.at.elapsed() < EARLY_FREEZE_MAX_AGE
        {
            timing::point(format_args!("freeze: using the start-up freeze"));
            self.quick_overlay = e.quick.map(|q| (q, Instant::now()));
            return Some((e.frozen, e.windows));
        }
        let t0 = Instant::now();
        let windows = windows_enum::visible_windows();
        let t1 = Instant::now();
        let res = capture::freeze_all(freeze_draws_cursor(editor, &self.shared.config.capture));
        timing::point(format_args!(
            "freeze done: windows {:.1} ms, freeze_all {:.1} ms",
            (t1 - t0).as_secs_f64() * 1000.0,
            t1.elapsed().as_secs_f64() * 1000.0
        ));
        match res {
            Ok(f) => Some((f, windows)),
            Err(e) => {
                let body = report("freeze_all", &e, tr!("shell.capture.no_image.body"));
                self.notify(tr!("shell.capture.screen_failed"), body, true);
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
        // Occlusion stack for hover snapping, taken right after the freeze (same frame).
        let layers = if matches!(mode, OverlayMode::Annotate { .. }) {
            Vec::new()
        } else {
            windows_enum::layers()
        };
        let req = OverlayRequest {
            mode,
            frozen,
            windows,
            layers,
            last_region: self.shared.config.capture.last_region,
            notice: self.next_overlay_notice.take(),
            from_tray_click: std::mem::take(&mut self.next_overlay_tray),
        };
        self.tray_overlay_at = None;
        self.overlay = Some(Slot::new(Overlay::new(req, &self.shared)));
        timing::point(format_args!("overlay constructed (layers + Overlay::new)"));
        self.overlays_opened += 1;
        if let Some((device, n)) = &self.test_gpu_loss
            && *n == self.overlays_opened
        {
            // The overlay paints later in this pass, on the destroyed device.
            log::warn!("test: destroying the GPU device before overlay {n} paints");
            device.destroy();
        }
        self.touch();
    }

    fn begin_overlay(&mut self, mode: OverlayMode) {
        if self.capture_blocked() {
            return;
        }
        let editor = matches!(mode, OverlayMode::Capture);
        let Some((frozen, windows)) = self.freeze_for(editor) else {
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
                tr!("shell.capture.no_window"),
                tr!("shell.capture.no_window.body"),
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
        if !self.shared.config.capture.open_editor_after_capture {
            self.capture_fullscreen_quick();
            return;
        }
        let Some((frozen, windows)) = self.freeze_for(true) else {
            return;
        };
        let mut source = capture_ops::target_from(&windows)
            .map(|w| capture_ops::source_of(&w))
            .unwrap_or_default();
        let mon = capture_ops::monitor_rect_at(frozen.cursor, frozen.bounds);
        source.scale = Some(capture_ops::scale_at(mon));
        let base = capture_ops::crop_frozen(&frozen, mon);
        if base.width == 0 || base.height == 0 {
            self.notify(
                tr!("shell.capture.screen_failed"),
                tr!("shell.capture.no_monitor.body"),
                true,
            );
            return;
        }
        let mut doc = Doc::new(base.width, base.height);
        if self.shared.config.capture.cursor_object {
            capture_ops::add_cursor_object(&mut doc, &frozen, mon);
        }
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

    /// Full-screen capture with the editor off: no overlay needs the whole desktop, so only
    /// the monitor under the cursor is read back.
    fn capture_fullscreen_quick(&mut self) {
        let t0 = Instant::now();
        let windows = windows_enum::visible_windows();
        let cursor = stinkshot_platform::input::cursor_pos();
        let mon =
            capture_ops::monitor_rect_at(cursor, stinkshot_platform::monitors::virtual_bounds());
        let res = capture::freeze_rect(mon, self.shared.config.capture.include_cursor);
        timing::point(format_args!(
            "freeze done: monitor {}x{} in {:.1} ms",
            mon.w,
            mon.h,
            t0.elapsed().as_secs_f64() * 1000.0
        ));
        let base = match res {
            Ok(img) => img,
            Err(e) => {
                let body = report("freeze_rect", &e, tr!("shell.capture.no_image.body"));
                self.notify(tr!("shell.capture.screen_failed"), body, true);
                return;
            }
        };
        let source = capture_ops::target_from(&windows)
            .map(|w| capture_ops::source_of(&w))
            .unwrap_or_default();
        self.finish_quick(base, mon, source, "fullscreen");
    }

    fn repeat_last_region(&mut self) {
        if self.capture_blocked() {
            return;
        }
        let Some(rect) = self.shared.config.capture.last_region else {
            // No toast over the frozen screen: the overlay says it inline.
            self.next_overlay_notice = Some(tr!("shell.notice.no_last_region"));
            self.begin_overlay(OverlayMode::Capture);
            self.next_overlay_notice = None;
            return;
        };
        let Some((frozen, windows)) = self.freeze() else {
            return;
        };
        let img = capture_ops::crop_frozen(&frozen, rect);
        if img.width == 0 || img.height == 0 {
            self.next_overlay_notice = Some(tr!("shell.notice.last_region_off_screen"));
            self.open_overlay(OverlayMode::Capture, frozen, windows);
            return;
        }
        let source = capture_ops::source_for_rect(&windows, rect);
        let doc = Doc::new(img.width, img.height);
        let out = CaptureOutput {
            base: img.clone(),
            doc,
            flattened: img,
            unframed: None,
            rect,
            action: FinishAction::Copy,
            source,
            mode: "region",
            editing: None,
        };
        self.capture_done(out);
    }

    /// Capture Selected Text: a worker asks UI Automation where the selection is, then
    /// `capture_text_rect` takes that area. Nothing is sent to the app and its selection and
    /// the clipboard stay as they are.
    fn capture_selected_text(&mut self, paragraph: bool) {
        if self.capture_blocked() {
            return;
        }
        // After a tray click the focus is gone from the app: search its window instead.
        let fallback = capture_ops::target_window().map(|w| w.hwnd);
        self.window_capture_pending = true;
        workers::find_selected_text(&self.env(), fallback, paragraph);
    }

    /// Captures the selection's rect like a region: into the editor when Open Editor After
    /// Capture is on, otherwise finished right away.
    fn capture_text_rect(&mut self, rect: PxRect) {
        if self.capture_blocked() {
            return;
        }
        if self.shared.config.capture.open_editor_after_capture {
            let Some((frozen, windows)) = self.freeze() else {
                return;
            };
            let base = capture_ops::crop_frozen(&frozen, rect);
            if base.width == 0 || base.height == 0 {
                self.notify(
                    tr!("shell.selected_text.none"),
                    tr!("shell.selected_text.none.body"),
                    false,
                );
                return;
            }
            let source = capture_ops::source_for_rect(&windows, rect);
            let doc = Doc::new(base.width, base.height);
            self.open_overlay(
                OverlayMode::Annotate {
                    base,
                    doc,
                    mode: "text",
                    editing: None,
                    source,
                },
                frozen,
                windows,
            );
            return;
        }
        let windows = windows_enum::visible_windows();
        let base = match capture::freeze_rect(rect, self.shared.config.capture.include_cursor) {
            Ok(img) if img.width > 0 && img.height > 0 => img,
            Ok(_) => {
                self.notify(
                    tr!("shell.selected_text.none"),
                    tr!("shell.selected_text.none.body"),
                    false,
                );
                return;
            }
            Err(e) => {
                let body = report("freeze_rect", &e, tr!("shell.capture.no_image.body"));
                self.notify(tr!("shell.capture.screen_failed"), body, true);
                return;
            }
        };
        let source = capture_ops::source_for_rect(&windows, rect);
        self.finish_quick(base, rect, source, "text");
    }

    fn capture_done(&mut self, out: CaptureOutput) {
        self.freeze_cache = None;
        if out.mode == "region" && out.editing.is_none() {
            self.shared.config.capture.last_region = Some(out.rect);
            self.shared.config_dirty = true;
        }
        workers::finish_capture(&self.env(), out);
    }

    /// Window / full-screen capture with the editor off: finish it like an overlay Enter
    /// would, but with the clipboard per config. The toast's Edit reopens it.
    fn finish_quick(&mut self, base: Rgba, rect: PxRect, source: SourceInfo, mode: &'static str) {
        let doc = Doc::new(base.width, base.height);
        let out = CaptureOutput {
            // Nothing is drawn on a quick capture, so the unannotated base is never read (it
            // only feeds the re-edit sidecar, which needs shapes): skip a full-size copy.
            base: Rgba::new(0, 0),
            flattened: base,
            unframed: None,
            doc,
            rect,
            action: FinishAction::Quick,
            source,
            mode,
            editing: None,
        };
        self.capture_done(out);
    }

    /// Tray double click: History. The first click of the pair already opened the region
    /// overlay; drop it if nothing was selected yet (the overlay itself handles a second
    /// click that lands on it instead of on the tray icon).
    fn tray_double_click(&mut self) {
        let fresh = self
            .tray_overlay_at
            .is_some_and(|t| t.elapsed() < TRAY_DOUBLE_CLICK_GRACE);
        if fresh && self.overlay.as_ref().is_some_and(|o| o.s.is_untouched()) {
            self.overlay = None;
            self.freeze_cache = None;
            self.touch();
        }
        self.tray_overlay_at = None;
        self.tray_click_quiet_until = Some(Instant::now() + TRAY_CLICK_QUIET);
        self.open_history();
    }

    fn open_history(&mut self) {
        if self.history_win.is_some() {
            Self::focus_window("Stinkshot History");
            return;
        }
        self.history_win = Some(Slot::new(HistoryWindow::new(&self.shared)));
        self.touch();
    }

    fn open_tour(&mut self) {
        if self.tour.is_some() {
            Self::focus_window(crate::tour::TourWindow::TITLE);
            return;
        }
        self.tour = Some(Slot::new(crate::tour::TourWindow::new()));
        self.touch();
    }

    fn open_settings(&mut self, section: Option<crate::settings_ui::Section>) {
        if let Some(s) = &mut self.settings {
            if let Some(sec) = section {
                s.s.show_section(sec);
                self.touch();
            }
            Self::focus_window("Stinkshot Settings");
            return;
        }
        // A key another program held may be free by now: check before showing it as taken.
        self.hotkeys.retry_now();
        self.tick_hotkeys(false);
        let mut w = SettingsWindow::new(&self.shared);
        w.set_hotkey_errors(self.hotkey_errors.clone());
        w.set_snip_status(self.hotkeys.snip_status());
        if let Some(sec) = section {
            w.show_section(sec);
        }
        self.settings = Some(Slot::new(w));
        self.touch();
    }

    /// The wgpu device is gone (`gpu.rs`): no surface can paint again, and egui-wgpu panics
    /// on the next mesh upload. Hand over to a fresh instance before this frame paints.
    fn restart_after_gpu_loss(&mut self) -> ! {
        if let Rec::Active { .. } = &self.rec
            && let Rec::Active { rec, .. } = std::mem::replace(&mut self.rec, Rec::Idle)
            && let Err(e) = rec.stop()
        {
            // The recorder does not use wgpu; finish its file so the recording is kept.
            log::warn!("stop before restart failed: {e:#}");
        }
        self.hotkeys.shutdown();
        self.tray = None;
        // Saves, exports and a recording's stop worker must not be cut off by the exit.
        wait_for_jobs(GPU_RESTART_GRACE);
        crate::gpu::relaunch();
        log::logger().flush();
        std::process::exit(0);
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
            // Finish the file so a recording in progress is not lost. The window closes
            // only once this (and every other tracked job) is done, see `finish_quit`.
            let in_flight = workers::InFlight::begin();
            std::thread::spawn(move || {
                let _in_flight = in_flight;
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
        self.quit_deadline = Some(Instant::now() + QUIT_GRACE);
        self.ctx.request_repaint();
    }

    /// Closes the app once recordings, exports and saves in flight have finished (or the
    /// grace period ran out), so the process does not exit under them.
    fn finish_quit(&mut self) {
        let Some(deadline) = self.quit_deadline else {
            return;
        };
        let busy = workers::jobs_in_flight();
        if busy == 0 || Instant::now() >= deadline {
            if busy > 0 {
                log::warn!("quitting with {busy} job(s) still running");
            }
            self.quit_deadline = None;
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else {
            self.ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    // ------------------------------------------------------- region follow-ups

    /// Wheel sink for a scrolling capture starting now (`soak` builds can redirect it).
    #[cfg(not(feature = "soak"))]
    fn next_wheel(&mut self) -> workers::Wheel {
        workers::Wheel::Input
    }

    fn region_chosen(&mut self, purpose: RegionPurpose, rect: PxRect, source: SourceInfo) {
        match purpose {
            // The picker copies inside the overlay and never chooses a region.
            RegionPurpose::PickColor => {}
            RegionPurpose::Ocr => {
                let cached = self.freeze_cache.take();
                match cached {
                    Some(f) => {
                        let img = capture_ops::crop_frozen(&f, rect);
                        if img.width == 0 || img.height == 0 {
                            self.notify(tr!("shell.ocr.no_text"), tr!("shell.ocr.empty"), false);
                        } else {
                            workers::ocr_image(&self.env(), img);
                        }
                    }
                    None => self.notify(
                        tr!("shell.selection.lost"),
                        tr!("shell.selection.lost.body"),
                        true,
                    ),
                }
            }
            RegionPurpose::Scrolling => {
                if rect.w < 16 || rect.h < 16 {
                    self.notify(
                        tr!("shell.selection.too_small"),
                        tr!("shell.selection.too_small.scroll"),
                        false,
                    );
                    return;
                }
                let control = Arc::new(AtomicU8::new(SCROLL_RUN));
                self.scroll = Some(ScrollFlow {
                    control: control.clone(),
                });
                self.scroll_hud = Some(Slot::new(ScrollHud::new(rect)));
                workers::scroll_capture(&self.env(), rect, source, control, self.next_wheel());
                self.touch();
            }
            RegionPurpose::RecordMp4 | RegionPurpose::RecordGif => {
                if rect.w < 16 || rect.h < 16 {
                    self.notify(
                        tr!("shell.selection.too_small"),
                        tr!("shell.selection.too_small.record"),
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
                tr!("shell.busy.recording"),
                tr!("shell.busy.recording.record"),
                false,
            ),
        }
    }

    fn begin_recording(&mut self, pending: PendingRecord) {
        let rc = self.shared.config.recording.clone();
        let output = match workers::temp_recording_path(&self.shared.history) {
            Ok(p) => p,
            Err(e) => {
                let body = report(
                    "temp recording path",
                    &e,
                    tr!("shell.record.temp_failed.body"),
                );
                self.notify(tr!("shell.record.start_failed"), body, true);
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

    fn record_summary(&self, gif: bool, fps: u32) -> String {
        record_summary(&self.shared.config.recording, gif, fps)
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
            ..
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

    /// After an export that wrote nothing: put the trim window back in its idle state (reopening
    /// it if the user had closed it) so the raw recording can be saved again. False when the
    /// raw file is gone or cannot be read.
    fn keep_trim_for_retry(&mut self) -> bool {
        let Rec::Trim {
            source,
            done_issued,
            gif,
        } = &mut self.rec
        else {
            return false;
        };
        if !source.exists() {
            return false;
        }
        *done_issued = false;
        if let Some(t) = &mut self.trim {
            t.s.set_progress(None);
            return true;
        }
        match stinkshot_record::probe(source) {
            Ok(info) => {
                self.trim = Some(Slot::new(TrimWindow::new(source.clone(), info, *gif)));
                true
            }
            Err(e) => {
                log::warn!("reopen trim window for {}: {e:#}", source.display());
                false
            }
        }
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
        let mut mic_failed: Option<String> = None;
        if let Rec::Active { rec, .. } = &mut self.rec {
            let status = rec.status();
            if let Some(err) = status.error.clone() {
                stop_with_error = Some(err);
            }
            if let Some(hud) = &mut self.record_hud {
                hud.s.update_status(status);
                if let Some(t) = hud.s.take_toggle_changes() {
                    if t.mic && !rec.mic_open() {
                        let rc = &self.shared.config.recording;
                        let dev = (!rc.mic_device.is_empty()).then(|| rc.mic_device.clone());
                        if let Err(e) = rec.start_mic(dev) {
                            log::warn!("microphone mid-recording: {e:#}");
                            hud.s.set_mic_off();
                            mic_failed = Some(format!("{e:#}"));
                        }
                    }
                    rec.set_mic_muted(!t.mic);
                    rec.set_system_muted(!t.system);
                    rec.set_webcam_visible(t.webcam);
                    rec.set_overlays(t.overlays, t.overlays);
                }
            }
            wake::wake_after(Duration::from_millis(200));
        }
        if let Some(err) = mic_failed {
            let body = report_str("microphone", &err, tr!("shell.record.mic_failed.body"));
            self.notify(tr!("shell.record.mic_failed"), body, true);
        }
        if let Some(err) = stop_with_error {
            let body = report_str_with(
                "recorder reported an error",
                &err,
                tr!("shell.record.problem.body"),
                tr!("shell.record.problem.tail"),
            );
            self.notify(tr!("shell.record.problem"), body, true);
            self.record_stop();
        }
        // Trim window closed without a choice: keep the untrimmed recording.
        if self.trim_closed_by_user {
            self.trim_closed_by_user = false;
            if let Rec::Trim {
                source,
                done_issued: false,
                gif,
            } = &self.rec
                && !self.exporting
            {
                let (source, gif) = (source.clone(), *gif);
                self.exporting = true;
                workers::export_recording(
                    &self.env(),
                    source,
                    Duration::ZERO,
                    Duration::ZERO,
                    gif,
                    !gif,
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
                    Ok((base, source)) if !self.shared.config.capture.open_editor_after_capture => {
                        let rect = PxRect::new(0, 0, base.width as i32, base.height as i32);
                        self.finish_quick(base, rect, source, "window");
                    }
                    Ok((base, source)) => {
                        let doc = Doc::new(base.width, base.height);
                        let mode = OverlayMode::Annotate {
                            base,
                            doc,
                            mode: "window",
                            editing: None,
                            source,
                        };
                        if self.overlay.is_some() || self.busy_with_recording() {
                            self.queued_annotate = Some(mode);
                        } else {
                            self.open_overlay(mode, frozen, windows);
                        }
                    }
                    Err(e) => {
                        let body =
                            report_str("window capture", &e, tr!("shell.error.window_capture"));
                        self.notify(tr!("shell.capture.window_failed"), body, true)
                    }
                }
            }
            WorkerMsg::SelectedText(result) => {
                self.window_capture_pending = false;
                match result {
                    Ok(Some(rect)) => self.capture_text_rect(rect),
                    Ok(None) => self.notify(
                        tr!("shell.selected_text.none"),
                        tr!("shell.selected_text.none.body"),
                        false,
                    ),
                    Err(e) => {
                        log::warn!("selected text: {e}");
                        self.notify(
                            tr!("shell.selected_text.none"),
                            tr!("shell.selected_text.none.body"),
                            false,
                        );
                    }
                }
            }
            WorkerMsg::WindowCaptureFailed(e) => {
                self.window_capture_pending = false;
                let body = report_str("window capture", &e, tr!("shell.error.window_capture"));
                self.notify(tr!("shell.capture.window_failed"), body, true);
            }
            WorkerMsg::OpenAnnotate {
                base,
                doc,
                mode,
                editing,
                source,
                name,
            } => {
                // Re-edits open in an Editor Window: nothing freezes, so this works during
                // a recording or a scroll capture and beside an open overlay (#5).
                self.open_editor(crate::state::EditorRequest {
                    base,
                    doc,
                    mode,
                    editing,
                    source,
                    name,
                    rect: None,
                });
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
                #[cfg(feature = "soak")]
                let quick = std::mem::take(&mut self.soak.quick_scroll);
                #[cfg(not(feature = "soak"))]
                let quick = false;
                if cancelled {
                    return;
                }
                match result {
                    Ok(img) if quick => {
                        let rect = PxRect::new(0, 0, img.width as i32, img.height as i32);
                        self.finish_quick(img, rect, source, "scrolling");
                    }
                    Ok(img) => {
                        if partial {
                            self.notify(
                                tr!("shell.scroll.stopped"),
                                tr!("shell.scroll.stopped.body"),
                                false,
                            );
                        }
                        let doc = Doc::new(img.width, img.height);
                        let mode = OverlayMode::Annotate {
                            base: img,
                            doc,
                            mode: "scrolling",
                            editing: None,
                            source,
                        };
                        if self.overlay.is_some() || self.busy_with_recording() {
                            self.queued_annotate = Some(mode);
                        } else if let Some((frozen, windows)) = self.freeze() {
                            self.open_overlay(mode, frozen, windows);
                        }
                    }
                    Err(e) => {
                        let body =
                            report_str("scrolling capture", &e, tr!("shell.scroll.failed.body"));
                        self.notify(tr!("shell.scroll.failed"), body, true)
                    }
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
                            gif: pending.gif,
                        };
                        self.touch();
                    }
                    (_, Err(e)) => {
                        let body =
                            report_str("recorder stop", &e, tr!("shell.record.finish_failed.body"));
                        self.notify(tr!("shell.record.finish_failed"), body, true)
                    }
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
            WorkerMsg::UpdateProgress(percent) => {
                if let crate::state::UpdateState::Downloading { percent: p, .. } =
                    &mut self.shared.update
                {
                    *p = percent;
                }
            }
            WorkerMsg::UpdateDownloaded(result) => self.update_downloaded(result),
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
                let summary = self.record_summary(pending.gif, rec.fps());
                let wanted_fps = rc.fps;
                if !pending.gif && rec.fps() < wanted_fps {
                    self.notify(
                        tr!("shell.record.at_60"),
                        tr!("shell.record.needs_hw", fps = wanted_fps),
                        false,
                    );
                }
                let stop_key = crate::hotkeys::display(if pending.gif {
                    &self.shared.config.hotkeys.record_gif
                } else {
                    &self.shared.config.hotkeys.record_mp4
                });
                self.record_frame = Some(Slot::new(RecordFrame::new(pending.rect)));
                let mut hud = RecordHud::new(pending.rect, summary, toggles);
                // Sources off at start were never opened; the HUD must not offer them. The
                // microphone is the exception: with an audio track it can join mid-recording.
                let available = HudToggles {
                    overlays: true,
                    mic: toggles.mic || rec.can_add_mic(),
                    ..toggles
                };
                hud.set_context(available, stop_key);
                self.record_hud = Some(Slot::new(hud));
                self.rec = Rec::Active {
                    rec: Box::new(rec),
                    pending,
                };
                self.touch();
            }
            Err(e) => {
                let body = report_str("recorder start", &e, tr!("shell.record.start_failed.body"));
                if !cancel {
                    self.notify(tr!("shell.record.start_failed"), body, true);
                }
            }
        }
    }

    fn exported(&mut self, x: workers::Exported) {
        self.exporting = false;
        self.touch();
        // Nothing written: the raw recording is still on disk, so keep (or reopen) the trim
        // window and let Save be retried rather than strand the file.
        if let Some(err) = &x.error
            && x.items.is_empty()
            && self.keep_trim_for_retry()
        {
            let body = report_str_with(
                "export",
                err,
                tr!("shell.export.failed.body"),
                tr!("shell.export.failed.tail"),
            );
            self.notify(tr!("shell.export.failed"), body, true);
            return;
        }
        self.trim = None;
        self.rec = Rec::Idle;
        if let Some(err) = &x.error {
            let body = if x.items.is_empty() {
                report_str("export", err, tr!("shell.export.failed.body"))
            } else {
                report_str_with(
                    "export",
                    err,
                    tr!("shell.export.partial.body"),
                    tr!("shell.export.partial.tail"),
                )
            };
            self.notify(tr!("shell.export.failed"), body, true);
        }
        if !x.items.is_empty()
            && let Some(t) = &mut self.tray
        {
            t.flash_capture();
        }
        let Some((id, path, kind)) = x.items.first().cloned() else {
            return;
        };
        if !self.shared.config.general.show_toast {
            return;
        }
        let gif_only = x.items.iter().all(|(_, _, k)| *k == ItemKind::Gif);
        let title = if x.items.len() > 1 {
            tr!("shell.saved.mp4_gif").to_string()
        } else if gif_only {
            tr!("shell.saved.gif").to_string()
        } else {
            tr!("shell.saved.recording").to_string()
        };
        let body = format!(
            "{} \u{b7} {} \u{d7} {} \u{b7} {}",
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
        if let Some(t) = &mut self.tray {
            t.flash_capture();
        }
        if s.action == FinishAction::Pin
            && let Some(img) = s.pin_image
        {
            let title = s
                .path
                .as_ref()
                .and_then(|p| p.file_stem())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| tr!("shell.pin.default_title").into());
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
            (
                tr!("shell.saved.text").to_string(),
                workers::preview(text, 120),
            )
        } else if s.action == FinishAction::CopyText {
            return; // the worker already explained why nothing was copied
        } else {
            let title = if let Some(p) = &s.saved_as {
                tr!(
                    "shell.saved.as",
                    name = p
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                )
            } else if s.action == FinishAction::SaveCopy && s.item.is_some() {
                tr!("shell.saved.copy").to_string()
            } else if s.editing {
                tr!("shell.saved.updated").to_string()
            } else if s.action == FinishAction::Pin {
                tr!("shell.saved.pinned").to_string()
            } else if s.copied_image {
                tr!("shell.saved.copied").to_string()
            } else {
                tr!("shell.saved.history").to_string()
            };
            (
                title,
                format!("{} \u{d7} {} \u{b7} {}", s.width, s.height, detail),
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
                self.notify(tr!("shell.update.off"), tr!("shell.update.off.body"), false);
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
        self.update_forced = force;
        workers::check_updates(self.worker_tx.clone());
    }

    fn update_checked(&mut self, result: Result<Option<Release>, String>) {
        self.update_inflight = false;
        let forced = std::mem::take(&mut self.update_forced);
        let install = std::mem::take(&mut self.install_after_check);
        match result {
            Ok(release) => {
                self.shared.update_check_failed = false;
                self.shared.config.privacy.last_update_check = Some(chrono::Utc::now());
                self.shared.config_dirty = true;
                if let Some(r) = release {
                    let first = self
                        .update_release
                        .as_ref()
                        .map(|u| u.tag != r.tag)
                        .unwrap_or(true);
                    let self_update = r.installer.is_some() && crate::update::can_self_update();
                    if (first || forced) && !self.update_installing && !install {
                        self.push_toast(ToastKind::Update {
                            title: tr!("shell.update.available", tag = r.tag).to_string(),
                            body: if self_update {
                                tr!("shell.update.available.install").to_string()
                            } else {
                                tr!("shell.update.available.body").to_string()
                            },
                            self_update,
                        });
                    }
                    if let Some(t) = &mut self.tray {
                        t.set_update(Some(&r));
                    }
                    if !self.update_installing {
                        self.shared.update = crate::state::UpdateState::Available {
                            tag: r.tag.clone(),
                            self_update,
                        };
                    }
                    self.update_release = Some(r);
                    if install {
                        self.install_update();
                    }
                } else if forced {
                    self.notify(
                        tr!("shell.update.current"),
                        tr!("shell.update.current.body", v = env!("CARGO_PKG_VERSION")),
                        false,
                    );
                }
            }
            Err(e) => {
                let body = if e.starts_with(crate::update::UNTRUSTED) {
                    log::warn!("update check: {e}");
                    tr!("shell.update.untrusted").to_string()
                } else if e.starts_with(crate::update::UNSIGNED) {
                    log::info!("update check: {e}");
                    tr!("shell.update.unsigned").to_string()
                } else {
                    errors::report_network_str("update check", &e, tr!("shell.update.no_answer"))
                };
                if forced {
                    self.notify(tr!("shell.update.failed"), body, true);
                }
                self.shared.update_check_failed = true;
                self.update_retry_at = Instant::now() + Duration::from_secs(3600);
            }
        }
    }

    /// First start of a newer version: say so once, and remember this version. A config
    /// from before 0.1.3 has no `last_version`; one that finished the tour was used by an
    /// older version, so that counts as an update too.
    fn note_version(&mut self) {
        let current = env!("CARGO_PKG_VERSION");
        let g = &self.shared.config.general;
        if g.last_version == current {
            return;
        }
        let updated = if g.last_version.is_empty() {
            g.tour_seen
        } else {
            crate::update::is_newer(current, &g.last_version)
        };
        self.shared.config.general.last_version = current.to_string();
        self.shared.config_dirty = true;
        if updated && self.shared.config.general.show_toast {
            self.push_toast(ToastKind::Updated {
                title: tr!("shell.update.updated", v = current).to_string(),
                body: tr!("shell.update.updated.body").to_string(),
            });
        }
    }

    /// Update Now: download, verify and run the installer, or open the release page when
    /// this copy cannot update itself (portable, dev build, no installer listed).
    fn install_update(&mut self) {
        let Some(r) = self.update_release.clone() else {
            return;
        };
        if self.update_installing {
            return;
        }
        let inst = match &r.installer {
            Some(i) if crate::update::can_self_update() => i.clone(),
            _ => {
                self.pending.push_back(Command::OpenUrl(r.url.clone()));
                return;
            }
        };
        log::info!("update: downloading {} ({} bytes)", inst.name, inst.size);
        self.update_installing = true;
        self.shared.update = crate::state::UpdateState::Downloading {
            tag: r.tag.clone(),
            percent: 0,
        };
        self.notify(
            &tr!("shell.update.downloading", tag = r.tag),
            tr!("shell.update.downloading.body"),
            false,
        );
        workers::download_update(self.worker_tx.clone(), inst);
    }

    fn update_downloaded(&mut self, result: Result<PathBuf, String>) {
        let Some(r) = self.update_release.clone() else {
            return;
        };
        let started = result.and_then(|path| {
            crate::update::run_installer(&path)
                .map(|()| path)
                .map_err(|e| format!("{e:#}"))
        });
        match started {
            Ok(path) => {
                log::info!("update: started installer {}", path.display());
                self.shared.update = crate::state::UpdateState::Installing { tag: r.tag };
                // The installer asks this copy to quit (`stinkshot quit`) and starts the new
                // version afterwards; nothing else to do here.
            }
            Err(e) => {
                self.update_installing = false;
                let self_update = r.installer.is_some();
                self.shared.update = crate::state::UpdateState::Available {
                    tag: r.tag.clone(),
                    self_update,
                };
                let why = if e.starts_with(crate::update::UNTRUSTED) {
                    log::warn!("update: {e}");
                    tr!("shell.update.install_untrusted").to_string()
                } else {
                    errors::report_network_str(
                        "update download",
                        &e,
                        tr!("shell.update.install_failed.body"),
                    )
                };
                self.notify(
                    tr!("shell.update.install_failed"),
                    format!("{why} {}", tr!("shell.update.opening_page")),
                    true,
                );
                self.pending.push_back(Command::OpenUrl(r.url));
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
            let body = report_with(
                "saving config",
                &e,
                tr!("shell.config.save_failed.body"),
                tr!("shell.config.save_failed.tail"),
            );
            self.notify(tr!("shell.config.save_failed"), body, true);
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
                s.s.set_snip_status(self.hotkeys.snip_status());
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
                    let body = report_with(
                        &format!("open history folder {}", dir.display()),
                        &e,
                        tr!("shell.history_dir.failed.body"),
                        tr!("shell.history_dir.failed.tail"),
                    );
                    self.notify(tr!("shell.history_dir.failed"), body, true);
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
        // Contrast theme or Text size changed in Windows settings (`theme::install` reads them).
        let a11y = crate::a11y::prefs();
        if force || resolved != self.resolved_theme || a11y != self.a11y_prefs {
            self.resolved_theme = resolved;
            self.a11y_prefs = a11y;
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
        if let Some(h) = sysutil::find_own_window(ROOT_TITLE) {
            let h: Hwnd = h;
            window_util::set_tool_window(h, true);
            window_util::exclude_from_capture(h, true);
            self.root_setup_tries = 200;
        }
    }

    fn poll_inputs(&mut self) {
        let recording = matches!(self.rec, Rec::Active { .. });
        if let Some(t) = &mut self.tray {
            t.set_recording(recording);
            self.pending.extend(t.poll());
        }
        self.pending.extend(self.hotkeys.poll());
        self.tick_hotkeys(false);
        while let Ok(args) = self.ipc_rx.try_recv() {
            timing::point(format_args!("ipc: command taken by the shell"));
            match cli::command_from_args(&args) {
                Some(cmd) => self.pending.push_back(cmd),
                None => log::debug!("ignored forwarded args {args:?}"),
            }
        }
        while let Ok(msg) = self.worker_rx.try_recv() {
            self.handle_worker_msg(msg);
        }
    }

    /// Retry hotkeys that failed to register when due (`Hotkeys::tick`), pass changed
    /// problem texts to Settings, and wake up for the next retry. `checked`: Check Again was
    /// clicked, so Settings reports the outcome.
    fn tick_hotkeys(&mut self, checked: bool) {
        if self.hotkeys.tick() {
            self.hotkey_errors = self.hotkeys.errors();
            if let Some(s) = &mut self.settings {
                s.s.set_hotkey_errors(self.hotkey_errors.clone());
            }
            self.touch();
        }
        if checked && let Some(s) = &mut self.settings {
            s.s.hotkeys_checked();
            self.touch();
        }
        if let Some(wait) = self.hotkeys.next_retry_in() {
            self.ctx
                .request_repaint_after(wait.max(Duration::from_millis(100)));
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
        if let Some(at) = self.logo_prewarm_at {
            let now = Instant::now();
            if now < at {
                self.ctx.request_repaint_after(at - now);
            } else if self.overlay.is_some() || !matches!(self.rec, Rec::Idle) {
                // Never compete with a capture or a recording; try again later.
                self.logo_prewarm_at = Some(now + LOGO_PREWARM_DELAY);
                self.ctx.request_repaint_after(LOGO_PREWARM_DELAY);
            } else {
                self.logo_prewarm_at = None;
                crate::logo_anim::prewarm_cache(&self.ctx, system_scale());
            }
        }
        // The capture session's shadow textures are dropped after a quiet spell
        // (`release_idle_shadows`); come back when the next one is due.
        let next = if self.overlay.is_some() {
            SHADOW_IDLE
        } else {
            capture::release_idle_shadows(SHADOW_IDLE)
        };
        let next = next.min(self.trim_when_idle());
        self.ctx
            .request_repaint_after(next.clamp(Duration::from_secs(1), Duration::from_secs(60)));
    }

    /// After `TRIM_IDLE` with no window, recording or job, give the working set back to
    /// Windows once (`system::trim_working_set`): Task Manager's memory figure drops from about
    /// 120 MB to a few MB and the RAM is free for other programs; the next capture faults
    /// the pages back in (measured cost in docs/notes/app-shell.md, "Idle memory"). Private
    /// bytes do not change. Returns how long until the next check is due.
    fn trim_when_idle(&mut self) -> Duration {
        let busy = self.overlay.is_some()
            || self.history_win.is_some()
            || self.settings.is_some()
            || self.tour.is_some()
            || self.countdown.is_some()
            || self.record_frame.is_some()
            || self.record_hud.is_some()
            || self.trim.is_some()
            || self.scroll_hud.is_some()
            || !self.toasts.is_empty()
            || !self.editors.is_empty()
            || !matches!(self.rec, Rec::Idle)
            || workers::jobs_in_flight() > 0;
        if busy {
            self.quiet_since = Instant::now();
            self.trimmed = false;
            return Duration::from_secs(60);
        }
        static AFTER: std::sync::OnceLock<Duration> = std::sync::OnceLock::new();
        let after = *AFTER.get_or_init(|| {
            std::env::var("STINKSHOT_TRIM_IDLE_SECS")
                .ok()
                .and_then(|v| v.trim().parse::<u64>().ok())
                .map_or(TRIM_IDLE, Duration::from_secs)
        });
        if self.trimmed || after.is_zero() {
            return Duration::from_secs(60);
        }
        let quiet = self.quiet_since.elapsed();
        if quiet < after {
            return after - quiet;
        }
        system::trim_working_set();
        self.trimmed = true;
        timing::point(format_args!(
            "working set trimmed after {} s idle",
            quiet.as_secs()
        ));
        Duration::from_secs(60)
    }

    fn show_all(&mut self, ctx: &egui::Context) {
        let shared = &mut self.shared;
        let mut closed = false;
        closed |= drive_opt(ctx, shared, &mut self.overlay, true);
        closed |= drive_opt(ctx, shared, &mut self.history_win, true);
        closed |= drive_opt(ctx, shared, &mut self.settings, true);
        closed |= drive_opt(ctx, shared, &mut self.tour, true);
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
        let before = self.editors.len();
        self.editors
            .retain_mut(|e| drive(ctx, shared, &mut e.s, &mut e.st, false) == SurfaceStatus::Open);
        closed |= self.editors.len() != before;
        if closed {
            timing::point(format_args!("surface closed"));
            // One more pass so egui destroys the windows of surfaces that just closed.
            self.touch();
        }
    }

    /// Hand over from the start-up quick overlay to the region overlay once that one is
    /// visible (or close it when no overlay came, or after `QUICK_OVERLAY_MAX`). Esc in the
    /// quick overlay cancels at once.
    fn settle_quick_overlay(&mut self) {
        if self
            .early_freeze
            .as_ref()
            .is_some_and(|e| e.at.elapsed() >= EARLY_FREEZE_MAX_AGE)
        {
            self.early_freeze = None;
        }
        let Some((quick, since)) = &self.quick_overlay else {
            return;
        };
        let inp = quick.input();
        let timed_out = since.elapsed() >= QUICK_OVERLAY_MAX;
        let handoff = match &mut self.overlay {
            Some(o) if inp.cancelled || o.s.revealed() => {
                o.s.take_quick_input(inp, &mut self.shared);
                true
            }
            Some(_) => timed_out,
            None => true,
        };
        if handoff {
            timing::point(format_args!("quick overlay handed over"));
            self.quick_overlay = None;
        } else {
            self.needs_repaint = true;
        }
    }

    /// While a Settings hotkey field waits for a key press, global hotkeys must be released
    /// (also covers the window closing mid-capture).
    fn sync_hotkey_capture(&mut self) {
        let capturing = self
            .settings
            .as_ref()
            .is_some_and(|s| s.s.is_capturing_hotkey());
        if capturing != self.hotkeys.is_suspended() {
            self.hotkeys.set_suspended(capturing);
        }
    }

    fn drain_outbox(&mut self) {
        if !self.shared.outbox.is_empty() {
            let drained: Vec<Command> = self.shared.outbox.drain(..).collect();
            self.pending.extend(drained);
        }
    }
}

impl Drop for Shell {
    /// A panic out of the event loop (a GPU device loss inside a paint ends this way, see
    /// `run_app`) drops the shell while unwinding. Dropping an active `Recorder` aborts it
    /// and deletes the file; finish it instead so the recording is kept. Normal exits stop
    /// recordings in `quit`.
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        if let Rec::Active { .. } = &self.rec
            && let Rec::Active { rec, .. } = std::mem::replace(&mut self.rec, Rec::Idle)
        {
            match rec.stop() {
                Ok(r) => log::warn!("recording kept after a panic: {}", r.path.display()),
                Err(e) => log::warn!("stop after a panic failed: {e:#}"),
            }
        }
    }
}

impl eframe::App for Shell {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if crate::gpu::lost() {
            self.restart_after_gpu_loss();
        }
        if !self.first_frame_seen {
            self.first_frame_seen = true;
            timing::point(format_args!("first frame"));
        }
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
        self.settle_quick_overlay();
        #[cfg(feature = "soak")]
        self.soak_write_state();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.show_all(&ctx);
        // The overlay may have been uncloaked in this pass: close the quick overlay now, not a
        // frame later.
        self.settle_quick_overlay();
        // After the surfaces: restyling the root hides and re-shows it, which costs about
        // 150 ms on the first frame and would delay an overlay requested at launch.
        self.setup_root_window();
        self.sync_hotkey_capture();
        self.open_queued_annotate();
        self.finish_quit();
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
        log::info!("Stinkshot exited");
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

    #[test]
    fn gif_summary_does_not_claim_h264_or_audio() {
        let rc = stinkshot_core::config::Recording::default();
        let gif = record_summary(&rc, true, rc.fps);
        assert!(gif.starts_with("GIF"));
        assert!(!gif.contains("H.264"));
        assert!(gif.contains(&format!("{} FPS", rc.gif_fps)));
        assert!(gif.contains("No Audio"));
        let mp4 = record_summary(&rc, false, rc.fps);
        assert!(mp4.contains("H.264"));
        assert!(mp4.contains(&format!("{} FPS", rc.fps)));
        // The rate in use, not the setting, when 120 FPS fell back to 60.
        assert!(record_summary(&rc, false, 60).contains("60 FPS"));
    }

    #[test]
    fn welcome_names_the_region_hotkey() {
        assert!(welcome_body("PrintScreen").contains("Press PrtSc"));
        let none = welcome_body("");
        assert!(!none.contains("Press"));
        assert!(none.contains("tray icon"));
    }
}
