//! Pixel format conversion and scaling helpers (pure CPU).

/// BGRA (top-down, `stride` bytes per row) -> NV12 (BT.709, limited range).
/// `dst` must hold `w*h*3/2` bytes. `w` and `h` must be even.
pub fn bgra_to_nv12(src: &[u8], stride: usize, w: usize, h: usize, dst: &mut [u8]) {
    debug_assert!(w.is_multiple_of(2) && h.is_multiple_of(2));
    debug_assert!(dst.len() >= w * h * 3 / 2);
    debug_assert!(src.len() >= stride * (h - 1) + w * 4);
    let (y_plane, uv_plane) = dst.split_at_mut(w * h);
    let uv_plane = &mut uv_plane[..w * h / 2];

    let threads = if w * h >= 640 * 360 {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(4)
    } else {
        1
    };
    if threads <= 1 {
        convert_rows(src, stride, w, 0, h, y_plane, uv_plane);
        return;
    }
    // Bands of an even number of rows, one per thread.
    let pairs = h / 2;
    let per = pairs.div_ceil(threads);
    std::thread::scope(|s| {
        let mut y_rest = y_plane;
        let mut uv_rest = uv_plane;
        let mut row = 0usize;
        while row < h {
            let rows = (per * 2).min(h - row);
            let (y_band, yr) = y_rest.split_at_mut(rows * w);
            let (uv_band, uvr) = uv_rest.split_at_mut(rows / 2 * w);
            y_rest = yr;
            uv_rest = uvr;
            let r0 = row;
            s.spawn(move || convert_rows(src, stride, w, r0, r0 + rows, y_band, uv_band));
            row += rows;
        }
    });
}

fn convert_rows(
    src: &[u8],
    stride: usize,
    w: usize,
    r0: usize,
    r1: usize,
    y_out: &mut [u8],
    uv_out: &mut [u8],
) {
    let mut r = r0;
    while r < r1 {
        let row_a = &src[r * stride..r * stride + w * 4];
        let row_b = &src[(r + 1) * stride..(r + 1) * stride + w * 4];
        let ya = (r - r0) * w;
        let yb = ya + w;
        let uvo = (r - r0) / 2 * w;
        for x in (0..w).step_by(2) {
            let mut u_sum = 0i32;
            let mut v_sum = 0i32;
            for (row, yoff) in [(row_a, ya), (row_b, yb)] {
                for dx in 0..2 {
                    let i = (x + dx) * 4;
                    let b = row[i] as i32;
                    let g = row[i + 1] as i32;
                    let rr = row[i + 2] as i32;
                    y_out[yoff + x + dx] = (((47 * rr + 157 * g + 16 * b + 128) >> 8) + 16) as u8;
                    u_sum += ((-26 * rr - 87 * g + 112 * b + 128) >> 8) + 128;
                    v_sum += ((112 * rr - 102 * g - 10 * b + 128) >> 8) + 128;
                }
            }
            uv_out[uvo + x] = ((u_sum + 2) >> 2).clamp(0, 255) as u8;
            uv_out[uvo + x + 1] = ((v_sum + 2) >> 2).clamp(0, 255) as u8;
        }
        r += 2;
    }
}

