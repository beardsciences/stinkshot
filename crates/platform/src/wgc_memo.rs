//! Remembers across launches that Windows.Graphics.Capture hangs on this PC.
//!
//! The in-process [`crate::hang::HangGate`] stops repeated waits within one run, but every
//! launch still paid the full WGC timeout (about 3 s) on its first window capture and its
//! first recording. A hang is written to `wgc-hang.txt` in the config folder and WGC is then
//! skipped outright: for [`FIRST_RETRY_AFTER`] after one hang (a single timeout can be one
//! slow window or a busy moment), for [`RETRY_AFTER`] once the retry hung too. A WGC capture
//! that works clears the note.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const FILE: &str = "wgc-hang.txt";
pub const FIRST_RETRY_AFTER: Duration = Duration::from_secs(24 * 3600);
pub const RETRY_AFTER: Duration = Duration::from_secs(7 * 24 * 3600);

/// The remembered hang: unix seconds of the last one, and how many in a row.
type Note = (u64, u32);

/// None: not loaded yet. Some(None): no hang remembered.
static STATE: Mutex<Option<Option<Note>>> = Mutex::new(None);

fn path() -> PathBuf {
    stinkshot_core::config::Config::dir().join(FILE)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// "<unix seconds> <count>"; a file from before the count existed holds only the time and
/// reads as one hang.
fn read(p: &Path) -> Option<Note> {
    let text = std::fs::read_to_string(p).ok()?;
    let mut parts = text.split_whitespace();
    let at = parts.next()?.parse().ok()?;
    let count = match parts.next() {
        Some(c) => c.parse().ok()?,
        None => 1,
    };
    Some((at, count))
}

fn is_recent((at, count): Note, now: u64) -> bool {
    let wait = if count > 1 {
        RETRY_AFTER
    } else {
        FIRST_RETRY_AFTER
    };
    now >= at && now - at < wait.as_secs()
}

/// True if WGC hung on this PC recently (in this run or an earlier one) and is being skipped.
pub fn known_hung() -> bool {
    let mut s = STATE.lock().unwrap_or_else(|p| p.into_inner());
    let note = *s.get_or_insert_with(|| read(&path()));
    note.is_some_and(|n| is_recent(n, now_secs()))
}

/// Record a WGC hang for this and later launches.
pub fn remember_hang() {
    let now = now_secs();
    let count = {
        let mut s = STATE.lock().unwrap_or_else(|p| p.into_inner());
        let before = *s.get_or_insert_with(|| read(&path()));
        let count = before.map_or(1, |(_, c)| c.saturating_add(1));
        *s = Some(Some((now, count)));
        count
    };
    let p = path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&p, format!("{now} {count}")) {
        log::warn!("could not save the WGC hang note: {e}");
    }
}

/// A WGC capture worked: forget any remembered hang.
pub fn note_ok() {
    let had = {
        let mut s = STATE.lock().unwrap_or_else(|p| p.into_inner());
        let before = *s.get_or_insert_with(|| read(&path()));
        *s = Some(None);
        before.is_some()
    };
    if had {
        let _ = std::fs::remove_file(path());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_window() {
        let day = FIRST_RETRY_AFTER.as_secs();
        let week = RETRY_AFTER.as_secs();
        // One hang: a day.
        assert!(is_recent((1000, 1), 1000));
        assert!(is_recent((1000, 1), 1000 + day - 1));
        assert!(!is_recent((1000, 1), 1000 + day));
        // The retry hung too: a week.
        assert!(is_recent((1000, 2), 1000 + day));
        assert!(is_recent((1000, 2), 1000 + week - 1));
        assert!(!is_recent((1000, 2), 1000 + week));
        // A clock set backwards does not keep WGC disabled forever.
        assert!(!is_recent((5000, 3), 1000));
    }

    #[test]
    fn reads_what_it_writes() {
        let d = std::env::temp_dir().join(format!("stinkshot-wgc-memo-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join(FILE);
        std::fs::write(&p, "12345\n").unwrap();
        assert_eq!(read(&p), Some((12345, 1)));
        std::fs::write(&p, "12345 3").unwrap();
        assert_eq!(read(&p), Some((12345, 3)));
        std::fs::write(&p, "garbage").unwrap();
        assert_eq!(read(&p), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
