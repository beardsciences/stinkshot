//! Prints the console display power state (0 off, 1 on, 2 dimmed). Desktop Duplication
//! delivers no frames while the display is off (DWM does not compose), which looks exactly
//! like a static screen.
//!
//! `cargo run -p stinkshot-record --example display_state`

use std::sync::atomic::{AtomicI32, Ordering};

use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Power::{POWERBROADCAST_SETTING, RegisterPowerSettingNotification};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{GUID, w};

static STATE: AtomicI32 = AtomicI32::new(-1);

const CONSOLE_DISPLAY_STATE: GUID = GUID::from_u128(0x6fe69556_704a_47a0_8f24_c28d936fda47);

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_POWERBROADCAST && wp.0 as u32 == PBT_POWERSETTINGCHANGE {
            let s = &*(lp.0 as *const POWERBROADCAST_SETTING);
            if s.PowerSetting == CONSOLE_DISPLAY_STATE {
                STATE.store(s.Data[0] as i32, Ordering::SeqCst);
            }
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

fn main() -> windows::core::Result<()> {
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: w!("StinkshotDisplayState"),
            ..Default::default()
        };
        RegisterClassW(&wc);
        // A top-level (hidden) window: message-only windows get no broadcasts.
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("StinkshotDisplayState"),
            w!("display state"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(hinst.into()),
            None,
        )?;
        let _h = RegisterPowerSettingNotification(
            HANDLE(hwnd.0),
            &CONSOLE_DISPLAY_STATE,
            REGISTER_NOTIFICATION_FLAGS(0),
        )?;
        let t = std::time::Instant::now();
        let mut msg = MSG::default();
        while STATE.load(Ordering::SeqCst) < 0 && t.elapsed().as_secs() < 3 {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                DispatchMessageW(&msg);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let _ = DestroyWindow(hwnd);
    }
    println!(
        "console display state: {} (0 off, 1 on, 2 dimmed, -1 unknown)",
        STATE.load(Ordering::SeqCst)
    );
    Ok(())
}
