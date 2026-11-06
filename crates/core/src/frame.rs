//! Post-processing for window captures: Win11 rounded corners with real alpha, drop shadow,
//! optional padded background. Applied after capture, before annotation.

use crate::image::Rgba;
use crate::redact::gaussian_blur;

#[derive(Clone, Debug, PartialEq)]
pub struct WindowFx {
    /// Corner radius in physical px (8 * monitor scale on Win11). 0 = none.
    pub corner_radius: f32,
    pub shadow: bool,
    /// Shadow blur radius in px; offset is (0, blur/3).
    pub shadow_blur: f32,
    pub shadow_opacity: f32,
    /// Padding around the window (px). Only used when `background` is Some.
    pub padding: u32,
    /// Solid padded background; None = transparent margin just big enough for the shadow.
    pub background: Option<[u8; 4]>,
}

/// Returns a new, larger image when shadow/padding apply.
///
/// Layout: with a background, the canvas is `window + 2 * padding` on every side (the shadow
/// is clipped to the canvas if padding is smaller than its reach). Without a background the
/// margin is exactly what the shadow needs and is transparent. With neither shadow nor
/// background the result is just the rounded window at its original size.
pub fn apply_window_fx(window: &Rgba, fx: &WindowFx) -> Rgba {
    if window.width == 0 || window.height == 0 {
        return window.clone();
    }
    let mut win = window.clone();
    if fx.corner_radius > 0.0 {
        round_corners(&mut win, fx.corner_radius);
    }

    let blur = if fx.shadow {
        fx.shadow_blur.max(0.0)
    } else {
        0.0
    };
    let offset_y = (blur / 3.0).round() as i64;
    let shadow_on = fx.shadow && fx.shadow_opacity > 0.0 && blur > 0.0;
    // Gaussian reach is about 3 sigma, and sigma is blur / 2.
    let reach = if shadow_on {
        (blur * 1.5).ceil() as i64
    } else {
        0
    };
    let shadow_margin = if shadow_on { reach + offset_y.abs() } else { 0 };

    let margin = match fx.background {
        Some(_) => fx.padding as i64,
        None => shadow_margin,
    };
    if margin == 0 && fx.background.is_none() {
        return win;
    }

    let cw = win.width as i64 + 2 * margin;
    let ch = win.height as i64 + 2 * margin;
    let mut canvas = match fx.background {
        Some(bg) => Rgba::filled(cw as u32, ch as u32, bg),
        None => Rgba::new(cw as u32, ch as u32),
    };

    if shadow_on {
        // Silhouette alpha placed at the shadow position, blurred, then tinted black.
        let (cwu, chu) = (cw as usize, ch as usize);
        let mut mask = vec![0f32; cwu * chu];
        for y in 0..win.height as i64 {
            for x in 0..win.width as i64 {
                let a = win.data[((y as usize * win.width as usize) + x as usize) * 4 + 3];
                if a == 0 {
                    continue;
                }
                let dx = x + margin;
                let dy = y + margin + offset_y;
                if dx < 0 || dy < 0 || dx >= cw || dy >= ch {
                    continue;
                }
                mask[dy as usize * cwu + dx as usize] = a as f32 / 255.0;
            }
        }
        gaussian_blur(&mut mask, cwu, chu, 1, blur / 2.0);
        let opacity = fx.shadow_opacity.clamp(0.0, 1.0);
        for (i, m) in mask.iter().enumerate() {
            let a = (m * opacity * 255.0).round().clamp(0.0, 255.0) as u8;
            if a > 0 {
                let d = &mut canvas.data[i * 4..i * 4 + 4];
                let out = over([d[0], d[1], d[2], d[3]], [0, 0, 0, a]);
                d.copy_from_slice(&out);
            }
        }
    }

    // Window on top, source-over so rounded edges blend with the shadow/background.
    for y in 0..win.height {
        for x in 0..win.width {
            let s = win.pixel(x, y);
            if s[3] == 0 {
                continue;
            }
            let (dx, dy) = ((x as i64 + margin) as u32, (y as i64 + margin) as u32);
            let d = canvas.pixel(dx, dy);
            canvas.set_pixel(dx, dy, over(d, s));
        }
    }
    canvas
}

