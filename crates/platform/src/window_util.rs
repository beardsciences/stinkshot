//! Helpers the app uses on its own egui/winit windows (found by unique title, since egui
//! viewports don't expose HWNDs).

use stinkshot_core::PxRect;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::Graphics::Dwm::{
    DWM_SYSTEMBACKDROP_TYPE, DWMSBT_MAINWINDOW, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentProcessId, GetCurrentThreadId,
};
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    ASFW_ANY, AllowSetForegroundWindow, BringWindowToTop, EnumWindows, GWL_EXSTYLE,
    GetForegroundWindow, GetWindowLongPtrW, GetWindowTextW, GetWindowThreadProcessId,
    HWND_NOTOPMOST, HWND_TOPMOST, IsIconic, IsWindowVisible, SW_HIDE, SW_RESTORE, SW_SHOWNA,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow,
    SetWindowDisplayAffinity, SetWindowLongPtrW, SetWindowPos, ShowWindow, WDA_EXCLUDEFROMCAPTURE,
    WDA_NONE, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};
use windows::core::BOOL;

use crate::Hwnd;

fn h(hwnd: Hwnd) -> HWND {
    HWND(hwnd as *mut _)
}

struct FindCtx {
    title: Vec<u16>,
    pid: u32,
    found: Option<isize>,
}

unsafe extern "system" fn find_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam is the FindCtx owned by `find_own_window` for the duration of the call.
    let ctx = unsafe { &mut *(lparam.0 as *mut FindCtx) };
    let mut pid = 0u32;
    // SAFETY: out pointer is a live local.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid != ctx.pid {
        return BOOL(1);
    }
    let mut buf = vec![0u16; ctx.title.len() + 2];
    // SAFETY: buffer is a live Vec of the stated length.
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) } as usize;
    if n == ctx.title.len() && buf[..n] == ctx.title[..] {
        ctx.found = Some(hwnd.0 as isize);
        return BOOL(0);
    }
    BOOL(1)
}

/// Window of THIS process with exactly this title.
pub fn find_own_window(title: &str) -> Option<Hwnd> {
    let mut ctx = FindCtx {
        title: title.encode_utf16().collect(),
        // SAFETY: trivial query.
        pid: unsafe { GetCurrentProcessId() },
        found: None,
    };
    // EnumWindows returns an error when the callback stops enumeration early; that is expected.
    // SAFETY: ctx outlives the call and is only accessed by the callback.
    let _ = unsafe { EnumWindows(Some(find_proc), LPARAM(&mut ctx as *mut FindCtx as isize)) };
    ctx.found
}

/// SetWindowPos in physical px (bypasses winit's logical sizing across mixed-DPI monitors).
pub fn set_rect(hwnd: Hwnd, rect: PxRect) {
    let w = h(hwnd);
    // Moving first puts the window on the destination monitor, so the DPI change (and winit's
    // WM_DPICHANGED resize) happens before the final size is applied.
    // SAFETY: plain window-management calls on a caller-provided handle; failures are logged.
    unsafe {
        let flags = SWP_NOZORDER | SWP_NOACTIVATE;
        if let Err(e) = SetWindowPos(w, None, rect.x, rect.y, 0, 0, flags | SWP_NOSIZE) {
            log::debug!("set_rect move: {e}");
        }
        if let Err(e) = SetWindowPos(w, None, rect.x, rect.y, rect.w, rect.h, flags) {
            log::debug!("set_rect size: {e}");
        }
    }
}

pub fn set_topmost(hwnd: Hwnd, topmost: bool) {
    let after = if topmost {
        HWND_TOPMOST
    } else {
        HWND_NOTOPMOST
    };
    // SAFETY: plain window-management call.
    if let Err(e) = unsafe {
        SetWindowPos(
            h(hwnd),
            Some(after),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        )
    } {
        log::debug!("set_topmost: {e}");
    }
}

