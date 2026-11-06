//! Owned RGBA8 bitmap plus encode/decode helpers.

use std::path::Path;

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
        Self { width, height, data: vec![0; (width as usize) * (height as usize) * 4] }
    }
    pub fn filled(width: u32, height: u32, rgba: [u8; 4]) -> Self {
        let mut img = Self::new(width, height);
        for px in img.data.chunks_exact_mut(4) {
            px.copy_from_slice(&rgba);
        }
        img
    }
    pub fn from_raw(width: u32, height: u32, data: Vec<u8>) -> Self {
        assert_eq!(data.len(), width as usize * height as usize * 4, "Rgba::from_raw size mismatch");
        Self { width, height, data }
    }
    pub fn bounds(&self) -> PxRect {
        PxRect::new(0, 0, self.width as i32, self.height as i32)
    }
    #[inline]
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [self.data[i], self.data[i + 1], self.data[i + 2], self.data[i + 3]]
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
        let Some(r) = dst_r.intersect(&self.bounds()) else { return };
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
        let _ = (format, jpeg_quality);
        todo!("core::image::Rgba::encode")
    }
    pub fn to_png(&self) -> anyhow::Result<Vec<u8>> {
        self.encode(ImageFormat::Png, 90)
    }
    /// Encode and write atomically (temp file + rename).
    pub fn save(&self, path: &Path, format: ImageFormat, jpeg_quality: u8) -> anyhow::Result<()> {
        let _ = (path, format, jpeg_quality);
        todo!("core::image::Rgba::save")
    }
    pub fn load(path: &Path) -> anyhow::Result<Rgba> {
        let _ = path;
        todo!("core::image::Rgba::load")
    }
    pub fn decode(bytes: &[u8]) -> anyhow::Result<Rgba> {
        let _ = bytes;
        todo!("core::image::Rgba::decode")
    }
    /// High-quality downscale preserving aspect so the result fits in max_w x max_h.
    pub fn thumbnail(&self, max_w: u32, max_h: u32) -> Rgba {
        let _ = (max_w, max_h);
        todo!("core::image::Rgba::thumbnail")
    }
}
