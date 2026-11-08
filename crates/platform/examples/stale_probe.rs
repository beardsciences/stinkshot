//! Capture freshness and timing probe.
//!
//! `cargo run -p stinkshot-platform --example stale_probe --release -- <mode> [runs]`
//!
//! Modes:
//! - `cycle [runs]`: per run, drop the session, warm it up normally, wait, open a small
//!   solid marker window on every monitor (this process's own windows, no input is sent),
//!   wait with nothing else changing, then check that `freeze_all` and `capture_rect` show
//!   the markers; close them, wait, and check that they are gone.
//! - `latefirst [runs]`: the same, but the warm-up does not wait for each output's first
//!   duplication frame (simulates a busy start-up where that frame arrives late).
//! - `timing [runs]`: warm up once, then time `freeze_all` `runs` times (300 ms apart) and
//!   print the sorted distribution.
//! - `gdi [runs]`: time a GDI-only capture of each monitor.
//!
//! Set `PROBE_LOG=debug` (or `trace`) to see the capture module's log on stderr, and
//! `PROBE_ASSUME_ON=1` to time the duplication path in `timing` while the displays are off.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use stinkshot_core::{PxRect, Rgba};
use stinkshot_platform::{capture, dpi, monitors};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateSolidBrush, GetDC, GetPixel, HBRUSH, ReleaseDC};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

const MARK: [u8; 3] = [19, 199, 90];
const SIZE: i32 = 160;

struct Logger;
impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::max_level()
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            eprintln!("[{:>5}] {}", r.level(), r.args());
        }
    }
    fn flush(&self) {}
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn marker_rects() -> Vec<PxRect> {
    monitors::list()
        .iter()
        .map(|m| PxRect::new(m.rect.x + 200, m.rect.y + 200, SIZE, SIZE))
        .collect()
}

/// Open the markers on a private thread; returns a sender that closes them.
fn open_markers(rects: Vec<PxRect>) -> mpsc::Sender<()> {
    let (tx, rx) = mpsc::channel::<()>();
    let (ready_tx, ready_rx) = mpsc::channel();
    std::thread::spawn(move || unsafe {
        let _aware = dpi::ThreadPerMonitorAware::new();
        let hinst = GetModuleHandleW(None).unwrap();
        let brush = CreateSolidBrush(COLORREF(
            MARK[0] as u32 | (MARK[1] as u32) << 8 | (MARK[2] as u32) << 16,
        ));
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: w!("StinkshotStaleProbeMarker"),
            hbrBackground: HBRUSH(brush.0),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let mut hwnds = Vec::new();
        for r in &rects {
            let h = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("StinkshotStaleProbeMarker"),
                w!("stale probe marker"),
                WS_POPUP,
                r.x,
                r.y,
                r.w,
                r.h,
                None,
                None,
                Some(hinst.into()),
                None,
            )
            .unwrap();
            let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
            hwnds.push(h);
        }
        let _ = ready_tx.send(());
        let mut msg = MSG::default();
        loop {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                DispatchMessageW(&msg);
            }
            if rx.try_recv().is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        for h in hwnds {
            let _ = DestroyWindow(h);
        }
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            DispatchMessageW(&msg);
        }
    });
    let _ = ready_rx.recv();
    tx
}

fn is_mark(p: [u8; 4]) -> bool {
    p[..3]
        .iter()
        .zip(MARK)
        .all(|(&a, b)| (a as i32 - b as i32).abs() <= 2)
}

fn gdi_sees_mark(r: PxRect) -> bool {
    unsafe {
        let dc = GetDC(None);
        let c = GetPixel(dc, r.x + r.w / 2, r.y + r.h / 2).0;
        ReleaseDC(None, dc);
        is_mark([c as u8, (c >> 8) as u8, (c >> 16) as u8, 255])
    }
}

fn frozen_sees_mark(img: &Rgba, bounds: PxRect, r: PxRect) -> bool {
    let (x, y) = (r.x + r.w / 2 - bounds.x, r.y + r.h / 2 - bounds.y);
    is_mark(img.pixel(x as u32, y as u32))
}

