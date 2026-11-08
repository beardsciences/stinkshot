//! Desktop integration tests. They need an interactive Windows session, so they are
//! `#[ignore]`d by default:
//!
//!     cargo test -p stinkshot-platform -- --ignored --test-threads=1
//!
//! Some tests touch real shared state (clipboard contents, the single-instance mutex).
#![cfg(windows)]

use std::time::{Duration, Instant};

use stinkshot_core::{PxPoint, PxRect, Rgba};
use stinkshot_platform::ocr::{OcrEngine, WindowsOcr};
use stinkshot_platform::{
    capture, clipboard, dpi, input, monitors, system, uia, window_util, windows_enum,
};
use windows::Win32::Foundation::{COLORREF, HWND};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS,
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DEFAULT_CHARSET, DEFAULT_PITCH,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, FF_DONTCARE, FW_NORMAL, GetDC, HGDIOBJ,
    OUT_DEFAULT_PRECIS, ReleaseDC, SelectObject, SetBkColor, SetTextColor, TextOutW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};
use windows::core::{PCWSTR, w};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn non_black_fraction(img: &Rgba) -> f64 {
    let n = img
        .data
        .chunks_exact(4)
        .filter(|p| p[0] != 0 || p[1] != 0 || p[2] != 0)
        .count();
    n as f64 / (img.width as f64 * img.height as f64)
}

#[test]
#[ignore = "needs a desktop session"]
fn monitors_list() {
    dpi::init();
    let mons = monitors::list();
    assert!(!mons.is_empty(), "no monitors");
    assert_eq!(
        mons.iter().filter(|m| m.primary).count(),
        1,
        "exactly one primary"
    );
    let vb = monitors::virtual_bounds();
    for m in &mons {
        println!(
            "{} rect={:?} work={:?} scale={} primary={}",
            m.name, m.rect, m.work, m.scale, m.primary
        );
        assert!(m.rect.w > 0 && m.rect.h > 0);
        assert!(m.scale >= 1.0 - f32::EPSILON);
        assert_eq!(
            vb.union(&m.rect),
            vb,
            "virtual bounds contain every monitor"
        );
        assert_eq!(
            monitors::at_point(PxPoint {
                x: m.rect.x + 1,
                y: m.rect.y + 1
            })
            .map(|x| x.handle),
            Some(m.handle)
        );
    }
}

#[test]
#[ignore = "needs a desktop session"]
fn freeze_all_is_non_black_and_correct_size() {
    dpi::init();
    let vb = monitors::virtual_bounds();
    for round in 0..3 {
        let t = Instant::now();
        let f = capture::freeze_all(true).expect("freeze_all");
        println!(
            "freeze_all #{round}: {:?} ({}x{})",
            t.elapsed(),
            f.image.width,
            f.image.height
        );
        assert_eq!(f.bounds, vb);
        assert_eq!((f.image.width as i32, f.image.height as i32), (vb.w, vb.h));
        assert_eq!(f.image.data.len(), (vb.w * vb.h * 4) as usize);
        assert!(non_black_fraction(&f.image) > 0.05, "capture looks black");
        assert!(
            f.image
                .data
                .chunks_exact(4)
                .step_by(997)
                .all(|p| p[3] == 255)
        );
    }
}

#[test]
#[ignore = "needs a desktop session"]
fn capture_rect_dimensions_and_content() {
    dpi::init();
    let m = monitors::list()
        .into_iter()
        .find(|m| m.primary)
        .expect("primary");
    let r = PxRect::new(m.rect.x + 10, m.rect.y + 10, 400, 300);
    for _ in 0..3 {
        let t = Instant::now();
        let img = capture::capture_rect(r, false).expect("capture_rect");
        println!("capture_rect: {:?}", t.elapsed());
        assert_eq!((img.width, img.height), (400, 300));
        println!("non-black fraction {}", non_black_fraction(&img));
        assert!(non_black_fraction(&img) > 0.02);
    }
    // Rect spanning beyond the desktop edge: outside pixels are opaque black, no error.
    let vb = monitors::virtual_bounds();
    let edge = PxRect::new(vb.right() - 50, vb.y, 100, 40);
    let img = capture::capture_rect(edge, false).expect("edge capture");
    assert_eq!((img.width, img.height), (100, 40));
    assert_eq!(img.pixel(99, 0)[3], 255);
}

