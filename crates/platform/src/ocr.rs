//! OCR behind a swappable trait. Default engine: Windows.Media.Ocr (offline, uses the
//! user's installed language packs; `TryCreateFromUserProfileLanguages`). Small images are
//! upscaled 2x before recognition (UI text at 100% scale OCRs poorly otherwise); images
//! larger than `OcrEngine.MaxImageDimension` are tiled vertically with overlap and merged.

use stinkshot_core::{PxRect, Rgba};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct OcrLine {
    pub text: String,
    /// In the input image's pixel space.
    pub rect: PxRect,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct OcrResult {
    /// Lines joined with '\n', in reading order.
    pub text: String,
    pub lines: Vec<OcrLine>,
}

pub trait OcrEngine: Send + Sync {
    fn name(&self) -> &str;
    fn recognize(&self, img: &Rgba) -> anyhow::Result<OcrResult>;
}

pub struct WindowsOcr {
    #[allow(dead_code)]
    inner: (),
}

impl WindowsOcr {
    /// Errors if no OCR language is installed.
    pub fn new() -> anyhow::Result<WindowsOcr> {
        todo!("platform::ocr::WindowsOcr::new")
    }
}

impl OcrEngine for WindowsOcr {
    fn name(&self) -> &str {
        "Windows OCR"
    }
    fn recognize(&self, img: &Rgba) -> anyhow::Result<OcrResult> {
        let _ = img;
        todo!("platform::ocr::WindowsOcr::recognize")
    }
}
