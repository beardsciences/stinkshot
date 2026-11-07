//! Regression test for the Nov 6 2026 tray-app crash: an access violation inside
//! `OcrEngine::MaxImageDimension` after OCR ran on short-lived worker threads. Each worker
//! joined the MTA and left it on exit; once the last MTA thread was gone, COM tore the
//! apartment down while windows-rs still held cached WinRT factories. Run with:
//! `cargo test -p stinkshot-platform --test ocr_threads -- --ignored`

use stinkshot_core::Rgba;
use stinkshot_platform::ocr::{OcrEngine, WindowsOcr};

#[test]
#[ignore = "needs an installed OCR language"]
fn ocr_survives_sequential_short_lived_threads() {
    // Engine created on one worker that exits, like the app's OCR worker setup.
    let ocr = std::thread::spawn(|| WindowsOcr::new().expect("OCR engine"))
        .join()
        .expect("create thread");
    let ocr = std::sync::Arc::new(ocr);
    let img = Rgba::filled(320, 120, [255, 255, 255, 255]);
    for i in 0..8 {
        let o = ocr.clone();
        let im = img.clone();
        std::thread::spawn(move || {
            o.recognize(&im).expect("recognize");
        })
        .join()
        .unwrap_or_else(|_| panic!("worker {i} panicked"));
        // Give COM time to run its apartment teardown and unload unused DLLs.
        std::thread::sleep(std::time::Duration::from_millis(300));
        // SAFETY: plain COM housekeeping call; makes the dangling-factory case deterministic.
        unsafe { windows_free_unused() };
    }
}

unsafe extern "system" {
    fn CoFreeUnusedLibrariesEx(dwunloaddelay: u32, dwreserved: u32);
}

unsafe fn windows_free_unused() {
    // SAFETY: documented no-op if nothing is unloadable.
    unsafe { CoFreeUnusedLibrariesEx(0, 0) };
}