#[test]
#[ignore = "needs a desktop session"]
fn window_enumeration() {
    dpi::init();
    let ws = windows_enum::visible_windows();
    for w in ws.iter().take(8) {
        println!("{:?}", w);
    }
    assert!(!ws.is_empty());
    assert!(ws.iter().all(|w| !w.title.is_empty() && w.rect.w > 0));
    let fg = windows_enum::foreground();
    println!("foreground: {fg:?}");
}

#[test]
#[ignore = "needs a desktop session"]
fn capture_a_window() {
    dpi::init();
    let ws = windows_enum::visible_windows();
    let mut ok = 0;
    for w in ws
        .iter()
        .filter(|w| w.process != "taskbar" && w.rect.w > 200 && w.rect.h > 150)
        .take(6)
    {
        let t = Instant::now();
        match capture::capture_window(
            w.hwnd,
            &capture::WindowCaptureOpts {
                include_cursor: false,
            },
        ) {
            Ok(img) => {
                println!(
                    "captured '{}' ({}) as {}x{} (frame {}x{}) in {:?}",
                    w.title,
                    w.process,
                    img.width,
                    img.height,
                    w.rect.w,
                    w.rect.h,
                    t.elapsed()
                );
                assert!(img.width > 0 && img.height > 0);
                assert!(
                    (img.width as i32 - w.rect.w).abs() <= 2
                        && (img.height as i32 - w.rect.h).abs() <= 2,
                    "size should match the DWM frame"
                );
                assert!(img.data.chunks_exact(4).any(|p| p[3] != 0));
                ok += 1;
            }
            Err(e) => println!("window '{}' not capturable: {e}", w.title),
        }
    }
    assert!(ok > 0, "no window could be captured with WGC");
}

#[test]
#[ignore = "needs a desktop session"]
fn uia_snaps_to_elements() {
    dpi::init();
    let ws = windows_enum::visible_windows();
    let w = ws
        .iter()
        .find(|w| w.process != "taskbar" && w.rect.w > 300 && w.rect.h > 200)
        .expect("a normal window");
    let p = PxPoint {
        x: w.rect.x + w.rect.w / 2,
        y: w.rect.y + w.rect.h / 2,
    };
    let snapper = uia::ElementSnapper::new().expect("snapper");
    snapper.request(w.hwnd, p);
    let deadline = Instant::now() + Duration::from_secs(10);
    let res = loop {
        if let Some(r) = snapper.latest() {
            break r;
        }
        assert!(Instant::now() < deadline, "no UIA result within 10 s");
        std::thread::sleep(Duration::from_millis(20));
    };
    println!("window '{}' rects at centre: {:?}", w.title, res.rects);
    assert_eq!(res.point, p);
    assert!(!res.rects.is_empty());
    let last = res.rects.last().copied().expect("rect");
    assert_eq!(last, w.rect, "outermost rect is the window frame");
    assert!(
        res.rects
            .iter()
            .all(|r| r.w >= 4 && r.h >= 4 && w.rect.intersect(r) == Some(*r))
    );
    // Second request on the same window is served from the cache and must be quick.
    let p2 = PxPoint {
        x: p.x + 7,
        y: p.y + 5,
    };
    let t = Instant::now();
    snapper.request(w.hwnd, p2);
    while snapper.latest().map(|r| r.point) != Some(p2) {
        assert!(t.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(5));
    }
    println!("cached resolve in {:?}", t.elapsed());
}

