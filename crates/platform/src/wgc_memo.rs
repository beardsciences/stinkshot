//! Remembers across launches that Windows.Graphics.Capture hangs on this PC.
//!
//! The in-process [`crate::hang::HangGate`] stops repeated waits within one run, but every
//! launch still paid the full WGC timeout (about 3 s) on its first window capture and its
//! first recording. A hang is written to `wgc-hang.txt` in the config folder; for the next
//! [`RETRY_AFTER`] WGC is skipped outright, then tried once more in case a driver or Windows
//! update fixed it.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const FILE: &str = "wgc-hang.txt";
pub const RETRY_AFTER: Duration = Duration::from_secs(7 * 24 * 3600);

/// Unix seconds of the last remembered hang (None: none, or not loaded yet).
static STATE: Mutex<Option<Option<u64>>> = Mutex::new(None);

fn path() -> PathBuf {
    stinkshot_core::config::Config::dir().join(FILE)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn read(p: &Path) -> Option<u64> {
    std::fs::read_to_string(p).ok()?.trim().parse().ok()
}

fn is_recent(at: u64, now: u64) -> bool {
    now >= at && now - at < RETRY_AFTER.as_secs()
}

/// True if WGC hung on this PC within `RETRY_AFTER` (in this run or an earlier one).
pub fn known_hung() -> bool {
    let mut s = STATE.lock().unwrap_or_else(|p| p.into_inner());
    let at = *s.get_or_insert_with(|| read(&path()));
    at.is_some_and(|t| is_recent(t, now_secs()))
}

/// Record a WGC hang for this and later launches.
pub fn remember_hang() {
    let now = now_secs();
    *STATE.lock().unwrap_or_else(|p| p.into_inner()) = Some(Some(now));
    let p = path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&p, now.to_string()) {
        log::warn!("could not save the WGC hang note: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_window() {
        let week = RETRY_AFTER.as_secs();
        assert!(is_recent(1000, 1000));
        assert!(is_recent(1000, 1000 + week - 1));
        assert!(!is_recent(1000, 1000 + week));
        // A clock set backwards does not keep WGC disabled forever.
        assert!(!is_recent(5000, 1000));
    }

    #[test]
    fn reads_what_it_writes() {
        let d = std::env::temp_dir().join(format!("stinkshot-wgc-memo-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join(FILE);
        std::fs::write(&p, "12345\n").unwrap();
        assert_eq!(read(&p), Some(12345));
        std::fs::write(&p, "garbage").unwrap();
        assert_eq!(read(&p), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
