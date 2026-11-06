use std::cell::RefCell;
use std::path::Path;
use std::sync::mpsc::Sender;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT,
    WPARAM,
};
use windows::Win32::Media::Audio::{PlaySoundW, SND_ALIAS, SND_ASYNC, SND_NODEFAULT};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Shell::{SHOpenFolderAndSelectItems, SHParseDisplayName, ShellExecuteW};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, FindWindowExW, GetMessageW, HWND_MESSAGE,
    MSG, RegisterClassW, SMTO_ABORTIFHUNG, SW_SHOWNORMAL, SendMessageTimeoutW, TranslateMessage,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_COPYDATA, WNDCLASSW,
};
use windows::core::{PCWSTR, w};

use crate::util::{
    reg_delete_value, reg_get_dword, reg_set_dword, reg_set_string, reg_value_exists, wide, wide_os,
};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "StinkShot";
const KEYBOARD_KEY: &str = r"Control Panel\Keyboard";
const PRTSC_VALUE: &str = "PrintScreenKeyForSnippingEnabled";
const APP_KEY: &str = r"Software\StinkShot";
const PRTSC_PREV_VALUE: &str = "PrevPrintScreenSnipping";
/// Stored in `PrevPrintScreenSnipping` when the Windows value did not exist.
const PREV_ABSENT: u32 = u32::MAX;
const PERSONALIZE_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";

const IPC_CLASS: &str = "StinkShot.IPC.Window";
const IPC_TITLE: &str = "StinkShot.IPC";
/// Tag in `COPYDATASTRUCT::dwData` so stray WM_COPYDATA is ignored.
const IPC_MAGIC: usize = 0x5354_4B53;

/// HKCU\Software\Microsoft\Windows\CurrentVersion\Run\StinkShot = "<exe>" --tray
pub fn set_autostart(enabled: bool, exe: &Path) -> anyhow::Result<()> {
    if enabled {
        let value = format!("\"{}\" --tray", exe.display());
        reg_set_string(RUN_KEY, RUN_VALUE, &value)
    } else {
        reg_delete_value(RUN_KEY, RUN_VALUE)
    }
}

pub fn autostart_enabled() -> bool {
    reg_value_exists(RUN_KEY, RUN_VALUE)
}

/// Windows 11 routes PrintScreen to Snipping Tool when
/// HKCU\Control Panel\Keyboard\PrintScreenKeyForSnippingEnabled = 1, which steals the key
/// from RegisterHotKey. `take_over(true)` sets it to 0 (remembering the previous value in
/// HKCU\Software\StinkShot\PrevPrintScreenSnipping); `take_over(false)` restores it.
/// Takes effect without sign-out on current builds; returns whether a change was made.
pub fn take_over_printscreen(take: bool) -> anyhow::Result<bool> {
    let current = reg_get_dword(KEYBOARD_KEY, PRTSC_VALUE);
    let saved = reg_get_dword(APP_KEY, PRTSC_PREV_VALUE);
    if take {
        if saved.is_none() {
            reg_set_dword(APP_KEY, PRTSC_PREV_VALUE, current.unwrap_or(PREV_ABSENT))?;
        }
        if current == Some(0) {
            return Ok(false);
        }
        reg_set_dword(KEYBOARD_KEY, PRTSC_VALUE, 0)?;
        Ok(true)
    } else {
        let Some(prev) = saved else { return Ok(false) };
        let changed = if prev == PREV_ABSENT {
            let had = current.is_some();
            reg_delete_value(KEYBOARD_KEY, PRTSC_VALUE)?;
            had
        } else {
            reg_set_dword(KEYBOARD_KEY, PRTSC_VALUE, prev)?;
            current != Some(prev)
        };
        reg_delete_value(APP_KEY, PRTSC_PREV_VALUE)?;
        Ok(changed)
    }
}