/// Black text on white rendered with GDI, as an `Rgba`.
fn render_text(text: &str, height: i32) -> Rgba {
    let (w, h) = (900, height * 2);
    // SAFETY: standard GDI drawing into a DIB section; everything is released before returning.
    unsafe {
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(Some(screen));
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bmp =
            CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0).expect("dib");
        let old = SelectObject(dc, HGDIOBJ(bmp.0));
        let font = CreateFontW(
            -height,
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            w!("Segoe UI"),
        );
        let oldf = SelectObject(dc, HGDIOBJ(font.0));
        std::slice::from_raw_parts_mut(bits as *mut u8, (w * h * 4) as usize).fill(255);
        SetBkColor(dc, COLORREF(0x00FF_FFFF));
        SetTextColor(dc, COLORREF(0));
        let t: Vec<u16> = text.encode_utf16().collect();
        let _ = TextOutW(dc, 20, height / 2, &t);
        let src = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
        let mut img = Rgba::new(w as u32, h as u32);
        for (d, s) in img.data.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            d.copy_from_slice(&[s[2], s[1], s[0], 255]);
        }
        SelectObject(dc, oldf);
        let _ = DeleteObject(HGDIOBJ(font.0));
        SelectObject(dc, old);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen);
        img
    }
}

#[test]
#[ignore = "needs a desktop session and an installed OCR language"]
fn smart_redact_finds_rendered_email_and_phone() {
    use stinkshot_core::sensitive::{SensitiveKind, TextWord, find_in_lines};
    let ocr = WindowsOcr::new().expect("OCR engine (install an OCR language pack)");
    let img = render_text("Mail jane.doe@example.com or call 555-123-4567", 28);
    let r = ocr.recognize(&img).expect("recognize");
    println!("smart redact OCR: {:?}", r.text);
    let lines: Vec<Vec<TextWord>> = r
        .lines
        .iter()
        .map(|l| {
            assert!(!l.words.is_empty(), "words reported");
            for w in &l.words {
                assert!(
                    l.rect.intersect(&w.rect) == Some(w.rect),
                    "word inside line"
                );
            }
            l.words
                .iter()
                .map(|w| TextWord {
                    text: w.text.clone(),
                    rect: w.rect,
                })
                .collect()
        })
        .collect();
    let found = find_in_lines(&lines);
    println!("findings: {found:?}");
    let kinds: Vec<SensitiveKind> = found.iter().map(|f| f.kind).collect();
    assert!(kinds.contains(&SensitiveKind::Email), "{kinds:?}");
    assert!(kinds.contains(&SensitiveKind::Phone), "{kinds:?}");
    // The email box covers the address, not the whole line.
    let email = found
        .iter()
        .find(|f| f.kind == SensitiveKind::Email)
        .unwrap();
    assert!(email.rect.w < r.lines[0].rect.w / 2, "{email:?}");
}

#[test]
#[ignore = "needs a desktop session and an installed OCR language"]
fn ocr_reads_rendered_text() {
    let ocr = WindowsOcr::new().expect("OCR engine (install an OCR language pack)");
    assert_eq!(ocr.name(), "Windows OCR");
    for height in [18, 40] {
        let img = render_text("Stinkshot captures screenshots quickly", height);
        let t = Instant::now();
        let r = ocr.recognize(&img).expect("recognize");
        println!("OCR h={height} in {:?}: {:?}", t.elapsed(), r.text);
        let lower = r.text.to_lowercase();
        assert!(
            lower.contains("screenshots") && lower.contains("captures"),
            "got {:?}",
            r.text
        );
        assert!(!r.lines.is_empty());
        for l in &r.lines {
            assert!(
                img.bounds().intersect(&l.rect) == Some(l.rect),
                "line rect inside image"
            );
            assert!(l.rect.w > 20 && l.rect.h > 5);
        }
    }
    // Usable from other threads concurrently.
    let ocr = std::sync::Arc::new(ocr);
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let o = ocr.clone();
            std::thread::spawn(move || {
                o.recognize(&render_text("thread safe text", 30))
                    .map(|r| r.text)
            })
        })
        .collect();
    for h in handles {
        let text = h.join().expect("join").expect("recognize on worker");
        assert!(text.to_lowercase().contains("thread"), "got {text:?}");
    }
    // Blank images produce an empty result rather than an error.
    let blank = Rgba::filled(300, 100, [255, 255, 255, 255]);
    assert!(ocr.recognize(&blank).expect("blank").text.trim().is_empty());
    // A tall image is tiled and merged; both ends must be found.
    let mut tall = Rgba::filled(900, 7000, [255, 255, 255, 255]);
    tall.blit(&render_text("alpha topmost line", 30), 0, 0);
    tall.blit(&render_text("omega bottommost line", 30), 0, 6900 - 60);
    let r = ocr.recognize(&tall).expect("tall");
    println!("tall OCR: {:?}", r.text);
    let lower = r.text.to_lowercase();
    assert!(
        lower.contains("topmost") && lower.contains("bottommost"),
        "got {:?}",
        r.text
    );
    let top = r
        .lines
        .iter()
        .find(|l| l.text.to_lowercase().contains("topmost"))
        .expect("top line");
    let bot = r
        .lines
        .iter()
        .find(|l| l.text.to_lowercase().contains("bottommost"))
        .expect("bottom line");
    assert!(
        top.rect.y < 100 && bot.rect.y > 6700,
        "rects mapped back to image space"
    );
}

