//! Records `SECS` seconds of a region (default: primary monitor) with debug logging on
//! stderr, without making the process DPI aware (like the test binaries), and prints the
//! status and result. For diagnosing capture start-up.
//!
//! `cargo run -p stinkshot-record --example rec_probe -- [SECS [X Y W H]]`

use std::time::Duration;

use stinkshot_core::PxRect;
use stinkshot_record::*;

struct Stderr;

impl log::Log for Stderr {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.target().starts_with("stinkshot")
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            eprintln!("[{} {}] {}", r.level(), r.target(), r.args());
        }
    }
    fn flush(&self) {}
}

fn main() -> anyhow::Result<()> {
    log::set_logger(&Stderr).unwrap();
    log::set_max_level(log::LevelFilter::Trace);
    let a: Vec<i32> = std::env::args()
        .skip(1)
        .map(|s| s.parse().unwrap())
        .collect();
    let secs = a.first().copied().unwrap_or(2) as u64;
    let region = if a.len() >= 5 {
        PxRect::new(a[1], a[2], a[3] & !1, a[4] & !1)
    } else {
        primary_monitor_rect().unwrap()
    };
    let out = std::env::temp_dir().join(format!("stinkshot-rec-probe-{}.mp4", std::process::id()));
    let rec = Recorder::start(RecordOptions {
        region,
        output: out.clone(),
        fps: 30,
        bitrate_kbps: 8000,
        system_audio: false,
        microphone: false,
        mic_device: None,
        webcam: None,
        show_clicks: false,
        show_keys: false,
        include_cursor: true,
        accent: [0x4C, 0xA3, 0xFF, 255],
    })?;
    for _ in 0..secs * 2 {
        std::thread::sleep(Duration::from_millis(500));
        eprintln!("status {:?}", rec.status());
    }
    eprintln!("backend {}", rec.capture_backend());
    let r = rec.stop();
    eprintln!("stop: {r:?}");
    let _ = std::fs::remove_file(&out);
    Ok(())
}
