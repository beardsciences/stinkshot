//! Clipboard. Images are placed as CF_DIBV5 (with alpha) + registered "PNG" format +
//! CF_DIB fallback, so both alpha-aware apps (Office, Discord, browsers) and old apps work.
//! Retries OpenClipboard briefly if another process holds it.

use stinkshot_core::Rgba;

pub fn set_image(img: &Rgba) -> anyhow::Result<()> {
    let _ = img;
    todo!("platform::clipboard::set_image")
}

pub fn set_text(text: &str) -> anyhow::Result<()> {
    let _ = text;
    todo!("platform::clipboard::set_text")
}

/// Also used by "copy file" from history: CF_HDROP with the path, plus the image formats.
pub fn set_file_and_image(path: &std::path::Path, img: Option<&Rgba>) -> anyhow::Result<()> {
    let _ = (path, img);
    todo!("platform::clipboard::set_file_and_image")
}
