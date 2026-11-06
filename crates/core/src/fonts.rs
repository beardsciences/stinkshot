//! Fonts for rasterizing annotation text. Loaded from the Windows font folder at runtime
//! (Segoe UI Variable -> Segoe UI -> Arial), never bundled. Tests may use `from_bytes`.

use std::sync::Arc;

#[derive(Clone)]
pub struct Fonts {
    /// Regular weight for text annotations.
    pub regular: Arc<ab_glyph::FontArc>,
    /// Semibold/bold for step numbers.
    pub bold: Arc<ab_glyph::FontArc>,
    /// Raw bytes of `regular`, so the UI can register the same face with egui.
    pub regular_bytes: Arc<Vec<u8>>,
    pub bold_bytes: Arc<Vec<u8>>,
}

impl Fonts {
    pub fn load_system() -> anyhow::Result<Fonts> {
        todo!("core::fonts::Fonts::load_system")
    }
    pub fn from_bytes(regular: Vec<u8>, bold: Vec<u8>) -> anyhow::Result<Fonts> {
        let _ = (regular, bold);
        todo!("core::fonts::Fonts::from_bytes")
    }
    /// Pixel size of laid-out multi-line text (line height = 1.25 * size).
    pub fn measure(&self, text: &str, size: f32, bold: bool) -> (f32, f32) {
        let _ = (text, size, bold);
        todo!("core::fonts::Fonts::measure")
    }
}
