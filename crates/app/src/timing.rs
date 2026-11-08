//! Startup and capture-latency trace points. Everything logs at debug level with the time
//! since process start and the process's private bytes, so `grep timing: stinkshot.log` shows
//! where the first overlay after a launch spends its time (IPC received, command dispatched,
//! freeze, overlay viewport created, first frame) and what each start-up step costs in
//! memory. Release builds log at info; `STINKSHOT_LOG=debug` turns these on there too.

use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();

/// Call first thing in `main`.
pub fn mark_start() {
    let _ = START.get_or_init(Instant::now);
}

/// Milliseconds since `mark_start`.
pub fn ms() -> f64 {
    START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

/// Private bytes (commit charge) of this process in MB, what Task Manager calls "Commit size".
pub fn private_mb() -> f64 {
    use windows::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;
    let mut c = PROCESS_MEMORY_COUNTERS_EX {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    // SAFETY: the out pointer and size describe a live local of the extended struct, which
    // starts with PROCESS_MEMORY_COUNTERS.
    let ok = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut c as *mut PROCESS_MEMORY_COUNTERS_EX as *mut PROCESS_MEMORY_COUNTERS,
            c.cb,
        )
    };
    if ok.is_ok() {
        c.PrivateUsage as f64 / (1024.0 * 1024.0)
    } else {
        0.0
    }
}

/// Milliseconds from process creation (as the kernel recorded it) to now: what loading the exe
/// and its DLLs cost before `main` ran, when called first thing.
pub fn since_process_creation_ms() -> f64 {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: all out pointers are live locals.
    let ok = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    };
    if ok.is_err() {
        return 0.0;
    }
    // SAFETY: no arguments; returns the current UTC time.
    let now = unsafe { GetSystemTimePreciseAsFileTime() };
    let t = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    t(now).saturating_sub(t(created)) as f64 / 10_000.0
}

/// Log one trace point: `timing: +1234.5 ms [priv 123 MB] <what>`. Takes `format_args!` so
/// nothing is formatted (and no memory query runs) when debug logging is off.
pub fn point(what: std::fmt::Arguments<'_>) {
    if log::log_enabled!(log::Level::Debug) {
        log::debug!(
            "timing: +{:.1} ms [priv {:.0} MB] {what}",
            ms(),
            private_mb()
        );
    }
}
