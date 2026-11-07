//! Shared app contracts. Every UI surface and the shell agree on these types.
//! Changing anything here affects all surfaces: keep it stable.

use std::sync::Arc;

use stinkshot_core::annotate::Doc;
use stinkshot_core::config::Config;
use stinkshot_core::fonts::Fonts;
use stinkshot_core::{PxRect, Rgba};
use stinkshot_history::{History, ItemId};
use stinkshot_platform::capture::Frozen;
use stinkshot_platform::ocr::OcrEngine;
use stinkshot_platform::windows_enum::WinInfo;

/// Everything a surface may read or mutate while drawing. Owned by the shell.
pub struct Shared {
    pub config: Config,
    /// Set true after mutating `config`; the shell saves it and re-registers hotkeys etc.
    pub config_dirty: bool,
    pub history: Arc<History>,
    pub fonts: Arc<Fonts>,
    /// None if no OCR language is installed (UI must show a helpful message).
    pub ocr: Option<Arc<dyn OcrEngine>>,
    /// Surfaces push requests here; the shell drains it after every frame.
    pub outbox: Vec<Command>,
    /// Resolved dark mode for this frame (config.general.theme + system setting).
    pub dark: bool,
    /// Resolved accent RGBA.
    pub accent: [u8; 4],
}

/// Requests to the shell. Hotkeys, tray, CLI and IPC all produce these too.
#[derive(Debug)]
pub enum Command {
    CaptureRegion,
    CaptureWindow,
    CaptureFullscreen,
    CaptureScrolling,
    OcrRegion,
    RepeatLastRegion,
    RecordMp4,
    RecordGif,
    OpenHistory,
    OpenSettings,
    /// Re-open a history item in the overlay editor (annotations editable).
    EditItem(ItemId),
    PinItem(ItemId),
    PinImage {
        image: Rgba,
        title: String,
    },
    /// Overlay finished an image capture.
    CaptureDone(CaptureOutput),
    /// Overlay finished selecting a region for a non-image mode.
    RegionChosen {
        purpose: RegionPurpose,
        rect: PxRect,
        source: SourceInfo,
    },
    /// Overlay or selector was cancelled (Esc).
    Cancelled,
    /// Recording HUD actions.
    RecordPause,
    RecordResume,
    RecordStop,
    RecordCancel,
    /// Trim window finished: save [start,end) as mp4 and/or gif.
    TrimDone {
        source: std::path::PathBuf,
        start: std::time::Duration,
        end: std::time::Duration,
        as_gif: bool,
        as_mp4: bool,
    },
    /// Scrolling HUD: stop and keep what's stitched so far.
    ScrollStop,
    ScrollCancel,
    CopyItemImage(ItemId),
    CopyItemText(ItemId),
    RevealItem(ItemId),
    OpenItem(ItemId),
    DeleteItem(ItemId),
    StarItem(ItemId, bool),
    /// Show a transient message toast (errors, "Text copied", etc.).
    Notify {
        title: String,
        body: String,
        error: bool,
    },
    CheckForUpdates,
    Quit,
    /// Open a web page (the update release page) in the default browser.
    OpenUrl(String),
    /// Settings window: pause/resume global hotkeys while it records a new key combo.
    SetHotkeysSuspended(bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionPurpose {
    Ocr,
    Scrolling,
    RecordMp4,
    RecordGif,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceInfo {
    pub app: String,
    pub window_title: String,
}

/// What the user chose to do when finishing an overlay capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FinishAction {
    /// Enter / Ctrl+C: clipboard (+ history + toast per config).
    Copy,
    /// Ctrl+S: Save As dialog, also history.
    SaveAs,
    Pin,
    /// Copy recognized text instead of the image.
    CopyText,
}

#[derive(Debug)]
pub struct CaptureOutput {
    /// Unannotated pixels of the selection (window fx already applied for window mode).
    pub base: Rgba,
    pub doc: Doc,
    /// `stinkshot_core::annotate::render(base, doc)`; the overlay produces it.
    pub flattened: Rgba,
    /// Virtual-desktop rect that was captured (for repeat-last-region).
    pub rect: PxRect,
    pub action: FinishAction,
    pub source: SourceInfo,
    /// "region" | "window" | "fullscreen" | "scrolling"
    pub mode: &'static str,
    /// Set when re-editing an existing history item: overwrite it instead of adding new.
    pub editing: Option<ItemId>,
}

/// How the overlay was opened.
pub enum OverlayMode {
    /// Normal capture: drag region / click window or element, then annotate inline.
    Capture,
    /// Select region only; result is `Command::RegionChosen` with this purpose.
    SelectRegion(RegionPurpose),
    /// Already-captured image to annotate (window capture, scrolling result, full screen,
    /// re-edit). Shown centered over a dimmed frozen screen, selection = whole image.
    Annotate {
        base: Rgba,
        doc: Doc,
        mode: &'static str,
        editing: Option<ItemId>,
        source: SourceInfo,
    },
}

pub struct OverlayRequest {
    pub mode: OverlayMode,
    pub frozen: Frozen,
    /// Z-ordered window snapshot taken before the overlay appeared.
    pub windows: Vec<WinInfo>,
    pub last_region: Option<PxRect>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceStatus {
    Open,
    Close,
}

/// A window the shell shows as an egui immediate viewport.
pub trait Surface {
    /// Unique and stable for the surface's lifetime. Also used as the OS window title
    /// (so `window_util::find_own_window` can locate the HWND).
    fn title(&self) -> String;
    fn viewport_builder(&self, shared: &Shared) -> egui::ViewportBuilder;
    /// Draw one frame. Runs inside the viewport's context.
    fn show(&mut self, ctx: &egui::Context, shared: &mut Shared) -> SurfaceStatus;
    /// Called once after the OS window exists (HWND found by title): apply topmost,
    /// tool-window, mica, exclude-from-capture, exact physical rect, etc.
    fn on_created(&mut self, hwnd: stinkshot_platform::Hwnd, shared: &Shared) {
        let _ = (hwnd, shared);
    }
}
