//! Soak test hooks (`soak` feature only, never in release builds). The soak driver
//! (`C:\dev\stinkshot-bench\soak`) runs the app's real capture, scrolling, recording and
//! export paths against its own test window without synthetic input:
//!
//!   stinkshot soak window <hwnd>          window capture of that window (WGC), quick finish
//!   stinkshot soak scroll <hwnd>          scrolling capture of the window's rect; wheel
//!                                         notches are posted to that window, no system input
//!   stinkshot soak record <hwnd> mp4|gif  record the window's rect (countdown per config)
//!   stinkshot soak trim <start_ms> <end_ms>  the Trim window's Save, as MP4 and GIF
//!   stinkshot soak search <text>          open History (if closed) and search for <text>
//!
//! The shell writes its state to `<config dir>\soak-state.json` whenever it changes, so the
//! driver can wait for a recording to reach the Trim window or an export to finish.

use std::time::Duration;

use stinkshot_core::{PxPoint, PxRect};
use stinkshot_platform::{Hwnd, windows_enum};

use super::{Rec, Shell, Slot};
use crate::history_ui::HistoryWindow;
use crate::state::RegionPurpose;
use crate::workers::{self, Wheel};

#[derive(Default)]
pub(super) struct SoakState {
    /// Wheel sink for the next scrolling capture.
    pub wheel: Option<Hwnd>,
    /// The scrolling capture running now was started by `soak scroll`: finish it without the
    /// editor (Quick), like a window capture with the editor off.
    pub quick_scroll: bool,
    /// Soak commands handled, and the last one that was refused (for the driver's log).
    pub handled: u64,
    pub refused: u64,
    pub last_written: String,
}

/// Post `notches` wheel notches (negative = down) to `hwnd` at screen point `at`, one message
/// per notch, like `input::scroll_at` sends them, but to that window only.
pub(crate) fn post_wheel(hwnd: Hwnd, at: PxPoint, notches: i32) {
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_MOUSEWHEEL};
    let delta: i16 = if notches > 0 { 120 } else { -120 };
    let wparam = WPARAM(((delta as u16 as usize) << 16) & 0xffff_0000);
    let lparam = LPARAM((((at.y as u16 as u32) << 16) | (at.x as u16 as u32)) as isize);
    for _ in 0..notches.unsigned_abs() {
        // SAFETY: posting a plain message; a stale handle only makes the call fail.
        if let Err(e) =
            unsafe { PostMessageW(Some(HWND(hwnd as *mut _)), WM_MOUSEWHEEL, wparam, lparam) }
        {
            log::warn!("soak: post wheel failed: {e}");
            return;
        }
    }
    std::thread::sleep(Duration::from_millis(25));
}

fn parse_hwnd(s: Option<&String>) -> Option<Hwnd> {
    let s = s?;
    let v = if let Some(hex) = s.strip_prefix("0x") {
        isize::from_str_radix(hex, 16).ok()?
    } else {
        s.parse().ok()?
    };
    (v != 0).then_some(v)
}

fn rect_of(hwnd: Hwnd) -> Option<PxRect> {
    windows_enum::info(hwnd)
        .map(|w| w.rect)
        .filter(|r| r.w >= 16 && r.h >= 16)
}

fn rec_name(rec: &Rec) -> &'static str {
    match rec {
        Rec::Idle => "idle",
        Rec::Countdown(_) => "countdown",
        Rec::Starting { .. } => "starting",
        Rec::Active { .. } => "active",
        Rec::Stopping(_) => "stopping",
        Rec::Trim { .. } => "trim",
    }
}

impl Shell {
    pub(super) fn next_wheel(&mut self) -> Wheel {
        match self.soak.wheel.take() {
            Some(h) => Wheel::Post(h),
            None => Wheel::Input,
        }
    }

    pub(super) fn soak(&mut self, args: Vec<String>) {
        let ok = self.soak_inner(&args);
        if ok {
            self.soak.handled += 1;
        } else {
            self.soak.refused += 1;
            log::warn!("soak: refused {args:?}");
        }
    }

    fn soak_inner(&mut self, args: &[String]) -> bool {
        let verb = args.first().map(String::as_str).unwrap_or("");
        match verb {
            "window" => {
                let Some(win) = parse_hwnd(args.get(1)).and_then(windows_enum::info) else {
                    return false;
                };
                if self.capture_blocked() {
                    return false;
                }
                let Some((frozen, windows)) = self.freeze() else {
                    return false;
                };
                self.window_capture_pending = true;
                workers::capture_window(&self.env(), frozen, windows, win);
                true
            }
            "scroll" => {
                let Some(hwnd) = parse_hwnd(args.get(1)) else {
                    return false;
                };
                let Some(rect) = rect_of(hwnd) else {
                    return false;
                };
                if self.capture_blocked() {
                    return false;
                }
                self.soak.wheel = Some(hwnd);
                self.soak.quick_scroll = true;
                let source = windows_enum::info(hwnd)
                    .map(|w| crate::capture_ops::source_of(&w))
                    .unwrap_or_default();
                self.region_chosen(RegionPurpose::Scrolling, rect, source);
                self.scroll.is_some()
            }
            "record" => {
                let Some(rect) = parse_hwnd(args.get(1)).and_then(rect_of) else {
                    return false;
                };
                if !matches!(self.rec, Rec::Idle) || self.overlay.is_some() {
                    return false;
                }
                let purpose = if args.get(2).is_some_and(|a| a == "gif") {
                    RegionPurpose::RecordGif
                } else {
                    RegionPurpose::RecordMp4
                };
                self.region_chosen(purpose, rect, Default::default());
                true
            }
            "trim" => {
                let ms = |i: usize| args.get(i).and_then(|v| v.parse::<u64>().ok());
                let (Some(a), Some(b)) = (ms(1), ms(2)) else {
                    return false;
                };
                if !matches!(
                    self.rec,
                    Rec::Trim {
                        done_issued: false,
                        ..
                    }
                ) {
                    return false;
                }
                self.trim_done(
                    Duration::from_millis(a),
                    Duration::from_millis(b),
                    true,
                    true,
                );
                true
            }
            "search" => {
                let text = args[1..].join(" ");
                if self.history_win.is_none() {
                    self.history_win = Some(Slot::new(HistoryWindow::new(&self.shared)));
                }
                if let Some(h) = &mut self.history_win {
                    h.s.soak_search(&text);
                }
                self.touch();
                true
            }
            _ => false,
        }
    }

    /// Write `soak-state.json` when anything the driver waits on changed.
    pub(super) fn soak_write_state(&mut self) {
        let state = serde_json::json!({
            "rec": rec_name(&self.rec),
            "trim_open": self.trim.is_some(),
            "exporting": self.exporting,
            "scroll": self.scroll.is_some(),
            "overlay": self.overlay.is_some(),
            "history": self.history_win.is_some(),
            "window_pending": self.window_capture_pending,
            "toasts": self.toasts.len(),
            "jobs": workers::jobs_in_flight(),
            "handled": self.soak.handled,
            "refused": self.soak.refused,
        })
        .to_string();
        if state == self.soak.last_written {
            return;
        }
        let path = stinkshot_core::config::Config::dir().join("soak-state.json");
        let tmp = path.with_extension("json.tmp");
        let res = std::fs::write(&tmp, &state).and_then(|()| std::fs::rename(&tmp, &path));
        match res {
            Ok(()) => self.soak.last_written = state,
            Err(e) => log::warn!("soak: write state: {e}"),
        }
    }
}
