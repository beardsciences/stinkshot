use std::collections::HashMap;

use stinkshot_core::{PxPoint, PxRect};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetClassNameW, GetForegroundWindow, GetWindowLongPtrW, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow,
    IsWindowVisible, IsZoomed, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TRANSPARENT,
};
use windows::core::{BOOL, PWSTR};

use crate::Hwnd;
use crate::monitors::rect_from_win;

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

fn to_hwnd(h: Hwnd) -> HWND {
    HWND(h as *mut _)
}

fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: out pointer is a live local.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    // SAFETY: buffer is a live local.
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    if n <= 0 {
        String::new()
    } else {
        String::from_utf16_lossy(&buf[..n as usize])
    }
}

fn window_title(hwnd: HWND) -> String {
    // SAFETY: simple queries on a window handle; failures yield 0.
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; len as usize + 1];
    // SAFETY: buffer sized for len+1 characters.
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
    if n <= 0 {
        String::new()
    } else {
        String::from_utf16_lossy(&buf[..n as usize])
    }
}

/// The visible frame rect, falling back to GetWindowRect.
pub(crate) fn frame_rect(hwnd: HWND) -> Option<PxRect> {
    let mut r = RECT::default();
    // SAFETY: out buffer is a RECT of the exact size requested.
    let ok = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut RECT as *mut _,
            std::mem::size_of::<RECT>() as u32,
        )
    };
    if ok.is_err() {
        // SAFETY: out pointer is a live local.
        unsafe { GetWindowRect(hwnd, &mut r) }.ok()?;
    }
    Some(rect_from_win(&r))
}

fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked: u32 = 0;
    // SAFETY: out buffer is a u32 of the requested size.
    let ok = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut _,
            std::mem::size_of::<u32>() as u32,
        )
    };
    ok.is_ok() && cloaked != 0
}

fn process_name(pid: u32, cache: &mut HashMap<u32, String>) -> String {
    if let Some(n) = cache.get(&pid) {
        return n.clone();
    }
    let mut name = String::new();
    // SAFETY: handle is closed below; buffer length is passed in/out.
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            if QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
                .is_ok()
            {
                let full = String::from_utf16_lossy(&buf[..len as usize]);
                name = std::path::Path::new(&full)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
            }
            let _ = CloseHandle(h);
        }
    }
    cache.insert(pid, name.clone());
    name
}

fn build(hwnd: HWND, cache: &mut HashMap<u32, String>) -> Option<WinInfo> {
    let rect = frame_rect(hwnd)?;
    let mut pid = 0u32;
    // SAFETY: out pointer is a live local.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    // SAFETY: simple query.
    let maximized = unsafe { IsZoomed(hwnd) }.as_bool();
    Some(WinInfo {
        hwnd: hwnd.0 as isize,
        rect,
        title: window_title(hwnd),
        process: process_name(pid, cache),
        pid,
        maximized,
    })
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam is the address of the Vec in `visible_windows`.
    let out = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
    out.push(hwnd);
    BOOL(1)
}

/// Visible, non-cloaked, non-minimized top-level windows in Z-order (topmost first),
/// excluding this process's windows, tool windows with no title, and the desktop/shell
/// (Progman/WorkerW/Shell_TrayWnd are excluded; the taskbar is included as "taskbar").
pub fn visible_windows() -> Vec<WinInfo> {
    let mut handles: Vec<HWND> = Vec::new();
    // SAFETY: the callback only touches the Vec for the duration of the call.
    if let Err(e) = unsafe {
        EnumWindows(
            Some(enum_proc),
            LPARAM(&mut handles as *mut Vec<HWND> as isize),
        )
    } {
        log::warn!("EnumWindows failed: {e}");
        return Vec::new();
    }
    // SAFETY: trivial query.
    let own_pid = unsafe { GetCurrentProcessId() };
    let mut cache = HashMap::new();
    let mut out = Vec::new();
    for hwnd in handles {
        // SAFETY: trivial queries on handles returned by EnumWindows.
        if !unsafe { IsWindowVisible(hwnd) }.as_bool() || unsafe { IsIconic(hwnd) }.as_bool() {
            continue;
        }
        if is_cloaked(hwnd) {
            continue;
        }
        // Own windows are never targets. Skip them before anything that reads their title:
        // GetWindowTextW on a window of this process sends WM_GETTEXT synchronously and
        // blocks forever on helper windows whose thread does not pump messages.
        if window_pid(hwnd) == own_pid {
            continue;
        }
        let class = class_name(hwnd);
        if matches!(class.as_str(), "Progman" | "WorkerW") {
            continue;
        }
        let taskbar = matches!(class.as_str(), "Shell_TrayWnd" | "Shell_SecondaryTrayWnd");
        let Some(mut info) = build(hwnd, &mut cache) else {
            continue;
        };
        if info.pid == own_pid || info.rect.w <= 0 || info.rect.h <= 0 {
            continue;
        }
        if taskbar {
            info.title = "taskbar".to_string();
            info.process = "taskbar".to_string();
        } else {
            // Untitled windows (tool windows, helper surfaces) are never useful capture targets.
            if info.title.is_empty() {
                continue;
            }
            // Click-through layered windows (game/GPU overlays) are not real targets either.
            // SAFETY: trivial style query.
            let ex = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
            let layered = ex & WS_EX_LAYERED.0 != 0;
            let click_through = ex & WS_EX_TRANSPARENT.0 != 0 && layered;
            // Non-activating layered tool windows are GPU/game overlays (e.g. the NVIDIA overlay
            // is a full-screen CEF widget with exactly these styles).
            let overlay = ex & WS_EX_NOACTIVATE.0 != 0 && ex & WS_EX_TOOLWINDOW.0 != 0 && layered;
            if click_through || overlay {
                continue;
            }
        }
        out.push(info);
    }
    out
}

pub fn foreground() -> Option<WinInfo> {
    // SAFETY: trivial query.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }
    // Our own windows (tray host, overlays, toasts) are not capture targets, and reading
    // their titles can block.
    // SAFETY: trivial query.
    if window_pid(hwnd) == unsafe { GetCurrentProcessId() } {
        return None;
    }
    info(hwnd.0 as isize)
}

/// Topmost window from `windows` containing `p` (pure helper over a snapshot).
pub fn window_at(windows: &[WinInfo], p: PxPoint) -> Option<&WinInfo> {
    windows.iter().find(|w| w.rect.contains(p))
}

pub fn info(hwnd: Hwnd) -> Option<WinInfo> {
    let h = to_hwnd(hwnd);
    // SAFETY: trivial query.
    if hwnd == 0 || !unsafe { IsWindow(Some(h)) }.as_bool() {
        return None;
    }
    build(h, &mut HashMap::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(hwnd: isize, r: PxRect) -> WinInfo {
        WinInfo {
            hwnd,
            rect: r,
            title: String::new(),
            process: String::new(),
            pid: 1,
            maximized: false,
        }
    }

    #[test]
    fn window_at_picks_topmost() {
        let ws = vec![
            w(1, PxRect::new(0, 0, 100, 100)),
            w(2, PxRect::new(0, 0, 500, 500)),
        ];
        assert_eq!(
            window_at(&ws, PxPoint { x: 50, y: 50 }).map(|x| x.hwnd),
            Some(1)
        );
        assert_eq!(
            window_at(&ws, PxPoint { x: 300, y: 300 }).map(|x| x.hwnd),
            Some(2)
        );
        assert!(window_at(&ws, PxPoint { x: 600, y: 600 }).is_none());
    }

    #[test]
    fn info_rejects_null() {
        assert!(info(0).is_none());
    }
}
