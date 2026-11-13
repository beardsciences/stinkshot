//! A plain GDI window that shows a frozen screen while the real overlay is not ready yet.
//!
//! On a cold start the region overlay waits for the GPU: the D3D driver load alone is about
//! 0.3 s, then eframe and the overlay window take another 0.2 s. The screen itself is frozen
//! with GDI within about 0.15 s of the launch, so the app shows that picture at once in this
//! window (dimmed like the overlay, the window under the cursor clear, crosshair cursor) and
//! hands over to the real overlay when it is up (docs/notes/app-shell.md, "Cold start").
//!
//! It takes the first input so nothing reaches the apps underneath: a drag selects (drawn as
//! a clear rectangle with an accent border) and Esc cancels. `input` reports what happened
//! so the overlay can continue the drag or use the finished rectangle.

use std::sync::{Arc, Mutex};

use stinkshot_core::{PxPoint, PxRect, Rgba};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, BitBlt, CreateCompatibleDC, CreateDIBSection,
    CreateSolidBrush, DIB_RGB_COLORS, DeleteDC, DeleteObject, EndPaint, FillRect, HBITMAP, HDC,
    HGDIOBJ, InvalidateRect, PAINTSTRUCT, SRCCOPY, SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWLP_USERDATA,
    GetMessageW, GetWindowLongPtrW, IDC_CROSS, LoadCursorW, MSG, PostMessageW, PostQuitMessage,
    RegisterClassW, SW_SHOW, SetWindowDisplayAffinity, SetWindowLongPtrW, ShowWindow,
    TranslateMessage, WDA_EXCLUDEFROMCAPTURE, WM_CLOSE, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCCREATE, WM_PAINT, WNDCLASSW, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::PCWSTR;

use crate::Hwnd;
use crate::dpi::ThreadPerMonitorAware;

/// Black veil over the frozen screen, as the overlay's `overlay_dim` (alpha 148 of 255).
const DIM_ALPHA: u32 = 148;
/// Pointer travel (px) below which a press and release is a click, not a drag.
const CLICK_SLOP: i32 = 4;

/// What the user did in the quick overlay so far (virtual-desktop px).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QuickInput {
    /// Where the left button went down, if it did.
    pub press: Option<PxPoint>,
    /// Last pointer position seen.
    pub cursor: Option<PxPoint>,
    /// The button is still down.
    pub down: bool,
    /// Where the button went up again (the end of a finished drag).
    pub release: Option<PxPoint>,
    /// Where a plain click (press and release without a drag) landed: the overlay takes the
    /// window highlighted under it, like a click in the region overlay.
    pub clicked: Option<PxPoint>,
    /// Esc was pressed.
    pub cancelled: bool,
}

/// The window; closed when dropped.
pub struct QuickOverlay {
    hwnd: Hwnd,
    input: Arc<Mutex<QuickInput>>,
}

