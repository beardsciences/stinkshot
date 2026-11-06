//! Crate-private helpers: COM apartment init, wide strings, registry access.

use std::cell::{Cell, RefCell};

use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_DWORD, REG_SZ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegDeleteKeyValueW,
    RegGetValueW, RegSetKeyValueW,
};
use windows::core::PCWSTR;

struct ComGuard;

impl Drop for ComGuard {
    fn drop(&mut self) {
        // SAFETY: balanced with the successful CoInitializeEx on this same thread.
        unsafe { CoUninitialize() };
    }
}

thread_local! {
    static TRIED: Cell<bool> = const { Cell::new(false) };
    static GUARD: RefCell<Option<ComGuard>> = const { RefCell::new(None) };
}

/// Make sure the calling thread has a COM apartment. Fresh worker threads become MTA. A
/// thread that is already STA (the UI thread) keeps its apartment (RPC_E_CHANGED_MODE is
/// ignored), which is fine for the free-threaded WinRT and UIA objects used here.
pub(crate) fn ensure_com() {
    if TRIED.with(|t| t.replace(true)) {
        return;
    }
    // SAFETY: plain COM init call on the current thread.
    let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if hr.is_ok() {
        GUARD.with(|g| *g.borrow_mut() = Some(ComGuard));
    } else if hr != RPC_E_CHANGED_MODE {
        log::warn!("CoInitializeEx failed: {hr:?}");
    }
}

/// NUL-terminated UTF-16.
pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub(crate) fn wide_os(s: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// UTF-16 buffer (possibly NUL-terminated) to String.
pub(crate) fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

pub(crate) fn reg_get_dword(subkey: &str, name: &str) -> Option<u32> {
    let sk = wide(subkey);
    let nm = wide(name);
    let mut data: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: pointers reference live locals; size matches the buffer.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(sk.as_ptr()),
            PCWSTR(nm.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    if r.is_ok() { Some(data) } else { None }
}

pub(crate) fn reg_set_dword(subkey: &str, name: &str, value: u32) -> anyhow::Result<()> {
    let sk = wide(subkey);
    let nm = wide(name);
    // SAFETY: pointer/size describe `value`.
    let r = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            PCWSTR(sk.as_ptr()),
            PCWSTR(nm.as_ptr()),
            REG_DWORD.0,
            Some(&value as *const u32 as *const _),
            std::mem::size_of::<u32>() as u32,
        )
    };
    r.ok()
        .map_err(|e| anyhow::anyhow!("registry write {subkey}\\{name}: {e}"))
}

pub(crate) fn reg_get_string(subkey: &str, name: &str) -> Option<String> {
    let sk = wide(subkey);
    let nm = wide(name);
    let mut size: u32 = 0;
    // SAFETY: first call only queries the required size.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(sk.as_ptr()),
            PCWSTR(nm.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
    };
    if r.is_err() || size == 0 {
        return None;
    }
    let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
    let mut size2 = (buf.len() * 2) as u32;
    // SAFETY: buffer is at least `size2` bytes.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(sk.as_ptr()),
            PCWSTR(nm.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size2),
        )
    };
    if r.is_ok() {
        Some(from_wide(&buf))
    } else {
        None
    }
}

pub(crate) fn reg_set_string(subkey: &str, name: &str, value: &str) -> anyhow::Result<()> {
    let sk = wide(subkey);
    let nm = wide(name);
    let data = wide(value);
    // SAFETY: data is a live NUL-terminated UTF-16 buffer; size is in bytes.
    let r = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            PCWSTR(sk.as_ptr()),
            PCWSTR(nm.as_ptr()),
            REG_SZ.0,
            Some(data.as_ptr() as *const _),
            (data.len() * 2) as u32,
        )
    };
    r.ok()
        .map_err(|e| anyhow::anyhow!("registry write {subkey}\\{name}: {e}"))
}

/// Delete a value; a missing value counts as success.
pub(crate) fn reg_delete_value(subkey: &str, name: &str) -> anyhow::Result<()> {
    let sk = wide(subkey);
    let nm = wide(name);
    // SAFETY: valid NUL-terminated strings.
    let r =
        unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, PCWSTR(sk.as_ptr()), PCWSTR(nm.as_ptr())) };
    if r.is_ok() || r == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err(anyhow::anyhow!("registry delete {subkey}\\{name}: {r:?}"))
    }
}

pub(crate) fn reg_value_exists(subkey: &str, name: &str) -> bool {
    reg_get_string(subkey, name).is_some() || reg_get_dword(subkey, name).is_some()
}
