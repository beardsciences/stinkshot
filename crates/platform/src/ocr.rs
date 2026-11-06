//! OCR behind a swappable trait. Default engine: Windows.Media.Ocr (offline, uses the
//! user's installed language packs; `TryCreateFromUserProfileLanguages`). Small images are
//! upscaled 2x before recognition (UI text at 100% scale OCRs poorly otherwise); images
//! larger than `OcrEngine.MaxImageDimension` are tiled vertically with overlap and merged.

use stinkshot_core::{PxRect, Rgba};
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine as WinOcrEngine;
use windows::Storage::Streams::DataWriter;

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
    engine: WinOcrEngine,
    /// One OcrEngine instance rejects overlapping RecognizeAsync calls, so serialize them.
    gate: std::sync::Mutex<()>,
}

// SAFETY: Windows.Media.Ocr.OcrEngine is an agile (free-threaded) runtime class; it can be
// used from any thread with an initialized apartment, which `recognize` ensures.
unsafe impl Send for WindowsOcr {}
// SAFETY: see above; the engine holds no thread-affine state.
unsafe impl Sync for WindowsOcr {}

/// Images whose longest side is below this get the 2x upscale.
const UPSCALE_BELOW: u32 = 1600;
/// Windows OCR needs at least this many pixels per side.
const MIN_SIDE: u32 = 48;
/// Vertical overlap between tiles, in recognized-image pixels.
const TILE_OVERLAP: u32 = 96;
/// Preferred tile height; a smaller tile keeps recognition latency predictable.
const TILE_TARGET: u32 = 3000;

impl WindowsOcr {
    /// Errors if no OCR language is installed.
    pub fn new() -> anyhow::Result<WindowsOcr> {
        crate::util::ensure_com();
        if let Ok(engine) = WinOcrEngine::TryCreateFromUserProfileLanguages() {
            return Ok(WindowsOcr {
                engine,
                gate: std::sync::Mutex::new(()),
            });
        }
        // Profile languages have no OCR pack; use any installed recognizer language.
        if let Ok(langs) = WinOcrEngine::AvailableRecognizerLanguages() {
            for lang in langs {
                if let Ok(engine) = WinOcrEngine::TryCreateFromLanguage(&lang) {
                    return Ok(WindowsOcr {
                        engine,
                        gate: std::sync::Mutex::new(()),
                    });
                }
            }
        }
        anyhow::bail!(
            "no OCR language is installed (Settings > Time & language > Language & region > add a language with OCR support)"
        )
    }

    fn recognize_tile(
        &self,
        w: u32,
        h: u32,
        bgra: &[u8],
    ) -> anyhow::Result<Vec<(String, [f32; 4])>> {
        let writer = DataWriter::new()?;
        writer.WriteBytes(bgra)?;
        let buffer = writer.DetachBuffer()?;
        let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
            &buffer,
            BitmapPixelFormat::Bgra8,
            w as i32,
            h as i32,
        )?;
        let _gate = self.gate.lock().unwrap_or_else(|p| p.into_inner());
        let result = self.engine.RecognizeAsync(&bitmap)?.join()?;
        let mut out = Vec::new();
        for line in result.Lines()? {
            let text = line.Text()?.to_string();
            let mut bounds: Option<[f32; 4]> = None; // x0, y0, x1, y1
            for word in line.Words()? {
                let r = word.BoundingRect()?;
                let b = [r.X, r.Y, r.X + r.Width, r.Y + r.Height];
                bounds = Some(match bounds {
                    None => b,
                    Some(o) => [
                        o[0].min(b[0]),
                        o[1].min(b[1]),
                        o[2].max(b[2]),
                        o[3].max(b[3]),
                    ],
                });
            }
            if let Some(b) = bounds {
                out.push((text, b));
            }
        }
        Ok(out)
    }
}

/// Composite onto white and convert to opaque BGRA.
fn to_opaque_bgra(img: &Rgba) -> Vec<u8> {
    let mut out = Vec::with_capacity(img.data.len());
    for p in img.data.chunks_exact(4) {
        let a = p[3] as u32;
        let f = |c: u8| -> u8 { ((c as u32 * a + 255 * (255 - a) + 127) / 255) as u8 };
        out.extend_from_slice(&[f(p[2]), f(p[1]), f(p[0]), 255]);
    }
    out
}

/// Scale (in [1/16, 2]) applied before recognition so the result fits the engine limits.
pub(crate) fn choose_scale(w: u32, h: u32, max_dim: u32) -> f32 {
    let longest = w.max(h);
    let mut s = if longest < UPSCALE_BELOW { 2.0 } else { 1.0 };
    // The width cannot be tiled, so it bounds the scale.
    if (w as f32 * s) > max_dim as f32 {
        s = max_dim as f32 / w as f32;
    }
    s.max(1.0 / 16.0)
}

/// Vertical tile spans `(start, len)` covering `h` pixels with `overlap` between tiles.
pub(crate) fn tile_spans(h: u32, tile: u32, overlap: u32) -> Vec<(u32, u32)> {
    if h <= tile {
        return vec![(0, h)];
    }
    let step = tile.saturating_sub(overlap).max(1);
    let mut spans = Vec::new();
    let mut y = 0;
    loop {
        let len = tile.min(h - y);
        spans.push((y, len));
        if y + len >= h {
            break;
        }
        y += step;
    }
    spans
}

