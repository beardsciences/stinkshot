//! Background PNG re-compression of saved captures (docs/notes/app-shell.md, "Perf track").
//!
//! The capture path saves with the fast encoder so the file exists at once (clipboard,
//! toast, history). Each saved PNG is then queued here; one thread at background priority
//! re-encodes it losslessly (`stinkshot_core::png_opt`: color reductions, filter trials,
//! miniz level 10, or zopfli for images up to 1 MP) and swaps the smaller file in.
//!
//! Never loses data and never fights other programs:
//! - the new file is written and flushed beside the original first; the original is only
//!   replaced by an atomic rename, and only when it still has the size and modification time
//!   it had when it was saved and no other program has it open
//!   (`files::replace_if_unchanged`); otherwise the job is dropped (moved, re-edited, opened
//!   elsewhere);
//! - Stinkshot's own writes to capture files take `write_lock`, so a re-edit cannot land
//!   between that check and the swap;
//! - the result must decode to exactly the original pixels (checked in `png_opt`);
//! - a temp file left by a crash or exit mid-write (`*.stinkshot-tmp`, never listed as a
//!   capture) is deleted at the next start.
//!
//! Not counted as a job in flight: quitting does not wait for it.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use stinkshot_core::png_opt::{self, Effort, Outcome};
use stinkshot_history::{History, ItemId};
use stinkshot_platform::files::{self, Replace};

/// Suffix of the temp file written next to the capture.
const TEMP_SUFFIX: &str = ".stinkshot-tmp";
/// Wait after the save before starting: whatever the user does with a fresh capture
/// (paste, drag, open) gets the file first, and a burst of captures queues up.
const SETTLE: Duration = Duration::from_secs(2);
/// A file another program had open is tried once more after this long.
const RETRY_IN_USE: Duration = Duration::from_secs(20);
/// Images up to this many pixels get zopfli (about 13 s per megapixel on one background
/// core); bigger ones miniz level 10 (about 1 s per megapixel), see the notes.
const ZOPFLI_MAX_PIXELS: u64 = 1_000_000;

struct Job {
    path: PathBuf,
    len: u64,
    modified: SystemTime,
    item: Option<(Arc<History>, ItemId)>,
    not_before: Instant,
    retried: bool,
}

struct Queue {
    jobs: Mutex<VecDeque<Job>>,
    cv: Condvar,
}

static QUEUE: OnceLock<Queue> = OnceLock::new();
static WRITE_LOCK: Mutex<()> = Mutex::new(());
/// Bytes saved since start (logged).
static SAVED: AtomicU64 = AtomicU64::new(0);

/// Hold while writing a capture file that may be queued here.
pub fn write_lock() -> MutexGuard<'static, ()> {
    WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn queue() -> &'static Queue {
    QUEUE.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("png-recompress".into())
            .spawn(worker)
            .map_err(|e| log::warn!("png re-compression thread: {e}"));
        Queue {
            jobs: Mutex::new(VecDeque::new()),
            cv: Condvar::new(),
        }
    })
}

/// Delete temp files a previous run left in `folder` (crash or exit mid-write).
pub fn clean_up(folder: &Path) {
    let folder = folder.to_path_buf();
    let _ = std::thread::Builder::new()
        .name("png-recompress-cleanup".into())
        .spawn(move || {
            let Ok(dir) = std::fs::read_dir(&folder) else {
                return;
            };
            for e in dir.flatten() {
                let name = e.file_name();
                if name.to_string_lossy().ends_with(TEMP_SUFFIX) {
                    match std::fs::remove_file(e.path()) {
                        Ok(()) => log::info!("removed leftover {}", e.path().display()),
                        Err(err) => log::debug!("leftover {}: {err}", e.path().display()),
                    }
                }
            }
        });
}

/// Queue a just-saved file. Anything but `.png` is ignored. `item`: its history entry, whose
/// stored size is refreshed after a swap.
pub fn enqueue(path: &Path, item: Option<(Arc<History>, ItemId)>) {
    let is_png = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("png"));
    if !is_png {
        return;
    }
    let md = match std::fs::metadata(path) {
        Ok(md) => md,
        Err(e) => {
            log::debug!("recompress: stat {}: {e}", path.display());
            return;
        }
    };
    let Ok(modified) = md.modified() else {
        return;
    };
    let q = queue();
    let mut jobs = q.jobs.lock().unwrap_or_else(|e| e.into_inner());
    jobs.retain(|j| j.path != path);
    jobs.push_back(Job {
        path: path.to_path_buf(),
        len: md.len(),
        modified,
        item,
        not_before: Instant::now() + SETTLE,
        retried: false,
    });
    q.cv.notify_one();
}

