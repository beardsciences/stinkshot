//! Recording shakedown: records the `av_window` test window through `Recorder` (no UI, no
//! synthetic input) with system audio, pauses mid-way, then decodes the file and measures
//! frame pacing (CFR, duplicates, drops) and A/V offset over time. Also trims a 20 s piece,
//! checks that the offset survives the trim, and converts that piece to GIF.
//!
//! `cargo run -p stinkshot-record --example av_harness --release -- OUT_DIR [SECONDS [PAUSE_AT PAUSE_LEN]]`
//!
//! `FPS=60` (or 120) sets the recording frame rate (default 30).
//!
//! Every file it writes is deleted at the end unless `KEEP=1` is set (the recording contains
//! the system audio mix).

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use stinkshot_core::PxRect;
use stinkshot_record::*;
use windows::Win32::Foundation::{LPARAM, RECT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_CLOSE,
};
use windows::core::w;

const W: i32 = 640;
const H: i32 = 400;
/// Recording frame rate: `FPS` environment variable, default 30.
fn fps() -> u32 {
    std::env::var("FPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30)
}

#[derive(Default)]
struct WinLog {
    start: Option<i64>,
    beeps: HashMap<u32, i64>,
}

#[derive(Clone, Copy, Debug)]
struct Frame {
    pts: i64,
    /// Content render time (100 ns QPC) read from the barcode.
    t: i64,
    flash: bool,
}

fn luma(p: &[u8]) -> f32 {
    0.114 * p[0] as f32 + 0.587 * p[1] as f32 + 0.299 * p[2] as f32
}

/// Frames of `path` with their barcode times (relative to `t0`, 100 ns).
fn read_frames(path: &Path, t0: i64) -> anyhow::Result<Vec<Frame>> {
    let mut frames = Vec::new();
    let mut bad = 0;
    let mut on_video = |f: DecodedFrame<'_>| {
        let w = f.width as usize;
        let px = |x: usize, y: usize| &f.bgra[(y * w + x) * 4..][..4];
        let mut ms = 0u32;
        for i in 0..24 {
            let l = luma(px(16 + i * 16 + 8, 24));
            ms = (ms << 1) | (l > 128.0) as u32;
        }
        let mut sum = 0.0;
        let mut n = 0.0;
        for y in (f.height as usize - 50)..(f.height as usize - 20) {
            for x in (20..200).step_by(4) {
                sum += luma(px(x, y));
                n += 1.0;
            }
        }
        if ms > 10_000_000 {
            bad += 1;
        }
        frames.push(Frame {
            pts: f.pts_hns,
            t: t0 + ms as i64 * 10_000,
            flash: sum / n > 128.0,
        });
    };
    decode_all(path, Some(&mut on_video), None)?;
    if bad > 0 {
        println!("  warning: {bad} frames with an unreadable barcode");
    }
    Ok(frames)
}

/// Beep onsets (audio pts, 100 ns) found with a 1 kHz Goertzel detector, refined to the first
/// sample above a quarter of the beep amplitude. Also returns (first pts, gaps found).
fn read_beeps(path: &Path) -> anyhow::Result<(Vec<i64>, usize)> {
    let mut mono: Vec<f32> = Vec::new();
    let mut first: Option<i64> = None;
    let mut expect: i64 = 0;
    let mut gaps = 0;
    let mut on_audio = |pts: i64, pcm: &[i16]| {
        let start = *first.get_or_insert(pts);
        let at = start + mono.len() as i64 * 10_000_000 / 48_000;
        if mono.is_empty() {
            expect = at;
        }
        if (pts - at).abs() > 10_000 {
            gaps += 1;
        }
        let _ = expect;
        mono.extend(pcm.chunks_exact(2).map(|s| s[0] as f32 / 32768.0));
    };
    decode_all(path, None, Some(&mut on_audio))?;
    let start = first.unwrap_or(0);
    const B: usize = 48; // 1 ms
    let coeff = 2.0 * (std::f32::consts::TAU * 1000.0 / 48_000.0).cos();
    let mags: Vec<f32> = mono
        .chunks(B)
        .map(|c| {
            let (mut s1, mut s2) = (0.0f32, 0.0f32);
            for &x in c {
                let s = x + coeff * s1 - s2;
                s2 = s1;
                s1 = s;
            }
            (s1 * s1 + s2 * s2 - coeff * s1 * s2).max(0.0).sqrt()
        })
        .collect();
    let peak = mags.iter().cloned().fold(0.0, f32::max);
    let thr = peak * 0.3;
    let mut onsets = Vec::new();
    let mut quiet = 1000usize;
    for (i, &m) in mags.iter().enumerate() {
        if m > thr {
            if quiet >= 500 {
                // refine to the sample
                let amp = mono[i * B..((i + 30) * B).min(mono.len())]
                    .iter()
                    .fold(0.0f32, |a, &x| a.max(x.abs()));
                let from = i.saturating_sub(3) * B;
                let idx = (from..(i + 2) * B)
                    .find(|&s| mono[s].abs() > amp * 0.25)
                    .unwrap_or(i * B);
                onsets.push(start + idx as i64 * 10_000_000 / 48_000);
            }
            quiet = 0;
        } else {
            quiet += 1;
        }
    }
    Ok((onsets, gaps))
}

