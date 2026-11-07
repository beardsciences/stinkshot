//! Turns an idle-off display back on for desktop tests: Desktop Duplication delivers nothing
//! while the display is off. Creates its own small window under the pointer, and only if
//! that window is visible, in the foreground and contains the pointer, injects one
//! zero-distance mouse move (the pointer does not move). Otherwise it does nothing.
//!
//! `cargo run -p stinkshot-record --example wake_display`

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT, SendInput,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn pump() {
    let mut msg = MSG::default();
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            DispatchMessageW(&msg);
        }
    }
}

fn main() -> windows::core::Result<()> {
    stinkshot_platform::dpi::init();
    unsafe {
        let mut cur = POINT::default();
        GetCursorPos(&mut cur)?;
        let hinst = GetModuleHandleW(None)?;
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: w!("StinkshotWakeDisplay"),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            w!("StinkshotWakeDisplay"),
            w!("Stinkshot Wake Display"),
            WS_POPUP | WS_VISIBLE,
            cur.x - 40,
            cur.y - 40,
            80,
            80,
            None,
            None,
            Some(hinst.into()),
            None,
        )?;
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(500) {
            let _ = SetForegroundWindow(hwnd);
            pump();
            if GetForegroundWindow() == hwnd {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let fg = GetForegroundWindow();
        let mut pid = 0u32;
        GetWindowThreadProcessId(fg, Some(&mut pid));
        let mut r = RECT::default();
        GetWindowRect(hwnd, &mut r)?;
        GetCursorPos(&mut cur)?;
        let inside = cur.x >= r.left && cur.x < r.right && cur.y >= r.top && cur.y < r.bottom;
        let ok =
            fg == hwnd && pid == GetCurrentProcessId() && IsWindowVisible(hwnd).as_bool() && inside;
        println!(
            "own window foreground={} visible={} contains pointer={inside}",
            fg == hwnd,
            IsWindowVisible(hwnd).as_bool()
        );
        if ok {
            let input = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: 0,
                        dy: 0,
                        dwFlags: MOUSEEVENTF_MOVE,
                        ..Default::default()
                    },
                },
            };
            let n = SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
            println!("sent {n} zero-distance mouse move");
        } else {
            println!("safety check failed; no input sent");
        }
        std::thread::sleep(Duration::from_millis(300));
        pump();
        let _ = DestroyWindow(hwnd);
    }
    Ok(())
}