/// Explorer window with the file selected.
pub fn reveal_in_explorer(path: &Path) -> anyhow::Result<()> {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    if !abs.exists() {
        anyhow::bail!("path does not exist: {}", abs.display());
    }
    let (tx, rx) = std::sync::mpsc::channel::<anyhow::Result<()>>();
    let p = abs.clone();
    // COM work happens on a private thread so a caller on the UI (STA) thread is never blocked
    // by the shell or has its apartment altered.
    std::thread::Builder::new()
        .name("stinkshot-reveal".into())
        .spawn(move || {
            crate::util::ensure_com();
            let _ = tx.send(reveal_blocking(&p));
        })
        .map_err(|e| anyhow::anyhow!("spawn reveal thread: {e}"))?;
    match rx.recv_timeout(std::time::Duration::from_secs(4)) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => {
            log::warn!(
                "SHOpenFolderAndSelectItems failed ({e}); falling back to explorer.exe /select"
            );
            std::process::Command::new("explorer.exe")
                .arg(format!("/select,{}", abs.display()))
                .spawn()
                .map(|_| ())
                .map_err(|e| anyhow::anyhow!("explorer.exe: {e}"))
        }
        // Explorer is slow to answer; the window will still appear.
        Err(_) => Ok(()),
    }
}

fn reveal_blocking(path: &Path) -> anyhow::Result<()> {
    let w = wide_os(path.as_os_str());
    // SAFETY: `w` is NUL-terminated and outlives the calls; the PIDL is freed below.
    unsafe {
        let mut pidl = std::ptr::null_mut();
        SHParseDisplayName(PCWSTR(w.as_ptr()), None, &mut pidl, 0, None)?;
        let r = SHOpenFolderAndSelectItems(pidl, None, 0);
        windows::Win32::System::Com::CoTaskMemFree(Some(pidl as *const _));
        r?;
    }
    Ok(())
}

/// ShellExecute "open".
pub fn open_path(path: &Path) -> anyhow::Result<()> {
    let w = wide_os(path.as_os_str());
    // SAFETY: `w` is NUL-terminated and outlives the call.
    let r: HINSTANCE = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(w.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    // ShellExecute reports success with a value greater than 32.
    if (r.0 as isize) > 32 {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "ShellExecute failed for {} (code {})",
            path.display(),
            r.0 as isize
        ))
    }
}

/// True if Windows apps use dark mode (AppsUseLightTheme == 0).
pub fn system_dark_mode() -> bool {
    reg_get_dword(PERSONALIZE_KEY, "AppsUseLightTheme") == Some(0)
}

/// Windows accent color, if available.
pub fn system_accent() -> Option<[u8; 4]> {
    use windows::UI::ViewManagement::{UIColorType, UISettings};
    if let Ok(ui) = UISettings::new()
        && let Ok(c) = ui.GetColorValue(UIColorType::Accent)
    {
        return Some([c.R, c.G, c.B, c.A]);
    }
    // DWM stores ABGR in a DWORD.
    reg_get_dword(r"Software\Microsoft\Windows\DWM", "AccentColor").map(|v| {
        [
            (v & 0xff) as u8,
            ((v >> 8) & 0xff) as u8,
            ((v >> 16) & 0xff) as u8,
            0xff,
        ]
    })
}

/// Single-instance guard: named mutex "Local\StinkShot.Instance". Returns None if another
/// instance owns it. Keep the guard alive for the process lifetime.
pub struct InstanceGuard {
    #[allow(dead_code)]
    handle: isize,
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // SAFETY: handle came from CreateMutexW and is closed exactly once.
        unsafe {
            let _ = CloseHandle(HANDLE(self.handle as *mut _));
        }
    }
}

pub fn acquire_single_instance() -> Option<InstanceGuard> {
    // SAFETY: valid NUL-terminated name; GetLastError is read immediately after the call.
    unsafe {
        let h = match CreateMutexW(None, true, w!("Local\\StinkShot.Instance")) {
            Ok(h) => h,
            Err(e) => {
                log::warn!("CreateMutexW failed: {e}");
                return None;
            }
        };
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(h);
            return None;
        }
        Some(InstanceGuard {
            handle: h.0 as isize,
        })
    }
}

fn encode_args(args: &[String]) -> Vec<u8> {
    args.join("\0").into_bytes()
}

fn decode_args(bytes: &[u8]) -> Vec<String> {
    if bytes.is_empty() {
        return Vec::new();
    }
    String::from_utf8_lossy(bytes)
        .split('\0')
        .map(str::to_owned)
        .collect()
}

