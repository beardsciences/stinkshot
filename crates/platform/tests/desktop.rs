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
fn ocr_reads_rendered_text() {
    let ocr = WindowsOcr::new().expect("OCR engine (install an OCR language pack)");
    assert_eq!(ocr.name(), "Windows OCR");
    for height in [18, 40] {
        let img = render_text("StinkShot captures screenshots quickly", height);
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
    let s = "StinkShot clipboard test \u{00e9}\u{4e2d} 123";
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
    let title = "StinkShot platform test window";
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
