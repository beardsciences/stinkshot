//! Owned RGBA8 bitmap plus encode/decode helpers.

use std::path::Path;

use anyhow::Context;

use crate::geom::PxRect;

/// Straight-alpha RGBA8, row-major, tightly packed.
#[derive(Clone, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl std::fmt::Debug for Rgba {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Rgba({}x{})", self.width, self.height)
    }
}

/// Output encodings offered to the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    #[default]
    Png,
    Jpeg,
    Webp,
}

impl ImageFormat {
    pub fn extension(&self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Webp => "webp",
        }
    }
}

impl Rgba {
    /// Fully transparent image.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            data: vec![0; (width as usize) * (height as usize) * 4],
        }
    }
    pub fn filled(width: u32, height: u32, rgba: [u8; 4]) -> Self {
        let mut img = Self::new(width, height);
        for px in img.data.chunks_exact_mut(4) {
            px.copy_from_slice(&rgba);
        }
        img
    }
    pub fn from_raw(width: u32, height: u32, data: Vec<u8>) -> Self {
        assert_eq!(
            data.len(),
            width as usize * height as usize * 4,
            "Rgba::from_raw size mismatch"
        );
        Self {
            width,
            height,
            data,
        }
    }
    pub fn bounds(&self) -> PxRect {
        PxRect::new(0, 0, self.width as i32, self.height as i32)
    }
    #[inline]
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ]
    }
    #[inline]
    pub fn set_pixel(&mut self, x: u32, y: u32, v: [u8; 4]) {
        let i = ((y * self.width + x) * 4) as usize;
        self.data[i..i + 4].copy_from_slice(&v);
    }
    /// Copy of `rect` clipped to the image. Returns an empty image if no overlap.
    pub fn crop(&self, rect: PxRect) -> Rgba {
        let Some(r) = rect.intersect(&self.bounds()) else {
            return Rgba::new(0, 0);
        };
        let mut out = Rgba::new(r.w as u32, r.h as u32);
        let row = r.w as usize * 4;
        for y in 0..r.h as usize {
            let src = ((r.y as usize + y) * self.width as usize + r.x as usize) * 4;
            out.data[y * row..(y + 1) * row].copy_from_slice(&self.data[src..src + row]);
        }
        out
    }
    /// Copy `src` onto self at (x, y) without blending (overwrites), clipped.
    pub fn blit(&mut self, src: &Rgba, x: i32, y: i32) {
        let dst_r = PxRect::new(x, y, src.width as i32, src.height as i32);
        let Some(r) = dst_r.intersect(&self.bounds()) else {
            return;
        };
        let row = r.w as usize * 4;
        for yy in 0..r.h {
            let sy = (r.y - y + yy) as usize;
            let sx = (r.x - x) as usize;
            let s = (sy * src.width as usize + sx) * 4;
            let d = ((r.y + yy) as usize * self.width as usize + r.x as usize) * 4;
            self.data[d..d + row].copy_from_slice(&src.data[s..s + row]);
        }
    }
    pub fn encode(&self, format: ImageFormat, jpeg_quality: u8) -> anyhow::Result<Vec<u8>> {
        use ::image::ExtendedColorType;
        use ::image::ImageEncoder;
        if self.width == 0 || self.height == 0 {
            anyhow::bail!("cannot encode an empty image");
        }
        anyhow::ensure!(
            self.data.len() == self.width as usize * self.height as usize * 4,
            "image buffer size does not match its dimensions"
        );
        let mut buf: Vec<u8> = Vec::with_capacity(self.data.len() / 3);
        match format {
            ImageFormat::Png => {
                let enc = ::image::codecs::png::PngEncoder::new_with_quality(
                    &mut buf,
                    ::image::codecs::png::CompressionType::Default,
                    ::image::codecs::png::FilterType::Adaptive,
                );
                enc.write_image(
                    &self.data,
                    self.width,
                    self.height,
                    ExtendedColorType::Rgba8,
                )
                .context("PNG encode failed")?;
            }
            ImageFormat::Jpeg => {
                // JPEG has no alpha: composite over white.
                let mut rgb = Vec::with_capacity(self.width as usize * self.height as usize * 3);
                for px in self.data.chunks_exact(4) {
                    let a = px[3] as u32;
                    for &c in &px[..3] {
                        rgb.push(((c as u32 * a + 255 * (255 - a) + 127) / 255) as u8);
                    }
                }
                let q = jpeg_quality.clamp(1, 100);
                let enc = ::image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, q);
                enc.write_image(&rgb, self.width, self.height, ExtendedColorType::Rgb8)
                    .context("JPEG encode failed")?;
            }
            ImageFormat::Webp => {
                // The pure-Rust encoder is lossless only, which suits screenshots.
                let enc = ::image::codecs::webp::WebPEncoder::new_lossless(&mut buf);
                enc.write_image(
                    &self.data,
                    self.width,
                    self.height,
                    ExtendedColorType::Rgba8,
                )
                .context("WebP encode failed")?;
            }
        }
        Ok(buf)
    }
    pub fn to_png(&self) -> anyhow::Result<Vec<u8>> {
        self.encode(ImageFormat::Png, 90)
    }
    /// Encode and write atomically (temp file + rename).
    pub fn save(&self, path: &Path, format: ImageFormat, jpeg_quality: u8) -> anyhow::Result<()> {
        let bytes = self.encode(format, jpeg_quality)?;
        write_atomic(path, &bytes)
    }
    pub fn load(path: &Path) -> anyhow::Result<Rgba> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Self::decode(&bytes).with_context(|| format!("decoding {}", path.display()))
    }
    pub fn decode(bytes: &[u8]) -> anyhow::Result<Rgba> {
        let img = ::image::load_from_memory(bytes)
            .context("image decode failed")?
            .to_rgba8();
        let (w, h) = img.dimensions();
        Ok(Rgba {
            width: w,
            height: h,
            data: img.into_raw(),
        })
    }
    /// High-quality downscale preserving aspect so the result fits in max_w x max_h.
    /// Never upscales: an image that already fits is returned unchanged.
    pub fn thumbnail(&self, max_w: u32, max_h: u32) -> Rgba {
        if self.width == 0 || self.height == 0 || max_w == 0 || max_h == 0 {
            return Rgba::new(0, 0);
        }
        if self.width <= max_w && self.height <= max_h {
            return self.clone();
        }
        let scale = (max_w as f64 / self.width as f64).min(max_h as f64 / self.height as f64);
        let nw = ((self.width as f64 * scale).round() as u32).clamp(1, max_w);
        let nh = ((self.height as f64 * scale).round() as u32).clamp(1, max_h);
        // Resample premultiplied so transparent pixels do not bleed their color.
        let has_alpha = self.data.chunks_exact(4).any(|p| p[3] != 255);
        let mut data = self.data.clone();
        if has_alpha {
            for p in data.chunks_exact_mut(4) {
                let a = p[3] as u32;
                for c in &mut p[..3] {
                    *c = ((*c as u32 * a + 127) / 255) as u8;
                }
            }
        }
        let Some(buf) = ::image::RgbaImage::from_raw(self.width, self.height, data) else {
            return Rgba::new(0, 0);
        };
        let out = ::image::imageops::resize(&buf, nw, nh, ::image::imageops::FilterType::Lanczos3);
        let mut data = out.into_raw();
        if has_alpha {
            for p in data.chunks_exact_mut(4) {
                let a = p[3] as u32;
                match a {
                    0 => p[..3].fill(0),
                    a => {
                        for c in &mut p[..3] {
                            *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                        }
                    }
                }
            }
        }
        Rgba {
            width: nw,
            height: nh,
            data,
        }
    }
}

