//! Procedural application icon: a rounded accent square with a white shutter mark.
//! Pure std so `build.rs` can include it (for the exe .ico) and the tray can reuse it at
//! runtime (16/20/24/32 px depending on DPI).

/// Straight-alpha RGBA8, `size * size * 4` bytes.
pub fn render_rgba(size: u32, accent: [u8; 3]) -> Vec<u8> {
    let n = size as usize;
    let mut out = vec![0u8; n * n * 4];
    // 4x4 supersampling keeps small sizes clean.
    const SS: usize = 4;
    for y in 0..n {
        for x in 0..n {
            let (mut cover_bg, mut cover_mark) = (0u32, 0u32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let px = (x as f32 + (sx as f32 + 0.5) / SS as f32) / size as f32;
                    let py = (y as f32 + (sy as f32 + 0.5) / SS as f32) / size as f32;
                    if in_square(px, py) {
                        cover_bg += 1;
                        if in_mark(px, py) {
                            cover_mark += 1;
                        }
                    }
                }
            }
            let total = (SS * SS) as f32;
            let bg = cover_bg as f32 / total;
            if bg <= 0.0 {
                continue;
            }
            let mark = cover_mark as f32 / cover_bg as f32;
            let i = (y * n + x) * 4;
            for c in 0..3 {
                let v = accent[c] as f32 * (1.0 - mark) + 255.0 * mark;
                out[i + c] = v.round().clamp(0.0, 255.0) as u8;
            }
            out[i + 3] = (bg * 255.0).round() as u8;
        }
    }
    out
}

fn in_square(x: f32, y: f32) -> bool {
    let (cx, cy) = (0.5, 0.5);
    let half = 0.47;
    let r = 0.22;
    let qx = (x - cx).abs() - (half - r);
    let qy = (y - cy).abs() - (half - r);
    let d = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r;
    d < 0.0
}

fn in_mark(x: f32, y: f32) -> bool {
    let dx = x - 0.5;
    let dy = y - 0.5;
    let d = (dx * dx + dy * dy).sqrt();
    // Shutter ring.
    if (0.19..=0.30).contains(&d) {
        return true;
    }
    // Centre dot.
    if d <= 0.085 {
        return true;
    }
    // Viewfinder flash dot, top right.
    let fx = x - 0.76;
    let fy = y - 0.25;
    (fx * fx + fy * fy).sqrt() <= 0.05
}

/// Smallest tray size that is not blurry at the given system DPI.
#[allow(dead_code)] // the build script includes this file but only the binary draws the tray
pub fn tray_size_for_dpi(dpi: u32) -> u32 {
    let want = (16 * dpi.max(96)).div_ceil(96);
    [16u32, 20, 24, 32]
        .into_iter()
        .find(|s| *s >= want)
        .unwrap_or(32)
}

/// A multi-resolution .ico (BMP/DIB frames, no PNG dependency).
#[allow(dead_code)] // used by build.rs (embedded .exe icon), not by the binary
pub fn ico_bytes(accent: [u8; 3]) -> Vec<u8> {
    let sizes: [u32; 6] = [16, 24, 32, 48, 64, 256];
    let frames: Vec<Vec<u8>> = sizes.iter().map(|s| dib_frame(*s, accent)).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (s, f) in sizes.iter().zip(&frames) {
        out.push(if *s >= 256 { 0 } else { *s as u8 });
        out.push(if *s >= 256 { 0 } else { *s as u8 });
        out.push(0); // palette
        out.push(0); // reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bpp
        out.extend_from_slice(&(f.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += f.len() as u32;
    }
    for f in frames {
        out.extend_from_slice(&f);
    }
    out
}

#[allow(dead_code)] // used by build.rs through ico_bytes
fn dib_frame(size: u32, accent: [u8; 3]) -> Vec<u8> {
    let rgba = render_rgba(size, accent);
    let n = size as usize;
    let mut out = Vec::new();
    // BITMAPINFOHEADER; height is doubled to cover the AND mask.
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(size as i32).to_le_bytes());
    out.extend_from_slice(&((size * 2) as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&((n * n * 4) as u32).to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    // Pixels, bottom-up, BGRA.
    for y in (0..n).rev() {
        for x in 0..n {
            let i = (y * n + x) * 4;
            out.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
        }
    }
    // AND mask (all zero: alpha channel decides), rows padded to 32 bits.
    let row_bytes = n.div_ceil(32) * 4;
    out.extend(std::iter::repeat_n(0u8, row_bytes * n));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_has_expected_geometry() {
        let img = render_rgba(32, [0x7f, 0xd3, 0x6a]);
        assert_eq!(img.len(), 32 * 32 * 4);
        // Corner is transparent, centre is the white shutter dot.
        assert_eq!(img[3], 0);
        let c = (16 * 32 + 16) * 4;
        assert_eq!(&img[c..c + 4], &[255, 255, 255, 255]);
        // Between dot and ring the accent shows through.
        let a = (16 * 32 + 16 + 5) * 4;
        assert_eq!(&img[a..a + 3], &[0x7f, 0xd3, 0x6a]);
    }

    #[test]
    fn ico_header_is_valid() {
        let ico = ico_bytes([1, 2, 3]);
        assert_eq!(&ico[0..4], &[0, 0, 1, 0]);
        assert_eq!(u16::from_le_bytes([ico[4], ico[5]]), 6);
    }

    #[test]
    fn tray_sizes() {
        assert_eq!(tray_size_for_dpi(96), 16);
        assert_eq!(tray_size_for_dpi(120), 20);
        assert_eq!(tray_size_for_dpi(144), 24);
        assert_eq!(tray_size_for_dpi(192), 32);
        assert_eq!(tray_size_for_dpi(288), 32);
    }
}
