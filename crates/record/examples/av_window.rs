//! A/V sync test window for the recording shakedown (`av_harness`).
//!
//! Shows, in a borderless always-on-top window that never takes focus:
//! - a 24-bit barcode (top-left, 16 px blocks) holding the render time in ms since start,
//! - a large seven-segment frame counter,
//! - a bar moving at 400 px/s,
//! - a white flash for exactly as long as a short, quiet 1 kHz beep plays (every 2 s).
//!
//! The beep is written into its own WASAPI stream at known sample positions and the flash is
//! driven by that stream's audio clock, so picture and sound leave the PC together. On stdout
//! it prints `start <qpc100>` once, then `beep <k> <qpc100>` with the QPC time (100 ns units)
//! at which each beep's first sample played, for the harness to compare against the file.
//!
//! `av_window X Y W H SECONDS` (physical px; exits after SECONDS or when closed)

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, GetDC, ReleaseDC, SetDIBitsToDevice,
};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

const BEEP_HZ: f32 = 1000.0;
/// Quiet: about -30 dBFS.
const BEEP_AMP: f32 = 0.03;
const BEEP_LEN_S: f64 = 0.1;
const PERIOD_S: f64 = 2.0;
const FIRST_S: f64 = 1.0;

fn qpc100() -> u64 {
    let (mut c, mut f) = (0i64, 0i64);
    unsafe {
        let _ = QueryPerformanceCounter(&mut c);
        let _ = QueryPerformanceFrequency(&mut f);
    }
    (c as i128 * 10_000_000 / f as i128) as u64
}

struct Clock {
    /// Stream position (frames) and the QPC (100 ns) it was sampled at.
    pos: AtomicU64,
    qpc: AtomicU64,
    rate: AtomicU64,
    ready: AtomicBool,
}

impl Clock {
    /// Estimated stream position now, in seconds.
    fn now_s(&self) -> Option<f64> {
        if !self.ready.load(Ordering::Acquire) {
            return None;
        }
        let rate = self.rate.load(Ordering::Relaxed) as f64;
        let pos = self.pos.load(Ordering::Relaxed) as f64;
        let at = self.qpc.load(Ordering::Relaxed) as f64;
        Some(pos / rate + (qpc100() as f64 - at) / 1e7)
    }
}

fn beeping(t: f64) -> bool {
    t >= FIRST_S && (t - FIRST_S).rem_euclid(PERIOD_S) < BEEP_LEN_S
}

fn audio_thread(clock: Arc<Clock>, stop: Arc<AtomicBool>) -> windows::core::Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let dev = en.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
        let mix = client.GetMixFormat()?;
        let wf = std::ptr::read_unaligned(mix);
        let (rate, ch, bits) = (wf.nSamplesPerSec, wf.nChannels as usize, wf.wBitsPerSample);
        client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 500_000, 0, mix, None)?;
        let buf_frames = client.GetBufferSize()?;
        let render: IAudioRenderClient = client.GetService()?;
        let aclock: IAudioClock = client.GetService()?;
        let freq = aclock.GetFrequency()?;
        clock.rate.store(rate as u64, Ordering::Relaxed);
        let mut written: u64 = 0;
        let mut started = false;
        let beep_frames = (BEEP_LEN_S * rate as f64) as u64;
        let mut next_report = 0u64;
        while !stop.load(Ordering::Relaxed) {
            let pad = client.GetCurrentPadding()?;
            let n = buf_frames - pad;
            if n > 0 {
                let p = render.GetBuffer(n)?;
                for i in 0..n as u64 {
                    let s = written + i;
                    let t = s as f64 / rate as f64;
                    let v = if beeping(t) {
                        let since = t - FIRST_S - ((t - FIRST_S) / PERIOD_S).floor() * PERIOD_S;
                        BEEP_AMP * (std::f64::consts::TAU * BEEP_HZ as f64 * since).sin() as f32
                    } else {
                        0.0
                    };
                    for c in 0..ch {
                        let idx = i as usize * ch + c;
                        match bits {
                            32 => *(p as *mut f32).add(idx) = v,
                            16 => *(p as *mut i16).add(idx) = (v * 32767.0) as i16,
                            _ => {}
                        }
                    }
                }
                render.ReleaseBuffer(n, 0)?;
                written += n as u64;
            }
            if !started {
                client.Start()?;
                started = true;
            }
            let (mut pos, mut qpc) = (0u64, 0u64);
            aclock.GetPosition(&mut pos, Some(&mut qpc))?;
            let frames = (pos as u128 * rate as u128 / freq as u128) as u64;
            clock.pos.store(frames, Ordering::Relaxed);
            clock.qpc.store(qpc, Ordering::Relaxed);
            clock.ready.store(true, Ordering::Release);
            // Report each beep once its first sample has played.
            let first =
                (FIRST_S * rate as f64) as u64 + next_report * (PERIOD_S * rate as f64) as u64;
            if frames > first + beep_frames {
                let played_at = qpc as i64 - ((frames - first) as i64 * 10_000_000 / rate as i64);
                println!("beep {next_report} {played_at}");
                let _ = std::io::stdout().flush();
                next_report += 1;
            }
            std::thread::sleep(Duration::from_millis(3));
        }
        let _ = client.Stop();
    }
    Ok(())
}

