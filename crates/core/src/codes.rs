//! QR code and barcode reading, offline (rxing, a pure Rust port of zxing). Reads QR, Micro
//! QR, Data Matrix, Aztec, PDF417 and the common 1D symbologies (EAN / UPC, Code 128 / 39 /
//! 93, ITF, Codabar). Decoded text is only ever shown and copied, never opened.

use std::collections::HashSet;

use rxing::{BarcodeFormat, DecodeHints};

use crate::geom::PxRect;
use crate::image::Rgba;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Code {
    /// Display name of the symbology ("QR Code", "EAN-13").
    pub format: &'static str,
    pub text: String,
    /// Where it was found, image px (empty when the decoder gave no points).
    pub rect: PxRect,
}

impl Code {
    /// The text looks like a web link (http / https), for the "Copy Link" wording. Links are
    /// never opened by Stinkshot.
    pub fn is_url(&self) -> bool {
        let t = self.text.trim();
        let lower = t.get(..8).unwrap_or(t).to_ascii_lowercase();
        (lower.starts_with("http://") || lower.starts_with("https://"))
            && !t.contains(char::is_whitespace)
    }
}

fn format_name(f: &BarcodeFormat) -> &'static str {
    match f {
        BarcodeFormat::QR_CODE => "QR Code",
        BarcodeFormat::MICRO_QR_CODE => "Micro QR Code",
        BarcodeFormat::RECTANGULAR_MICRO_QR_CODE => "rMQR Code",
        BarcodeFormat::DATA_MATRIX => "Data Matrix",
        BarcodeFormat::AZTEC => "Aztec",
        BarcodeFormat::PDF_417 => "PDF417",
        BarcodeFormat::EAN_8 => "EAN-8",
        BarcodeFormat::EAN_13 => "EAN-13",
        BarcodeFormat::UPC_A => "UPC-A",
        BarcodeFormat::UPC_E => "UPC-E",
        BarcodeFormat::CODE_128 => "Code 128",
        BarcodeFormat::CODE_39 => "Code 39",
        BarcodeFormat::CODE_93 => "Code 93",
        BarcodeFormat::ITF => "ITF",
        BarcodeFormat::CODABAR => "Codabar",
        BarcodeFormat::RSS_14 => "GS1 DataBar",
        BarcodeFormat::RSS_EXPANDED => "GS1 DataBar Expanded",
        _ => "Barcode",
    }
}