fn worker() {
    files::background_thread();
    // `queue()` may still be initializing on the thread that spawned this one.
    let q = loop {
        if let Some(q) = QUEUE.get() {
            break q;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    loop {
        let job = {
            let mut jobs = q.jobs.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                let now = Instant::now();
                if let Some(i) = jobs.iter().position(|j| j.not_before <= now) {
                    break jobs.remove(i);
                }
                let wait = jobs
                    .iter()
                    .map(|j| j.not_before.saturating_duration_since(now))
                    .min()
                    .unwrap_or(Duration::from_secs(3600));
                jobs =
                    q.cv.wait_timeout(jobs, wait)
                        .map(|(g, _)| g)
                        .unwrap_or_else(|e| e.into_inner().0);
            }
        };
        let Some(job) = job else { continue };
        if let Some(retry) = run(job) {
            let mut jobs = q.jobs.lock().unwrap_or_else(|e| e.into_inner());
            jobs.push_back(retry);
        }
    }
}

/// Pixel count from the IHDR chunk.
fn pixels(png: &[u8]) -> Option<u64> {
    if png.len() < 24 || &png[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(png[16..20].try_into().ok()?) as u64;
    let h = u32::from_be_bytes(png[20..24].try_into().ok()?) as u64;
    Some(w * h)
}

fn effort_for(pixels: u64) -> Effort {
    if pixels <= ZOPFLI_MAX_PIXELS {
        Effort::Zopfli(1)
    } else {
        Effort::Miniz
    }
}

fn unchanged(job: &Job) -> bool {
    std::fs::metadata(&job.path)
        .ok()
        .is_some_and(|md| md.len() == job.len && md.modified().ok() == Some(job.modified))
}

/// Process one job. Returns it again when it should be retried later.
fn run(mut job: Job) -> Option<Job> {
    let name = job.path.display().to_string();
    if !unchanged(&job) {
        log::debug!("recompress: {name} moved or changed; skipped");
        return None;
    }
    let original = match std::fs::read(&job.path) {
        Ok(b) if b.len() as u64 == job.len => b,
        Ok(_) => return None,
        Err(e) => {
            log::debug!("recompress: read {name}: {e}");
            return None;
        }
    };
    let effort = effort_for(pixels(&original).unwrap_or(u64::MAX));
    let t0 = Instant::now();
    let smaller = match png_opt::optimize(&original, effort) {
        Ok((Outcome::Smaller(b), report)) => {
            log::debug!(
                "recompress: {name} {} -> {} bytes ({} / {}, {effort:?}, {:.0} ms)",
                original.len(),
                b.len(),
                report.color,
                report.filter,
                t0.elapsed().as_secs_f64() * 1000.0
            );
            b
        }
        Ok((Outcome::NotSmaller, _)) => return None,
        Ok((Outcome::Skipped(why), _)) => {
            log::debug!("recompress: {name} skipped ({why})");
            return None;
        }
        Err(e) => {
            log::warn!("recompress: {name}: {e:#}");
            return None;
        }
    };
    let file_name = job.path.file_name()?;
    let mut temp_name = file_name.to_os_string();
    temp_name.push(format!(".{}{TEMP_SUFFIX}", std::process::id()));
    let temp = job.path.with_file_name(temp_name);
    let written = (|| -> std::io::Result<()> {
        use std::io::Write;
        let mut f = std::fs::File::create(&temp)?;
        f.write_all(&smaller)?;
        f.sync_all()
    })();
    if let Err(e) = written {
        log::debug!("recompress: write {}: {e}", temp.display());
        let _ = std::fs::remove_file(&temp);
        return None;
    }
    let outcome = {
        let _g = write_lock();
        files::replace_if_unchanged(&job.path, &temp, job.len, job.modified)
    };
    match outcome {
        Ok(Replace::Replaced) => {
            let saved = original.len().saturating_sub(smaller.len()) as u64;
            let total = SAVED.fetch_add(saved, Ordering::Relaxed) + saved;
            log::info!(
                "recompressed {name}: {} -> {} bytes ({:.0}%), {} MB saved this session",
                original.len(),
                smaller.len(),
                100.0 * smaller.len() as f64 / original.len() as f64,
                total / 1_048_576
            );
            if let Some((history, id)) = &job.item
                && let Err(e) = history.refresh_bytes(*id)
            {
                log::debug!("recompress: history size of {name}: {e:#}");
            }
            None
        }
        Ok(other) => {
            let _ = std::fs::remove_file(&temp);
            log::debug!("recompress: {name} not replaced ({other:?})");
            if other == Replace::InUse && !job.retried {
                job.retried = true;
                job.not_before = Instant::now() + RETRY_IN_USE;
                return Some(job);
            }
            None
        }
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            log::warn!("recompress: replace {name}: {e:#}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effort_by_size() {
        assert_eq!(effort_for(800 * 600), Effort::Zopfli(1));
        assert_eq!(effort_for(3840 * 2160), Effort::Miniz);
    }

    #[test]
    fn pixel_count_from_header() {
        let img = stinkshot_core::Rgba::new(7, 5);
        assert_eq!(pixels(&img.to_png().unwrap()), Some(35));
        assert_eq!(pixels(b"short"), None);
    }

    #[test]
    fn end_to_end_replaces_with_identical_pixels() {
        let dir = std::env::temp_dir().join(format!("stinkshot-recompress-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cap.png");
        let mut img = stinkshot_core::Rgba::new(120, 80);
        for (i, p) in img.data.chunks_exact_mut(4).enumerate() {
            p.copy_from_slice(&[(i % 120) as u8, (i / 120) as u8, 40, 255]);
        }
        std::fs::write(&path, img.to_png().unwrap()).unwrap();
        let md = std::fs::metadata(&path).unwrap();
        let job = Job {
            path: path.clone(),
            len: md.len(),
            modified: md.modified().unwrap(),
            item: None,
            not_before: Instant::now(),
            retried: false,
        };
        assert!(run(job).is_none());
        let after = std::fs::metadata(&path).unwrap();
        assert!(after.len() < md.len(), "{} -> {}", md.len(), after.len());
        assert_eq!(after.modified().unwrap(), md.modified().unwrap());
        assert_eq!(stinkshot_core::Rgba::load(&path).unwrap().data, img.data);
        let leftovers = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(leftovers, 1, "temp file left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
