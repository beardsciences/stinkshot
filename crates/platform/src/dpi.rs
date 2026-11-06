use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
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