#[test]
#[ignore = "touches the real clipboard"]
fn clipboard_roundtrips() {
    let s = "Stinkshot clipboard test \u{00e9}\u{4e2d} 123";
    clipboard::set_text(s).expect("set_text");
    assert_eq!(clipboard::get_text().as_deref(), Some(s));

    let mut img = Rgba::filled(64, 32, [200, 10, 20, 255]);
    img.set_pixel(3, 4, [1, 2, 3, 128]);
    clipboard::set_image(&img).expect("set_image");
    assert!(clipboard::has_dibv5());
    let back = clipboard::get_image().expect("PNG format present");
    assert_eq!(back, img, "PNG clipboard format preserves pixels and alpha");

    let path = std::env::temp_dir().join("stinkshot_clip_test.txt");
    std::fs::write(&path, b"x").expect("write");
    clipboard::set_file_and_image(&path, Some(&img)).expect("set_file_and_image");
    assert!(clipboard::has_dibv5());
    clipboard::set_file_and_image(&path, None).expect("file only");
    let _ = std::fs::remove_file(path);
}

#[test]
#[ignore = "needs a desktop session"]
fn input_basics() {
    dpi::init();
    let p = input::cursor_pos();
    let vb = monitors::virtual_bounds();
    assert!(vb.contains(p), "cursor {p:?} inside {vb:?}");
    let (tx, _rx) = std::sync::mpsc::channel();
    let hook = input::InputHook::install(tx).expect("install hooks");
    std::thread::sleep(Duration::from_millis(100));
    drop(hook); // must uninstall and join without hanging
}

#[test]
#[ignore = "injects a harmless F24 key press"]
fn input_hook_sees_injected_key() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
        VIRTUAL_KEY,
    };
    dpi::init();
    let (tx, rx) = std::sync::mpsc::channel();
    let hook = input::InputHook::install(tx).expect("install hooks");
    let key = |flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0x87),
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    // SAFETY: the INPUT array outlives the call.
    unsafe {
        SendInput(
            &[key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)],
            std::mem::size_of::<INPUT>() as i32,
        );
    }
    let mut seen_down = false;
    let mut seen_up = false;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && !(seen_down && seen_up) {
        if let Ok((_, ev)) = rx.recv_timeout(Duration::from_millis(100)) {
            match ev {
                input::InputEvent::KeyDown { label, vk: 0x87 } => {
                    assert_eq!(label, "F24");
                    seen_down = true;
                }
                input::InputEvent::KeyUp { vk: 0x87 } => seen_up = true,
                _ => {}
            }
        }
    }
    drop(hook);
    assert!(
        seen_down && seen_up,
        "hook did not deliver the injected key events"
    );
}

#[test]
#[ignore = "needs a desktop session"]
fn draw_cursor_marks_the_image() {
    dpi::init();
    let p = input::cursor_pos();
    let origin = PxPoint {
        x: p.x - 32,
        y: p.y - 32,
    };
    let mut img = Rgba::filled(64, 64, [128, 128, 128, 255]);
    let before = img.clone();
    capture::draw_cursor(&mut img, origin);
    assert_ne!(
        img, before,
        "cursor should be drawn near the middle of the image"
    );
    assert!(img.data.chunks_exact(4).all(|p| p[3] == 255));
}

