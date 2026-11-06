use stinkshot_core::{PxPoint, PxRect};

use crate::Hwnd;

#[derive(Clone, Debug, PartialEq)]
pub struct WinInfo {
    pub hwnd: Hwnd,
    /// Visible frame (DWMWA_EXTENDED_FRAME_BOUNDS), physical virtual coords.
    pub rect: PxRect,
    pub title: String,
    /// Process image name without ".exe", e.g. "firefox".
    pub process: String,
    pub pid: u32,
    /// True when the window is maximized (no rounded corners on Win11).
    pub maximized: bool,
}

/// Visible, non-cloaked, non-minimized top-level windows in Z-order (topmost first),
/// excluding this process's windows, tool windows with no title, and the desktop/shell
/// (Progman/WorkerW/Shell_TrayWnd are excluded; the taskbar is included as "taskbar").
pub fn visible_windows() -> Vec<WinInfo> {
    todo!("platform::windows_enum::visible_windows")
}

pub fn foreground() -> Option<WinInfo> {
    todo!("platform::windows_enum::foreground")
}

/// Topmost window from `windows` containing `p` (pure helper over a snapshot).
pub fn window_at(windows: &[WinInfo], p: PxPoint) -> Option<&WinInfo> {
    windows.iter().find(|w| w.rect.contains(p))
}

pub fn info(hwnd: Hwnd) -> Option<WinInfo> {
    let _ = hwnd;
    todo!("platform::windows_enum::info")
}
