//! Release smoke test (`scripts/smoke-test.ps1`): records SECONDS of its own swatch window
//! (three 200x200 blocks #7FD36A, #808080, #FFFFFF; topmost, never takes focus, no input) to
//! OUT as MP4, then checks the file: duration, size and the colors of the middle frame.
//! The window runs in a child process (`--swatch X Y SECONDS`). Keeps OUT. Prints `ok ...` and exits 0, or prints the failure and exits 1.
//!
//! `smoke_record OUT.mp4 [SECONDS [X Y]]` (physical px; default 3 s at 64,64 on the primary
//! monitor)

use std::path::PathBuf;
use std::time::{Duration, Instant};

use stinkshot_core::PxRect;
use stinkshot_record::*;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, GetDC, ReleaseDC, SetDIBitsToDevice,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

const W: usize = 600;
const H: usize = 200;
/// 0x00RRGGBB, as a 32-bit DIB stores them.
const COLORS: [u32; 3] = [0x007F_D36A, 0x0080_8080, 0x00FF_FFFF];

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // SAFETY: default handling for a window this process created.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

/// Shows the swatch window for `secs` on this thread. The 2x2 corner block toggles every
/// 250 ms so Desktop Duplication keeps delivering frames.
fn swatch(x: i32, y: i32, secs: f64) -> windows::core::Result<()> {
    // SAFETY: plain Win32 window lifecycle on one thread; `px` and `bmi` outlive every call.
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: w!("StinkshotSmokeSwatch"),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            w!("StinkshotSmokeSwatch"),
            w!("Stinkshot Smoke Test"),
            WS_POPUP,
            x,
            y,
            W as i32,
            H as i32,
            None,
            None,
            Some(hinst.into()),
            None,
        )?;
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: W as i32,
                biHeight: -(H as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut px = vec![0u32; W * H];
        let t0 = Instant::now();
        let mut msg = MSG::default();
        while t0.elapsed().as_secs_f64() < secs {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                DispatchMessageW(&msg);
            }
            for (i, p) in px.iter_mut().enumerate() {
                *p = COLORS[(i % W) / 200];
            }
            if (t0.elapsed().as_millis() / 250) % 2 == 1 {
                for row in 0..2 {
                    px[row * W..row * W + 2].fill(0);
                }
            }
            let dc = GetDC(Some(hwnd));
            SetDIBitsToDevice(
                dc,
                0,
                0,
                W as u32,
                H as u32,
                0,
                0,
                0,
                H as u32,
                px.as_ptr() as *const _,
                &bmi,
                DIB_RGB_COLORS,
            );
            ReleaseDC(Some(hwnd), dc);
            std::thread::sleep(Duration::from_millis(30));
        }
        let _ = DestroyWindow(hwnd);
    }
    Ok(())
}

/// `SMOKE_LOG=1`: recorder log on stderr, with seconds since start.
struct StderrLog(Instant);

impl log::Log for StderrLog {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Debug
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            let t = self.0.elapsed().as_secs_f64();
            eprintln!("[{t:7.3}] {} {}", r.level(), r.args());
        }
    }
    fn flush(&self) {}
}

/// Ends the swatch child when dropped (also on an early error return).
struct Child(std::process::Child);

impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn run() -> anyhow::Result<String> {
    stinkshot_platform::dpi::init();
    if std::env::var_os("SMOKE_LOG").is_some() {
        static LOG: std::sync::OnceLock<StderrLog> = std::sync::OnceLock::new();
        let _ = log::set_logger(LOG.get_or_init(|| StderrLog(Instant::now())));
        log::set_max_level(log::LevelFilter::Debug);
    }
    let mut args = std::env::args().skip(1);
    if std::env::args().nth(1).as_deref() == Some("--swatch") {
        let v: Vec<f64> = args.skip(1).map(|s| s.parse()).collect::<Result<_, _>>()?;
        swatch(v[0] as i32, v[1] as i32, v[2])?;
        return Ok(String::new());
    }
    let out = PathBuf::from(
        args.next()
            .ok_or_else(|| anyhow::anyhow!("usage: smoke_record OUT.mp4 [SECONDS [X Y]]"))?,
    );
    let secs: f64 = args.next().map(|s| s.parse()).transpose()?.unwrap_or(3.0);
    let x: i32 = args.next().map(|s| s.parse()).transpose()?.unwrap_or(64);
    let y: i32 = args.next().map(|s| s.parse()).transpose()?.unwrap_or(64);

    // Window up first; it stays until the recording has stopped. It runs in a child process:
    // the recorder does not see windows of its own process (measured Nov 8: the frames showed
    // what was under the swatch). A fixed lifetime is not enough: `Recorder::start` can take
    // several seconds (3 s WGC timeout on PCs where WGC hangs), and the swatch closed
    // mid-recording (Nov 8: black videos). The lifetime is only a cap if this process dies.
    let mut window = Child(
        std::process::Command::new(std::env::current_exe()?)
            .args([
                "--swatch",
                &x.to_string(),
                &y.to_string(),
                &(secs + 120.0).to_string(),
            ])
            .spawn()?,
    );
    std::thread::sleep(Duration::from_millis(2000));
    let _ = std::fs::remove_file(&out);
    let rec = Recorder::start(RecordOptions {
        region: PxRect::new(x, y, W as i32, H as i32),
        output: out.clone(),
        fps: 30,
        bitrate_kbps: 8000,
        system_audio: false,
        microphone: false,
        mic_device: None,
        webcam: None,
        show_clicks: false,
        show_keys: false,
        include_cursor: false,
        accent: [0x4C, 0xA3, 0xFF, 255],
    })?;
    std::thread::sleep(Duration::from_secs_f64(secs));
    let backend = rec.capture_backend();
    let swatch_gone = window.0.try_wait()?;
    let res = rec.stop()?;
    drop(window);
    if let Some(status) = swatch_gone {
        anyhow::bail!("the swatch window closed during the recording ({status})");
    }

    let info = probe(&res.path)?;
    let got = info.duration.as_secs_f64();
    if (got - secs).abs() > 1.0 {
        anyhow::bail!("duration {got:.2} s, expected about {secs} s");
    }
    if (info.width, info.height) != (W as u32, H as u32) {
        anyhow::bail!("size {}x{}, expected {W}x{H}", info.width, info.height);
    }
    let f = frame_at(&res.path, Duration::from_secs_f64(secs / 2.0), info.height)?;
    let cw = f.width as i32 / 3;
    for (i, want) in COLORS.iter().enumerate() {
        let r = PxRect::new(
            i as i32 * cw + cw / 4,
            f.height as i32 / 4,
            cw / 2,
            f.height as i32 / 2,
        );
        let c = f.crop(r);
        let mut s = [0f64; 3];
        for p in c.data.chunks_exact(4) {
            for k in 0..3 {
                s[k] += p[k] as f64;
            }
        }
        let n = (c.width * c.height).max(1) as f64;
        let avg = s.map(|v| v / n);
        let want = [(want >> 16) & 0xFF, (want >> 8) & 0xFF, want & 0xFF].map(|v| v as f64);
        // H.264 4:2:0 and limited range shift colors a little; a wrong or black frame is far off.
        if avg.iter().zip(want).any(|(a, b)| (a - b).abs() > 24.0) {
            let png = out.with_extension("frame.png");
            let _ = f.save(&png, stinkshot_core::image::ImageFormat::Png, 90);
            anyhow::bail!(
                "block {i} color {avg:?}, expected {want:?} (frame: {})",
                png.display()
            );
        }
    }
    Ok(format!(
        "ok {} {:.2}s {}x{} {:.0}fps backend {backend}",
        out.display(),
        got,
        info.width,
        info.height,
        info.fps
    ))
}

fn main() {
    match run() {
        Ok(line) if line.is_empty() => {}
        Ok(line) => println!("{line}"),
        Err(e) => {
            println!("fail {e:#}");
            std::process::exit(1);
        }
    }
}