#[test]
#[ignore = "needs a desktop session"]
fn scroll_restores_cursor() {
    dpi::init();
    let before = input::cursor_pos();
    let m = monitors::list()
        .into_iter()
        .find(|m| m.primary)
        .expect("primary");
    input::scroll_at(
        PxPoint {
            x: m.rect.x + 5,
            y: m.rect.y + m.rect.h / 2,
        },
        -1,
    );
    input::scroll_at(
        PxPoint {
            x: m.rect.x + 5,
            y: m.rect.y + m.rect.h / 2,
        },
        1,
    );
    let after = input::cursor_pos();
    assert!(
        (after.x - before.x).abs() <= 1 && (after.y - before.y).abs() <= 1,
        "{before:?} vs {after:?}"
    );
}

#[test]
#[ignore = "needs a desktop session"]
fn system_single_instance_and_ipc() {
    let g = system::acquire_single_instance().expect("first instance");
    assert!(
        system::acquire_single_instance().is_none(),
        "second acquire must fail while held"
    );
    assert!(
        !system::send_to_running_instance(&["x".into()]),
        "no listener yet"
    );
    let (tx, rx) = std::sync::mpsc::channel();
    system::start_ipc_listener(tx).expect("listener");
    let args = vec![
        "region".to_string(),
        "--delay".to_string(),
        "\u{00e9} 3".to_string(),
    ];
    assert!(system::send_to_running_instance(&args));
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(3)).expect("payload"),
        args
    );
    assert!(system::send_to_running_instance(&[]));
    assert!(
        rx.recv_timeout(Duration::from_secs(3))
            .expect("empty payload")
            .is_empty()
    );
    drop(g);
    assert!(
        system::acquire_single_instance().is_some(),
        "mutex free again after drop"
    );
}

#[test]
#[ignore = "needs a desktop session"]
fn system_theme_and_registry_reads() {
    let dark = system::system_dark_mode();
    let accent = system::system_accent();
    println!("dark={dark} accent={accent:?}");
    assert!(accent.is_some());
    let _ = system::autostart_enabled();
}

#[test]
#[ignore = "needs a desktop session"]
fn printscreen_takeover_restores() {
    let first = system::take_over_printscreen(true).expect("take");
    println!("take-over changed value: {first}");
    // Taking over twice is idempotent.
    assert!(!system::take_over_printscreen(true).expect("take again"));
    let restored = system::take_over_printscreen(false).expect("restore");
    assert_eq!(
        first, restored,
        "restore undoes exactly what take-over changed"
    );
    assert!(!system::take_over_printscreen(false).expect("restore again"));
}

#[test]
#[ignore = "needs a desktop session"]
fn window_util_on_own_window() {
    dpi::init();
    let title = "Stinkshot platform test window";
    let t = wide(title);
    // SAFETY: plain window creation/destruction on this thread with a built-in class.
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            PCWSTR(t.as_ptr()),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            100,
            100,
            320,
            240,
            None,
            None,
            None,
            None,
        )
        .expect("create window")
    };
    let h = hwnd.0 as isize;
    assert_eq!(window_util::find_own_window(title), Some(h));
    assert_eq!(window_util::find_own_window("no such window title"), None);
    window_util::set_rect(h, PxRect::new(200, 150, 480, 360));
    let info = windows_enum::info(h).expect("info");
    // The DWM frame excludes the invisible resize border, so it is slightly smaller.
    assert!(
        (460..=480).contains(&info.rect.w) && (350..=360).contains(&info.rect.h),
        "{:?}",
        info.rect
    );
    window_util::set_topmost(h, true);
    window_util::set_topmost(h, false);
    window_util::exclude_from_capture(h, true);
    window_util::exclude_from_capture(h, false);
    window_util::apply_mica(h, true);
    window_util::set_tool_window(h, true);
    window_util::set_tool_window(h, false);
    window_util::force_foreground(h);
    // SAFETY: destroying the window created above.
    unsafe { DestroyWindow(HWND(hwnd.0)).expect("destroy") };
}