/// Zero alpha outside a rounded rectangle covering the whole image (anti-aliased edge).
pub fn round_corners(img: &mut Rgba, radius: f32) {
    if img.width == 0 || img.height == 0 || radius.is_nan() || radius <= 0.0 {
        return;
    }
    let (w, h) = (img.width as f32, img.height as f32);
    let r = radius.min(w / 2.0).min(h / 2.0);
    if r <= 0.0 {
        return;
    }
    let ri = r.ceil() as u32;
    let corner = |img: &mut Rgba, px: u32, py: u32, cx: f32, cy: f32| {
        // Pixel center distance to the corner circle center.
        let dx = px as f32 + 0.5 - cx;
        let dy = py as f32 + 0.5 - cy;
        let d = (dx * dx + dy * dy).sqrt();
        let cov = (r - d + 0.5).clamp(0.0, 1.0);
        if cov < 1.0 {
            let i = ((py * img.width + px) * 4 + 3) as usize;
            img.data[i] = (img.data[i] as f32 * cov).round() as u8;
        }
    };
    for j in 0..ri.min(img.height) {
        for i in 0..ri.min(img.width) {
            // Top-left.
            if (i as f32) < r && (j as f32) < r {
                corner(img, i, j, r, r);
            }
            // Top-right.
            let xr = img.width - 1 - i;
            if (i as f32) < r && (j as f32) < r {
                corner(img, xr, j, w - r, r);
            }
            let yb = img.height - 1 - j;
            if (i as f32) < r && (j as f32) < r {
                corner(img, i, yb, r, h - r);
                corner(img, xr, yb, w - r, h - r);
            }
        }
    }
}