/// Luminance with alpha composited over white (transparent window corners read as paper).
fn luma(img: &Rgba, scale: u32) -> Vec<u8> {
    let (w, h) = (img.width as usize, img.height as usize);
    let s = scale.max(1) as usize;
    let mut out = vec![0u8; w * s * h * s];
    for y in 0..h {
        for x in 0..w {
            let p = &img.data[(y * w + x) * 4..(y * w + x) * 4 + 4];
            let l = (p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000;
            let a = p[3] as u32;
            let v = ((l * a + 255 * (255 - a)) / 255) as u8;
            for dy in 0..s {
                let row = (y * s + dy) * w * s;
                out[row + x * s..row + x * s + s].fill(v);
            }
        }
    }
    out
}

fn decode_pass(img: &Rgba, scale: u32) -> Vec<Code> {
    let (w, h) = (img.width * scale, img.height * scale);
    let mut hints = DecodeHints {
        TryHarder: Some(true),
        AlsoInverted: Some(true),
        ..Default::default()
    };
    let results = match rxing::helpers::detect_multiple_in_luma_with_hints(
        luma(img, scale),
        w,
        h,
        &mut hints,
    ) {
        Ok(r) => r,
        Err(e) => {
            log::debug!("codes: none found at {scale}x: {e}");
            return Vec::new();
        }
    };
    let mut seen = HashSet::new();
    results
        .into_iter()
        .filter(|r| !r.getText().is_empty())
        .filter(|r| seen.insert((r.getText().to_string(), *r.getBarcodeFormat())))
        .map(|r| {
            let pts = r.getPoints();
            let rect = if pts.is_empty() {
                PxRect::default()
            } else {
                let s = scale as f32;
                let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                for p in pts {
                    x0 = x0.min(p.x / s);
                    y0 = y0.min(p.y / s);
                    x1 = x1.max(p.x / s);
                    y1 = y1.max(p.y / s);
                }
                PxRect::new(
                    x0.floor() as i32,
                    y0.floor() as i32,
                    ((x1 - x0).ceil() as i32).max(1),
                    ((y1 - y0).ceil() as i32).max(1),
                )
            };
            Code {
                format: format_name(r.getBarcodeFormat()),
                text: r.getText().to_string(),
                rect,
            }
        })
        .collect()
}

/// Every code found in `img`, top to bottom. Small images get a second, 2x pass (QR modules
/// of 1-2 px in a 100 % screenshot are below what the detector locks onto).
pub fn decode(img: &Rgba) -> Vec<Code> {
    if img.width == 0 || img.height == 0 {
        return Vec::new();
    }
    let mut found = decode_pass(img, 1);
    if found.is_empty() && img.width.max(img.height) <= 1200 {
        found = decode_pass(img, 2);
    }
    found.sort_by_key(|c| (c.rect.y, c.rect.x));
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use rxing::Writer;

    /// Draw `text` as `format` with `module` px per module on a white canvas of `w` x `h`
    /// at (`x`, `y`).
    fn draw(
        canvas: &mut Rgba,
        text: &str,
        format: BarcodeFormat,
        size: (i32, i32),
        at: (u32, u32),
    ) {
        let m = rxing::MultiFormatWriter
            .encode(text, &format, size.0, size.1)
            .expect("encode");
        for yy in 0..m.height() {
            for xx in 0..m.width() {
                let (px, py) = (at.0 + xx, at.1 + yy);
                if px < canvas.width && py < canvas.height && m.get(xx, yy) {
                    let i = ((py * canvas.width + px) * 4) as usize;
                    canvas.data[i..i + 4].copy_from_slice(&[0, 0, 0, 255]);
                }
            }
        }
    }

    fn white(w: u32, h: u32) -> Rgba {
        Rgba {
            width: w,
            height: h,
            data: vec![255; (w * h * 4) as usize],
        }
    }

    #[test]
    fn reads_a_qr_code_url() {
        let mut img = white(400, 300);
        draw(
            &mut img,
            "https://example.com/a?b=1",
            BarcodeFormat::QR_CODE,
            (200, 200),
            (100, 50),
        );
        let codes = decode(&img);
        assert_eq!(codes.len(), 1, "{codes:?}");
        assert_eq!(codes[0].format, "QR Code");
        assert_eq!(codes[0].text, "https://example.com/a?b=1");
        assert!(codes[0].is_url());
        // The box is roughly where it was drawn.
        let r = codes[0].rect;
        assert!(
            r.x >= 90 && r.right() <= 310 && r.y >= 40 && r.bottom() <= 260,
            "{r:?}"
        );
    }

    #[test]
    fn reads_unicode_text_and_inverted_codes() {
        let mut img = white(300, 300);
        draw(
            &mut img,
            "Wi-Fi \u{65e5}\u{672c}",
            BarcodeFormat::QR_CODE,
            (220, 220),
            (40, 40),
        );
        // White on black (dark UI).
        for p in img.data.chunks_exact_mut(4) {
            p[0] = 255 - p[0];
            p[1] = 255 - p[1];
            p[2] = 255 - p[2];
        }
        let codes = decode(&img);
        assert_eq!(codes.len(), 1, "{codes:?}");
        assert_eq!(codes[0].text, "Wi-Fi \u{65e5}\u{672c}");
        assert!(!codes[0].is_url());
    }

    #[test]
    fn reads_1d_barcodes() {
        let mut img = white(500, 200);
        draw(
            &mut img,
            "4006381333931",
            BarcodeFormat::EAN_13,
            (300, 100),
            (100, 50),
        );
        let codes = decode(&img);
        assert_eq!(codes.len(), 1, "{codes:?}");
        assert_eq!(codes[0].format, "EAN-13");
        assert_eq!(codes[0].text, "4006381333931");

        let mut img = white(500, 200);
        draw(
            &mut img,
            "STINK-42",
            BarcodeFormat::CODE_128,
            (320, 100),
            (90, 50),
        );
        let codes = decode(&img);
        assert_eq!(codes.len(), 1, "{codes:?}");
        assert_eq!(codes[0].format, "Code 128");
        assert_eq!(codes[0].text, "STINK-42");
    }

    #[test]
    fn small_qr_needs_the_upscale_pass() {
        // A dense code at ~1.5 px per module, as in a 100 % scale screenshot.
        let mut img = white(160, 160);
        draw(
            &mut img,
            "https://stinkshot.example/very/long/path/to/make/it/dense",
            BarcodeFormat::QR_CODE,
            (70, 70),
            (40, 40),
        );
        let codes = decode(&img);
        assert_eq!(codes.len(), 1, "{codes:?}");
    }

    #[test]
    fn nothing_in_a_blank_or_empty_image() {
        assert!(decode(&white(200, 100)).is_empty());
        assert!(decode(&white(0, 0)).is_empty());
    }

    /// `cargo test -p stinkshot-core codes_perf -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn codes_perf() {
        for (w, h) in [(800, 600), (1920, 1080), (3840, 2160)] {
            // Busy, text-like noise with one QR code in a corner.
            let mut img = white(w, h);
            for y in (0..h).step_by(18) {
                for x in 0..w {
                    if (x * 7 + y * 13) % 11 < 4 {
                        for dy in 0..9 {
                            img.set_pixel(x, (y + dy).min(h - 1), [40, 40, 40, 255]);
                        }
                    }
                }
            }
            let t = std::time::Instant::now();
            let none = decode(&img);
            let empty = t.elapsed();
            for y in 20..280 {
                for x in 20..280 {
                    img.set_pixel(x, y, [255, 255, 255, 255]);
                }
            }
            draw(
                &mut img,
                "https://example.com",
                BarcodeFormat::QR_CODE,
                (240, 240),
                (30, 30),
            );
            let t = std::time::Instant::now();
            let one = decode(&img);
            println!(
                "{w}x{h}: no code {:.1} ms ({} found), one QR {:.1} ms ({} found)",
                empty.as_secs_f64() * 1000.0,
                none.len(),
                t.elapsed().as_secs_f64() * 1000.0,
                one.len()
            );
        }
    }

    #[test]
    fn url_detection() {
        let c = |t: &str| Code {
            format: "QR Code",
            text: t.into(),
            rect: PxRect::default(),
        };
        assert!(c("HTTPS://EXAMPLE.COM").is_url());
        assert!(c("http://x.y/z").is_url());
        assert!(!c("ftp://x").is_url());
        assert!(!c("https://a b").is_url());
        assert!(!c("WIFI:S:home;;").is_url());
    }
}
