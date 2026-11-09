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
    /// Standalone color picker: freeze, click a pixel, its color is copied.
    PickColor,
    RecordMp4,
    RecordGif,
    OpenHistory,
    OpenSettings,
    /// Open Settings on a given page, or switch an open Settings window to it.
    OpenSettingsAt(crate::settings_ui::Section),
    /// The first-run tour (also Settings > About > Show Tour).
    OpenTour,
    /// Retry hotkeys another program held at registration (Settings > Hotkeys > Check Again).
    RetryHotkeys,
    /// Re-open a history item in the overlay editor (annotations editable).
    EditItem(ItemId),
    PinItem(ItemId),
    /// Overlay finished an image capture.
    CaptureDone(Box<CaptureOutput>),
    /// Open a capture in an Editor Window (the overlay's Edit in Window, or Enter with
    /// Settings > Capture > After Selecting a Region = Open Editor Window).
    OpenEditor(Box<EditorRequest>),
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
    /// Tray icon single left click: region capture, started immediately.
    TrayClick,
    /// Tray icon double click: History. Drops the overlay the first click just opened. Also
    /// sent by that overlay when the second click lands on it instead of the icon.
    TrayDoubleClick,
    /// Soak test driver command (`stinkshot soak ...`, `soak` feature only).
    #[cfg(feature = "soak")]
    Soak(Vec<String>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionPurpose {
    Ocr,
    Scrolling,
    RecordMp4,
    RecordGif,
    /// Color picker: no region; a click copies the pixel's color and closes the overlay.
    PickColor,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceInfo {
    pub app: String,
    pub window_title: String,
    /// DPI scale of the monitor the pixels came from, when known (the measure tool's logical
    /// px readout). Not stored in history.
    pub scale: Option<f32>,
}

/// What the user chose to do when finishing an overlay capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FinishAction {
    /// Enter / Ctrl+C: clipboard (+ history + toast per config).
    Copy,
    /// Save As dialog, also history (Ctrl+S, or Ctrl+Shift+S while re-editing).
    SaveAs,
    /// Re-editing a history item, Ctrl+S: write the edits back to that item
    /// (`edit_save::write_to_history`). For a new capture it just saves to history.
    Save,
    /// Re-editing: add the edits as a new history item, leaving the original as it was.
    SaveCopy,
    Pin,
    /// Copy recognized text instead of the image.
    CopyText,
    /// Finished without the editor (window / full screen with the editor setting off):
    /// clipboard per `general.copy_to_clipboard`, history, toast whose Edit opens the editor.
    Quick,
}

#[derive(Debug)]
pub struct CaptureOutput {
    /// Unannotated pixels of the selection (window fx already applied for window mode).
    pub base: Rgba,
    pub doc: Doc,
    /// `stinkshot_core::annotate::render(base, doc)`; the overlay produces it. With a
    /// Beautify frame applied, the framed image.
    pub flattened: Rgba,
    /// Set when a Beautify frame was applied: the flattened image without it. History keeps
    /// it for re-editing, so a re-edit never frames the frame again.
    pub unframed: Option<Rgba>,
    /// Virtual-desktop rect that was captured (for repeat-last-region).
    pub rect: PxRect,
    pub action: FinishAction,
    pub source: SourceInfo,
    /// "region" | "window" | "fullscreen" | "scrolling"
    pub mode: &'static str,
    /// Set when re-editing an existing history item: overwrite it instead of adding new
    /// (except `FinishAction::SaveCopy`, see `edit_save::history_target`).
    pub editing: Option<ItemId>,
}

/// A capture to open in an Editor Window (`overlay::EditorWindow`).
#[derive(Debug)]
pub struct EditorRequest {
    pub base: Rgba,
    pub doc: Doc,
    /// "region" | "window" | "fullscreen" | "scrolling"
    pub mode: &'static str,
    /// Set when re-editing a history item: finishing overwrites it.
    pub editing: Option<ItemId>,
    pub source: SourceInfo,
    /// File name for the window title (history items).
    pub name: Option<String>,
    /// Virtual-desktop rect the pixels came from (a fresh region capture): reported as the
    /// capture's rect (repeat-last-region, pin placement) instead of the window's.
    pub rect: Option<PxRect>,
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
    /// Every visible top-level window in Z-order (untitled and own capturable ones too),
    /// for occlusion-aware snapping. Empty for Annotate mode; the overlay then falls back
    /// to `windows`.
    pub layers: Vec<stinkshot_platform::occlusion::Layer>,
    pub last_region: Option<PxRect>,
    /// Short inline hint for the hint bar until a selection exists (e.g. "No Region
    /// Remembered Yet"), instead of a toast over the frozen screen.
    pub notice: Option<&'static str>,
    /// Opened by a tray icon click: a second click on the icon within the double-click
    /// time means "History", not "select the taskbar".
    pub from_tray_click: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceStatus {
    Open,
    Close,
}

/// A window the shell shows as an egui immediate viewport.
pub trait Surface {
    /// Unique and stable for the surface's lifetime. Also used as the OS window title
    /// (so `sysutil::find_own_window` can locate the HWND).
    fn title(&self) -> String;
    fn viewport_builder(&self, shared: &Shared) -> egui::ViewportBuilder;
    /// Draw one frame. Runs inside the viewport's context.
    fn show(&mut self, ctx: &egui::Context, shared: &mut Shared) -> SurfaceStatus;
    /// Called once after the OS window exists (HWND found by title): apply topmost,
    /// tool-window, mica, exclude-from-capture, exact physical rect, etc.
    fn on_created(&mut self, hwnd: stinkshot_platform::Hwnd, shared: &Shared) {
        let _ = (hwnd, shared);
    }
    /// Called after every frame of this surface has been painted and presented. The overlay
    /// uses it to uncloak its window once a frame at the final size is on it.
    fn after_paint(&mut self) {}
    /// Paint another frame right away, in the same shell pass, instead of waiting for the
    /// next one (which can be tens of ms away). The overlay asks for it while it waits for a
    /// second present before uncloaking.
    fn wants_frame_now(&self) -> bool {
        false
    }
}
