//! Scrolling-capture shakedown: opens its own tall scrollable test window (6000 px of
//! varied text and images, sticky header and footer, a real scrollbar), drives the app's
//! scroll loop (`stinkshot_stitch::drive::drive` with `capture::capture_rect` and the
//! Stitcher, exactly as `workers::scroll_capture`), and compares the stitched image with the
//! window's ground truth. Scrolling is sent as WM_MOUSEWHEEL messages to this window only:
//! no system input, the pointer never moves, other windows are never touched.
//!
//! `cargo run -p stinkshot --example scroll_harness -- [instant|smooth|slow|lazy|web|all] [DELAY_MS]`
//!
//! Modes: `lazy` shows a skeleton placeholder with a spinner for 400-1200 ms (random per
//! 100-row block) wherever rows first come into view; `web` is what a browser does to a
//! long page: smooth scrolling, a 100 px header that collapses to 60 px once the page is
//! scrolled, and lazy blocks. The default delay is the app's default (120 ms), so a pass
//! means no tuning was needed.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use stinkshot_core::{PxRect, Rgba};
use stinkshot_platform::capture;
use stinkshot_stitch::drive::{self, DriveControl, DriveParams};
use stinkshot_stitch::{StitchOptions, Stitcher};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

const WIN_W: i32 = 800;
const WIN_H: i32 = 600;
const HEADER: usize = 60;
/// `web` mode: header height at the top of the page, before it collapses to `HEADER`.
const HEADER_BIG: usize = 100;
const FOOTER: usize = 40;
const DOC_H: usize = 6000;
const NOTCH_PX: i32 = 100;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Instant,
    /// Scrolls animate over 250 ms (ease-out).
    Smooth,
    /// Newly exposed 100-row blocks show a placeholder for 400 ms.
    Slow,
    /// Newly exposed blocks show a skeleton and a spinner for 400-1200 ms.
    Lazy,
    /// Smooth scrolling, collapsing header and lazy blocks.
    Web,
}

impl Mode {
    fn lazy(self) -> bool {
        matches!(self, Mode::Lazy | Mode::Web)
    }
}

/// How long block `b` stays a placeholder.
fn load_time(mode: Mode, b: usize) -> Duration {
    match mode {
        Mode::Slow => Duration::from_millis(400),
        m if m.lazy() => {
            let mut s = (b as u32 + 1).wrapping_mul(0x9E37_79B9) ^ 0x5bd1_e995;
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            Duration::from_millis(400 + (s % 801) as u64)
        }
        _ => Duration::ZERO,
    }
}

/// Skeleton placeholder pixel for document row `dy`, column `x`: three gray bars per
/// 100-row block on the page background, and a spinner (a rotating arc) at the right.
fn skeleton(dy: usize, x: usize, w: usize, now_ms: u128) -> u32 {
    let r = dy % 100;
    let bar = |y0: usize, y1: usize, frac: usize| {
        (y0..y1).contains(&r) && x >= 24 && x < 24 + (w - 80) * frac / 10
    };
    if bar(18, 34, 7) || bar(48, 64, 5) || bar(76, 88, 6) {
        return 0x00E6_E6EA;
    }
    // `NO_SPINNER=1`: a static skeleton, found only by the placeholder check.
    static NO_SPINNER: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *NO_SPINNER.get_or_init(|| std::env::var("NO_SPINNER").as_deref() == Ok("1")) {
        return 0x00FA_FAF8;
    }
    let (cx, cy) = (w as f32 - 90.0, 50.0);
    let (dx, dyf) = (x as f32 - cx, r as f32 - cy);
    let d = (dx * dx + dyf * dyf).sqrt();
    if (8.0..11.0).contains(&d) {
        let a = dyf.atan2(dx);
        let t = (now_ms % 1000) as f32 / 1000.0 * std::f32::consts::TAU;
        let rel = (a - t).rem_euclid(std::f32::consts::TAU);
        if rel < 4.0 {
            return 0x0040_80E0;
        }
    }
    0x00FA_FAF8
}

