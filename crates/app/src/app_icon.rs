//! The logo on app windows: `window_icon()` for every egui viewport (`with_icon`), and
//! `apply_native(hwnd)`, which swaps in the exe's multi-resolution resource icon (built by
//! build.rs from `icon_gen`) at the exact small/big sizes for the window's DPI so the title
//! bar, taskbar and Alt+Tab are crisp instead of a scaled bitmap.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::WindowsAndMessaging::{
    ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW, SM_CXICON, SM_CXSMICON,
    SendMessageW, WM_SETICON,
};
use windows::core::PCWSTR;

use crate::icon_gen;

/// Default accent; the window icon matches the exe icon, not a custom accent.
const ACCENT: [u8; 3] = [0x7f, 0xd3, 0x6a];
const WINDOW_ICON_SIZE: u32 = 64;

/// Shared, rendered once. The same `Arc` every frame keeps egui from re-applying it.
pub fn window_icon() -> Arc<egui::IconData> {
    static ICON: OnceLock<Arc<egui::IconData>> = OnceLock::new();
    ICON.get_or_init(|| {
        Arc::new(egui::IconData {
            rgba: icon_gen::render_rgba(WINDOW_ICON_SIZE, ACCENT),
            width: WINDOW_ICON_SIZE,
            height: WINDOW_ICON_SIZE,
        })
    })
    .clone()
}

/// Loaded icon handles by pixel size (kept for the life of the process).
fn resource_icon(px: i32) -> Option<isize> {
    static CACHE: OnceLock<Mutex<HashMap<i32, isize>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = cache.lock().ok()?;
    if let Some(h) = map.get(&px) {
        return Some(*h);
    }
    // SAFETY: loads resource id 1 (the ICON in build.rs) from our own module; the handle is
    // cached and never destroyed, so it outlives every window that uses it.
    let handle = unsafe {
        let module = GetModuleHandleW(PCWSTR::null()).ok()?;
        LoadImageW(
            Some(module.into()),
            // MAKEINTRESOURCE(1)
            PCWSTR(std::ptr::without_provenance::<u16>(1)),
            IMAGE_ICON,
            px,
            px,
            LR_DEFAULTCOLOR,
        )
        .ok()?
    };
    let h = handle.0 as isize;
    map.insert(px, h);
    Some(h)
}

/// Give a window the exe's icon at its DPI. Best effort: on failure egui's icon stays.
pub fn apply_native(hwnd: stinkshot_platform::Hwnd) {
    let hwnd = HWND(hwnd as *mut _);
    // SAFETY: plain queries and messages to a window we own.
    unsafe {
        let dpi = GetDpiForWindow(hwnd).max(96);
        for (kind, metric) in [(ICON_SMALL, SM_CXSMICON), (ICON_BIG, SM_CXICON)] {
            let px = GetSystemMetricsForDpi(metric, dpi);
            if px <= 0 {
                continue;
            }
            if let Some(h) = resource_icon(px) {
                SendMessageW(
                    hwnd,
                    WM_SETICON,
                    Some(WPARAM(kind as usize)),
                    Some(LPARAM(h)),
                );
            }
        }
    }
}
