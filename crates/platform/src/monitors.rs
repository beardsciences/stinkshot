use stinkshot_core::{PxPoint, PxRect};
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};
use windows::core::BOOL;

#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    /// HMONITOR value.
    pub handle: isize,
    /// Full monitor rect, physical px, virtual-desktop coords.
    pub rect: PxRect,
    /// Work area (excludes taskbar).
    pub work: PxRect,
    /// DPI scale, 1.0 = 96 dpi.
    pub scale: f32,
    /// Device name like "\\.\DISPLAY1".
    pub name: String,
    pub primary: bool,
}

pub(crate) fn rect_from_win(r: &RECT) -> PxRect {
    PxRect::new(
        r.left,
        r.top,
        (r.right - r.left).max(0),
        (r.bottom - r.top).max(0),
    )
}

unsafe extern "system" fn enum_proc(
    hmon: HMONITOR,
    _hdc: HDC,
    _rc: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    // SAFETY: lparam is the address of the Vec passed by `list`, alive for the whole call.
    let out = unsafe { &mut *(lparam.0 as *mut Vec<Monitor>) };
    if let Some(m) = describe(hmon) {
        out.push(m);
    }
    BOOL(1)
}

fn describe(hmon: HMONITOR) -> Option<Monitor> {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    // SAFETY: cbSize is set for the EX struct, whose first member is MONITORINFO.
    let ok = unsafe { GetMonitorInfoW(hmon, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) };
    if !ok.as_bool() {
        return None;
    }
    let (mut dx, mut dy) = (96u32, 96u32);
    // SAFETY: out pointers reference locals.
    if unsafe { GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) }.is_err() || dx == 0 {
        dx = 96;
    }
    Some(Monitor {
        handle: hmon.0 as isize,
        rect: rect_from_win(&info.monitorInfo.rcMonitor),
        work: rect_from_win(&info.monitorInfo.rcWork),
        scale: dx as f32 / 96.0,
        name: crate::util::from_wide(&info.szDevice),
        primary: info.monitorInfo.dwFlags & 1 != 0,
    })
}

/// All attached monitors, primary first, then left-to-right / top-to-bottom.
pub fn list() -> Vec<Monitor> {
    let mut out: Vec<Monitor> = Vec::new();
    // SAFETY: the callback only dereferences the Vec pointer for the duration of the call.
    let ok = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(enum_proc),
            LPARAM(&mut out as *mut Vec<Monitor> as isize),
        )
    };
    if !ok.as_bool() {
        log::warn!("EnumDisplayMonitors failed");
    }
    out.sort_by_key(|m| (!m.primary, m.rect.x, m.rect.y));
    out
}

/// Bounding rect of all monitors.
pub fn virtual_bounds() -> PxRect {
    let mons = list();
    if let Some(first) = mons.first() {
        return mons
            .iter()
            .skip(1)
            .fold(first.rect, |acc, m| acc.union(&m.rect));
    }
    // SAFETY: GetSystemMetrics has no preconditions.
    unsafe {
        PxRect::new(
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

pub fn at_point(p: PxPoint) -> Option<Monitor> {
    list().into_iter().find(|m| m.rect.contains(p))
}
