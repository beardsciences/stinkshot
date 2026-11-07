//! Startup and capture-latency trace points. Everything logs at debug level with the time
//! since process start, so `grep timing: stinkshot.log` shows where the first overlay after a
//! launch spends its time (IPC received, command dispatched, freeze, overlay viewport
//! created, first frame).

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

/// Log one trace point: `timing: +1234.5 ms <what>`. Takes `format_args!` so nothing is
/// formatted when debug logging is off (release builds log at info).
pub fn point(what: std::fmt::Arguments<'_>) {
    log::debug!("timing: +{:.1} ms {what}", ms());
}
