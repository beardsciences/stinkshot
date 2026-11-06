//! Shared Media Foundation / COM plumbing.

use anyhow::{Context, Result};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::core::GUID;

/// COM initialisation for the current thread (MTA). Tolerates a thread that is already STA.
pub struct ComGuard(bool);

impl ComGuard {
    pub fn new() -> Self {
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        ComGuard(hr.is_ok())
    }
}

impl Default for ComGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

/// COM + Media Foundation startup for the lifetime of the guard (both are ref counted).
pub struct MfSession {
    _mf: MfGuard,
    _com: ComGuard,
}

/// Media Foundation startup/shutdown only (ref counted, usable from any thread). Held by
/// objects that must outlive the worker threads that created Media Foundation resources.
pub struct MfGuard;

impl MfGuard {
    pub fn new() -> Result<Self> {
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }.context("MFStartup failed")?;
        Ok(MfGuard)
    }
}

impl Drop for MfGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
        }
    }
}

impl MfSession {
    pub fn new() -> Result<Self> {
        let com = ComGuard::new();
        let mf = MfGuard::new()?;
        Ok(Self { _mf: mf, _com: com })
    }
}

/// Wrapper to move COM objects (agile / MTA) between threads.
pub struct SendCom<T>(pub T);
unsafe impl<T> Send for SendCom<T> {}
unsafe impl<T> Sync for SendCom<T> {}

pub fn pack(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

pub fn unpack(v: u64) -> (u32, u32) {
    ((v >> 32) as u32, v as u32)
}

pub fn duration_to_hns(d: std::time::Duration) -> i64 {
    (d.as_nanos() / 100) as i64
}

pub fn hns_to_duration(h: i64) -> std::time::Duration {
    std::time::Duration::from_nanos((h.max(0) as u64).saturating_mul(100))
}

/// Build a sample holding a copy of `data`.
pub fn make_sample(data: &[u8], pts_hns: i64, dur_hns: i64) -> Result<IMFSample> {
    unsafe {
        let buffer = MFCreateMemoryBuffer(data.len() as u32).context("MFCreateMemoryBuffer")?;
        let mut ptr = std::ptr::null_mut();
        let mut max = 0u32;
        buffer
            .Lock(&mut ptr, Some(&mut max), None)
            .context("buffer lock")?;
        if ptr.is_null() || (max as usize) < data.len() {
            let _ = buffer.Unlock();
            anyhow::bail!("media buffer too small");
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
        buffer.Unlock().context("buffer unlock")?;
        buffer
            .SetCurrentLength(data.len() as u32)
            .context("SetCurrentLength")?;
        let sample = MFCreateSample().context("MFCreateSample")?;
        sample.AddBuffer(&buffer).context("AddBuffer")?;
        sample.SetSampleTime(pts_hns).context("SetSampleTime")?;
        sample
            .SetSampleDuration(dur_hns)
            .context("SetSampleDuration")?;
        Ok(sample)
    }
}

/// Friendly name of an activation object (device sources).
pub fn activate_name(act: &IMFActivate, key: &GUID) -> Option<String> {
    unsafe {
        let mut p = windows::core::PWSTR::null();
        let mut len = 0u32;
        act.GetAllocatedString(key, &mut p, &mut len).ok()?;
        if p.is_null() {
            return None;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p.0, len as usize));
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
        Some(s)
    }
}

/// Precise sleep: coarse sleep then a short spin. Windows' default timer granularity is
/// ~15 ms, so callers also hold a `TimerResolution`.
pub fn sleep_until(deadline: std::time::Instant) {
    loop {
        let now = std::time::Instant::now();
        if now >= deadline {
            return;
        }
        let left = deadline - now;
        if left > std::time::Duration::from_millis(2) {
            std::thread::sleep(left - std::time::Duration::from_millis(1));
        } else {
            std::thread::yield_now();
        }
    }
}

/// Requests 1 ms system timer resolution while alive.
pub struct TimerResolution;

impl TimerResolution {
    pub fn new() -> Self {
        unsafe {
            windows::Win32::Media::timeBeginPeriod(1);
        }
        TimerResolution
    }
}

impl Default for TimerResolution {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TimerResolution {
    fn drop(&mut self) {
        unsafe {
            windows::Win32::Media::timeEndPeriod(1);
        }
    }
}
