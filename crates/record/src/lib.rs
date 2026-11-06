//! Screen recording.
//!
//! Pipeline (all on worker threads; `Recorder` methods never block the UI for long):
//! - Video: Windows.Graphics.Capture of the monitor containing `region` (or each monitor
//!   the region spans, composited) -> crop -> overlays (click ripples, key labels, webcam
//!   bubble) composited on the GPU or CPU -> NV12 -> Media Foundation SinkWriter, H.264
//!   with hardware MFT preferred (MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS; NVENC on the
//!   user's RTX 3060 Ti), software fallback. Constant frame rate output: duplicate the last
//!   frame when WGC delivers nothing new (static screen), drop when early.
//! - Audio: WASAPI loopback (default render device) and/or microphone (capture device),
//!   resampled to 48 kHz stereo float, mixed, encoded AAC 192 kbps into the same MP4.
//!   Each source can be muted live (writes silence, keeps sync).
//! - Webcam: Media Foundation source reader on the chosen device, latest frame kept,
//!   drawn as a circle (with white 2px ring) in the configured corner.
//! - Click/key visualizer: `stinkshot_platform::input::InputHook`; clicks become expanding
//!   accent-colored ripples (~400 ms); keys show as a bottom-center chip of current combo
//!   for ~1.2 s after release.
//! - Pause/resume: while paused nothing is written and the timeline offset grows, so the
//!   output is one continuous file without a gap.
//! - Dimensions are rounded down to even numbers (H.264 4:2:0 requirement).

use std::path::{Path, PathBuf};
use std::time::Duration;

use stinkshot_core::{PxRect, Rgba};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    #[default]
    BottomRight,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WebcamOptions {
    /// None = default device; otherwise a name from `list_webcams`.
    pub device: Option<String>,
    pub diameter: u32,
    pub corner: Corner,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecordOptions {
    /// Virtual-desktop physical px.
    pub region: PxRect,
    /// Final .mp4 path (written to a temp sibling and renamed on stop).
    pub output: PathBuf,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub system_audio: bool,
    pub microphone: bool,
    /// None = default device; otherwise a name from `list_microphones`.
    pub mic_device: Option<String>,
    pub webcam: Option<WebcamOptions>,
    pub show_clicks: bool,
    pub show_keys: bool,
    pub include_cursor: bool,
    /// RGBA used for click ripples.
    pub accent: [u8; 4],
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecordingResult {
    pub path: PathBuf,
    pub duration: Duration,
    pub width: u32,
    pub height: u32,
    /// Human-readable encoder actually used, e.g. "NVIDIA H.264 Encoder MFT (hardware)".
    pub encoder: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub elapsed: Duration,
    pub paused: bool,
    pub frames: u64,
    pub dropped: u64,
    /// Set if a worker hit a fatal error; the UI should stop and report it.
    pub error: Option<String>,
}

pub struct Recorder {
    #[allow(dead_code)]
    inner: (),
}

impl Recorder {
    pub fn start(opts: RecordOptions) -> anyhow::Result<Recorder> {
        let _ = opts;
        todo!("record::Recorder::start")
    }
    pub fn pause(&self) {
        todo!("record::Recorder::pause")
    }
    pub fn resume(&self) {
        todo!("record::Recorder::resume")
    }
    pub fn status(&self) -> Status {
        todo!("record::Recorder::status")
    }
    pub fn set_mic_muted(&self, muted: bool) {
        let _ = muted;
        todo!("record::Recorder::set_mic_muted")
    }
    pub fn set_system_muted(&self, muted: bool) {
        let _ = muted;
        todo!("record::Recorder::set_system_muted")
    }
    pub fn set_webcam_visible(&self, visible: bool) {
        let _ = visible;
        todo!("record::Recorder::set_webcam_visible")
    }
    pub fn set_overlays(&self, clicks: bool, keys: bool) {
        let _ = (clicks, keys);
        todo!("record::Recorder::set_overlays")
    }
    /// Finalize the file. Blocks until the writer flushes (call from a worker thread).
    pub fn stop(self) -> anyhow::Result<RecordingResult> {
        todo!("record::Recorder::stop")
    }
    /// Abort and delete the partial file.
    pub fn cancel(self) {
        todo!("record::Recorder::cancel")
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MediaInfo {
    pub duration: Duration,
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    pub has_audio: bool,
}

pub fn probe(input: &Path) -> anyhow::Result<MediaInfo> {
    let _ = input;
    todo!("record::probe")
}

/// `count` evenly spaced frames scaled to `height` px tall (for the trim filmstrip).
pub fn thumbnails(input: &Path, count: usize, height: u32) -> anyhow::Result<Vec<Rgba>> {
    let _ = (input, count, height);
    todo!("record::thumbnails")
}

/// Single frame at `at` (for the trim preview).
pub fn frame_at(input: &Path, at: Duration, max_height: u32) -> anyhow::Result<Rgba> {
    let _ = (input, at, max_height);
    todo!("record::frame_at")
}

/// Cut [start, end) into `output` (re-encodes so cuts are frame-accurate; keeps audio).
/// `progress` receives 0.0..=1.0.
pub fn trim(input: &Path, output: &Path, start: Duration, end: Duration, progress: &dyn Fn(f32)) -> anyhow::Result<()> {
    let _ = (input, output, start, end, progress);
    todo!("record::trim")
}

#[derive(Clone, Debug, PartialEq)]
pub struct GifOptions {
    pub fps: u32,
    /// Downscale so width <= this (0 = keep).
    pub max_width: u32,
    pub start: Duration,
    /// None = to the end.
    pub end: Option<Duration>,
    /// imagequant quality 0..=100.
    pub quality: u8,
    pub loop_forever: bool,
}

/// Decode `input` (MP4) and write an optimized GIF: per-frame imagequant palette with
/// dithering, frame-diff transparency to shrink unchanged areas.
pub fn to_gif(input: &Path, output: &Path, opts: &GifOptions, progress: &dyn Fn(f32)) -> anyhow::Result<()> {
    let _ = (input, output, opts, progress);
    todo!("record::to_gif")
}

/// Friendly device names.
pub fn list_microphones() -> Vec<String> {
    todo!("record::list_microphones")
}

pub fn list_webcams() -> Vec<String> {
    todo!("record::list_webcams")
}
