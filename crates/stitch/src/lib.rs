//! Scrolling capture stitcher. Pure image logic; the app drives scrolling and capture.
//!
//! Driver loop (in the app): capture region -> `push` -> if `Appended`/`Unchanged` send
//! one scroll step -> wait `scroll_delay_ms` -> repeat until `End` (two consecutive
//! unchanged frames), `max_height` reached, or the user clicks Stop. Then `finish`.
//!
//! Algorithm requirements:
//! - Detect a fixed header and footer (rows identical between consecutive frames at the
//!   same position) and exclude them from the matched band; keep the header once at the
//!   top and the footer once at the bottom of the result.
//! - Find the vertical offset between consecutive frames by matching the scrolling band
//!   (row-hash candidates, then verify with a per-pixel tolerance to survive subpixel AA
//!   and cursor blink). Handle partial last scroll (offset smaller than usual).
//! - Ignore narrow columns that change independently (scrollbar thumb): match on the
//!   middle 90% of width.
//! - Never panic on mismatched sizes; return `Failed`.

use stinkshot_core::Rgba;

#[derive(Clone, Debug)]
pub struct StitchOptions {
    pub max_height: u32,
    /// Max per-channel difference counted as "same pixel".
    pub tolerance: u8,
}

impl Default for StitchOptions {
    fn default() -> Self {
        Self { max_height: 30000, tolerance: 6 }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PushResult {
    /// First frame or new content appended; `added_rows` new rows.
    Appended { added_rows: u32 },
    /// Frame identical to previous (scroll had no effect yet).
    Unchanged,
    /// Two consecutive unchanged frames, or max height reached: stop scrolling.
    End,
    /// No reliable overlap found; caller should stop and `finish` with what it has.
    Failed,
}

pub struct Stitcher {
    opts: StitchOptions,
    #[allow(dead_code)]
    state: (),
}

impl Stitcher {
    pub fn new(opts: StitchOptions) -> Self {
        Self { opts, state: () }
    }
    pub fn options(&self) -> &StitchOptions {
        &self.opts
    }
    pub fn push(&mut self, frame: &Rgba) -> PushResult {
        let _ = frame;
        todo!("stitch::Stitcher::push")
    }
    /// Current stitched height in px (for the progress HUD).
    pub fn height(&self) -> u32 {
        todo!("stitch::Stitcher::height")
    }
    pub fn frames(&self) -> usize {
        todo!("stitch::Stitcher::frames")
    }
    pub fn finish(self) -> Rgba {
        todo!("stitch::Stitcher::finish")
    }
}