/// Hide from screen capture (WDA_EXCLUDEFROMCAPTURE): recording HUD, toasts, overlays.
pub fn exclude_from_capture(hwnd: Hwnd, exclude: bool) {
    let aff = if exclude {
        WDA_EXCLUDEFROMCAPTURE
    } else {
        WDA_NONE
    };
    // SAFETY: plain call on a window of this process.
    if let Err(e) = unsafe { SetWindowDisplayAffinity(h(hwnd), aff) } {
        log::warn!("SetWindowDisplayAffinity: {e}");
    }
}

fn set_dwm_u32(hwnd: HWND, attr: windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE, v: u32) {
    // SAFETY: pointer/size describe a live u32.
    let r = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            attr,
            &v as *const u32 as *const _,
            std::mem::size_of::<u32>() as u32,
        )
    };
    if let Err(e) = r {
        log::debug!("DwmSetWindowAttribute({attr:?}): {e}");
    }
}

/// Mica backdrop + immersive dark title bar + rounded corners (DWM attributes).
pub fn apply_mica(hwnd: Hwnd, dark: bool) {
    let w = h(hwnd);
    set_dwm_u32(w, DWMWA_USE_IMMERSIVE_DARK_MODE, dark as u32);
    set_dwm_u32(w, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND.0 as u32);
    let backdrop: DWM_SYSTEMBACKDROP_TYPE = DWMSBT_MAINWINDOW;
    set_dwm_u32(w, DWMWA_SYSTEMBACKDROP_TYPE, backdrop.0 as u32);
    // Sheet of glass: lets the backdrop show through wherever the client area is transparent.
    let m = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    // SAFETY: m is a live MARGINS.
    if let Err(e) = unsafe { DwmExtendFrameIntoClientArea(w, &m) } {
        log::debug!("DwmExtendFrameIntoClientArea: {e}");
    }
}

/// Bring to foreground reliably (AllowSetForegroundWindow / AttachThreadInput dance).
pub fn force_foreground(hwnd: Hwnd) {
    let w = h(hwnd);
    // SAFETY: window-management calls; thread attachment is always detached before returning.
    unsafe {
        if IsIconic(w).as_bool() {
            let _ = ShowWindow(w, SW_RESTORE);
        }
        let _ = AllowSetForegroundWindow(ASFW_ANY);
        let fg = GetForegroundWindow();
        let cur_thread = GetCurrentThreadId();
        let fg_thread = if fg.0.is_null() {
            0
        } else {
            GetWindowThreadProcessId(fg, None)
        };
        let attached = fg_thread != 0
            && fg_thread != cur_thread
            && AttachThreadInput(cur_thread, fg_thread, true).as_bool();
        let _ = BringWindowToTop(w);
        if !SetForegroundWindow(w).as_bool() {
            log::debug!("SetForegroundWindow refused");
        }
        let _ = SetFocus(Some(w));
        if attached {
            let _ = AttachThreadInput(cur_thread, fg_thread, false);
        }
    }
}

/// Remove from taskbar / Alt+Tab (WS_EX_TOOLWINDOW).
pub fn set_tool_window(hwnd: Hwnd, tool: bool) {
    let w = h(hwnd);
    // SAFETY: style read-modify-write on a window handle.
    unsafe {
        let ex = GetWindowLongPtrW(w, GWL_EXSTYLE) as u32;
        let new = if tool {
            (ex | WS_EX_TOOLWINDOW.0) & !WS_EX_APPWINDOW.0
        } else {
            (ex & !WS_EX_TOOLWINDOW.0) | WS_EX_APPWINDOW.0
        };
        if new == ex {
            return;
        }
        let visible = IsWindowVisible(w).as_bool();
        // The shell only re-reads the style when the window is re-shown.
        if visible {
            let _ = ShowWindow(w, SW_HIDE);
        }
        SetWindowLongPtrW(w, GWL_EXSTYLE, new as isize);
        let _ = SetWindowPos(
            w,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        if visible {
            let _ = ShowWindow(w, SW_SHOWNA);
        }
    }
}
