//! File logger at `%APPDATA%\Stinkshot\stinkshot.log` (`logger::path`, profile-aware), rotated to `stinkshot.log.1` at 5 MB.
//! In debug builds with a console, warnings and errors are mirrored to stderr.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use log::{LevelFilter, Log, Metadata, Record};

const MAX_BYTES: u64 = 5 * 1024 * 1024;

struct FileLogger {
    file: Mutex<File>,
    path: PathBuf,
    written: Mutex<u64>,
}

impl FileLogger {
    fn rotate_if_needed(&self, file: &mut File, written: &mut u64) {
        if *written < MAX_BYTES {
            return;
        }
        let old = self.path.with_extension("log.1");
        let _ = std::fs::remove_file(&old);
        // Windows cannot rename an open file; reopen after the rename via a fresh handle.
        let _ = file.flush();
        if std::fs::rename(&self.path, &old).is_ok()
            && let Ok(f) = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
        {
            *file = f;
            *written = 0;
        }
    }
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        let t = metadata.target();
        let noisy = t.starts_with("wgpu")
            || t.starts_with("naga")
            || t.starts_with("winit")
            || t.starts_with("egui")
            || t.starts_with("eframe");
        if noisy {
            metadata.level() <= log::Level::Warn
        } else {
            true
        }
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "{} {:<5} {} - {}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
            record.level(),
            record.target(),
            record.args()
        );
        if cfg!(debug_assertions) && record.level() <= log::Level::Warn {
            eprint!("{line}");
        }
        if let (Ok(mut file), Ok(mut written)) = (self.file.lock(), self.written.lock()) {
            if file.write_all(line.as_bytes()).is_ok() {
                *written += line.len() as u64;
            }
            self.rotate_if_needed(&mut file, &mut written);
        }
    }

    fn flush(&self) {
        if let Ok(mut f) = self.file.lock() {
            let _ = f.flush();
        }
    }
}

/// The log file of the current profile (Settings > About > Open Log).
pub fn path() -> PathBuf {
    stinkshot_core::config::Config::dir().join("stinkshot.log")
}

/// Install the logger and a panic hook that records panics. Safe to call once; failures
/// (read-only profile, ...) leave logging disabled rather than aborting the app.
pub fn init() {
    let dir = stinkshot_core::config::Config::dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("stinkshot: cannot create {}: {e}", dir.display());
        return;
    }
    let path = path();
    let written = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let file = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("stinkshot: cannot open {}: {e}", path.display());
            return;
        }
    };
    let logger = FileLogger {
        file: Mutex::new(file),
        path,
        written: Mutex::new(written),
    };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        // `STINKSHOT_LOG=debug|trace` raises the level of a release build (timing trace points,
        // capture session decisions) for diagnosis.
        let level = match std::env::var("STINKSHOT_LOG")
            .ok()
            .map(|v| v.to_ascii_lowercase())
            .as_deref()
        {
            Some("trace") => LevelFilter::Trace,
            Some("debug") => LevelFilter::Debug,
            _ if cfg!(debug_assertions) => LevelFilter::Debug,
            _ => LevelFilter::Info,
        };
        log::set_max_level(level);
    }
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("panic: {info}");
        default_hook(info);
    }));
    log::info!(
        "Stinkshot {} starting (pid {})",
        env!("CARGO_PKG_VERSION"),
        std::process::id()
    );
}