fn resize(img: &Rgba, nw: u32, nh: u32) -> Rgba {
    let buf = image::RgbaImage::from_raw(img.width, img.height, img.data.clone());
    match buf {
        Some(b) => {
            let filter = if nw >= img.width {
                image::imageops::FilterType::CatmullRom
            } else {
                image::imageops::FilterType::Lanczos3
            };
            let r = image::imageops::resize(&b, nw, nh, filter);
            Rgba::from_raw(r.width(), r.height(), r.into_raw())
        }
        None => img.clone(),
    }
}

/// Pad (bottom/right) with the image's background so both sides reach `MIN_SIDE`.
fn pad_min(img: &Rgba) -> (Rgba, u32, u32) {
    let (nw, nh) = (img.width.max(MIN_SIDE), img.height.max(MIN_SIDE));
    if nw == img.width && nh == img.height {
        return (img.clone(), nw, nh);
    }
    let bg = if img.data.len() >= 4 {
        img.pixel(0, 0)
    } else {
        [255, 255, 255, 255]
    };
    let mut out = Rgba::filled(nw, nh, bg);
    out.blit(img, 0, 0);
    (out, nw, nh)
}

impl OcrEngine for WindowsOcr {
    fn name(&self) -> &str {
        "Windows OCR"
    }

    fn recognize(&self, img: &Rgba) -> anyhow::Result<OcrResult> {
        crate::util::ensure_com();
        if img.width == 0 || img.height == 0 {
            return Ok(OcrResult::default());
        }
        let max_dim = WinOcrEngine::MaxImageDimension().unwrap_or(10000).max(256);
        let scale = choose_scale(img.width, img.height, max_dim);
        let nw = ((img.width as f32 * scale).round() as u32).max(1);
        let nh = ((img.height as f32 * scale).round() as u32).max(1);
        let scaled = if nw == img.width && nh == img.height {
            img.clone()
        } else {
            resize(img, nw, nh)
        };
        let tile_h = TILE_TARGET.min(max_dim);
        let spans = tile_spans(scaled.height, tile_h, TILE_OVERLAP);
        let last = spans.len() - 1;
        let mut lines: Vec<OcrLine> = Vec::new();
        for (i, (y0, len)) in spans.iter().copied().enumerate() {
            let tile = if spans.len() == 1 {
                scaled.clone()
            } else {
                scaled.crop(PxRect::new(0, y0 as i32, scaled.width as i32, len as i32))
            };
            let (tile, tw, th) = pad_min(&tile);
            let found = self.recognize_tile(tw, th, &to_opaque_bgra(&tile))?;
            // In overlap zones only the tile whose interior contains the line's centre keeps it.
            let keep_from = if i == 0 {
                f32::MIN
            } else {
                (y0 + TILE_OVERLAP / 2) as f32
            };
            let keep_to = if i == last {
                f32::MAX
            } else {
                (y0 + len - TILE_OVERLAP / 2) as f32
            };
            for (text, b) in found {
                let cy = y0 as f32 + (b[1] + b[3]) / 2.0;
                if cy < keep_from || cy >= keep_to {
                    continue;
                }
                let x0 = (b[0] / scale).floor() as i32;
                let y0f = ((y0 as f32 + b[1]) / scale).floor() as i32;
                let x1 = (b[2] / scale).ceil() as i32;
                let y1 = ((y0 as f32 + b[3]) / scale).ceil() as i32;
                let rect = PxRect::new(x0.max(0), y0f.max(0), (x1 - x0).max(0), (y1 - y0f).max(0))
                    .intersect(&img.bounds())
                    .unwrap_or_default();
                lines.push(OcrLine { text, rect });
            }
        }
        let text = lines
            .iter()
            .map(|l| l.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        Ok(OcrResult { text, lines })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_rules() {
        assert_eq!(choose_scale(400, 300, 10000), 2.0);
        assert_eq!(choose_scale(2000, 1000, 10000), 1.0);
        // wide image whose 2x width would exceed the engine limit stays within it
        let s = choose_scale(1500, 200, 2000);
        assert!(1500.0 * s <= 2000.0 + 0.5);
        assert!(choose_scale(40000, 100, 10000) < 1.0);
    }

    #[test]
    fn tiles_cover_and_overlap() {
        assert_eq!(tile_spans(100, 3000, 96), vec![(0, 100)]);
        let spans = tile_spans(7000, 3000, 96);
        assert_eq!(spans[0], (0, 3000));
        let (ly, ll) = *spans.last().unwrap();
        assert_eq!(ly + ll, 7000);
        for pair in spans.windows(2) {
            let (a, al) = pair[0];
            let (b, _) = pair[1];
            assert_eq!(a + al - b, 96);
        }
    }

    #[test]
    fn padding_reaches_min_side() {
        let img = Rgba::filled(10, 20, [200, 200, 200, 255]);
        let (p, w, h) = pad_min(&img);
        assert_eq!((w, h), (MIN_SIDE, MIN_SIDE));
        assert_eq!(p.pixel(47, 47), [200, 200, 200, 255]);
    }
}
