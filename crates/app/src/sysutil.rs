//! Small Win32 helpers the shell needs that `stinkshot-platform` does not provide:
//! console attach for the CLI (the exe uses the GUI subsystem in release), fatal error box,
//! system DPI.

use windows::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    SetStdHandle,
};
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MessageBoxW};
use windows::core::PCWSTR;

/// Make `println!`/`eprintln!` reach the parent console when launched from a terminal.
/// Redirected/piped handles are left untouched. No-op when there is no parent console.
pub fn attach_parent_console() {
    // SAFETY: plain Win32 calls with no pointers into our memory except handles we own.
    unsafe {
        let out_ok = GetStdHandle(STD_OUTPUT_HANDLE)
            .map(|h| !h.is_invalid() && !h.0.is_null())
            .unwrap_or(false);
        let err_ok = GetStdHandle(STD_ERROR_HANDLE)
            .map(|h| !h.is_invalid() && !h.0.is_null())
            .unwrap_or(false);
        if out_ok && err_ok {
            return;
        }
        if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
            return;
        }
        use std::os::windows::io::IntoRawHandle;
        let open = |name: &str| {
            std::fs::OpenOptions::new()
                .write(true)
                .read(true)
                .open(name)
        };
        if !out_ok && let Ok(f) = open("CONOUT$") {
            let h = windows::Win32::Foundation::HANDLE(f.into_raw_handle());
            let _ = SetStdHandle(STD_OUTPUT_HANDLE, h);
        }
        if !err_ok && let Ok(f) = open("CONOUT$") {
            let h = windows::Win32::Foundation::HANDLE(f.into_raw_handle());
            let _ = SetStdHandle(STD_ERROR_HANDLE, h);
        }
    }
}

/// Modal error box for failures before any UI exists.
pub fn fatal_box(title: &str, message: &str) {
    let t: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let m: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: both buffers are NUL-terminated and outlive the call.
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(m.as_ptr()),
            PCWSTR(t.as_ptr()),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        );
    }
}

pub fn system_dpi() -> u32 {
    // SAFETY: no arguments.
    let d = unsafe { GetDpiForSystem() };
    if d == 0 { 96 } else { d }
}

/// Move a file to the Recycle Bin (no UI, no confirmation). Errors if the shell refused or
/// the file is still there afterwards.
pub fn recycle(path: &std::path::Path) -> anyhow::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW,
        SHFileOperationW,
    };
    if !path.exists() {
        return Ok(());
    }
    // pFrom is a list of NUL-terminated paths ending with an extra NUL.
    let mut from: Vec<u16> = path.as_os_str().encode_wide().collect();
    from.extend([0, 0]);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT).0 as u16,
        ..Default::default()
    };
    // SAFETY: `from` is double-NUL terminated and outlives the call; no other pointers set.
    let rc = unsafe { SHFileOperationW(&mut op) };
    if rc != 0 || op.fAnyOperationsAborted.as_bool() {
        anyhow::bail!(
            "could not move {} to the Recycle Bin (code {rc})",
            path.display()
        );
    }
    if path.exists() {
        anyhow::bail!("{} is still there after recycling", path.display());
    }
    Ok(())
}