struct Doc {
    w: usize,
    /// header + document + footer, 0x00RRGGBB
    header: Vec<u32>,
    /// `web` mode: the taller header shown at the top of the page.
    header_big: Vec<u32>,
    body: Vec<u32>,
    footer: Vec<u32>,
}

impl Doc {
    fn truth(&self, mode: Mode) -> Rgba {
        let header = if mode == Mode::Web {
            &self.header_big
        } else {
            &self.header
        };
        let h = header.len() / self.w + DOC_H + FOOTER;
        let mut img = Rgba::new(self.w as u32, h as u32);
        for (i, px) in header
            .iter()
            .chain(&self.body)
            .chain(&self.footer)
            .enumerate()
        {
            img.data[i * 4..i * 4 + 4].copy_from_slice(&[
                (px >> 16) as u8,
                (px >> 8) as u8,
                *px as u8,
                255,
            ]);
        }
        img
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// Draws `rows` x `w` with GDI (text) plus direct pixel work, returns 0x00RRGGBB pixels.
fn render(w: usize, rows: usize, paint: impl Fn(HDC, &mut [u32])) -> Vec<u32> {
    unsafe {
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(rows as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let dc = CreateCompatibleDC(None);
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
        let old = SelectObject(dc, bmp.into());
        let px = std::slice::from_raw_parts_mut(bits as *mut u32, w * rows);
        paint(dc, px);
        let _ = GdiFlush();
        let out = px.iter().map(|p| p & 0x00FF_FFFF).collect();
        SelectObject(dc, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(dc);
        out
    }
}

fn font(h: i32, weight: i32) -> HFONT {
    unsafe {
        CreateFontW(
            h,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            ANTIALIASED_QUALITY,
            0,
            w!("Segoe UI"),
        )
    }
}

fn text(dc: HDC, x: i32, y: i32, s: &str, f: HFONT, rgb: u32) {
    unsafe {
        let old = SelectObject(dc, f.into());
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(
            dc,
            COLORREF(((rgb & 0xFF) << 16) | (rgb & 0xFF00) | ((rgb >> 16) & 0xFF)),
        );
        let _ = TextOutW(dc, x, y, &wide(s));
        SelectObject(dc, old);
    }
}

fn fill(px: &mut [u32], w: usize, x: usize, y: usize, rw: usize, rh: usize, c: u32) {
    for yy in y..y + rh {
        px[yy * w + x..yy * w + x + rw].fill(c);
    }
}

const WORDS: &[&str] = &[
    "stitch",
    "frame",
    "overlap",
    "scroll",
    "header",
    "footer",
    "pixel",
    "region",
    "capture",
    "window",
    "monitor",
    "offset",
    "candidate",
    "verify",
    "tolerance",
    "band",
    "anchor",
    "hash",
];

fn build_doc(w: usize) -> Doc {
    let f_body = font(-17, 400);
    let f_head = font(-28, 700);
    let f_small = font(-13, 400);
    let header = render(w, HEADER, |dc, px| {
        px.fill(0x0020_3040);
        text(
            dc,
            16,
            14,
            "Sticky Header: Stinkshot Scroll Test",
            f_head,
            0x00FF_FFFF,
        );
    });
    let header_big = render(w, HEADER_BIG, |dc, px| {
        px.fill(0x0020_3040);
        text(dc, 16, 14, "Stinkshot Scroll Test", f_head, 0x00FF_FFFF);
        text(
            dc,
            16,
            56,
            "This header collapses once the page scrolls",
            f_small,
            0x00A0_C0E0,
        );
    });
    let footer = render(w, FOOTER, |dc, px| {
        px.fill(0x0040_2030);
        text(
            dc,
            16,
            10,
            "Sticky Footer (always visible)",
            f_small,
            0x00FF_E0A0,
        );
    });
    let body = render(w, DOC_H, |dc, px| {
        px.fill(0x00FA_FAF8);
        let mut y = 12usize;
        let mut n = 0u32;
        let mut seed = 0x1234_5678u32;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        while y < DOC_H - 40 {
            n += 1;
            match n % 7 {
                0 => {
                    // image: gradient with a noise patch
                    let h = (120 + rnd() % 140) as usize;
                    let h = h.min(DOC_H - 20 - y);
                    let iw = w - 80;
                    for yy in 0..h {
                        for xx in 0..iw {
                            let r = (xx * 255 / iw) as u32;
                            let g = (yy * 255 / h.max(1)) as u32;
                            let b = (n * 37) % 255;
                            px[(y + yy) * w + 24 + xx] = (r << 16) | (g << 8) | b;
                        }
                    }
                    for _ in 0..400 {
                        let xx = 24 + (rnd() as usize % iw);
                        let yy = y + (rnd() as usize % h);
                        px[yy * w + xx] = rnd() & 0x00FF_FFFF;
                    }
                    y += h + 16;
                }
                3 => {
                    // section title and a rule
                    text(
                        dc,
                        24,
                        y as i32,
                        &format!("Section {n}"),
                        f_head,
                        0x0010_1010,
                    );
                    y += 40;
                    fill(px, w, 24, y, w - 80, 2, 0x00C0_C0C0);
                    y += 12;
                }
                5 => {
                    // blank space: a weak band for matching
                    y += 90;
                }
                _ => {
                    let lines = 2 + rnd() % 5;
                    for _ in 0..lines {
                        let mut s = format!("{:>4}  ", y);
                        let words = 6 + rnd() % 8;
                        for _ in 0..words {
                            s.push_str(WORDS[rnd() as usize % WORDS.len()]);
                            s.push(' ');
                        }
                        text(dc, 24, y as i32, &s, f_body, 0x0030_3030);
                        y += 24;
                        if y >= DOC_H - 40 {
                            break;
                        }
                    }
                    y += 10;
                }
            }
        }
        text(
            dc,
            24,
            (DOC_H - 34) as i32,
            "End of document",
            f_body,
            0x00A0_0000,
        );
    });
    unsafe {
        let _ = DeleteObject(f_body.into());
        let _ = DeleteObject(f_head.into());
        let _ = DeleteObject(f_small.into());
    }
    Doc {
        w,
        header,
        header_big,
        body,
        footer,
    }
}

struct View {
    mode: Mode,
    target: f32,
    pos: f32,
    anim_from: f32,
    anim_start: Option<Instant>,
    /// When each 100-row block was first visible.
    seen: Vec<Option<Instant>>,
}

static WHEEL: AtomicI32 = AtomicI32::new(0);

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_MOUSEWHEEL => {
                let delta = ((wp.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
                WHEEL.fetch_add(delta, Ordering::SeqCst);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

struct Shared {
    hwnd: AtomicIsize,
    client_w: AtomicI32,
    doc: Mutex<Option<Arc<Doc>>>,
    quit: AtomicBool,
}

fn window_thread(mode: Mode, x: i32, y: i32, shared: Arc<Shared>) {
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: w!("StinkshotScrollTest"),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap(),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            w!("StinkshotScrollTest"),
            w!("Stinkshot Scroll Test"),
            WS_POPUP | WS_VSCROLL | WS_VISIBLE,
            x,
            y,
            WIN_W,
            WIN_H,
            None,
            None,
            Some(hinst.into()),
            None,
        )
        .unwrap();
        let mut cr = RECT::default();
        GetClientRect(hwnd, &mut cr).unwrap();
        let (cw, ch) = (cr.right as usize, cr.bottom as usize);
        assert_eq!(ch, WIN_H as usize);
        let doc = Arc::new(build_doc(cw));
        let vh_min = ch - HEADER - FOOTER;
        let max_pos = (DOC_H - vh_min) as f32;
        let mut view = View {
            mode,
            target: 0.0,
            pos: 0.0,
            anim_from: 0.0,
            anim_start: None,
            seen: vec![None; DOC_H / 100 + 1],
        };
        let mut frame = vec![0u32; cw * ch];
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: cw as i32,
                biHeight: -(ch as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        *shared.doc.lock().unwrap() = Some(doc.clone());
        shared.client_w.store(cw as i32, Ordering::SeqCst);
        shared.hwnd.store(hwnd.0 as isize, Ordering::SeqCst);
        let mut msg = MSG::default();
        let mut last_drawn = (-1.0f32, 0usize);
        let t_start = Instant::now();
        while !shared.quit.load(Ordering::Relaxed) {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                DispatchMessageW(&msg);
            }
            let now = Instant::now();
            let wheel = WHEEL.swap(0, Ordering::SeqCst);
            if wheel != 0 {
                view.target =
                    (view.target - wheel as f32 / 120.0 * NOTCH_PX as f32).clamp(0.0, max_pos);
                if matches!(view.mode, Mode::Smooth | Mode::Web) {
                    view.anim_from = view.pos;
                    view.anim_start = Some(now);
                } else {
                    view.pos = view.target;
                }
            }
            if let Some(t0) = view.anim_start {
                let t = (now.duration_since(t0).as_secs_f32() / 0.25).min(1.0);
                let e = 1.0 - (1.0 - t).powi(3);
                view.pos = view.anim_from + (view.target - view.anim_from) * e;
                if t >= 1.0 {
                    view.anim_start = None;
                }
            }
            let top = view.pos.round() as usize;
            let (hh, header) = if view.mode == Mode::Web && top == 0 {
                (HEADER_BIG, &doc.header_big)
            } else {
                (HEADER, &doc.header)
            };
            let vh = ch - hh - FOOTER;
            // lazy loading: which blocks are visible, which are loaded
            let mut loaded_mask = 0usize;
            let mut animating = false;
            for b in top / 100..=((top + vh - 1) / 100).min(view.seen.len() - 1) {
                let seen = *view.seen[b].get_or_insert(now);
                if now.duration_since(seen) >= load_time(view.mode, b) {
                    loaded_mask = loaded_mask.wrapping_mul(31).wrapping_add(b + 1);
                } else {
                    animating |= view.mode.lazy();
                }
            }
            if last_drawn != (view.pos, loaded_mask) || animating {
                last_drawn = (view.pos, loaded_mask);
                let now_ms = t_start.elapsed().as_millis();
                frame[..hh * cw].copy_from_slice(header);
                for r in 0..vh {
                    let dy = top + r;
                    let row = &mut frame[(hh + r) * cw..(hh + r + 1) * cw];
                    let loaded = view.seen[dy / 100]
                        .is_some_and(|s| now.duration_since(s) >= load_time(view.mode, dy / 100));
                    if loaded {
                        row.copy_from_slice(&doc.body[dy * cw..(dy + 1) * cw]);
                    } else if view.mode.lazy() {
                        for (x, px) in row.iter_mut().enumerate() {
                            *px = skeleton(dy, x, cw, now_ms);
                        }
                    } else {
                        row.fill(if (dy / 8).is_multiple_of(2) {
                            0x00E4_E4E4
                        } else {
                            0x00DC_DCDC
                        });
                    }
                }
                frame[(ch - FOOTER) * cw..].copy_from_slice(&doc.footer);
                let dc = GetDC(Some(hwnd));
                SetDIBitsToDevice(
                    dc,
                    0,
                    0,
                    cw as u32,
                    ch as u32,
                    0,
                    0,
                    0,
                    ch as u32,
                    frame.as_ptr() as *const _,
                    &bmi,
                    DIB_RGB_COLORS,
                );
                ReleaseDC(Some(hwnd), dc);
                let si = SCROLLINFO {
                    cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
                    fMask: SIF_ALL,
                    nMin: 0,
                    nMax: DOC_H as i32 - 1,
                    nPage: vh_min as u32,
                    nPos: top as i32,
                    nTrackPos: 0,
                };
                windows::Win32::UI::Controls::SetScrollInfo(hwnd, SB_VERT, &si, true);
            }
            std::thread::sleep(Duration::from_millis(4));
        }
        let _ = DestroyWindow(hwnd);
    }
}

struct Report {
    ok: bool,
    line: String,
}

fn compare(got: &Rgba, truth: &Rgba, cw: u32) -> Report {
    let rows = got.height.min(truth.height);
    let row_diff = |a: &Rgba, ya: u32, b: &Rgba, yb: u32| -> u32 {
        let mut m = 0u32;
        for x in 0..cw {
            let pa = &a.data[((ya * a.width + x) * 4) as usize..][..3];
            let pb = &b.data[((yb * b.width + x) * 4) as usize..][..3];
            for c in 0..3 {
                m = m.max((pa[c] as i32 - pb[c] as i32).unsigned_abs());
            }
        }
        m
    };
    let mut exact = 0;
    let mut bad = Vec::new();
    let mut max = 0;
    for y in 0..rows {
        let d = row_diff(got, y, truth, y);
        max = max.max(d);
        if d <= 2 {
            exact += 1;
        } else {
            bad.push(y);
        }
    }
    // for the first bad row, find where it would match (shift)
    let shift = bad.first().map(|&y| {
        (-200i32..=200)
            .filter(|s| {
                let t = y as i32 + s;
                t >= 0 && (t as u32) < truth.height && row_diff(got, y, truth, t as u32) <= 2
            })
            .min_by_key(|s| s.abs())
    });
    let ok = got.height == truth.height && bad.is_empty();
    Report {
        ok,
        line: format!(
            "stitched {}x{}, truth {}x{}; rows within 2 levels {exact}/{rows}, mismatched {} (first {:?}, matches truth at shift {:?}), max diff {max}",
            got.width,
            got.height,
            truth.width,
            truth.height,
            bad.len(),
            bad.first(),
            shift.flatten()
        ),
    }
}

fn run(mode: Mode, delay_ms: u64) -> anyhow::Result<bool> {
    let mons = stinkshot_platform::monitors::list();
    let mon = mons
        .iter()
        .find(|m| m.rect.x != 0 || m.rect.y != 0)
        .unwrap_or(&mons[0]);
    let (x, y) = (mon.work.x + 80, mon.work.y + 80);
    let shared = Arc::new(Shared {
        hwnd: AtomicIsize::new(0),
        client_w: AtomicI32::new(0),
        doc: Mutex::new(None),
        quit: AtomicBool::new(false),
    });
    let th = {
        let s = shared.clone();
        std::thread::spawn(move || window_thread(mode, x, y, s))
    };
    let t = Instant::now();
    while shared.hwnd.load(Ordering::SeqCst) == 0 && t.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let hwnd = HWND(shared.hwnd.load(Ordering::SeqCst) as *mut _);
    std::thread::sleep(Duration::from_millis(500));
    let mut r = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut r)? };
    anyhow::ensure!(
        unsafe { IsWindowVisible(hwnd) }.as_bool(),
        "test window not visible"
    );
    let rect = PxRect::new(r.left, r.top, r.right - r.left, r.bottom - r.top);
    let cw = shared.client_w.load(Ordering::SeqCst) as u32;
    let doc = shared.doc.lock().unwrap().clone().unwrap();
    let truth = doc.truth(mode);

    let mut stitcher = Stitcher::new(StitchOptions {
        max_height: 30000,
        ..StitchOptions::default()
    });
    let center = (rect.x + rect.w / 2, rect.y + rect.h / 2);
    let t0 = Instant::now();
    let mut capture_ms = Vec::new();
    let out = drive::drive(
        &mut stitcher,
        &DriveParams::new(
            rect.h as f32 / mon.scale.max(0.1),
            Duration::from_millis(delay_ms),
        ),
        || {
            let t = Instant::now();
            let r = capture::capture_rect(rect, false).map_err(|e| format!("{e:#}"));
            capture_ms.push(t.elapsed().as_secs_f64() * 1e3);
            if std::env::var("DUMP").as_deref() == Ok("1")
                && let Ok(im) = &r
            {
                let p = std::env::temp_dir()
                    .join(format!("scroll-{mode:?}-frame{:03}.png", capture_ms.len()));
                let _ =
                    image::save_buffer(&p, &im.data, im.width, im.height, image::ColorType::Rgba8);
            }
            r
        },
        |notches| {
            // WM_MOUSEWHEEL to our own window only (lParam = pointer position, unused).
            let wp = WPARAM(((notches * 120) as u16 as usize) << 16);
            let lp = LPARAM((((center.1 & 0xFFFF) << 16) | (center.0 & 0xFFFF)) as isize);
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_MOUSEWHEEL, wp, lp);
            }
        },
        || DriveControl::Run,
        |st, _| {
            if std::env::var("STITCH_LOG").as_deref() == Ok("1") {
                eprintln!("  pushed: {} frames, height {}", st.frames(), st.height());
            }
        },
    );
    let took = t0.elapsed();
    let frames = stitcher.frames();
    let repairs = stitcher.repairs();
    let img = stitcher.finish();
    let rep = compare(&img, &truth, cw);
    capture_ms.sort_by(f64::total_cmp);
    let mut settle = out.settle_ms.clone();
    settle.sort_unstable();
    println!(
        "{mode:?}, delay {delay_ms} ms: {} steps, {frames} frames appended, failed {}, error {:?}, scrolls {:?}, took {:.1} s, capture_rect median {:.1} ms max {:.1} ms",
        out.steps,
        out.failed,
        out.error,
        out.scrolls.iter().map(|n| -n).collect::<Vec<_>>(),
        took.as_secs_f64(),
        capture_ms[capture_ms.len() / 2],
        capture_ms[capture_ms.len() - 1]
    );
    println!(
        "  settle median {} ms max {} ms; {} captures; held for placeholders {}; timeouts {}; retried {}; stitch repairs {repairs}",
        settle.get(settle.len() / 2).copied().unwrap_or(0),
        settle.last().copied().unwrap_or(0),
        out.captures,
        out.placeholder_waits,
        out.settle_timeouts,
        out.retried,
    );
    println!("  {}", rep.line);
    if std::env::var("KEEP").as_deref() == Ok("1") {
        let dir = std::env::temp_dir();
        let save = |name: &str, im: &Rgba| {
            let p = dir.join(name);
            image::save_buffer(&p, &im.data, im.width, im.height, image::ColorType::Rgba8).unwrap();
            println!("  saved {}", p.display());
        };
        save(&format!("scroll-{mode:?}-stitched.png"), &img);
        save(&format!("scroll-{mode:?}-truth.png"), &truth);
    }
    shared.quit.store(true, Ordering::Relaxed);
    let _ = th.join();
    Ok(rep.ok)
}

/// `STITCH_LOG=1`: print the stitcher's and driver's debug lines.
struct StderrLog(Instant);

impl log::Log for StderrLog {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.target().starts_with("stinkshot_stitch")
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            eprintln!("[{:8.3}] {}", self.0.elapsed().as_secs_f64(), r.args());
        }
    }
    fn flush(&self) {}
}

fn main() -> anyhow::Result<()> {
    stinkshot_platform::dpi::init();
    if std::env::var("STITCH_LOG").as_deref() == Ok("1") {
        let l: &'static StderrLog = Box::leak(Box::new(StderrLog(Instant::now())));
        if log::set_logger(l).is_ok() {
            log::set_max_level(log::LevelFilter::Debug);
        }
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let which = args.first().map_or("all", |s| s.as_str());
    let delay: u64 = args.get(1).map_or(120, |s| s.parse().unwrap());
    let modes: Vec<Mode> = match which {
        "instant" => vec![Mode::Instant],
        "smooth" => vec![Mode::Smooth],
        "slow" => vec![Mode::Slow],
        "lazy" => vec![Mode::Lazy],
        "web" => vec![Mode::Web],
        _ => vec![
            Mode::Instant,
            Mode::Smooth,
            Mode::Slow,
            Mode::Lazy,
            Mode::Web,
        ],
    };
    let _ = PCWSTR::null();
    let mut all = true;
    for m in modes {
        all &= run(m, delay)?;
        capture::release_cached_capture();
    }
    println!("{}", if all { "ALL MATCH" } else { "MISMATCH" });
    Ok(())
}
