//! Screen and window capture.
//!
//! - `freeze_all`: instant snapshot of every monitor composited into one image covering
//!   `monitors::virtual_bounds()` (gaps between monitors are opaque black). Must be fast
//!   (< 60 ms on 2x 4K): use DXGI Desktop Duplication per output (one-shot AcquireNextFrame
//!   with a short timeout, falling back to GDI BitBlt from the screen DC on failure, e.g.
//!   when duplication is unavailable). HDR outputs: convert to SDR sRGB.
//! - `capture_window`: Windows.Graphics.Capture on the HWND so covered/occluded windows
//!   capture correctly; crop to DWM extended frame bounds; returns straight alpha with
//!   transparent pixels outside the window's visible area where WGC provides them.
//!   Cursor and yellow capture border disabled when supported (IsCursorCaptureEnabled,
//!   IsBorderRequired = false).

use stinkshot_core::{PxPoint, PxRect, Rgba};

use crate::Hwnd;

#[derive(Clone, Debug)]
pub struct Frozen {
    /// Virtual-desktop rect that `image` covers (== virtual_bounds at freeze time).
    pub bounds: PxRect,
    pub image: Rgba,
    /// Cursor position at freeze time (virtual coords).
    pub cursor: PxPoint,
}

pub fn freeze_all(include_cursor: bool) -> anyhow::Result<Frozen> {
    let _ = include_cursor;
    todo!("platform::capture::freeze_all")
}

/// Live capture of a virtual-desktop rect (used by scrolling capture each step).
/// Fast path: reuse a cached duplication session between calls.
pub fn capture_rect(rect: PxRect, include_cursor: bool) -> anyhow::Result<Rgba> {
    let _ = (rect, include_cursor);
    todo!("platform::capture::capture_rect")
}

#[derive(Clone, Debug, Default)]
pub struct WindowCaptureOpts {
    pub include_cursor: bool,
}

pub fn capture_window(hwnd: Hwnd, opts: &WindowCaptureOpts) -> anyhow::Result<Rgba> {
    let _ = (hwnd, opts);
    todo!("platform::capture::capture_window")
}

/// Draw the current cursor image into `img` (whose top-left is at `origin` in virtual
/// coords). Used when include_cursor is on and the capture path didn't include it.
pub fn draw_cursor(img: &mut Rgba, origin: PxPoint) {
    let _ = (img, origin);
    todo!("platform::capture::draw_cursor")
}