/// Scale a 4-channel 8-bit image (channel order irrelevant). Downscaling averages the source
/// footprint of each destination pixel; upscaling is bilinear.
pub fn scale_4ch(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<u8> {
    scale_4ch_crop(src, sw, sh, (0, 0, sw, sh), dw, dh)
}

/// Like `scale_4ch` but only reads `crop = (x, y, w, h)` from the source.
pub fn scale_4ch_crop(
    src: &[u8],
    sw: usize,
    sh: usize,
    crop: (usize, usize, usize, usize),
    dw: usize,
    dh: usize,
) -> Vec<u8> {
    let (cx, cy, cw, ch) = crop;
    let mut out = vec![0u8; dw * dh * 4];
    if dw == 0
        || dh == 0
        || cw == 0
        || ch == 0
        || src.len() < sw * sh * 4
        || cx + cw > sw
        || cy + ch > sh
    {
        return out;
    }
    if cw == dw && ch == dh {
        for y in 0..dh {
            let s = ((cy + y) * sw + cx) * 4;
            out[y * dw * 4..(y + 1) * dw * 4].copy_from_slice(&src[s..s + dw * 4]);
        }
        return out;
    }
    let fx = cw as f32 / dw as f32;
    let fy = ch as f32 / dh as f32;
    for dy in 0..dh {
        for dx in 0..dw {
            let o = (dy * dw + dx) * 4;
            if fx > 1.0 || fy > 1.0 {
                let x0 = cx + (dx as f32 * fx).floor() as usize;
                let x1 = (cx + ((dx + 1) as f32 * fx).ceil() as usize).clamp(x0 + 1, cx + cw);
                let y0 = cy + (dy as f32 * fy).floor() as usize;
                let y1 = (cy + ((dy + 1) as f32 * fy).ceil() as usize).clamp(y0 + 1, cy + ch);
                let x0 = x0.min(x1 - 1);
                let y0 = y0.min(y1 - 1);
                let mut acc = [0u32; 4];
                for y in y0..y1 {
                    for x in x0..x1 {
                        let i = (y * sw + x) * 4;
                        for c in 0..4 {
                            acc[c] += src[i + c] as u32;
                        }
                    }
                }
                let n = ((x1 - x0) * (y1 - y0)) as u32;
                for c in 0..4 {
                    out[o + c] = ((acc[c] + n / 2) / n) as u8;
                }
            } else {
                let sx = (cx as f32 + (dx as f32 + 0.5) * fx - 0.5)
                    .clamp(cx as f32, (cx + cw - 1) as f32);
                let sy = (cy as f32 + (dy as f32 + 0.5) * fy - 0.5)
                    .clamp(cy as f32, (cy + ch - 1) as f32);
                let x0 = sx.floor() as usize;
                let y0 = sy.floor() as usize;
                let x1 = (x0 + 1).min(cx + cw - 1);
                let y1 = (y0 + 1).min(cy + ch - 1);
                let tx = sx - x0 as f32;
                let ty = sy - y0 as f32;
                for c in 0..4 {
                    let p = |x: usize, y: usize| src[(y * sw + x) * 4 + c] as f32;
                    let top = p(x0, y0) * (1.0 - tx) + p(x1, y0) * tx;
                    let bot = p(x0, y1) * (1.0 - tx) + p(x1, y1) * tx;
                    out[o + c] = (top * (1.0 - ty) + bot * ty + 0.5) as u8;
                }
            }
        }
    }
    out
}

/// Largest size not exceeding `max_w` x `max_h` (0 = unbounded) that keeps the aspect ratio.
pub fn fit_within(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let mut s = 1.0f64;
    if max_w > 0 && w > max_w {
        s = s.min(max_w as f64 / w as f64);
    }
    if max_h > 0 && h > max_h {
        s = s.min(max_h as f64 / h as f64);
    }
    (
        ((w as f64 * s).round() as u32).max(1),
        ((h as f64 * s).round() as u32).max(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, b: u8, g: u8, r: u8) -> Vec<u8> {
        let mut v = vec![0u8; w * h * 4];
        for p in v.chunks_exact_mut(4) {
            p.copy_from_slice(&[b, g, r, 255]);
        }
        v
    }

    fn nv12(src: &[u8], w: usize, h: usize) -> Vec<u8> {
        let mut dst = vec![0u8; w * h * 3 / 2];
        bgra_to_nv12(src, w * 4, w, h, &mut dst);
        dst
    }

    #[test]
    fn nv12_black_white() {
        let (w, h) = (8, 8);
        let d = nv12(&solid(w, h, 0, 0, 0), w, h);
        assert!(d[..w * h].iter().all(|&y| y == 16));
        assert!(d[w * h..].iter().all(|&c| c == 128));
        let d = nv12(&solid(w, h, 255, 255, 255), w, h);
        assert!(d[..w * h].iter().all(|&y| (234..=236).contains(&y)));
        assert!(d[w * h..].iter().all(|&c| (127..=129).contains(&c)));
    }

    #[test]
    fn nv12_red_has_high_v_low_u() {
        let (w, h) = (4, 4);
        let d = nv12(&solid(w, h, 0, 0, 255), w, h);
        let (u, v) = (d[w * h], d[w * h + 1]);
        assert!(v > 230, "v={v}");
        assert!(u < 120, "u={u}");
        let y = d[0];
        assert!((60..70).contains(&y), "y={y}");
    }

    #[test]
    fn nv12_threaded_matches_single() {
        let (w, h) = (1280, 720);
        let mut src = vec![0u8; w * h * 4];
        for (i, p) in src.chunks_exact_mut(4).enumerate() {
            p.copy_from_slice(&[(i * 7) as u8, (i / 3) as u8, (i / 1280 * 5) as u8, 255]);
        }
        let multi = nv12(&src, w, h);
        let mut single = vec![0u8; w * h * 3 / 2];
        let (y, uv) = single.split_at_mut(w * h);
        convert_rows(&src, w * 4, w, 0, h, y, uv);
        assert_eq!(multi, single);
    }

    #[test]
    fn nv12_respects_stride() {
        let (w, h, stride) = (4usize, 2usize, 24usize);
        let mut src = vec![255u8; stride * h];
        for y in 0..h {
            for x in 0..w {
                src[y * stride + x * 4..y * stride + x * 4 + 4].copy_from_slice(&[0, 0, 0, 255]);
            }
        }
        let mut dst = vec![0u8; w * h * 3 / 2];
        bgra_to_nv12(&src, stride, w, h, &mut dst);
        assert!(dst[..w * h].iter().all(|&y| y == 16));
    }

    #[test]
    fn scale_down_averages() {
        let mut src = vec![0u8; 4 * 4 * 4];
        for (i, p) in src.chunks_exact_mut(4).enumerate() {
            let x = i % 4;
            let v = if x < 2 { 0 } else { 200 };
            p.copy_from_slice(&[v, v, v, 255]);
        }
        let out = scale_4ch(&src, 4, 4, 2, 2);
        assert_eq!(&out[0..4], &[0, 0, 0, 255]);
        assert_eq!(&out[4..8], &[200, 200, 200, 255]);
    }

    #[test]
    fn scale_identity_and_up() {
        let src = solid(3, 3, 10, 20, 30);
        assert_eq!(scale_4ch(&src, 3, 3, 3, 3), src);
        let up = scale_4ch(&src, 3, 3, 6, 6);
        assert_eq!(up.len(), 6 * 6 * 4);
        assert!(up.chunks_exact(4).all(|p| p == [10, 20, 30, 255]));
    }

    #[test]
    fn scale_crop_reads_only_region() {
        let mut src = vec![0u8; 8 * 8 * 4];
        for y in 0..8 {
            for x in 0..8 {
                let i = (y * 8 + x) * 4;
                let v = if (2..6).contains(&x) && (2..6).contains(&y) {
                    255
                } else {
                    0
                };
                src[i..i + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        let out = scale_4ch_crop(&src, 8, 8, (2, 2, 4, 4), 2, 2);
        assert!(out.chunks_exact(4).all(|p| p == [255, 255, 255, 255]));
    }

    #[test]
    fn fit_within_cases() {
        assert_eq!(fit_within(1920, 1080, 0, 0), (1920, 1080));
        assert_eq!(fit_within(1920, 1080, 960, 0), (960, 540));
        assert_eq!(fit_within(1920, 1080, 0, 90), (160, 90));
        assert_eq!(fit_within(100, 50, 500, 500), (100, 50));
    }
}
