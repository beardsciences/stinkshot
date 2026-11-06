//! Recording and scrolling HUDs (docs/design/Recording.dc.html, Pin.dc.html):
//! - `Countdown`: big 3-2-1 circle centered on the region; the shell polls `finished()`;
//!   Esc cancels (pushes `Command::RecordCancel`).
//! - `RecordFrame`: dashed red border around the region, click-through, excluded from capture.
//! - `RecordHud`: bar under (or above, if no room) the region: red dot, elapsed, pause/resume,
//!   mic, system audio, webcam, clicks/keys toggles, Stop. Excluded from capture. Toggle
//!   changes are read by the shell each frame via `take_toggle_changes`.
//! - `TrimWindow`: filmstrip (record::thumbnails), draggable in/out handles, preview frame,
//!   Discard / Save GIF / Save MP4 -> `Command::TrimDone`.
//! - `ScrollHud`: "Stitching frame N", W x H, progress, Cancel / Stop here.

use std::path::PathBuf;

use stinkshot_core::PxRect;
use stinkshot_record::{MediaInfo, Status};

use crate::state::{Shared, Surface, SurfaceStatus};

pub struct Countdown {
    #[allow(dead_code)]
    region: PxRect,
}

impl Countdown {
    pub fn new(seconds: u32, region: PxRect) -> Self {
        let _ = seconds;
        Self { region }
    }
    pub fn finished(&self) -> bool {
        todo!("record_ui::Countdown::finished")
    }
}

impl Surface for Countdown {
    fn title(&self) -> String {
        "StinkShot.Countdown".into()
    }
    fn viewport_builder(&self, shared: &Shared) -> egui::ViewportBuilder {
        let _ = shared;
        todo!("record_ui::Countdown::viewport_builder")
    }
    fn show(&mut self, ctx: &egui::Context, shared: &mut Shared) -> SurfaceStatus {
        let _ = (ctx, shared);
        todo!("record_ui::Countdown::show")
    }
}

pub struct RecordFrame {
    #[allow(dead_code)]
    region: PxRect,
}

impl RecordFrame {
    pub fn new(region: PxRect) -> Self {
        Self { region }
    }
}

impl Surface for RecordFrame {
    fn title(&self) -> String {
        "StinkShot.RecordFrame".into()
    }
    fn viewport_builder(&self, shared: &Shared) -> egui::ViewportBuilder {
        let _ = shared;
        todo!("record_ui::RecordFrame::viewport_builder")
    }
    fn show(&mut self, ctx: &egui::Context, shared: &mut Shared) -> SurfaceStatus {
        let _ = (ctx, shared);
        todo!("record_ui::RecordFrame::show")
    }
}

/// Live toggles the HUD exposes; the shell applies changes to the Recorder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HudToggles {
    pub mic: bool,
    pub system: bool,
    pub webcam: bool,
    pub overlays: bool,
}

pub struct RecordHud {
    #[allow(dead_code)]
    region: PxRect,
}

impl RecordHud {
    /// `summary` e.g. "MP4 · NVENC H.264 · 60 fps · mic + system".
    pub fn new(region: PxRect, summary: String, toggles: HudToggles) -> Self {
        let _ = (summary, toggles);
        Self { region }
    }
    pub fn update_status(&mut self, status: Status) {
        let _ = status;
    }
    /// Some(new) if the user changed a toggle since the last call.
    pub fn take_toggle_changes(&mut self) -> Option<HudToggles> {
        None
    }
}

impl Surface for RecordHud {
    fn title(&self) -> String {
        "StinkShot.RecordHud".into()
    }
    fn viewport_builder(&self, shared: &Shared) -> egui::ViewportBuilder {
        let _ = shared;
        todo!("record_ui::RecordHud::viewport_builder")
    }
    fn show(&mut self, ctx: &egui::Context, shared: &mut Shared) -> SurfaceStatus {
        let _ = (ctx, shared);
        todo!("record_ui::RecordHud::show")
    }
}

pub struct TrimWindow {
    #[allow(dead_code)]
    source: PathBuf,
}

impl TrimWindow {
    /// `prefer_gif`: recording was started as GIF, so "Save GIF" is the primary button.
    pub fn new(source: PathBuf, info: MediaInfo, prefer_gif: bool) -> Self {
        let _ = (info, prefer_gif);
        Self { source }
    }
    /// Shell reports export progress (0..1) while a TrimDone job runs; None = idle.
    pub fn set_progress(&mut self, p: Option<f32>) {
        let _ = p;
    }
}

impl Surface for TrimWindow {
    fn title(&self) -> String {
        "StinkShot Trim".into()
    }
    fn viewport_builder(&self, shared: &Shared) -> egui::ViewportBuilder {
        let _ = shared;
        todo!("record_ui::TrimWindow::viewport_builder")
    }
    fn show(&mut self, ctx: &egui::Context, shared: &mut Shared) -> SurfaceStatus {
        let _ = (ctx, shared);
        todo!("record_ui::TrimWindow::show")
    }
}

pub struct ScrollHud {
    #[allow(dead_code)]
    region: PxRect,
}

impl ScrollHud {
    pub fn new(region: PxRect) -> Self {
        Self { region }
    }
    pub fn update(&mut self, frames: usize, width: u32, height: u32) {
        let _ = (frames, width, height);
    }
}

impl Surface for ScrollHud {
    fn title(&self) -> String {
        "StinkShot.ScrollHud".into()
    }
    fn viewport_builder(&self, shared: &Shared) -> egui::ViewportBuilder {
        let _ = shared;
        todo!("record_ui::ScrollHud::viewport_builder")
    }
    fn show(&mut self, ctx: &egui::Context, shared: &mut Shared) -> SurfaceStatus {
        let _ = (ctx, shared);
        todo!("record_ui::ScrollHud::show")
    }
}
