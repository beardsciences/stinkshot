use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    SetProcessDpiAwarenessContext, SetThreadDpiAwarenessContext,
};

/// Set Per-Monitor-V2 DPI awareness for the process. Must run before any window exists.
/// (The exe manifest also declares it; this is the belt to that suspender.)
pub fn init() {
    // SAFETY: plain process-wide call with a documented constant.
    if let Err(e) =
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
    {
        // Typically ACCESS_DENIED because awareness was already set (manifest); harmless.
        log::debug!("SetProcessDpiAwarenessContext: {e}");
    }
}

/// Makes the current thread Per-Monitor-V2 DPI aware until dropped, whatever the process
/// awareness is. Needed around calls that see physical pixels or that refuse DPI-unaware
/// callers: `IDXGIOutput5::DuplicateOutput1` returns `DXGI_ERROR_UNSUPPORTED` for a
/// DPI-unaware caller on a display scaled above 100% (measured Nov 7 2026: 4K HDR at 125%).
/// Test binaries and tools have no manifest, so library code must not rely on `init()`.
pub struct ThreadPerMonitorAware {
    old: DPI_AWARENESS_CONTEXT,
}

impl ThreadPerMonitorAware {
    pub fn new() -> Self {
        // SAFETY: thread-local setting; the previous context is restored on drop.
        let old =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        Self { old }
    }
}

impl Default for ThreadPerMonitorAware {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ThreadPerMonitorAware {
    fn drop(&mut self) {
        if !self.old.0.is_null() {
            // SAFETY: restores the context returned by the matching set call.
            unsafe { SetThreadDpiAwarenessContext(self.old) };
        }
    }
}