struct Sync {
    /// (beep k, audio onset pts, predicted video pts, flash pts, beep QPC)
    rows: Vec<(u32, i64, i64, Option<i64>, i64)>,
}

fn analyze(path: &Path, log: &WinLog, label: &str) -> anyhow::Result<Sync> {
    let t0 = log.start.unwrap();
    let frames = read_frames(path, t0)?;
    let (onsets, gaps) = read_beeps(path)?;
    println!(
        "== {label}: {} frames, {} beep onsets, audio gaps {gaps}",
        frames.len(),
        onsets.len()
    );
    // CFR
    let step = 10_000_000 / fps() as i64;
    let step_ms = 1000 / fps() as i64;
    let mut bad_pts = 0;
    let (mut dmin, mut dmax) = (i64::MAX, i64::MIN);
    for p in frames.windows(2) {
        let d = p[1].pts - p[0].pts;
        dmin = dmin.min(d);
        dmax = dmax.max(d);
        if (d - step).abs() > 2 {
            bad_pts += 1;
        }
    }
    println!(
        "  pts step min {:.3} ms, max {:.3} ms, off-grid steps {bad_pts}; first pts {:.3} ms",
        dmin as f64 / 1e4,
        dmax as f64 / 1e4,
        frames.first().map_or(0.0, |f| f.pts as f64 / 1e4)
    );
    // content pacing
    let mut dups = 0;
    let mut dup_run = 0;
    let mut max_dup_run = 0;
    let mut big = Vec::new();
    let mut hist = [0usize; 6];
    for (i, p) in frames.windows(2).enumerate() {
        let d = (p[1].t - p[0].t) / 10_000; // ms
        if d == 0 {
            dups += 1;
            dup_run += 1;
            max_dup_run = max_dup_run.max(dup_run);
        } else {
            dup_run = 0;
        }
        // buckets in frame intervals: <0.75, 0.75-1.4, 1.4-2.1, 2.1 s.. (as 30 fps: 24/45/70 ms)
        let b = if d == 0 {
            0
        } else if d * 4 < step_ms * 3 {
            1
        } else if d * 10 <= step_ms * 14 {
            2
        } else if d * 10 <= step_ms * 21 {
            3
        } else if d <= 1000 {
            4
        } else {
            5
        };
        hist[b] += 1;
        if d * 10 > step_ms * 21 {
            big.push((i + 1, d, p[1].pts as f64 / 1e7));
        }
    }
    println!(
        "  content step histogram (frame intervals of {step_ms} ms): dup {} | <0.75 {} | 0.75-1.4 {} | 1.4-2.1 {} | 2.1 to 1 s {} | >1s {}",
        hist[0], hist[1], hist[2], hist[3], hist[4], hist[5]
    );
    println!(
        "  duplicates {dups}, longest duplicate run {max_dup_run}, steps > 2.1 intervals: {} {:?}",
        big.len(),
        &big[..big.len().min(40)]
    );
    // Per-frame video lateness c = pts - render time of the content it shows, for frames
    // whose content changed (a duplicate's c only grows). The median over +-1 s is the
    // typical delay from screen to video timeline around a beep.
    let c: Vec<i64> = frames.iter().map(|f| f.pts - f.t).collect();
    let fresh: Vec<usize> = (1..frames.len())
        .filter(|&i| frames[i].t != frames[i - 1].t)
        .collect();
    {
        // global distribution relative to the median (excluding the pause jump)
        let mut v: Vec<i64> = fresh.iter().map(|&i| c[i]).collect();
        v.sort();
        if !v.is_empty() {
            let med = v[v.len() / 2];
            let near: Vec<f64> = v
                .iter()
                .filter(|&&x| (x - med).abs() < 5_000_000)
                .map(|&x| (x - med) as f64 / 1e4)
                .collect();
            let pct = |p: f64| near[((near.len() - 1) as f64 * p) as usize];
            println!(
                "  video lateness around its median (fresh frames, before pause), ms: p1 {:.1}, p5 {:.1}, p50 {:.1}, p95 {:.1}, p99 {:.1}",
                pct(0.01),
                pct(0.05),
                pct(0.5),
                pct(0.95),
                pct(0.99)
            );
        }
    }
    let mut rows = Vec::new();
    let mut ks: Vec<_> = log.beeps.iter().map(|(k, b)| (*k, *b)).collect();
    ks.sort();
    for (k, b) in ks {
        let mut near: Vec<i64> = fresh
            .iter()
            .filter(|&&i| (frames[i].t - b).abs() < 10_000_000)
            .map(|&i| c[i])
            .collect();
        if near.len() < 20 {
            continue; // not (fully) recorded: before start, after stop, or paused
        }
        near.sort();
        if near[near.len() - 1] - near[0] > 10_000_000 {
            continue; // straddles the pause
        }
        let v = b + near[near.len() / 2];
        let Some(&a) = onsets.iter().min_by_key(|&&a| (a - v).abs()) else {
            continue;
        };
        if (a - v).abs() > 3_000_000 {
            continue;
        }
        let flash = frames
            .iter()
            .filter(|f| f.flash && (f.pts - v).abs() < 3_000_000)
            .map(|f| f.pts)
            .min();
        rows.push((k, a, v, flash, b));
    }
    if rows.is_empty() {
        println!("  no beeps matched");
        return Ok(Sync { rows });
    }
    let off = |r: &(u32, i64, i64, Option<i64>, i64)| (r.1 - r.2) as f64 / 1e4;
    let n = rows.len();
    let seg = |a: usize, b: usize| {
        let s: Vec<f64> = rows[a..b].iter().map(off).collect();
        let mean = s.iter().sum::<f64>() / s.len() as f64;
        let min = s.iter().cloned().fold(f64::MAX, f64::min);
        let max = s.iter().cloned().fold(f64::MIN, f64::max);
        (mean, min, max)
    };
    let k5 = 5.min(n);
    let (s0, s1, s2) = seg(0, k5);
    let (m0, m1, m2) = seg(n / 2 - k5 / 2, n / 2 - k5 / 2 + k5);
    let (e0, e1, e2) = seg(n - k5, n);
    println!(
        "  A/V offset (audio minus video, ms; + = audio late): start {s0:.1} [{s1:.1}..{s2:.1}], middle {m0:.1} [{m1:.1}..{m2:.1}], end {e0:.1} [{e1:.1}..{e2:.1}] over {n} beeps"
    );
    // drift: least squares of offset vs audio time
    let xs: Vec<f64> = rows.iter().map(|r| r.1 as f64 / 1e7).collect();
    let ys: Vec<f64> = rows.iter().map(off).collect();
    let mx = xs.iter().sum::<f64>() / n as f64;
    let my = ys.iter().sum::<f64>() / n as f64;
    let sxy: f64 = xs.iter().zip(&ys).map(|(x, y)| (x - mx) * (y - my)).sum();
    let sxx: f64 = xs.iter().map(|x| (x - mx).powi(2)).sum();
    let slope = if sxx > 0.0 { sxy / sxx } else { 0.0 };
    let resid = xs
        .iter()
        .zip(&ys)
        .map(|(x, y)| (y - (my + slope * (x - mx))).abs())
        .fold(0.0, f64::max);
    println!(
        "  drift {:.2} ms/min, max residual {resid:.1} ms",
        slope * 60.0
    );
    let fl: Vec<f64> = rows
        .iter()
        .filter_map(|r| r.3.map(|f| (r.1 - f) as f64 / 1e4))
        .collect();
    if !fl.is_empty() {
        println!(
            "  flash-frame check (audio onset minus first white frame, ms, {} ms resolution): first {:.1}, last {:.1}, min {:.1}, max {:.1}",
            step_ms,
            fl[0],
            fl[fl.len() - 1],
            fl.iter().cloned().fold(f64::MAX, f64::min),
            fl.iter().cloned().fold(f64::MIN, f64::max)
        );
    }
    let all: Vec<i64> = rows.iter().map(|r| off(r).round() as i64).collect();
    println!("  per-beep A/V offset, ms: {all:?}");
    // Audio alone: where each beep landed on the audio timeline relative to when it really
    // played (A - B), per stretch between pauses, relative to the stretch's first beep.
    let mut stretches: Vec<Vec<f64>> = Vec::new();
    let mut prev: Option<i64> = None;
    for r in &rows {
        let e = r.1 - r.4;
        if prev.is_none_or(|p| (e - p).abs() > 5_000_000) {
            stretches.push(Vec::new());
        }
        stretches.last_mut().unwrap().push(e as f64 / 1e4);
        prev = Some(e);
    }
    for s in &mut stretches {
        let first = s[0];
        s.iter_mut().for_each(|x| *x -= first);
    }
    for (i, s) in stretches.iter().enumerate() {
        let min = s.iter().cloned().fold(f64::MAX, f64::min);
        let max = s.iter().cloned().fold(f64::MIN, f64::max);
        println!(
            "  audio placement, stretch {i} ({} beeps): last {:.1} ms, range {min:.1}..{max:.1} ms vs first beep",
            s.len(),
            s.last().unwrap()
        );
    }
    Ok(Sync { rows })
}

