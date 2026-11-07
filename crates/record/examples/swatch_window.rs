//! HDR color check window (docs/notes/hdr.md method): three 200x200 swatches #7FD36A,
//! #808080, #FFFFFF side by side, with a 2x2 corner block toggling every 250 ms so Desktop
//! Duplication keeps delivering frames. Borderless, topmost, never takes focus.
//!
//! `swatch_window X Y SECONDS` then `hdr_record_probe X Y 600 200 3`

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, GetDC, ReleaseDC, SetDIBitsToDevice,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_DESTROY {
            PostQuitMessage(0);
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

fn main() -> windows::core::Result<()> {
    stinkshot_platform::dpi::init();
    let a: Vec<i32> = std::env::args()
        .skip(1)
        .map(|s| s.parse().unwrap())
        .collect();
    let (x, y, secs) = (a[0], a[1], a[2] as u64);
    let (w, h) = (600usize, 200usize);
    let colors = [0x007F_D36A_u32, 0x0080_8080, 0x00FF_FFFF];
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: w!("StinkshotSwatch"),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            w!("StinkshotSwatch"),
            w!("HDR Swatch"),
            WS_POPUP | WS_VISIBLE,
            x,
            y,
            w as i32,
            h as i32,
            None,
            None,
            Some(hinst.into()),
            None,
        )?;
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut px = vec![0u32; w * h];
        let t0 = Instant::now();
        let mut msg = MSG::default();
        'run: while t0.elapsed() < Duration::from_secs(secs) {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    break 'run;
                }
                DispatchMessageW(&msg);
            }
            let tick = (t0.elapsed().as_millis() / 250) % 2 == 1;
            for yy in 0..h {
                for xx in 0..w {
                    px[yy * w + xx] = colors[xx / 200];
                }
            }
            if tick {
                for yy in 0..2 {
                    px[yy * w..yy * w + 2].fill(0);
                }
            }
            let dc = GetDC(Some(hwnd));
            SetDIBitsToDevice(
                dc,
                0,
                0,
                w as u32,
                h as u32,
                0,
                0,
                0,
                h as u32,
                px.as_ptr() as *const _,
                &bmi,
                DIB_RGB_COLORS,
            );
            ReleaseDC(Some(hwnd), dc);
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = DestroyWindow(hwnd);
    }
    Ok(())
}