/// Install the Replace Snipping Tool hook and inject Win+Shift+S into this test's own
/// foreground window. Guarded so it cannot disturb a user at the PC: it waits until the
/// keyboard and mouse have been idle for 2 s with no key held, injects only while its own
/// window is the foreground window and under the cursor, and releases the keys as soon as the
/// chord is routed. If the hook failed, the Snipping Tool would open, so the assertions say so.
#[test]
#[ignore = "injects Win+Shift+S into its own foreground window"]
fn snip_hook_swallows_and_routes_win_shift_s() {
    use std::sync::atomic::{AtomicU32, Ordering};
    use stinkshot_platform::snip::{SnipChord, SnipHook};
    use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, GetLastInputInfo, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS,
        KEYBDINPUT, KEYEVENTF_KEYUP, LASTINPUTINFO, SendInput, VIRTUAL_KEY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        DefWindowProcW, DispatchMessageW, GA_ROOT, GetAncestor, GetCursorPos, GetForegroundWindow,
        MSG, PM_REMOVE, PeekMessageW, RegisterClassW, TranslateMessage, WM_KEYDOWN, WM_KEYUP,
        WNDCLASSW, WS_EX_TOPMOST, WindowFromPoint,
    };

    static S_DOWN: AtomicU32 = AtomicU32::new(0);
    static S_UP: AtomicU32 = AtomicU32::new(0);
    static WIN_UP: AtomicU32 = AtomicU32::new(0);
    unsafe extern "system" fn proc_(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match (m, wp.0) {
            (WM_KEYDOWN, 0x53) => {
                S_DOWN.fetch_add(1, Ordering::SeqCst);
            }
            (WM_KEYUP, 0x53) => {
                S_UP.fetch_add(1, Ordering::SeqCst);
            }
            (WM_KEYUP, 0x5B) => {
                WIN_UP.fetch_add(1, Ordering::SeqCst);
            }
            _ => {}
        }
        // SAFETY: default processing.
        unsafe { DefWindowProcW(h, m, wp, lp) }
    }
    fn pump() {
        let mut msg = MSG::default();
        // SAFETY: standard message pumping on this thread.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
    /// Tick of the last user input (changes on any keyboard or mouse input).
    fn last_input() -> u32 {
        let mut li = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        // SAFETY: out struct with cbSize set.
        let _ = unsafe { GetLastInputInfo(&mut li) };
        li.dwTime
    }
    fn any_key_held() -> bool {
        // SAFETY: trivial queries. Skips the mouse buttons and the toggle-only keys.
        (0x08..0xFF).any(|vk| unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000 != 0)
    }
    fn key(vk: u16, up: bool) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    wScan: 0,
                    dwFlags: if up {
                        KEYEVENTF_KEYUP
                    } else {
                        KEYBD_EVENT_FLAGS(0)
                    },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    dpi::init();
    // 1. Wait (up to 30 s) for the user to be idle with nothing held.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut seen = last_input();
    let mut quiet_since = Instant::now();
    loop {
        let now = last_input();
        if now != seen || any_key_held() {
            seen = now;
            quiet_since = Instant::now();
        } else if quiet_since.elapsed() >= Duration::from_secs(2) {
            break;
        }
        if Instant::now() > deadline {
            println!("SKIPPED: the keyboard or mouse was in use; nothing was injected");
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    // 2. Own topmost window under the cursor, made foreground.
    // SAFETY: class registration and window creation on this thread.
    let hwnd = unsafe {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc_),
            lpszClassName: w!("StinkShotSnipHookTest"),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let mut cur = POINT::default();
        let _ = GetCursorPos(&mut cur);
        CreateWindowExW(
            WS_EX_TOPMOST,
            w!("StinkShotSnipHookTest"),
            w!("Stinkshot Snip Hook Test"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            cur.x - 160,
            cur.y - 120,
            320,
            240,
            None,
            None,
            None,
            None,
        )
        .expect("create window")
    };
    window_util::force_foreground(hwnd.0 as isize);
    pump();
    std::thread::sleep(Duration::from_millis(150));
    pump();
    let ours = || {
        // SAFETY: plain window queries.
        unsafe {
            let mut cur = POINT::default();
            let _ = GetCursorPos(&mut cur);
            let under = GetAncestor(WindowFromPoint(cur), GA_ROOT);
            GetForegroundWindow() == hwnd && under == hwnd
        }
    };
    let destroy = || {
        // SAFETY: destroying the window created above.
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    };
    if !ours() || last_input() != seen || any_key_held() {
        destroy();
        println!(
            "SKIPPED: the test window was not foreground under the cursor; nothing was injected"
        );
        return;
    }

    // 3. Hook, inject the chord downs, wait for the route, release at once.
    let (tx, rx) = std::sync::mpsc::channel();
    let hook = SnipHook::install(tx).expect("install snip hook");
    let send = |inputs: &[INPUT]| {
        // SAFETY: the INPUT array outlives the call.
        unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) }
    };
    let downs = [key(0x5B, false), key(0xA0, false), key(0x53, false)];
    let ups = [key(0x53, true), key(0xA0, true), key(0x5B, true)];
    let t0 = Instant::now();
    let sent = send(&downs);
    let mut routed = Vec::new();
    while t0.elapsed() < Duration::from_millis(1500) && routed.is_empty() {
        pump();
        if let Ok(c) = rx.recv_timeout(Duration::from_millis(5)) {
            routed.push(c);
        }
    }
    let latency = t0.elapsed();
    let sent_up = send(&ups);
    let t1 = Instant::now();
    while t1.elapsed() < Duration::from_millis(400) {
        pump();
        while let Ok(c) = rx.try_recv() {
            routed.push(c);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let still_fg = ours();
    drop(hook);
    destroy();
    pump();
    // SAFETY: trivial queries.
    let stuck: Vec<i32> = [0x5B, 0x10, 0x53]
        .into_iter()
        .filter(|vk| unsafe { GetAsyncKeyState(*vk) } as u16 & 0x8000 != 0)
        .collect();
    println!(
        "sent {sent}+{sent_up} events, routed {routed:?} after {latency:?}, S down/up seen by window {}/{}, Win up seen {}, window still foreground {still_fg}, stuck keys {stuck:x?}",
        S_DOWN.load(Ordering::SeqCst),
        S_UP.load(Ordering::SeqCst),
        WIN_UP.load(Ordering::SeqCst),
    );
    assert_eq!(sent, 3);
    assert_eq!(sent_up, 3);
    assert_eq!(routed, vec![SnipChord::Region], "chord routed exactly once");
    assert_eq!(S_DOWN.load(Ordering::SeqCst), 0, "S down was swallowed");
    assert_eq!(S_UP.load(Ordering::SeqCst), 0, "S up was swallowed");
    assert_eq!(WIN_UP.load(Ordering::SeqCst), 1, "Win up passed through");
    assert!(stuck.is_empty(), "no key left down");
    assert!(
        still_fg,
        "nothing (Start, Snipping Tool) took the foreground"
    );
}

// ---------------------------------------------------------------------------------------
// Capture freshness (stale-frame regression, Nov 7 2026)
// ---------------------------------------------------------------------------------------

const MARK: [u8; 3] = [19, 199, 90];

unsafe extern "system" fn marker_wndproc(
    hwnd: HWND,
    msg: u32,
    wp: windows::Win32::Foundation::WPARAM,
    lp: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    // SAFETY: default processing.
    unsafe { windows::Win32::UI::WindowsAndMessaging::DefWindowProcW(hwnd, msg, wp, lp) }
}

/// Solid, non-activating topmost marker windows (this process's own) at `rects`, shown on a
/// private thread with a message loop until the returned sender is signalled or dropped.
fn open_markers(rects: Vec<PxRect>) -> std::sync::mpsc::Sender<()> {
    use windows::Win32::Graphics::Gdi::{CreateSolidBrush, HBRUSH};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, RegisterClassW, SW_SHOWNOACTIVATE,
        ShowWindow, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
    };
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _aware = dpi::ThreadPerMonitorAware::new();
        // SAFETY: window class registration, creation and a message loop on this thread; the
        // windows are destroyed before the thread ends.
        unsafe {
            let hinst = GetModuleHandleW(None).expect("module");
            let brush = CreateSolidBrush(COLORREF(
                MARK[0] as u32 | (MARK[1] as u32) << 8 | (MARK[2] as u32) << 16,
            ));
            let wc = WNDCLASSW {
                lpfnWndProc: Some(marker_wndproc),
                hInstance: hinst.into(),
                lpszClassName: w!("StinkshotTestFreshnessMarker"),
                hbrBackground: HBRUSH(brush.0),
                ..Default::default()
            };
            RegisterClassW(&wc);
            let mut hwnds = Vec::new();
            for r in &rects {
                let h = CreateWindowExW(
                    WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    w!("StinkshotTestFreshnessMarker"),
                    w!("freshness marker"),
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
                .expect("marker window");
                let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
                hwnds.push(h);
            }
            let _ = ready_tx.send(());
            let mut msg = MSG::default();
            loop {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&msg);
                }
                match rx.try_recv() {
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    _ => break,
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            for h in hwnds {
                let _ = DestroyWindow(h);
            }
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

/// Whether the live screen shows the marker at the centre of `r` (GDI, the reference).
fn screen_shows_mark(r: PxRect) -> bool {
    use windows::Win32::Graphics::Gdi::GetPixel;
    let _aware = dpi::ThreadPerMonitorAware::new();
    // SAFETY: screen DC acquired and released here.
    unsafe {
        let dc = GetDC(None);
        let c = GetPixel(dc, r.x + r.w / 2, r.y + r.h / 2).0;
        ReleaseDC(None, dc);
        is_mark([c as u8, (c >> 8) as u8, (c >> 16) as u8, 255])
    }
}

/// A warm capture session must never hand back pixels older than the request: a window
/// opened after the warm-up (with nothing else changing on screen) has to show up in
/// `freeze_all`, `freeze_rect` and `capture_rect`, and be gone again after it closes.
/// `first_frame_timeout_ms` 0 makes the warm-up miss each output's first duplication frame
/// (a busy start-up), the case where the warm-up picture could stick.
fn assert_fresh_after_warm_up(first_frame_timeout_ms: u32) {
    dpi::init();
    println!(
        "display power: {:?}",
        stinkshot_platform::display_power::state()
    );
    let rects: Vec<PxRect> = monitors::list()
        .iter()
        .map(|m| PxRect::new(m.rect.x + 240, m.rect.y + 240, 160, 160))
        .collect();
    for round in 0..2 {
        capture::release_cached_capture();
        capture::warm_up_with(first_frame_timeout_ms);
        std::thread::sleep(Duration::from_millis(800));
        let close = open_markers(rects.clone());
        std::thread::sleep(Duration::from_millis(1200));
        let visible: Vec<PxRect> = rects
            .iter()
            .copied()
            .filter(|r| screen_shows_mark(*r))
            .collect();
        if visible.is_empty() {
            let _ = close.send(());
            panic!("no marker is visible on screen (covered by another topmost window?)");
        }
        let f = capture::freeze_all(false).expect("freeze_all");
        for r in &visible {
            let (x, y) = (r.x + r.w / 2 - f.bounds.x, r.y + r.h / 2 - f.bounds.y);
            assert!(
                is_mark(f.image.pixel(x as u32, y as u32)),
                "round {round}: freeze_all misses the marker at {r:?} (stale picture)"
            );
            let one = capture::freeze_rect(*r, false).expect("freeze_rect");
            assert!(
                is_mark(one.pixel(r.w as u32 / 2, r.h as u32 / 2)),
                "round {round}: freeze_rect misses the marker at {r:?}"
            );
            let live = capture::capture_rect(*r, false).expect("capture_rect");
            assert!(
                is_mark(live.pixel(r.w as u32 / 2, r.h as u32 / 2)),
                "round {round}: capture_rect misses the marker at {r:?}"
            );
        }
        let _ = close.send(());
        std::thread::sleep(Duration::from_millis(1200));
        let f = capture::freeze_all(false).expect("freeze_all after close");
        for r in visible.iter().filter(|r| !screen_shows_mark(**r)) {
            let (x, y) = (r.x + r.w / 2 - f.bounds.x, r.y + r.h / 2 - f.bounds.y);
            assert!(
                !is_mark(f.image.pixel(x as u32, y as u32)),
                "round {round}: freeze_all still shows the closed marker at {r:?}"
            );
        }
    }
    capture::release_cached_capture();
}

#[test]
#[ignore = "needs a desktop session"]
fn capture_is_fresh_after_warm_up() {
    assert_fresh_after_warm_up(300);
}

#[test]
#[ignore = "needs a desktop session"]
fn capture_is_fresh_when_the_first_frame_comes_late() {
    assert_fresh_after_warm_up(0);
}
