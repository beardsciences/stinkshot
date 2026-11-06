//! Optional visual check. Set STINKSHOT_TEST_OUT to a directory to get PNGs of the captures:
//!
//!     $env:STINKSHOT_TEST_OUT = "C:\temp\shots"
//!     cargo test -p stinkshot-platform --test visual -- --ignored --nocapture
#![cfg(windows)]

use stinkshot_core::Rgba;
use stinkshot_platform::{capture, dpi, windows_enum};

fn dump(name: &str, img: &Rgba) {
    let Ok(dir) = std::env::var("STINKSHOT_TEST_OUT") else {
        return;
    };
    let buf = image::RgbaImage::from_raw(img.width, img.height, img.data.clone()).expect("buffer");
    let small = if img.width > 1600 {
        image::imageops::resize(
            &buf,
            1600,
            (img.height as u64 * 1600 / img.width as u64) as u32,
            image::imageops::FilterType::Triangle,
        )
    } else {
        buf
    };
    small
        .save(std::path::Path::new(&dir).join(format!("{name}.png")))
        .expect("save png");
}

#[test]
#[ignore = "needs a desktop session"]
fn dump_captures() {
    dpi::init();
    let f = capture::freeze_all(true).expect("freeze");
    dump("freeze", &f.image);
    let wins = windows_enum::visible_windows();
    if let Some(w) = wins
        .iter()
        .find(|w| w.process != "taskbar" && !w.maximized && w.rect.w > 300)
    {
        println!("window: {} ({})", w.title, w.process);
        let img = capture::capture_window(
            w.hwnd,
            &capture::WindowCaptureOpts {
                include_cursor: true,
            },
        )
        .expect("window");
        dump("window", &img);
    }
}
