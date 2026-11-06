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

pub fn run() -> anyhow::Result<()> {
    todo!("shell::run")
}
