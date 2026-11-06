//! Fonts for rasterizing annotation text. Loaded from the Windows font folder at runtime
//! (Segoe UI Variable -> Segoe UI -> Arial), never bundled. Tests may use `from_bytes`.

use std::path::PathBuf;
use std::sync::Arc;

use ab_glyph::{Font, FontArc, OutlinedGlyph, PxScale, ScaleFont, point};
use anyhow::Context;

/// Line height as a multiple of the font size.
pub const LINE_HEIGHT: f32 = 1.25;

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
    /// Load from `%WINDIR%\Fonts`. Regular: Segoe UI Variable, then Segoe UI, then Arial.
    /// Bold: Segoe UI Bold, then Arial Bold, then the regular face.
    pub fn load_system() -> anyhow::Result<Fonts> {
        let dir = windows_fonts_dir();
        let regular = first_loadable(&dir, &["SegUIVar.ttf", "segoeui.ttf", "arial.ttf"])
            .with_context(|| format!("no usable regular font in {}", dir.display()))?;
        let bold = first_loadable(&dir, &["segoeuib.ttf", "arialbd.ttf"])
            .unwrap_or_else(|| regular.clone());
        Self::from_bytes(regular, bold)
    }

    pub fn from_bytes(regular: Vec<u8>, bold: Vec<u8>) -> anyhow::Result<Fonts> {
        let r = FontArc::try_from_vec(regular.clone()).context("invalid regular font data")?;
        let b = FontArc::try_from_vec(bold.clone()).context("invalid bold font data")?;
        Ok(Fonts {
            regular: Arc::new(r),
            bold: Arc::new(b),
            regular_bytes: Arc::new(regular),
            bold_bytes: Arc::new(bold),
        })
    }

    fn face(&self, bold: bool) -> &FontArc {
        if bold { &self.bold } else { &self.regular }
    }

    /// Pixel size of laid-out multi-line text (line height = 1.25 * size).
    pub fn measure(&self, text: &str, size: f32, bold: bool) -> (f32, f32) {
        let mut w = 0f32;
        let mut lines = 0usize;
        for line in text.split('\n') {
            lines += 1;
            w = w.max(self.line_width(line, size, bold));
        }
        (w, lines as f32 * LINE_HEIGHT * size)
    }

    /// Advance width of a single line (no newlines), including kerning.
    pub fn line_width(&self, line: &str, size: f32, bold: bool) -> f32 {
        let font = self.face(bold);
        let scaled = font.as_scaled(PxScale::from(size.max(1.0)));
        let mut x = 0f32;
        let mut prev = None;
        for c in line.chars() {
            if c == '\r' {
                continue;
            }
            let c = if c == '\t' { ' ' } else { c };
            if c.is_control() {
                continue;
            }
            let id = scaled.glyph_id(c);
            if let Some(p) = prev {
                x += scaled.kern(p, id);
            }
            x += scaled.h_advance(id);
            prev = Some(id);
        }
        x + line.matches('\t').count() as f32 * 3.0 * scaled.h_advance(scaled.glyph_id(' '))
    }

    /// Ascent in px at `size` (distance from the top of the em box to the baseline).
    pub fn ascent(&self, size: f32, bold: bool) -> f32 {
        self.face(bold)
            .as_scaled(PxScale::from(size.max(1.0)))
            .ascent()
    }

    /// Outlined glyphs for one line with its pen starting at (`x`, `baseline`).
    pub fn outline_line(
        &self,
        line: &str,
        size: f32,
        bold: bool,
        x: f32,
        baseline: f32,
    ) -> Vec<OutlinedGlyph> {
        let font = self.face(bold);
        let scale = PxScale::from(size.max(1.0));
        let scaled = font.as_scaled(scale);
        let mut pen = x;
        let mut prev = None;
        let mut out = Vec::with_capacity(line.len());
        for c in line.chars() {
            if c == '\r' {
                continue;
            }
            if c == '\t' {
                pen += 4.0 * scaled.h_advance(scaled.glyph_id(' '));
                prev = None;
                continue;
            }
            if c.is_control() {
                continue;
            }
            let id = scaled.glyph_id(c);
            if let Some(p) = prev {
                pen += scaled.kern(p, id);
            }
            let glyph = id.with_scale_and_position(scale, point(pen, baseline));
            pen += scaled.h_advance(id);
            prev = Some(id);
            if let Some(o) = font.outline_glyph(glyph) {
                out.push(o);
            }
        }
        out
    }
}

fn windows_fonts_dir() -> PathBuf {
    let win = std::env::var_os("WINDIR")
        .or_else(|| std::env::var_os("SystemRoot"))
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    win.join("Fonts")
}

fn first_loadable(dir: &std::path::Path, names: &[&str]) -> Option<Vec<u8>> {
    for name in names {
        let path = dir.join(name);
        match std::fs::read(&path) {
            Ok(bytes) => {
                if FontArc::try_from_vec(bytes.clone()).is_ok() {
                    return Some(bytes);
                }
                log::warn!("font {} could not be parsed", path.display());
            }
            Err(e) => log::debug!("font {} not available: {e}", path.display()),
        }
    }
    None
}

#[cfg(test)]
pub(crate) fn test_fonts() -> Fonts {
    Fonts::load_system().expect("system fonts (Windows) are required for these tests")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_system_fonts() {
        let f = test_fonts();
        assert!(!f.regular_bytes.is_empty() && !f.bold_bytes.is_empty());
    }

    #[test]
    fn measure_scales_with_size_and_lines() {
        let f = test_fonts();
        let (w1, h1) = f.measure("Hello", 20.0, false);
        let (w2, h2) = f.measure("Hello", 40.0, false);
        assert!(w1 > 20.0 && w2 > w1 * 1.8 && w2 < w1 * 2.2, "{w1} {w2}");
        assert!((h1 - 25.0).abs() < 1e-3 && (h2 - 50.0).abs() < 1e-3);
        let (w3, h3) = f.measure("Hello\nHi there, wider", 20.0, false);
        assert!(w3 > w1);
        assert!((h3 - 50.0).abs() < 1e-3);
        let (we, he) = f.measure("", 20.0, false);
        assert_eq!(we, 0.0);
        assert!((he - 25.0).abs() < 1e-3);
    }

    #[test]
    fn bold_is_not_narrower_for_text() {
        let f = test_fonts();
        let (r, _) = f.measure("Number 42", 24.0, false);
        let (b, _) = f.measure("Number 42", 24.0, true);
        assert!(b >= r * 0.95);
    }

    #[test]
    fn outline_matches_measure() {
        let f = test_fonts();
        let glyphs = f.outline_line("Hi", 30.0, false, 10.0, 40.0);
        assert_eq!(glyphs.len(), 2);
        let right = glyphs
            .iter()
            .map(|g| g.px_bounds().max.x)
            .fold(0.0, f32::max);
        assert!(right <= 10.0 + f.line_width("Hi", 30.0, false) + 1.0);
        assert!(glyphs[0].px_bounds().min.x >= 9.0);
    }

    #[test]
    fn tabs_and_carriage_returns_do_not_panic() {
        let f = test_fonts();
        let (w, _) = f.measure("a\tb\r", 20.0, false);
        let (w0, _) = f.measure("ab", 20.0, false);
        assert!(w > w0);
    }

    #[test]
    fn rejects_garbage_font_bytes() {
        assert!(Fonts::from_bytes(vec![1, 2, 3], vec![4, 5, 6]).is_err());
    }
}