impl QuickOverlay {
    pub fn input(&self) -> QuickInput {
        *self.input.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn hwnd(&self) -> Hwnd {
        self.hwnd
    }
}

impl Drop for QuickOverlay {
    fn drop(&mut self) {
        // SAFETY: posting to a window of this process; harmless if it is already gone.
        unsafe {
            let _ = PostMessageW(
                Some(HWND(self.hwnd as *mut _)),
                WM_CLOSE,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}

struct Bitmaps {
    dim: HBITMAP,
    clear: HBITMAP,
}
// SAFETY: GDI bitmaps are process-wide handles; each is selected into a DC on one thread only.
unsafe impl Send for Bitmaps {}

struct State {
    bounds: PxRect,
    windows: Vec<PxRect>,
    accent: COLORREF,
    dim_dc: HDC,
    clear_dc: HDC,
    /// The rect shown clear right now (client px).
    clear: Option<PxRect>,
    input: Arc<Mutex<QuickInput>>,
}

fn dib(w: i32, h: i32) -> anyhow::Result<(HBITMAP, *mut u8)> {
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    // SAFETY: a fresh DIB section; `bits` is checked below.
    let bmp = unsafe { CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)? };
    if bits.is_null() {
        // SAFETY: deleting the bitmap just created.
        unsafe {
            let _ = DeleteObject(HGDIOBJ(bmp.0));
        }
        anyhow::bail!("CreateDIBSection returned no bits");
    }
    Ok((bmp, bits as *mut u8))
}

/// Fill the dimmed and the clear BGRA copies of `img`, split over a few threads.
fn fill(img: &Rgba, dim: &mut [u8], clear: &mut [u8]) {
    let mut lut = [0u8; 256];
    for (i, v) in lut.iter_mut().enumerate() {
        *v = ((i as u32 * (255 - DIM_ALPHA) + 127) / 255) as u8;
    }
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 4);
    let rows_per = (img.height as usize).div_ceil(threads).max(1);
    let stride = img.width as usize * 4;
    std::thread::scope(|s| {
        for ((src, d), c) in img
            .data
            .chunks(rows_per * stride)
            .zip(dim.chunks_mut(rows_per * stride))
            .zip(clear.chunks_mut(rows_per * stride))
        {
            s.spawn(move || {
                for ((p, dp), cp) in src
                    .chunks_exact(4)
                    .zip(d.chunks_exact_mut(4))
                    .zip(c.chunks_exact_mut(4))
                {
                    cp.copy_from_slice(&[p[2], p[1], p[0], 0]);
                    dp.copy_from_slice(&[
                        lut[p[2] as usize],
                        lut[p[1] as usize],
                        lut[p[0] as usize],
                        0,
                    ]);
                }
            });
        }
    });
}

/// Show `frozen` (covering `bounds`, virtual px) at once. `windows`: visible window rects,
/// topmost first, for the clear highlight under the cursor. `accent`: RGB of the selection
/// border. Returns once the first frame is painted.
pub fn show(
    frozen: &Rgba,
    bounds: PxRect,
    windows: Vec<PxRect>,
    accent: [u8; 3],
) -> anyhow::Result<QuickOverlay> {
    let expected_len = (bounds.w.max(0) as usize) * (bounds.h.max(0) as usize) * 4;
    if bounds.w <= 0
        || bounds.h <= 0
        || frozen.width as i32 != bounds.w
        || frozen.height as i32 != bounds.h
        || frozen.data.len() != expected_len
    {
        anyhow::bail!("frozen image does not match its bounds");
    }
    let (dim_bmp, dim_bits) = dib(bounds.w, bounds.h)?;
    let (clear_bmp, clear_bits) = match dib(bounds.w, bounds.h) {
        Ok(v) => v,
        Err(e) => {
            // SAFETY: deleting the bitmap made above.
            unsafe {
                let _ = DeleteObject(HGDIOBJ(dim_bmp.0));
            }
            return Err(e);
        }
    };
    let len = frozen.data.len();
    // SAFETY: both DIB sections are bounds.w x bounds.h x 4 bytes, exclusively ours here.
    unsafe {
        fill(
            frozen,
            std::slice::from_raw_parts_mut(dim_bits, len),
            std::slice::from_raw_parts_mut(clear_bits, len),
        );
    }
    let bitmaps = Bitmaps {
        dim: dim_bmp,
        clear: clear_bmp,
    };
    let input = Arc::new(Mutex::new(QuickInput::default()));
    let (tx, rx) = std::sync::mpsc::channel::<anyhow::Result<Hwnd>>();
    let thread_input = input.clone();
    let accent = COLORREF(accent[0] as u32 | (accent[1] as u32) << 8 | (accent[2] as u32) << 16);
    std::thread::Builder::new()
        .name("quick-overlay".into())
        .spawn(move || run(bitmaps, bounds, windows, accent, thread_input, tx))?;
    let hwnd = rx
        .recv()
        .map_err(|_| anyhow::anyhow!("quick overlay thread ended"))??;
    Ok(QuickOverlay { hwnd, input })
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn run(
    bitmaps: Bitmaps,
    bounds: PxRect,
    windows: Vec<PxRect>,
    accent: COLORREF,
    input: Arc<Mutex<QuickInput>>,
    ready: std::sync::mpsc::Sender<anyhow::Result<Hwnd>>,
) {
    let _dpi = ThreadPerMonitorAware::new();
    let class = wide("StinkShot.QuickOverlay");
    // SAFETY: class registration, window creation and the message loop of this thread; the
    // state box is owned by the window (freed in WM_DESTROY) and the DCs and bitmaps are
    // released after the loop.
    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: PCWSTR(class.as_ptr()),
            hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let dim_dc = CreateCompatibleDC(None);
        let clear_dc = CreateCompatibleDC(None);
        let old_dim = SelectObject(dim_dc, HGDIOBJ(bitmaps.dim.0));
        let old_clear = SelectObject(clear_dc, HGDIOBJ(bitmaps.clear.0));
        let mut state = Box::new(State {
            bounds,
            windows,
            accent,
            dim_dc,
            clear_dc,
            clear: None,
            input,
        });
        // The window under the cursor is clear from the first frame, as in the overlay.
        if let Some(c) = crate::input::try_cursor_pos() {
            if let Ok(mut i) = state.input.lock() {
                i.cursor = Some(c);
            }
            state.clear = clear_rect(&state, c.x - bounds.x, c.y - bounds.y);
        }
        let title = wide("StinkShot.QuickOverlay");
        let created = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            PCWSTR(class.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_POPUP,
            bounds.x,
            bounds.y,
            bounds.w,
            bounds.h,
            None,
            None,
            Some(hinst),
            Some(Box::into_raw(state) as *const core::ffi::c_void),
        );
        match created {
            Ok(hwnd) => {
                if crate::window_util::capture_exclusion_on() {
                    let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
                }
                let _ = ShowWindow(hwnd, SW_SHOW);
                // Paint now, before reporting the window as shown.
                let _ = windows::Win32::Graphics::Gdi::UpdateWindow(hwnd);
                crate::window_util::force_foreground(hwnd.0 as Hwnd);
                let _ = ready.send(Ok(hwnd.0 as Hwnd));
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            Err(e) => {
                let _ = ready.send(Err(anyhow::anyhow!("create quick overlay: {e}")));
            }
        }
        SelectObject(dim_dc, old_dim);
        SelectObject(clear_dc, old_clear);
        let _ = DeleteDC(dim_dc);
        let _ = DeleteDC(clear_dc);
        let _ = DeleteObject(HGDIOBJ(bitmaps.dim.0));
        let _ = DeleteObject(HGDIOBJ(bitmaps.clear.0));
    }
}

fn point_of(l: LPARAM) -> (i32, i32) {
    let x = (l.0 & 0xFFFF) as u16 as i16 as i32;
    let y = ((l.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
    (x, y)
}

fn win_rect(r: PxRect) -> RECT {
    RECT {
        left: r.x,
        top: r.y,
        right: r.x + r.w,
        bottom: r.y + r.h,
    }
}

/// The clear rect for the pointer at client (x, y): the drag rect while dragging, else the
/// window under the pointer.
fn clear_rect(st: &State, x: i32, y: i32) -> Option<PxRect> {
    let inp = *st.input.lock().unwrap_or_else(|e| e.into_inner());
    let local = |p: PxPoint| (p.x - st.bounds.x, p.y - st.bounds.y);
    let end = if inp.down {
        Some((x, y))
    } else {
        inp.release.map(local)
    };
    if let (Some(press), Some((x, y))) = (inp.press, end) {
        let (px, py) = local(press);
        let r = PxRect::new(px.min(x), py.min(y), (px - x).abs() + 1, (py - y).abs() + 1);
        return Some(r);
    }
    let v = PxPoint {
        x: x + st.bounds.x,
        y: y + st.bounds.y,
    };
    st.windows.iter().find(|r| r.contains(v)).and_then(|r| {
        r.translate(-st.bounds.x, -st.bounds.y)
            .intersect(&PxRect::new(0, 0, st.bounds.w, st.bounds.h))
    })
}

fn set_clear(hwnd: HWND, st: &mut State, new: Option<PxRect>) {
    if new == st.clear {
        return;
    }
    // SAFETY: invalidating parts of our own window.
    unsafe {
        for r in [st.clear, new].into_iter().flatten() {
            // Grow by the border width.
            let g = PxRect::new(r.x - 3, r.y - 3, r.w + 6, r.h + 6);
            let _ = InvalidateRect(Some(hwnd), Some(&win_rect(g)), false);
        }
    }
    st.clear = new;
}

fn paint(hwnd: HWND, st: &State) {
    // SAFETY: standard WM_PAINT handling on our window with our memory DCs.
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let p = ps.rcPaint;
        let _ = BitBlt(
            hdc,
            p.left,
            p.top,
            p.right - p.left,
            p.bottom - p.top,
            Some(st.dim_dc),
            p.left,
            p.top,
            SRCCOPY,
        );
        if let Some(c) = st.clear {
            let paint_rect = PxRect::new(p.left, p.top, p.right - p.left, p.bottom - p.top);
            if let Some(i) = c.intersect(&paint_rect) {
                let _ = BitBlt(
                    hdc,
                    i.x,
                    i.y,
                    i.w,
                    i.h,
                    Some(st.clear_dc),
                    i.x,
                    i.y,
                    SRCCOPY,
                );
            }
            let selecting = st
                .input
                .lock()
                .map(|i| i.press.is_some() && (i.down || i.release.is_some()))
                .unwrap_or(false);
            if selecting {
                let brush = CreateSolidBrush(st.accent);
                let b = 2;
                for e in [
                    PxRect::new(c.x - b, c.y - b, c.w + 2 * b, b),
                    PxRect::new(c.x - b, c.y + c.h, c.w + 2 * b, b),
                    PxRect::new(c.x - b, c.y, b, c.h),
                    PxRect::new(c.x + c.w, c.y, b, c.h),
                ] {
                    FillRect(hdc, &win_rect(e), brush);
                }
                let _ = DeleteObject(HGDIOBJ(brush.0));
            }
        }
        let _ = EndPaint(hwnd, &ps);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: GWLP_USERDATA holds the State box set in WM_NCCREATE until WM_DESTROY frees it;
    // only this thread touches it.
    unsafe {
        if msg == WM_NCCREATE {
            let cs = lparam.0 as *const CREATESTRUCTW;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, (*cs).lpCreateParams as isize);
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
        if ptr.is_null() {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let st = &mut *ptr;
        let to_virtual = |x: i32, y: i32| PxPoint {
            x: x + st.bounds.x,
            y: y + st.bounds.y,
        };
        match msg {
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                paint(hwnd, st);
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let (x, y) = point_of(lparam);
                if let Ok(mut i) = st.input.lock() {
                    i.cursor = Some(to_virtual(x, y));
                }
                let c = clear_rect(st, x, y);
                set_clear(hwnd, st, c);
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                let (x, y) = point_of(lparam);
                SetCapture(hwnd);
                if let Ok(mut i) = st.input.lock() {
                    i.press = Some(to_virtual(x, y));
                    i.cursor = Some(to_virtual(x, y));
                    i.down = true;
                    i.release = None;
                    i.clicked = None;
                }
                let c = clear_rect(st, x, y);
                set_clear(hwnd, st, c);
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let (x, y) = point_of(lparam);
                let _ = ReleaseCapture();
                if let Ok(mut i) = st.input.lock() {
                    i.cursor = Some(to_virtual(x, y));
                    i.down = false;
                    let up = to_virtual(x, y);
                    // A click (no drag) takes the highlighted window once the overlay is up.
                    let moved = i
                        .press
                        .is_some_and(|p| (p.x - up.x).abs().max((p.y - up.y).abs()) >= CLICK_SLOP);
                    if moved {
                        i.release = Some(up);
                    } else {
                        i.press = None;
                        i.clicked = Some(up);
                    }
                }
                let c = clear_rect(st, x, y);
                set_clear(hwnd, st, c);
                LRESULT(0)
            }
            WM_KEYDOWN if wparam.0 == VK_ESCAPE.0 as usize => {
                if let Ok(mut i) = st.input.lock() {
                    i.cancelled = true;
                }
                let _ = ShowWindow(hwnd, windows::Win32::UI::WindowsAndMessaging::SW_HIDE);
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(ptr));
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_dims_and_swizzles() {
        let mut img = Rgba::new(3, 2);
        for (i, p) in img.data.chunks_exact_mut(4).enumerate() {
            p.copy_from_slice(&[255, (i * 40) as u8, 0, 255]);
        }
        let mut d = vec![0u8; 24];
        let mut c = vec![0u8; 24];
        fill(&img, &mut d, &mut c);
        assert_eq!(&c[4..8], &[0, 40, 255, 0]);
        // 255 * (255 - 148) / 255 = 107.
        assert_eq!(&d[..4], &[0, 0, 107, 0]);
    }
}