/// Seven-segment digit masks (a b c d e f g).
const SEG: [u8; 10] = [
    0b1111110, 0b0110000, 0b1101101, 0b1111001, 0b0110011, 0b1011011, 0b1011111, 0b1110000,
    0b1111111, 0b1111011,
];

struct Canvas {
    w: usize,
    h: usize,
    px: Vec<u32>,
}

impl Canvas {
    fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, c: u32) {
        let x0 = x.clamp(0, self.w as i32) as usize;
        let x1 = (x + w).clamp(0, self.w as i32) as usize;
        let y0 = y.clamp(0, self.h as i32) as usize;
        let y1 = (y + h).clamp(0, self.h as i32) as usize;
        for yy in y0..y1 {
            self.px[yy * self.w + x0..yy * self.w + x1].fill(c);
        }
    }

    fn digit(&mut self, x: i32, y: i32, d: usize, s: i32, c: u32) {
        let m = SEG[d];
        let (w, t) = (s, s / 5);
        let segs = [
            (x, y, w, t),
            (x + w - t, y, t, w),
            (x + w - t, y + w, t, w),
            (x, y + 2 * w - t, w, t),
            (x, y + w, t, w),
            (x, y, t, w),
            (x, y + w - t / 2, w, t),
        ];
        for (i, r) in segs.iter().enumerate() {
            if m & (1 << (6 - i)) != 0 {
                self.fill(r.0, r.1, r.2, r.3, c);
            }
        }
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_DESTROY {
            PostQuitMessage(0);
            return LRESULT(0);
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

fn main() -> windows::core::Result<()> {
    stinkshot_platform::dpi::init();
    let a: Vec<i64> = std::env::args()
        .skip(1)
        .map(|s| s.parse().unwrap())
        .collect();
    let (x, y, w, h, secs) = (a[0] as i32, a[1] as i32, a[2] as usize, a[3] as usize, a[4]);
    let clock = Arc::new(Clock {
        pos: AtomicU64::new(0),
        qpc: AtomicU64::new(0),
        rate: AtomicU64::new(48_000),
        ready: AtomicBool::new(false),
    });
    let stop = Arc::new(AtomicBool::new(false));
    let audio = {
        let (c, s) = (clock.clone(), stop.clone());
        std::thread::spawn(move || {
            if let Err(e) = audio_thread(c, s) {
                eprintln!("audio: {e}");
            }
        })
    };
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: w!("StinkshotAvSyncTest"),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            w!("StinkshotAvSyncTest"),
            w!("Stinkshot AV Sync Test"),
            WS_POPUP | WS_VISIBLE,
            x,
            y,
            w as i32,
            h as i32,
            None,
            None,
            Some(hinst.into()),
            None,
        )?;
        let mut cv = Canvas {
            w,
            h,
            px: vec![0; w * h],
        };
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let t0 = qpc100();
        println!("start {t0}");
        let _ = std::io::stdout().flush();
        let wall0 = Instant::now();
        let mut frame: u64 = 0;
        let mut msg = MSG::default();
        'run: loop {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    break 'run;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if wall0.elapsed().as_secs() as i64 >= secs {
                break;
            }
            let now = qpc100();
            let ms = ((now - t0) / 10_000) as u32;
            let flash = clock.now_s().is_some_and(beeping);
            let bg = if flash { 0x00FF_FFFF } else { 0x0020_2020 };
            cv.px.fill(bg);
            // barcode: 24 bits of ms, MSB first, on a black strip
            cv.fill(8, 8, 24 * 16 + 16, 32, 0);
            for i in 0..24 {
                let bit = (ms >> (23 - i)) & 1;
                let c = if bit == 1 { 0x00FF_FFFF } else { 0x0000_0000 };
                cv.fill(16 + i * 16, 16, 16, 16, c);
            }
            // frame counter
            let s = format!("{frame:06}");
            for (i, ch) in s.bytes().enumerate() {
                cv.digit(
                    16 + i as i32 * 70,
                    64,
                    (ch - b'0') as usize,
                    50,
                    0x00FF_C040,
                );
            }
            // moving bar
            let bx = ((ms as u64 * 400 / 1000) % (w as u64 - 24)) as i32;
            cv.fill(bx, 190, 24, (h as i32 - 190 - 70).max(10), 0x004C_A3FF);
            let dc = GetDC(Some(hwnd));
            SetDIBitsToDevice(
                dc,
                0,
                0,
                w as u32,
                h as u32,
                0,
                0,
                0,
                h as u32,
                cv.px.as_ptr() as *const _,
                &bmi,
                DIB_RGB_COLORS,
            );
            ReleaseDC(Some(hwnd), dc);
            frame += 1;
            // Not DwmFlush: with the displays idle-off DWM composes at a few Hz and would
            // throttle the content clock.
            std::thread::sleep(Duration::from_millis(4));
        }
        eprintln!(
            "window rendered {frame} frames in {:.1} s",
            wall0.elapsed().as_secs_f64()
        );
        let _ = DestroyWindow(hwnd);
    }
    stop.store(true, Ordering::Relaxed);
    let _ = audio.join();
    Ok(())
}