fn main() -> anyhow::Result<()> {
    stinkshot_platform::dpi::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = PathBuf::from(&args[0]);
    let secs: f64 = args.get(1).map_or(180.0, |s| s.parse().unwrap());
    let pause_at: f64 = args.get(2).map_or(secs / 2.0, |s| s.parse().unwrap());
    let pause_len: f64 = args.get(3).map_or(5.0, |s| s.parse().unwrap());
    std::fs::create_dir_all(&dir)?;

    // Window on a secondary (SDR) monitor if there is one, top-right of its work area.
    let mons = stinkshot_platform::monitors::list();
    let mon = mons
        .iter()
        .find(|m| m.rect.x != 0 || m.rect.y != 0)
        .unwrap_or(&mons[0]);
    let (x, y) = (mon.work.x + mon.work.w - W - 80, mon.work.y + 80);
    let exe = std::env::current_exe()?.with_file_name("av_window.exe");
    let life = (secs + pause_len + 20.0) as u64;
    let mut child = Command::new(exe)
        .args([x, y, W, H].map(|v| v.to_string()))
        .arg(life.to_string())
        .stdout(Stdio::piped())
        .spawn()?;
    let log = Arc::new(Mutex::new(WinLog::default()));
    {
        let log = log.clone();
        let out = child.stdout.take().unwrap();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let p: Vec<&str> = line.split_whitespace().collect();
                let mut l = log.lock().unwrap();
                match p.as_slice() {
                    ["start", t] => l.start = t.parse().ok(),
                    ["beep", k, t] => {
                        l.beeps.insert(k.parse().unwrap(), t.parse().unwrap());
                    }
                    _ => {}
                }
            }
        });
    }
    let t_wait = Instant::now();
    while log.lock().unwrap().start.is_none() && t_wait.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(1500));
    // Our own window: owned by the child, visible.
    let hwnd = unsafe { FindWindowW(w!("StinkshotAvSyncTest"), None) }?;
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    anyhow::ensure!(
        pid == child.id(),
        "test window belongs to pid {pid}, not ours"
    );
    anyhow::ensure!(
        unsafe { IsWindowVisible(hwnd) }.as_bool(),
        "test window not visible"
    );
    let mut r = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut r) }?;
    let region = PxRect::new(
        r.left,
        r.top,
        (r.right - r.left) & !1,
        (r.bottom - r.top) & !1,
    );
    println!("test window {region:?} on {}, {} fps", mon.name, fps());

    let out = dir.join("av.mp4");
    let started = Instant::now();
    let rec = Recorder::start(RecordOptions {
        region,
        output: out.clone(),
        fps: fps(),
        bitrate_kbps: 8000,
        system_audio: true,
        microphone: false,
        mic_device: None,
        webcam: None,
        show_clicks: false,
        show_keys: false,
        include_cursor: false,
        accent: [0x4C, 0xA3, 0xFF, 255],
    })?;
    println!(
        "start took {:?}; backend {}",
        started.elapsed(),
        rec.capture_backend()
    );
    let t_rec = Instant::now();
    let mut paused = false;
    let mut resumed = false;
    let mut next_status = 30.0;
    loop {
        let t = t_rec.elapsed().as_secs_f64();
        if !paused && t >= pause_at {
            rec.pause();
            paused = true;
            println!("paused at {t:.2} s: {:?}", rec.status());
        }
        if paused && !resumed && t >= pause_at + pause_len {
            rec.resume();
            resumed = true;
            println!("resumed at {t:.2} s: {:?}", rec.status());
        }
        if t >= next_status {
            println!("{t:.0} s: {:?}", rec.status());
            next_status += 30.0;
        }
        if t >= secs + pause_len {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let st = rec.status();
    let res = rec.stop()?;
    println!("final status {st:?}");
    println!("result {res:?}");
    // Close our window politely (it prints its render rate), then make sure it is gone.
    unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) }?;
    let t_close = Instant::now();
    while child.try_wait()?.is_none() && t_close.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    let log = Arc::try_unwrap(log).ok().unwrap().into_inner().unwrap();
    println!("window logged {} beeps", log.beeps.len());

    let info = probe(&out)?;
    println!(
        "probe {info:?}, file {} KB",
        std::fs::metadata(&out)?.len() / 1024
    );
    let full = analyze(&out, &log, "full recording")?;

    // Trim 20 s from the middle of the first half and re-check sync.
    let trimmed = dir.join("av-trim.mp4");
    let t_trim = Instant::now();
    let (a, b) = if secs >= 60.0 {
        (Duration::from_secs(30), Duration::from_secs(50))
    } else {
        (Duration::from_secs(2), Duration::from_secs_f64(secs - 2.0))
    };
    trim(&out, &trimmed, a, b, &|_| {})?;
    let tinfo = probe(&trimmed)?;
    println!(
        "trim 30..50 s took {:?}: {tinfo:?}, {} KB",
        t_trim.elapsed(),
        std::fs::metadata(&trimmed)?.len() / 1024
    );
    let tsync = analyze(&trimmed, &log, "trimmed")?;
    // The same beeps must keep their offset.
    let mut diffs = Vec::new();
    for r in &tsync.rows {
        if let Some(f) = full.rows.iter().find(|f| f.0 == r.0) {
            diffs.push(((r.1 - r.2) - (f.1 - f.2)) as f64 / 1e4);
        }
    }
    println!("  offset change vs full recording for the same beeps (ms): {diffs:.1?}");

    // GIF of the trimmed piece.
    let gif = dir.join("av.gif");
    let t_gif = Instant::now();
    to_gif(
        &trimmed,
        &gif,
        &GifOptions {
            fps: 15,
            max_width: 640,
            start: Duration::ZERO,
            end: None,
            quality: 80,
            loop_forever: true,
        },
        &|_| {},
    )?;
    let gif_bytes = std::fs::metadata(&gif)?.len();
    let mut opts = gif::DecodeOptions::new();
    opts.set_color_output(gif::ColorOutput::RGBA);
    let mut dec = opts.read_info(std::fs::File::open(&gif)?)?;
    let (gw, gh) = (dec.width(), dec.height());
    let mut frames = 0;
    let mut cs = 0u32;
    let mut canvas = vec![0u8; gw as usize * gh as usize * 4];
    let mut barcode_ok = 0;
    while let Some(f) = dec.read_next_frame()? {
        frames += 1;
        cs += f.delay as u32;
        // composite (frames are cropped, with transparency)
        for yy in 0..f.height as usize {
            for xx in 0..f.width as usize {
                let s = &f.buffer[(yy * f.width as usize + xx) * 4..][..4];
                if s[3] > 0 {
                    let d = ((f.top as usize + yy) * gw as usize + f.left as usize + xx) * 4;
                    canvas[d..d + 4].copy_from_slice(s);
                }
            }
        }
        // barcode blocks must stay pure black/white (dither noise would break it)
        let ok = (0..24).all(|i| {
            let p = &canvas[((24) * gw as usize + 16 + i * 16 + 8) * 4..][..3];
            let l = 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32;
            !(60.0..195.0).contains(&l)
        });
        barcode_ok += ok as u32;
    }
    println!(
        "gif {gw}x{gh}: {} KB, {frames} frames, {:.2} s, took {:?}, frames with a clean barcode {barcode_ok}/{frames}",
        gif_bytes / 1024,
        cs as f64 / 100.0,
        t_gif.elapsed()
    );

    if std::env::var("KEEP").as_deref() != Ok("1") {
        for p in [&out, &trimmed, &gif] {
            let _ = std::fs::remove_file(p);
        }
        println!("deleted the recording, trim and GIF");
    }
    Ok(())
}