/// Write `bytes` to `path` atomically: create the parent folder, write a sibling temp
/// file, then rename over the destination.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut tmp_name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    tmp_name.push(format!(".tmp{}", std::process::id()));
    let tmp = path.with_file_name(tmp_name);
    let result = (|| -> anyhow::Result<()> {
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp)
                .with_context(|| format!("creating {}", tmp.display()))?;
            f.write_all(bytes)?;
            f.sync_all().ok();
        }
        std::fs::rename(&tmp, path).with_context(|| format!("renaming into {}", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> Rgba {
        let mut img = Rgba::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set_pixel(
                    x,
                    y,
                    [
                        (x * 255 / w.max(1)) as u8,
                        (y * 255 / h.max(1)) as u8,
                        90,
                        255,
                    ],
                );
            }
        }
        img
    }

    #[test]
    fn png_round_trip_is_exact() {
        let img = gradient(37, 23);
        let back = Rgba::decode(&img.to_png().unwrap()).unwrap();
        assert_eq!(img, back);
    }

    #[test]
    fn png_keeps_alpha() {
        let mut img = Rgba::new(4, 4);
        img.set_pixel(1, 1, [10, 20, 30, 77]);
        let back = Rgba::decode(&img.to_png().unwrap()).unwrap();
        assert_eq!(back.pixel(1, 1), [10, 20, 30, 77]);
        assert_eq!(back.pixel(0, 0)[3], 0);
    }

    #[test]
    fn webp_is_lossless() {
        let img = gradient(20, 20);
        let bytes = img.encode(ImageFormat::Webp, 90).unwrap();
        assert_eq!(Rgba::decode(&bytes).unwrap(), img);
    }

    #[test]
    fn jpeg_round_trip_is_close_and_flattens_alpha() {
        let img = Rgba::filled(16, 16, [200, 60, 60, 255]);
        let back = Rgba::decode(&img.encode(ImageFormat::Jpeg, 95).unwrap()).unwrap();
        let p = back.pixel(8, 8);
        assert!((p[0] as i32 - 200).abs() < 6 && (p[1] as i32 - 60).abs() < 6);
        let clear = Rgba::new(16, 16);
        let back = Rgba::decode(&clear.encode(ImageFormat::Jpeg, 95).unwrap()).unwrap();
        assert!(back.pixel(3, 3)[0] > 245);
    }

    #[test]
    fn empty_image_fails_to_encode() {
        assert!(Rgba::new(0, 0).to_png().is_err());
    }

    #[test]
    fn save_is_atomic_and_overwrites() {
        let dir = std::env::temp_dir().join(format!("stinkshot-core-img-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("sub").join("a.png");
        gradient(8, 8).save(&path, ImageFormat::Png, 90).unwrap();
        Rgba::filled(5, 5, [1, 2, 3, 255])
            .save(&path, ImageFormat::Png, 90)
            .unwrap();
        let back = Rgba::load(&path).unwrap();
        assert_eq!((back.width, back.height), (5, 5));
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(leftovers, vec!["a.png".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn thumbnail_fits_and_preserves_aspect() {
        let img = gradient(400, 200);
        let t = img.thumbnail(100, 100);
        assert_eq!((t.width, t.height), (100, 50));
        let t = img.thumbnail(1000, 1000);
        assert_eq!((t.width, t.height), (400, 200));
        let t = img.thumbnail(50, 10);
        assert_eq!((t.width, t.height), (20, 10));
    }

    #[test]
    fn thumbnail_averages_rather_than_samples() {
        // Fine checkerboard averages to mid gray under a good filter.
        let mut img = Rgba::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                let v = if (x + y) % 2 == 0 { 255 } else { 0 };
                img.set_pixel(x, y, [v, v, v, 255]);
            }
        }
        let t = img.thumbnail(8, 8);
        let p = t.pixel(4, 4);
        assert!((100..155).contains(&p[0]), "got {}", p[0]);
    }

    #[test]
    fn thumbnail_does_not_bleed_transparent_color() {
        let mut img = Rgba::new(32, 32);
        for y in 0..32 {
            for x in 0..16 {
                img.set_pixel(x, y, [255, 0, 0, 255]);
            }
            for x in 16..32 {
                img.set_pixel(x, y, [0, 0, 255, 0]);
            }
        }
        let t = img.thumbnail(8, 8);
        let p = t.pixel(1, 4);
        assert!(p[0] > 240 && p[2] < 15, "{p:?}");
    }

    #[test]
    fn crop_and_blit() {
        let img = gradient(10, 10);
        let c = img.crop(PxRect::new(2, 3, 4, 4));
        assert_eq!(c.pixel(0, 0), img.pixel(2, 3));
        let mut dst = Rgba::new(10, 10);
        dst.blit(&c, 8, 8);
        assert_eq!(dst.pixel(9, 9), img.pixel(3, 4));
    }
}
