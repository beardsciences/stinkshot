//! Memory probe for the capture session: private bytes (commit charge) after each step.
//!
//! `cargo run -p stinkshot-platform --example mem_probe --release [-- cycles]`
//!
//! Steps: start, `warm_up`, `freeze_all` (image dropped), a second `freeze_all`,
//! `release_cached_capture`, `warm_up` again, `cycles` more freezes, then
//! `release_idle_shadows` and a freeze after it. `PROBE_HOLD=secs` keeps the process alive at
//! the end (to inspect it from outside). Set `PROBE_LOG=debug`
//! for the capture module's log on stderr. The console display state is printed first: with
//! the displays off the duplication path is not taken (GDI serves every capture).

use std::time::Instant;

use stinkshot_platform::{capture, display_power};

#[repr(C)]
#[derive(Default)]
struct Counters {
    cb: u32,
    page_fault_count: u32,
    peak_working_set: usize,
    working_set: usize,
    quota_peak_paged_pool: usize,
    quota_paged_pool: usize,
    quota_peak_non_paged_pool: usize,
    quota_non_paged_pool: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
    private_usage: usize,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, cb: u32) -> i32;
    fn GetCurrentProcess() -> isize;
}

fn mem() -> (f64, f64) {
    let mut c = Counters {
        cb: std::mem::size_of::<Counters>() as u32,
        ..Default::default()
    };
    // SAFETY: live out pointer of the PROCESS_MEMORY_COUNTERS_EX layout.
    unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    (
        c.private_usage as f64 / 1048576.0,
        c.working_set as f64 / 1048576.0,
    )
}

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

fn step(name: &str, t0: Instant, base: f64) {
    let (p, w) = mem();
    println!(
        "{name:<28} private {p:>7.1} MB ({:>+7.1})  ws {w:>7.1} MB  {:>7.1} ms",
        p - base,
        t0.elapsed().as_secs_f64() * 1000.0
    );
}

fn main() {
    if let Ok(l) = std::env::var("PROBE_LOG") {
        let _ = log::set_logger(&Logger);
        log::set_max_level(match l.as_str() {
            "trace" => log::LevelFilter::Trace,
            _ => log::LevelFilter::Debug,
        });
    }
    let cycles: usize = std::env::args()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    stinkshot_platform::dpi::init();
    // Register the display-state callback and give Windows a moment to report.
    let _ = capture::capture_rect_once(stinkshot_core::PxRect::new(0, 0, 4, 4), false);
    capture::warm_up_with(0);
    capture::release_cached_capture();
    std::thread::sleep(std::time::Duration::from_millis(300));
    println!("display state: {:?}", display_power::state());
    let (base, _) = mem();
    step("start (after a probe session)", Instant::now(), base);
    let t = Instant::now();
    capture::warm_up();
    step("warm_up", t, base);
    let t = Instant::now();
    drop(capture::freeze_all(false));
    step("freeze_all", t, base);
    let t = Instant::now();
    drop(capture::freeze_all(false));
    step("freeze_all again", t, base);
    std::thread::sleep(std::time::Duration::from_secs(2));
    step("idle 2 s", Instant::now(), base);
    let t = Instant::now();
    capture::release_cached_capture();
    step("release_cached_capture", t, base);
    let t = Instant::now();
    capture::warm_up();
    step("warm_up again", t, base);
    for i in 0..cycles {
        let t = Instant::now();
        drop(capture::freeze_all(false));
        step(&format!("freeze_all #{i}"), t, base);
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    std::thread::sleep(std::time::Duration::from_secs(2));
    step("idle 2 s", Instant::now(), base);
    let t = Instant::now();
    capture::release_idle_shadows(std::time::Duration::ZERO);
    step("release_idle_shadows", t, base);
    let t = Instant::now();
    drop(capture::freeze_all(false));
    step("freeze_all after release", t, base);
    capture::release_idle_shadows(std::time::Duration::ZERO);
    std::thread::sleep(std::time::Duration::from_secs(2));
    step("released again, idle 2 s", Instant::now(), base);
    if let Some(secs) = std::env::var("PROBE_HOLD")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        println!("holding {secs} s (pid {})", std::process::id());
        std::thread::sleep(std::time::Duration::from_secs(secs));
    }
}
