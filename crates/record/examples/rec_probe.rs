//! Records `SECS` seconds of a region (default: primary monitor) with debug logging on
//! stderr, without making the process DPI aware (like the test binaries), and prints the
//! status and result. For diagnosing capture start-up.
//!
//! `cargo run -p stinkshot-record --example rec_probe -- [SECS [X Y W H]]`; `FPS=` sets the
//! frame rate (default 30).

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
    if std::env::var("AWARE").as_deref() == Ok("1") {
        stinkshot_platform::dpi::init();
    }
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
    // MIC=1: first microphone (seconds only; the file is deleted at the end).
    // CAM=1: first webcam as a 160 px bubble, bottom right.
    let mic = (std::env::var("MIC").as_deref() == Ok("1"))
        .then(|| list_microphones().into_iter().next())
        .flatten();
    let cam = (std::env::var("CAM").as_deref() == Ok("1"))
        .then(|| list_webcams().into_iter().next())
        .flatten();
    eprintln!("microphone {mic:?}, webcam {cam:?}");
    let rec = Recorder::start(RecordOptions {
        region,
        output: out.clone(),
        fps: std::env::var("FPS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30),
        bitrate_kbps: 8000,
        system_audio: false,
        microphone: mic.is_some(),
        mic_device: mic.clone(),
        webcam: cam.clone().map(|d| WebcamOptions {
            device: Some(d),
            diameter: 160,
            corner: Corner::BottomRight,
        }),
        show_clicks: false,
        show_keys: false,
        include_cursor: true,
        accent: [0x4C, 0xA3, 0xFF, 255],
    })?;
    for _ in 0..secs * 2 {
        std::thread::sleep(Duration::from_millis(500));
        eprintln!("status {:?}", rec.status());
    }
    eprintln!("backend {}, {} fps", rec.capture_backend(), rec.fps());
    let r = rec.stop();
    eprintln!("stop: {r:?}");
    if let Ok(res) = &r {
        eprintln!("probe {:?}", probe(&res.path));
        if mic.is_some() {
            let (mut sum, mut n, mut peak) = (0f64, 0u64, 0i32);
            let mut on_audio = |_pts: i64, pcm: &[i16]| {
                for &s in pcm {
                    sum += (s as f64).powi(2);
                    n += 1;
                    peak = peak.max((s as i32).abs());
                }
            };
            decode_all(&res.path, None, Some(&mut on_audio))?;
            let rms = (sum / n.max(1) as f64).sqrt();
            eprintln!(
                "audio track: {n} samples, RMS {:.1} dBFS, peak {:.1} dBFS",
                20.0 * (rms / 32768.0).log10(),
                20.0 * (peak.max(1) as f64 / 32768.0).log10()
            );
        }
        if cam.is_some() {
            let f = frame_at(&res.path, Duration::from_millis(1200), res.height)?;
            let s = (res.height as f32 / 1080.0).clamp(0.8, 2.0);
            let margin = (24.0 * s) as u32;
            let (x, y) = (res.width - 160 - margin, res.height - 160 - margin);
            eprintln!(
                "bubble ring pixel {:?}, centre {:?}, outside {:?}",
                f.pixel(x + 80, y + 1),
                f.pixel(x + 80, y + 80),
                f.pixel(x - 10, y + 80)
            );
        }
    }
    let _ = std::fs::remove_file(&out);
    eprintln!("deleted {}: {}", out.display(), !out.exists());
    Ok(())
}
