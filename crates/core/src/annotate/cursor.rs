//! The mouse pointer as an annotation object: its bitmap, the sidecar encoding (base64 PNG)
//! and the conversion from the two GDI renders the platform layer takes of it.

use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::image::Rgba;

/// Largest pointer bitmap accepted, per side (Windows tops out at 256 at 400 % scale).
pub const MAX_CURSOR_SIDE: u32 = 512;

/// A pointer bitmap (straight-alpha RGBA). Shared, so undo snapshots of a doc stay cheap.
/// Stored in sidecars as a base64 PNG string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorImage(pub Arc<Rgba>);

impl CursorImage {
    pub fn new(img: Rgba) -> Self {
        Self(Arc::new(img))
    }
    pub fn width(&self) -> u32 {
        self.0.width
    }
    pub fn height(&self) -> u32 {
        self.0.height
    }
    pub fn rgba(&self) -> &Rgba {
        &self.0
    }
}

impl Serialize for CursorImage {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let png = self.0.to_png().map_err(serde::ser::Error::custom)?;
        s.serialize_str(&b64_encode(&png))
    }
}

impl<'de> Deserialize<'de> for CursorImage {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let bytes = b64_decode(&s).ok_or_else(|| serde::de::Error::custom("bad base64"))?;
        let img = Rgba::decode(&bytes).map_err(serde::de::Error::custom)?;
        if img.width == 0
            || img.height == 0
            || img.width > MAX_CURSOR_SIDE
            || img.height > MAX_CURSOR_SIDE
        {
            return Err(serde::de::Error::custom("bad cursor image size"));
        }
        Ok(Self::new(img))
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding.
pub(crate) fn b64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16
            | (c.get(1).copied().unwrap_or(0) as u32) << 8
            | c.get(2).copied().unwrap_or(0) as u32;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Decodes standard base64 (padding optional, whitespace ignored). None on bad input.
pub(crate) fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let chars: Vec<u8> = s
        .bytes()
        .filter(|c| !c.is_ascii_whitespace())
        .collect::<Vec<_>>();
    let trimmed: &[u8] = {
        let mut end = chars.len();
        while end > 0 && chars[end - 1] == b'=' {
            end -= 1;
        }
        if chars.len() - end > 2 {
            return None;
        }
        &chars[..end]
    };
    if trimmed.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(trimmed.len() * 3 / 4);
    for c in trimmed.chunks(4) {
        let mut n = 0u32;
        for (i, &ch) in c.iter().enumerate() {
            n |= val(ch)? << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if c.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if c.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

/// Straight-alpha RGBA of a pointer from the same pointer drawn (`DrawIconEx`) over opaque
/// black and over opaque white, both BGRA. This handles every pointer kind the same way:
/// alpha cursors, colour cursors with an AND mask and monochrome AND/XOR cursors.
///
/// Over black a pixel is `C*a`, over white `C*a + 255*(1-a)`, so `a = 255 - (white - black)`.
/// Pixels that come out darker over white than over black invert the screen (XOR with an
/// AND mask of 1, like the I-beam). A still image cannot invert, so they become opaque black
/// with a one-pixel white outline around them: readable on light and dark backgrounds.
pub fn cursor_from_renders(black: &[u8], white: &[u8], w: u32, h: u32) -> Rgba {
    let mut out = Rgba::new(w, h);
    let n = w as usize * h as usize;
    if black.len() < n * 4 || white.len() < n * 4 {
        return out;
    }
    let mut invert = vec![false; n];
    for (i, ((o, b), wh)) in out
        .data
        .chunks_exact_mut(4)
        .zip(black.chunks_exact(4))
        .zip(white.chunks_exact(4))
        .enumerate()
    {
        let diff: i32 = (0..3).map(|c| wh[c] as i32 - b[c] as i32).sum::<i32>() / 3;
        if diff < -64 {
            invert[i] = true;
            o.copy_from_slice(&[0, 0, 0, 255]);
            continue;
        }
        let a = (255 - diff).clamp(0, 255) as u32;
        if a == 0 {
            continue;
        }
        o[0] = ((b[2] as u32 * 255 + a / 2) / a).min(255) as u8;
        o[1] = ((b[1] as u32 * 255 + a / 2) / a).min(255) as u8;
        o[2] = ((b[0] as u32 * 255 + a / 2) / a).min(255) as u8;
        o[3] = a as u8;
    }
    if invert.iter().any(|v| *v) {
        let (w, h) = (w as i32, h as i32);
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) as usize;
                if invert[i] || out.data[i * 4 + 3] != 0 {
                    continue;
                }
                let near = [(-1, 0), (1, 0), (0, -1), (0, 1)].iter().any(|(dx, dy)| {
                    let (nx, ny) = (x + dx, y + dy);
                    nx >= 0 && ny >= 0 && nx < w && ny < h && invert[(ny * w + nx) as usize]
                });
                if near {
                    out.data[i * 4..i * 4 + 4].copy_from_slice(&[255, 255, 255, 255]);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_all_lengths() {
        for len in 0..40usize {
            let data: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            let enc = b64_encode(&data);
            assert_eq!(enc.len() % 4, 0);
            assert_eq!(b64_decode(&enc).unwrap(), data, "len {len}");
        }
        assert_eq!(b64_encode(b"Man"), "TWFu");
        assert_eq!(b64_encode(b"Ma"), "TWE=");
        assert_eq!(b64_decode("TWE").unwrap(), b"Ma");
        assert!(b64_decode("T").is_none());
        assert!(b64_decode("TW!u").is_none());
    }

    /// One BGRA pixel per (black render, white render) pair.
    fn renders(px: &[([u8; 3], [u8; 3])]) -> (Vec<u8>, Vec<u8>) {
        let mut b = Vec::new();
        let mut w = Vec::new();
        for (pb, pw) in px {
            b.extend_from_slice(&[pb[0], pb[1], pb[2], 255]);
            w.extend_from_slice(&[pw[0], pw[1], pw[2], 255]);
        }
        (b, w)
    }

    #[test]
    fn monochrome_mask_cases_map_to_rgba() {
        // AND/XOR: (0,0) black, (0,1) white, (1,0) transparent, (1,1) invert.
        let (b, w) = renders(&[
            ([0; 3], [0; 3]),
            ([255; 3], [255; 3]),
            ([0; 3], [255; 3]),
            ([255; 3], [0; 3]),
        ]);
        let img = cursor_from_renders(&b, &w, 4, 1);
        assert_eq!(img.pixel(0, 0), [0, 0, 0, 255]);
        assert_eq!(img.pixel(1, 0), [255, 255, 255, 255]);
        // The transparent pixel touches the inverting one: it becomes its white outline.
        assert_eq!(img.pixel(2, 0), [255, 255, 255, 255]);
        assert_eq!(img.pixel(3, 0), [0, 0, 0, 255]);
    }

    #[test]
    fn inverting_pixels_get_a_white_outline_only_where_transparent() {
        // 3x3: the centre inverts, the rest is transparent.
        let mut px = vec![([0u8; 3], [255u8; 3]); 9];
        px[4] = ([255; 3], [0; 3]);
        let (b, w) = renders(&px);
        let img = cursor_from_renders(&b, &w, 3, 3);
        assert_eq!(img.pixel(1, 1), [0, 0, 0, 255]);
        for (x, y) in [(1, 0), (0, 1), (2, 1), (1, 2)] {
            assert_eq!(img.pixel(x, y), [255, 255, 255, 255], "{x},{y}");
        }
        for (x, y) in [(0, 0), (2, 0), (0, 2), (2, 2)] {
            assert_eq!(img.pixel(x, y)[3], 0, "{x},{y}");
        }
    }

    #[test]
    fn alpha_pixels_recover_straight_color() {
        // Red (BGRA 0,0,255) at 50 % alpha: over black 128, over white 255 / 127 / 127.
        let (b, w) = renders(&[([0, 0, 128], [127, 127, 255])]);
        let img = cursor_from_renders(&b, &w, 1, 1);
        let p = img.pixel(0, 0);
        assert!((p[3] as i32 - 127).abs() <= 2, "{p:?}");
        assert!(p[0] >= 250 && p[1] <= 4 && p[2] <= 4, "{p:?}");
    }

    #[test]
    fn short_buffers_give_a_transparent_image() {
        let img = cursor_from_renders(&[0; 4], &[0; 4], 2, 2);
        assert_eq!((img.width, img.height), (2, 2));
        assert!(img.data.iter().all(|v| *v == 0));
    }
}