/// Forward CLI args to the running instance via WM_COPYDATA to the hidden window titled
/// "StinkShot.IPC". Returns false if no instance window was found.
pub fn send_to_running_instance(args: &[String]) -> bool {
    let title = wide(IPC_TITLE);
    // SAFETY: strings are NUL-terminated; the payload buffer outlives the synchronous send.
    unsafe {
        let Ok(target) = FindWindowExW(Some(HWND_MESSAGE), None, None, PCWSTR(title.as_ptr()))
        else {
            return false;
        };
        let payload = encode_args(args);
        let cds = COPYDATASTRUCT {
            dwData: IPC_MAGIC,
            cbData: payload.len() as u32,
            lpData: if payload.is_empty() {
                std::ptr::null_mut()
            } else {
                payload.as_ptr() as *mut _
            },
        };
        let mut result = 0usize;
        let r = SendMessageTimeoutW(
            target,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM(&cds as *const COPYDATASTRUCT as isize),
            SMTO_ABORTIFHUNG,
            3000,
            Some(&mut result),
        );
        r.0 != 0
    }
}

thread_local! {
    static IPC_TX: RefCell<Option<Sender<Vec<String>>>> = const { RefCell::new(None) };
}

unsafe extern "system" fn ipc_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_COPYDATA && lparam.0 != 0 {
        // SAFETY: for WM_COPYDATA, lparam points to a COPYDATASTRUCT valid during the call.
        let cds = unsafe { &*(lparam.0 as *const COPYDATASTRUCT) };
        if cds.dwData == IPC_MAGIC {
            let bytes: &[u8] = if cds.lpData.is_null() || cds.cbData == 0 {
                &[]
            } else {
                // SAFETY: the sender guarantees cbData readable bytes at lpData.
                unsafe { std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize) }
            };
            let args = decode_args(bytes);
            IPC_TX.with(|t| {
                if let Some(tx) = t.borrow().as_ref() {
                    let _ = tx.send(args);
                }
            });
            return LRESULT(1);
        }
    }
    // SAFETY: default processing for everything else.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Create the hidden IPC message-only window on a dedicated thread; every WM_COPYDATA
/// payload (UTF-8 args joined by '\0') is sent to `tx`.
pub fn start_ipc_listener(tx: Sender<Vec<String>>) -> anyhow::Result<()> {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<anyhow::Result<()>>();
    std::thread::Builder::new()
        .name("stinkshot-ipc".into())
        .spawn(move || {
            IPC_TX.with(|t| *t.borrow_mut() = Some(tx));
            let class = wide(IPC_CLASS);
            let title = wide(IPC_TITLE);
            // SAFETY: standard window class registration and message loop on this thread.
            unsafe {
                let hinst: HINSTANCE = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(ipc_wndproc),
                    hInstance: hinst,
                    lpszClassName: PCWSTR(class.as_ptr()),
                    ..Default::default()
                };
                // A zero atom may just mean the class already exists; CreateWindowExW decides.
                RegisterClassW(&wc);
                let hwnd = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    PCWSTR(class.as_ptr()),
                    PCWSTR(title.as_ptr()),
                    WINDOW_STYLE(0),
                    0,
                    0,
                    0,
                    0,
                    Some(HWND_MESSAGE),
                    None,
                    Some(hinst),
                    None,
                );
                match hwnd {
                    Ok(_) => {
                        let _ = ready_tx.send(Ok(()));
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(anyhow::anyhow!("create IPC window: {e}")));
                        return;
                    }
                }
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        })
        .map_err(|e| anyhow::anyhow!("spawn IPC thread: {e}"))?;
    ready_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| anyhow::anyhow!("IPC thread did not start: {e}"))?
}

/// Short beep-free shutter sound via PlaySound on a system sound alias, async.
pub fn play_capture_sound() {
    // SND_NODEFAULT: stay silent rather than falling back to the default beep.
    // SAFETY: static alias string; async playback of a system sound.
    let ok = unsafe {
        PlaySoundW(
            w!("Notification.Default"),
            None,
            SND_ALIAS | SND_ASYNC | SND_NODEFAULT,
        )
    };
    if !ok.as_bool() {
        log::debug!("PlaySound failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_roundtrip() {
        let a = vec!["region".to_string(), "--delay".to_string(), "3".to_string()];
        assert_eq!(decode_args(&encode_args(&a)), a);
        assert!(decode_args(&encode_args(&[])).is_empty());
    }
}