/// Source-over for straight-alpha RGBA8.
pub(crate) fn over(dst: [u8; 4], src: [u8; 4]) -> [u8; 4] {
    let sa = src[3] as u32;
    if sa == 255 {
        return src;
    }
    if sa == 0 {
        return dst;
    }
    let da = dst[3] as u32;
    // out_a = sa + da * (255 - sa) / 255
    let out_a255 = sa * 255 + da * (255 - sa); // scaled by 255
    if out_a255 == 0 {
        return [0, 0, 0, 0];
    }
    let mut out = [0u8; 4];
    for c in 0..3 {
        let num = src[c] as u32 * sa * 255 + dst[c] as u32 * da * (255 - sa);
        out[c] = ((num + out_a255 / 2) / out_a255).min(255) as u8;
    }
    out[3] = ((out_a255 + 127) / 255).min(255) as u8;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32) -> Rgba {
        Rgba::filled(w, h, [200, 100, 50, 255])
    }

    #[test]
    fn round_corners_clears_corner_keeps_center_and_edges() {
        let mut img = solid(60, 40);
        round_corners(&mut img, 10.0);
        assert_eq!(img.pixel(0, 0)[3], 0);
        assert_eq!(img.pixel(59, 0)[3], 0);
        assert_eq!(img.pixel(0, 39)[3], 0);
        assert_eq!(img.pixel(59, 39)[3], 0);
        assert_eq!(img.pixel(30, 20)[3], 255);
        // Straight edges away from corners stay opaque.
        assert_eq!(img.pixel(30, 0)[3], 255);
        assert_eq!(img.pixel(0, 20)[3], 255);
        assert_eq!(img.pixel(59, 20)[3], 255);
        assert_eq!(img.pixel(30, 39)[3], 255);
    }

    #[test]
    fn round_corners_has_antialiased_edge_and_symmetry() {
        let mut img = solid(50, 50);
        round_corners(&mut img, 12.0);
        let mut partial = 0;
        for y in 0..12 {
            for x in 0..12 {
                let a = img.pixel(x, y)[3];
                if a > 0 && a < 255 {
                    partial += 1;
                }
                // Four-fold symmetry.
                assert_eq!(a, img.pixel(49 - x, y)[3]);
                assert_eq!(a, img.pixel(x, 49 - y)[3]);
                assert_eq!(a, img.pixel(49 - x, 49 - y)[3]);
            }
        }
        assert!(partial >= 8, "expected AA pixels, got {partial}");
        // Color is untouched, only alpha.
        assert_eq!(&img.pixel(25, 25)[..3], &[200, 100, 50]);
    }

    #[test]
    fn round_corners_radius_clamped_to_half_size() {
        let mut img = solid(20, 20);
        round_corners(&mut img, 500.0);
        assert_eq!(img.pixel(0, 0)[3], 0);
        assert_eq!(img.pixel(10, 10)[3], 255);
        round_corners(&mut img, 0.0);
        round_corners(&mut Rgba::new(0, 0), 4.0);
    }

    #[test]
    fn fx_without_shadow_or_padding_keeps_size() {
        let fx = WindowFx {
            corner_radius: 8.0,
            shadow: false,
            shadow_blur: 20.0,
            shadow_opacity: 0.5,
            padding: 40,
            background: None,
        };
        let out = apply_window_fx(&solid(100, 80), &fx);
        assert_eq!((out.width, out.height), (100, 80));
        assert_eq!(out.pixel(0, 0)[3], 0);
    }

    #[test]
    fn fx_shadow_grows_canvas_and_is_darker_below() {
        let fx = WindowFx {
            corner_radius: 8.0,
            shadow: true,
            shadow_blur: 24.0,
            shadow_opacity: 0.6,
            padding: 0,
            background: None,
        };
        let out = apply_window_fx(&solid(100, 80), &fx);
        assert!(out.width > 100 && out.height > 80);
        let m = (out.width - 100) / 2;
        // Window pixel intact.
        assert_eq!(out.pixel(m + 50, m + 40), [200, 100, 50, 255]);
        // Shadow visible just below the window, black, partially transparent.
        let below = out.pixel(m + 50, m + 80 + 3);
        assert!(below[3] > 20 && below[3] < 255, "{below:?}");
        assert_eq!(&below[..3], &[0, 0, 0]);
        // Shadow is offset down: more opaque below than above at the same distance.
        let above = out.pixel(m + 50, m - 4);
        assert!(below[3] > above[3], "below {} above {}", below[3], above[3]);
        // Far corner stays clear.
        assert_eq!(out.pixel(0, 0)[3], 0);
    }

    #[test]
    fn fx_background_padding_exact_size_and_color() {
        let fx = WindowFx {
            corner_radius: 10.0,
            shadow: true,
            shadow_blur: 16.0,
            shadow_opacity: 0.4,
            padding: 48,
            background: Some([30, 60, 90, 255]),
        };
        let out = apply_window_fx(&solid(100, 80), &fx);
        assert_eq!((out.width, out.height), (196, 176));
        assert_eq!(out.pixel(0, 0), [30, 60, 90, 255]);
        assert_eq!(out.pixel(98, 88), [200, 100, 50, 255]);
        // Rounded corner shows background/shadow blend, not transparency.
        assert_eq!(out.pixel(48, 48)[3], 255);
        assert_ne!(out.pixel(48, 48), [200, 100, 50, 255]);
    }

    #[test]
    fn fx_background_without_shadow() {
        let fx = WindowFx {
            corner_radius: 0.0,
            shadow: false,
            shadow_blur: 0.0,
            shadow_opacity: 0.0,
            padding: 10,
            background: Some([255, 255, 255, 255]),
        };
        let out = apply_window_fx(&solid(20, 20), &fx);
        assert_eq!((out.width, out.height), (40, 40));
        assert_eq!(out.pixel(5, 5), [255, 255, 255, 255]);
        assert_eq!(out.pixel(10, 10), [200, 100, 50, 255]);
    }

    #[test]
    fn over_blends_correctly() {
        assert_eq!(over([10, 20, 30, 255], [0, 0, 0, 0]), [10, 20, 30, 255]);
        assert_eq!(over([10, 20, 30, 255], [200, 0, 0, 255]), [200, 0, 0, 255]);
        let p = over([255, 255, 255, 255], [0, 0, 0, 128]);
        assert!((p[0] as i32 - 127).abs() <= 1 && p[3] == 255);
        let p = over([0, 0, 0, 0], [100, 150, 200, 128]);
        assert_eq!((p[0], p[1], p[2], p[3]), (100, 150, 200, 128));
    }
}
