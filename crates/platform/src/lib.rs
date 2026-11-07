//! Windows platform layer. Every function here works in PHYSICAL pixels in virtual-desktop
//! coordinates (the process is Per-Monitor-V2 DPI aware; call `dpi::init` first thing in
//! main). Window handles cross the API as `Hwnd` (raw isize) so callers don't need the
//! `windows` crate.
//!
//! Threading: functions are callable from any thread unless noted. COM is initialized
//! per-thread internally where needed (MTA for workers; UIA and OCR are used off the UI
//! thread by the app).

pub mod capture;
pub mod clipboard;
pub mod dpi;
pub mod hdr;
pub mod input;
pub mod monitors;
pub mod occlusion;
pub mod ocr;
pub mod snip;
pub mod system;
pub mod uia;
mod util;
pub mod window_util;
pub mod windows_enum;

pub use util::keep_mta_alive;

/// Raw HWND value.
pub type Hwnd = isize;

pub use stinkshot_core::{PxPoint, PxRect, Rgba};
