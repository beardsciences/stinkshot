//! Destructive pixel operations. All operate in place on the given rect (clipped).
//! These must destroy information: blur must be strong enough (and pixelate coarse enough)
//! that text is unrecoverable; blur additionally adds slight noise before blurring so
//! deconvolution can't reconstruct it.

use crate::geom::PxRect;
use crate::image::Rgba;

/// Smallest effective blur sigma. Weaker settings are raised to this so redaction is real.
pub const MIN_BLUR_SIGMA: f32 = 4.0;
/// Smallest effective pixelate block edge in px.
pub const MIN_PIXELATE_BLOCK: u32 = 4;

/// Peak amplitude of the pre-blur noise (per color channel, 0..255 scale).
const NOISE_AMPLITUDE: f32 = 7.0;

pub fn blur(img: &mut Rgba, rect: PxRect, radius: f32) {
    let Some(r) = rect.intersect(&img.bounds()) else {
        return;
    };
    let (w, h) = (r.w as usize, r.h as usize);
    let sigma = if radius.is_finite() {
        radius.max(MIN_BLUR_SIGMA)
    } else {
        MIN_BLUR_SIGMA
    };

    // Gather premultiplied floats, with deterministic noise added to the color channels.
    let mut buf = vec![0f32; w * h * 4];
    for yy in 0..h {
        for xx in 0..w {
            let sx = r.x as usize + xx;
            let sy = r.y as usize + yy;
            let si = (sy * img.width as usize + sx) * 4;
            let a = img.data[si + 3] as f32 / 255.0;
            let noise = hash_noise(sx as u32, sy as u32);
            let o = (yy * w + xx) * 4;
            for c in 0..3 {
                let n = noise[c] * NOISE_AMPLITUDE;
                buf[o + c] = (img.data[si + c] as f32 + n).clamp(0.0, 255.0) * a;
            }
            buf[o + 3] = a * 255.0;
        }
    }

    gaussian_blur(&mut buf, w, h, 4, sigma);

    for yy in 0..h {
        for xx in 0..w {
            let sx = r.x as usize + xx;
            let sy = r.y as usize + yy;
            let si = (sy * img.width as usize + sx) * 4;
            let o = (yy * w + xx) * 4;
            let a = buf[o + 3];
            if a <= 0.001 {
                img.data[si..si + 4].copy_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            let k = 255.0 / a;
            for c in 0..3 {
                img.data[si + c] = (buf[o + c] * k).round().clamp(0.0, 255.0) as u8;
            }
            img.data[si + 3] = a.round().clamp(0.0, 255.0) as u8;
        }
    }
}

pub fn pixelate(img: &mut Rgba, rect: PxRect, block: u32) {
    let Some(r) = rect.intersect(&img.bounds()) else {
        return;
    };
    let block = block.max(MIN_PIXELATE_BLOCK) as i32;
    let stride = img.width as usize * 4;
    // Cells are aligned to the rect origin so the grid hugs the selection.
    let mut cy = 0;
    while cy < r.h {
        let ch = block.min(r.h - cy);
        let mut cx = 0;
        while cx < r.w {
            let cw = block.min(r.w - cx);
            let (mut sr, mut sg, mut sb, mut sa) = (0u64, 0u64, 0u64, 0u64);
            for y in 0..ch {
                for x in 0..cw {
                    let i = (r.y + cy + y) as usize * stride + (r.x + cx + x) as usize * 4;
                    let a = img.data[i + 3] as u64;
                    sr += img.data[i] as u64 * a;
                    sg += img.data[i + 1] as u64 * a;
                    sb += img.data[i + 2] as u64 * a;
                    sa += a;
                }
            }
            let n = (cw * ch) as u64;
            let px = match sa {
                0 => [0, 0, 0, 0],
                sa => [
                    ((sr + sa / 2) / sa) as u8,
                    ((sg + sa / 2) / sa) as u8,
                    ((sb + sa / 2) / sa) as u8,
                    ((sa + n / 2) / n) as u8,
                ],
            };
            for y in 0..ch {
                for x in 0..cw {
                    let i = (r.y + cy + y) as usize * stride + (r.x + cx + x) as usize * 4;
                    img.data[i..i + 4].copy_from_slice(&px);
                }
            }
            cx += block;
        }
        cy += block;
    }
}

/// Overwrite (never blend) every pixel of the rect with `color`.
pub fn fill(img: &mut Rgba, rect: PxRect, color: [u8; 4]) {
    let Some(r) = rect.intersect(&img.bounds()) else {
        return;
    };
    let stride = img.width as usize * 4;
    for y in r.y..r.bottom() {
        let row = y as usize * stride;
        for x in r.x..r.right() {
            let i = row + x as usize * 4;
            img.data[i..i + 4].copy_from_slice(&color);
        }
    }
}

/// Deterministic per-pixel noise in [-1, 1] for three channels.
fn hash_noise(x: u32, y: u32) -> [f32; 3] {
    let mut s = (x as u64).wrapping_mul(0x9E37_79B1_85EB_CA87)
        ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ 0x1656_67B1_9E37_79F9;
    let mut out = [0f32; 3];
    for o in &mut out {
        s ^= s >> 33;
        s = s.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
        s ^= s >> 33;
        s = s.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
        s ^= s >> 33;
        *o = ((s & 0xFFFF) as f32 / 65535.0) * 2.0 - 1.0;
    }
    out
}

/// Box sizes whose successive application approximates a gaussian of `sigma`.
fn boxes_for_gauss(sigma: f32, n: usize) -> Vec<usize> {
    let nf = n as f32;
    let ideal = (12.0 * sigma * sigma / nf + 1.0).sqrt();
    let mut wl = ideal.floor() as i32;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wl = wl.max(1);
    let wu = wl + 2;
    let wlf = wl as f32;
    let m = ((12.0 * sigma * sigma - nf * wlf * wlf - 4.0 * nf * wlf - 3.0 * nf)
        / (-4.0 * wlf - 4.0))
        .round();
    (0..n)
        .map(|i| {
            if (i as f32) < m {
                wl as usize
            } else {
                wu as usize
            }
        })
        .collect()
}

/// In-place separable gaussian-ish blur (3 box passes) over interleaved `ch`-channel f32
/// data, clamping at the edges of the buffer.
pub(crate) fn gaussian_blur(data: &mut [f32], w: usize, h: usize, ch: usize, sigma: f32) {
    if w == 0 || h == 0 || sigma <= 0.0 {
        return;
    }
    let sizes = boxes_for_gauss(sigma, 3);
    let mut line_in = vec![0f32; w.max(h)];
    let mut line_out = vec![0f32; w.max(h)];
    for &size in &sizes {
        let r = (size - 1) / 2;
        if r == 0 {
            continue;
        }
        for c in 0..ch {
            // Horizontal.
            for y in 0..h {
                for x in 0..w {
                    line_in[x] = data[(y * w + x) * ch + c];
                }
                box_line(&line_in[..w], &mut line_out[..w], r);
                for x in 0..w {
                    data[(y * w + x) * ch + c] = line_out[x];
                }
            }
            // Vertical.
            for x in 0..w {
                for y in 0..h {
                    line_in[y] = data[(y * w + x) * ch + c];
                }
                box_line(&line_in[..h], &mut line_out[..h], r);
                for y in 0..h {
                    data[(y * w + x) * ch + c] = line_out[y];
                }
            }
        }
    }
}

fn box_line(src: &[f32], dst: &mut [f32], r: usize) {
    let n = src.len();
    if n == 0 {
        return;
    }
    let at = |i: isize| -> f32 { src[i.clamp(0, n as isize - 1) as usize] };
    let ri = r as isize;
    let inv = 1.0 / (2 * r + 1) as f32;
    let mut acc = 0.0f32;
    for k in -ri..=ri {
        acc += at(k);
    }
    for (i, d) in dst.iter_mut().enumerate().take(n) {
        *d = acc * inv;
        let i = i as isize;
        acc += at(i + ri + 1) - at(i - ri);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stripes(w: u32, h: u32) -> Rgba {
        // Stand-in for dense small text: 1px vertical bars with some vertical variation.
        let mut img = Rgba::filled(w, h, [255, 255, 255, 255]);
        for y in 0..h {
            for x in 0..w {
                if (x + (y / 3)) % 2 == 0 {
                    img.set_pixel(x, y, [0, 0, 0, 255]);
                }
            }
        }
        img
    }

    fn red_range(img: &Rgba, r: PxRect) -> (u8, u8) {
        let (mut lo, mut hi) = (255u8, 0u8);
        for y in r.y..r.bottom() {
            for x in r.x..r.right() {
                let v = img.pixel(x as u32, y as u32)[0];
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
        (lo, hi)
    }

    #[test]
    fn blur_loses_detail_and_stays_inside_rect() {
        let mut img = stripes(120, 60);
        let orig = img.clone();
        let rect = PxRect::new(20, 10, 80, 40);
        let inner = PxRect::new(40, 20, 40, 20);
        assert_eq!(red_range(&img, inner), (0, 255));
        blur(&mut img, rect, 6.0);
        let (lo, hi) = red_range(&img, inner);
        assert!(
            hi - lo < 40,
            "text-like pattern should flatten, got {lo}..{hi}"
        );
        // Outside untouched.
        for y in 0..60u32 {
            for x in 0..120u32 {
                if !rect.contains(crate::geom::PxPoint {
                    x: x as i32,
                    y: y as i32,
                }) {
                    assert_eq!(img.pixel(x, y), orig.pixel(x, y));
                }
            }
        }
    }

    #[test]
    fn blur_is_deterministic_and_adds_noise_on_flat_color() {
        let mut a = Rgba::filled(64, 64, [120, 120, 120, 255]);
        let mut b = a.clone();
        blur(&mut a, PxRect::new(0, 0, 64, 64), 8.0);
        blur(&mut b, PxRect::new(0, 0, 64, 64), 8.0);
        assert_eq!(a, b);
        // Mean stays about the same.
        let mean: f32 = a.data.chunks_exact(4).map(|p| p[0] as f32).sum::<f32>() / 4096.0;
        assert!((mean - 120.0).abs() < 2.0, "mean {mean}");
        // Alpha untouched for opaque input.
        assert!(a.data.chunks_exact(4).all(|p| p[3] == 255));
    }

    #[test]
    fn weak_radius_is_raised_to_minimum() {
        let mut img = stripes(40, 40);
        blur(&mut img, PxRect::new(0, 0, 40, 40), 0.0);
        let (lo, hi) = red_range(&img, PxRect::new(10, 10, 20, 20));
        assert!(hi - lo < 120, "{lo}..{hi}");
    }

    #[test]
    fn blur_clips_rect_to_image() {
        let mut img = stripes(30, 30);
        blur(&mut img, PxRect::new(-50, -50, 500, 500), 5.0);
        blur(&mut img, PxRect::new(100, 100, 5, 5), 5.0);
        blur(&mut img, PxRect::new(0, 0, 0, 0), 5.0);
    }

    #[test]
    fn pixelate_makes_flat_blocks_and_destroys_pattern() {
        let mut img = stripes(64, 64);
        let rect = PxRect::new(8, 8, 48, 48);
        pixelate(&mut img, rect, 12);
        // All pixels of the first cell are identical.
        let first = img.pixel(8, 8);
        for y in 8..20 {
            for x in 8..20 {
                assert_eq!(img.pixel(x, y), first);
            }
        }
        let (lo, hi) = red_range(&img, rect);
        assert!(hi - lo < 90, "{lo}..{hi}");
        // Outside untouched.
        assert_eq!(img.pixel(0, 0), stripes(64, 64).pixel(0, 0));
    }

    #[test]
    fn pixelate_handles_partial_edge_cells() {
        let mut img = stripes(50, 50);
        pixelate(&mut img, PxRect::new(0, 0, 50, 50), 16);
        assert_eq!(img.pixel(49, 49), img.pixel(48, 48));
    }

    #[test]
    fn tiny_block_is_raised() {
        let mut img = stripes(16, 16);
        pixelate(&mut img, PxRect::new(0, 0, 16, 16), 1);
        assert_eq!(img.pixel(0, 0), img.pixel(3, 3));
    }

    #[test]
    fn fill_overwrites_including_alpha() {
        let mut img = Rgba::filled(10, 10, [1, 2, 3, 255]);
        fill(&mut img, PxRect::new(2, 2, 4, 4), [9, 9, 9, 255]);
        assert_eq!(img.pixel(3, 3), [9, 9, 9, 255]);
        assert_eq!(img.pixel(6, 6), [1, 2, 3, 255]);
        fill(&mut img, PxRect::new(-5, -5, 7, 7), [0, 0, 0, 0]);
        assert_eq!(img.pixel(0, 0), [0, 0, 0, 0]);
        assert_eq!(img.pixel(1, 1), [0, 0, 0, 0]);
        assert_eq!(img.pixel(2, 2), [9, 9, 9, 255]);
    }

    #[test]
    fn boxes_are_odd_and_positive() {
        for s in [1.0, 4.0, 12.0, 40.0] {
            for b in boxes_for_gauss(s, 3) {
                assert!(b % 2 == 1 && b >= 1);
            }
        }
    }
}