fn cycle(runs: usize, first_timeout: u32) {
    let rects = marker_rects();
    let (mut stale, mut checks) = (0, 0);
    for run in 0..runs {
        capture::release_cached_capture();
        capture::warm_up_with(first_timeout);
        std::thread::sleep(Duration::from_millis(1000));
        let close = open_markers(rects.clone());
        std::thread::sleep(Duration::from_millis(1500));
        let gdi: Vec<bool> = rects.iter().map(|r| gdi_sees_mark(*r)).collect();
        let t = Instant::now();
        let f = capture::freeze_all(false).expect("freeze_all");
        let ft = t.elapsed();
        let fz: Vec<bool> = rects
            .iter()
            .map(|r| frozen_sees_mark(&f.image, f.bounds, *r))
            .collect();
        let cr: Vec<bool> = rects
            .iter()
            .map(|r| {
                let img = capture::capture_rect(*r, false).expect("capture_rect");
                is_mark(img.pixel(r.w as u32 / 2, r.h as u32 / 2))
            })
            .collect();
        let _ = close.send(());
        std::thread::sleep(Duration::from_millis(1500));
        let gone_gdi: Vec<bool> = rects.iter().map(|r| !gdi_sees_mark(*r)).collect();
        let f2 = capture::freeze_all(false).expect("freeze_all");
        let gone: Vec<bool> = rects
            .iter()
            .map(|r| !frozen_sees_mark(&f2.image, f2.bounds, *r))
            .collect();
        for i in 0..rects.len() {
            if gdi[i] {
                checks += 2;
                stale += (!fz[i]) as usize + (!cr[i]) as usize;
            }
            if gone_gdi[i] {
                checks += 1;
                stale += (!gone[i]) as usize;
            }
        }
        println!(
            "run {run}: gdi {gdi:?} freeze_all {fz:?} ({ft:.1?}) capture_rect {cr:?} | closed: gdi {gone_gdi:?} freeze_all {gone:?}"
        );
    }
    println!("stale results: {stale} of {checks} checks");
}

fn stats(label: &str, mut v: Vec<f64>) {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let q = |p: f64| v[((v.len() - 1) as f64 * p).round() as usize];
    println!(
        "{label}: n {} min {:.1} median {:.1} p90 {:.1} max {:.1} ms",
        v.len(),
        v[0],
        q(0.5),
        q(0.9),
        v[v.len() - 1]
    );
    println!(
        "  sorted: {}",
        v.iter()
            .map(|x| format!("{x:.0}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
}

fn main() {
    dpi::init();
    let level = std::env::var("PROBE_LOG").ok();
    let _ = log::set_logger(&Logger);
    log::set_max_level(match level.as_deref() {
        Some("trace") => log::LevelFilter::Trace,
        Some("debug") => log::LevelFilter::Debug,
        _ => log::LevelFilter::Info,
    });
    let args: Vec<String> = std::env::args().skip(1).collect();
    let runs = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(10);
    match args.first().map(|s| s.as_str()) {
        Some("cycle") => cycle(runs, 300),
        Some("latefirst") => cycle(runs, 0),
        Some("timing") => {
            if std::env::var("PROBE_ASSUME_ON").is_ok() {
                // Time the duplication path even while the displays are idle-off (the
                // pictures are then stale, only the timing counts).
                stinkshot_platform::display_power::override_state(Some(
                    stinkshot_platform::display_power::DisplayPower::On,
                ));
            }
            println!(
                "display power: {:?}",
                stinkshot_platform::display_power::state()
            );
            let t = Instant::now();
            capture::warm_up();
            println!("warm_up {:.1?}", t.elapsed());
            let mut v = Vec::new();
            for _ in 0..runs {
                std::thread::sleep(Duration::from_millis(300));
                let t = Instant::now();
                let f = capture::freeze_all(true).expect("freeze_all");
                v.push(t.elapsed().as_secs_f64() * 1000.0);
                drop(f);
            }
            stats("freeze_all", v);
        }
        Some("gdi") => {
            for m in monitors::list() {
                let mut v = Vec::new();
                for _ in 0..runs {
                    let t = Instant::now();
                    let img = capture::capture_rect_once(m.rect, false).expect("gdi");
                    v.push(t.elapsed().as_secs_f64() * 1000.0);
                    drop(img);
                }
                stats(&format!("gdi {}x{}", m.rect.w, m.rect.h), v);
            }
        }
        _ => eprintln!("usage: stale_probe cycle|latefirst|timing|gdi [runs]"),
    }
    capture::release_cached_capture();
}
