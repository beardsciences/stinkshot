//! Helpers the app uses on its own egui/winit windows (found by unique title, since egui
//! viewports don't expose HWNDs).

use stinkshot_core::PxRect;

use crate::Hwnd;

/// Window of THIS process with exactly this title.
pub fn find_own_window(title: &str) -> Option<Hwnd> {
    let _ = title;
    todo!("platform::window_util::find_own_window")
}

/// SetWindowPos in physical px (bypasses winit's logical sizing across mixed-DPI monitors).
pub fn set_rect(hwnd: Hwnd, rect: PxRect) {
    let _ = (hwnd, rect);
    todo!("platform::window_util::set_rect")
}

pub fn set_topmost(hwnd: Hwnd, topmost: bool) {
    let _ = (hwnd, topmost);
    todo!("platform::window_util::set_topmost")
}

/// Hide from screen capture (WDA_EXCLUDEFROMCAPTURE): recording HUD, toasts, overlays.
pub fn exclude_from_capture(hwnd: Hwnd, exclude: bool) {
    let _ = (hwnd, exclude);
    todo!("platform::window_util::exclude_from_capture")
}

/// Mica backdrop + immersive dark title bar + rounded corners (DWM attributes).
pub fn apply_mica(hwnd: Hwnd, dark: bool) {
    let _ = (hwnd, dark);
    todo!("platform::window_util::apply_mica")
}

/// Bring to foreground reliably (AllowSetForegroundWindow / AttachThreadInput dance).
pub fn force_foreground(hwnd: Hwnd) {
    let _ = hwnd;
    todo!("platform::window_util::force_foreground")
}

/// Remove from taskbar / Alt+Tab (WS_EX_TOOLWINDOW).
pub fn set_tool_window(hwnd: Hwnd, tool: bool) {
    let _ = (hwnd, tool);
    todo!("platform::window_util::set_tool_window")
}
