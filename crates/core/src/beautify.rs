//! Beautify: a frame for sharing a capture. Padding, a background (solid, gradient, or a
//! blurred, enlarged copy of the capture), rounded corners, a soft shadow and an optional
//! canvas aspect (16:9, 1:1, 4:5; the canvas grows, the capture is never cropped). Applied to
//! the flattened image at export; the settings live in `[editor.beautify]`.

use serde::{Deserialize, Serialize};

use crate::config::parse_hex_color;
use crate::frame::{over, round_corners};
use crate::image::Rgba;
use crate::redact::gaussian_blur;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundKind {
    Solid,
    #[default]
    Gradient,
    /// The capture itself, scaled to cover the canvas and heavily blurred.
    Blurred,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Aspect {
    /// Padding only.
    #[default]
    Free,
    /// 16:9
    Wide,
    /// 1:1
    Square,
    /// 4:5
    Portrait,
}

impl Aspect {
    pub const ALL: [Aspect; 4] = [Aspect::Free, Aspect::Wide, Aspect::Square, Aspect::Portrait];

    pub fn label(self) -> &'static str {
        match self {
            Aspect::Free => "Free",
            Aspect::Wide => "16:9",
            Aspect::Square => "1:1",
            Aspect::Portrait => "4:5",
        }
    }

    /// Width / height, None for Free.
    pub fn ratio(self) -> Option<f32> {
        match self {
            Aspect::Free => None,
            Aspect::Wide => Some(16.0 / 9.0),
            Aspect::Square => Some(1.0),
            Aspect::Portrait => Some(4.0 / 5.0),
        }
    }
}

/// Built-in gradient presets (from, to), top-left to bottom-right.
pub const GRADIENTS: [(&str, &str); 6] = [
    ("#ff8a3d", "#c43d8f"),
    ("#4cc2ff", "#6a5cff"),
    ("#7fd36a", "#1f9e89"),
    ("#ffd166", "#ff5a52"),
    ("#2b2d42", "#5c677d"),
    ("#e0e7ff", "#fbcfe8"),
];

/// Built-in solid presets.
pub const SOLIDS: [&str; 6] = [
    "#ffffff", "#f1f1f4", "#1f1f23", "#000000", "#ff8a3d", "#4cc2ff",
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Beautify {
    /// Apply when copying, saving or pinning from the editor.
    pub enabled: bool,
    /// Padding on every side, px.
    pub padding: u32,
    pub background: BackgroundKind,
    /// Solid background, hex.
    pub color: String,
    /// Gradient stops, hex.
    pub gradient_from: String,
    pub gradient_to: String,
    /// Corner radius of the capture, px.
    pub corner_radius: f32,
    pub shadow: bool,
    pub aspect: Aspect,
}

impl Default for Beautify {
    fn default() -> Self {
        Self {
            enabled: false,
            padding: 64,
            background: BackgroundKind::Gradient,
            color: SOLIDS[0].into(),
            gradient_from: GRADIENTS[0].0.into(),
            gradient_to: GRADIENTS[0].1.into(),
            corner_radius: 12.0,
            shadow: true,
            aspect: Aspect::Free,
        }
    }
}

/// Canvas size and where the capture sits on it.
pub fn layout(w: u32, h: u32, b: &Beautify) -> (u32, u32, u32, u32) {
    let p = b.padding.min(4096);
    let (mut cw, mut ch) = (w + 2 * p, h + 2 * p);
    if let Some(r) = b.aspect.ratio() {
        if (cw as f32) / (ch as f32) < r {
            cw = (ch as f32 * r).round() as u32;
        } else {
            ch = (cw as f32 / r).round() as u32;
        }
    }
    let (x, y) = ((cw - w) / 2, (ch - h) / 2);
    (cw, ch, x, y)
}

fn hex(s: &str, fallback: [u8; 4]) -> [u8; 4] {
    parse_hex_color(s)
        .map(|c| [c[0], c[1], c[2], 255])
        .unwrap_or(fallback)
}

fn gradient(cw: u32, ch: u32, a: [u8; 4], b: [u8; 4]) -> Rgba {
    const STEPS: usize = 1024;
    let lut: Vec<[u8; 4]> = (0..STEPS)
        .map(|i| {
            let t = i as f32 / (STEPS - 1) as f32;
            let mut c = [255u8; 4];
            for k in 0..3 {
                c[k] = (a[k] as f32 + (b[k] as f32 - a[k] as f32) * t).round() as u8;
            }
            c
        })
        .collect();
    let mut out = Rgba::new(cw, ch);
    // Projection on the top-left -> bottom-right diagonal.
    let (fw, fh) = (cw.max(1) as f32, ch.max(1) as f32);
    let k = (STEPS - 1) as f32 / (fw * fw + fh * fh);
    for (y, row) in out.data.chunks_exact_mut(cw as usize * 4).enumerate() {
        let base = y as f32 * fh;
        for (x, p) in row.chunks_exact_mut(4).enumerate() {
            let i = (((x as f32 * fw + base) * k) as usize).min(STEPS - 1);
            p.copy_from_slice(&lut[i]);
        }
    }
    out
}

/// Bilinear upsampling coordinates for one axis: for each output index, the two source
/// indexes (edge-clamped) and the weight of the second, in 1/256.
fn axis_taps(out_len: u32, src_len: u32, scale: f32, offset: f32) -> Vec<(usize, usize, u32)> {
    let max = src_len.max(1) as i32 - 1;
    (0..out_len)
        .map(|o| {
            let s = (o as f32 + 0.5 + offset) / scale - 0.5;
            let i0 = s.floor() as i32;
            let f = ((s - i0 as f32) * 256.0).round() as u32;
            (
                i0.clamp(0, max) as usize,
                (i0 + 1).clamp(0, max) as usize,
                f.min(256),
            )
        })
        .collect()
}

/// Bilinear upscale of `src` into a `cw` x `ch` canvas (`scale` canvas px per source px,
/// canvas origin at `offset` source-scaled px), through `f` per output pixel.
fn upsample(
    src: &Rgba,
    cw: u32,
    ch: u32,
    scale: f32,
    offset: (f32, f32),
    mut f: impl FnMut(usize, [u8; 4]),
) {
    let xs = axis_taps(cw, src.width, scale, offset.0);
    let ys = axis_taps(ch, src.height, scale, offset.1);
    let sw = src.width as usize;
    let d = &src.data;
    for (y, &(y0, y1, fy)) in ys.iter().enumerate() {
        for (x, &(x0, x1, fx)) in xs.iter().enumerate() {
            let (a, b) = ((y0 * sw + x0) * 4, (y0 * sw + x1) * 4);
            let (c, e) = ((y1 * sw + x0) * 4, (y1 * sw + x1) * 4);
            let mut p = [0u8; 4];
            for k in 0..4 {
                let top = d[a + k] as u32 * (256 - fx) + d[b + k] as u32 * fx;
                let bot = d[c + k] as u32 * (256 - fx) + d[e + k] as u32 * fx;
                p[k] = ((top * (256 - fy) + bot * fy + (1 << 15)) >> 16) as u8;
            }
            f(y * cw as usize + x, p);
        }
    }
}

/// The capture scaled to cover the canvas and blurred: blurred small, then upscaled, so the
/// cost does not grow with the canvas.
fn blurred_backdrop(img: &Rgba, cw: u32, ch: u32) -> Rgba {
    const SMALL: u32 = 96;
    let mut small = img.thumbnail(SMALL, SMALL);
    // Opaque, so transparent corners of window captures do not show through.
    for p in small.data.chunks_exact_mut(4) {
        let a = p[3] as u32;
        for c in &mut p[..3] {
            *c = ((*c as u32 * a + 255 * (255 - a)) / 255) as u8;
        }
        p[3] = 255;
    }
    let (sw, sh) = (small.width as usize, small.height as usize);
    let mut f: Vec<f32> = small.data.iter().map(|v| *v as f32).collect();
    gaussian_blur(&mut f, sw, sh, 4, (sw.max(sh) as f32 / 24.0).max(1.5));
    for (d, s) in small.data.iter_mut().zip(&f) {
        *d = s.round().clamp(0.0, 255.0) as u8;
    }
    // Cover: scale so the small image fills the canvas, centred.
    let s = (cw as f32 / sw as f32).max(ch as f32 / sh as f32);
    let (ox, oy) = (
        (sw as f32 * s - cw as f32) / 2.0,
        (sh as f32 * s - ch as f32) / 2.0,
    );
    let mut out = Rgba::new(cw, ch);
    upsample(&small, cw, ch, s, (ox, oy), |i, p| {
        // Slightly darker so the capture stands out.
        out.data[i * 4..i * 4 + 4].copy_from_slice(&[
            (p[0] as u32 * 218 / 256) as u8,
            (p[1] as u32 * 218 / 256) as u8,
            (p[2] as u32 * 218 / 256) as u8,
            255,
        ]);
    });
    out
}

/// Black at `a` over `d` (straight alpha), in place.
fn darken(d: &mut [u8], a: u8) {
    let out = over([d[0], d[1], d[2], d[3]], [0, 0, 0, a]);
    d.copy_from_slice(&out);
}

/// Shadow under a rounded rect of `w` x `h` at (`x`, `y`): alpha mask computed at a quarter
/// resolution (it is smooth) and composited black, only where it can reach and not under
/// the opaque middle of the capture.
fn draw_shadow(
    canvas: &mut Rgba,
    rect: (u32, u32, u32, u32),
    radius: f32,
    padding: u32,
    opaque: bool,
) {
    const DOWN: u32 = 4;
    let (x, y, w, h) = rect;
    let blur = (padding as f32 * 0.45).clamp(6.0, 48.0);
    let offset = (blur / 3.0).round() as i32;
    let (mw, mh) = (canvas.width.div_ceil(DOWN), canvas.height.div_ceil(DOWN));
    let (mwu, mhu) = (mw as usize, mh as usize);
    let mut mask = vec![0f32; mwu * mhu];
    let mut sil = Rgba::filled(
        w.div_ceil(DOWN).max(1),
        h.div_ceil(DOWN).max(1),
        [0, 0, 0, 255],
    );
    round_corners(&mut sil, radius / DOWN as f32);
    let (sx, sy) = (
        (x / DOWN) as i32,
        ((y as i32 + offset).max(0) as u32 / DOWN) as i32,
    );
    for j in 0..sil.height as i32 {
        for i in 0..sil.width as i32 {
            let (mx, my) = (sx + i, sy + j);
            if mx >= 0 && my >= 0 && (mx as u32) < mw && (my as u32) < mh {
                mask[my as usize * mwu + mx as usize] =
                    sil.data[((j as u32 * sil.width + i as u32) * 4 + 3) as usize] as f32 / 255.0;
            }
        }
    }
    gaussian_blur(&mut mask, mwu, mhu, 1, blur / 2.0 / DOWN as f32);
    let alpha: Vec<u8> = mask
        .iter()
        .map(|a| (a * 0.38 * 255.0).round().clamp(0.0, 255.0) as u8)
        .collect();
    // Where the shadow can reach (3 sigma plus the offset and a mask cell).
    let reach = (blur * 1.5).ceil() as i64 + offset as i64 + DOWN as i64;
    let (cw, chh) = (canvas.width as i64, canvas.height as i64);
    let x0 = (x as i64 - reach).clamp(0, cw) as u32;
    let x1 = (x as i64 + w as i64 + reach).clamp(0, cw) as u32;
    let y0 = (y as i64 - reach).clamp(0, chh) as u32;
    let y1 = (y as i64 + h as i64 + reach).clamp(0, chh) as u32;
    // The capture's opaque middle hides whatever is under it.
    let ri = radius.ceil() as u32 + 1;
    let hidden = |px: u32, py: u32| {
        opaque && px >= x + ri && px + ri < x + w && py >= y + ri && py + ri < y + h
    };
    let xs = axis_taps(canvas.width, mw, DOWN as f32, 0.0);
    let ys = axis_taps(canvas.height, mh, DOWN as f32, 0.0);
    let stride = canvas.width as usize * 4;
    for py in y0..y1 {
        let (r0, r1, fy) = ys[py as usize];
        for px in x0..x1 {
            if hidden(px, py) {
                continue;
            }
            let (c0, c1, fx) = xs[px as usize];
            let top = alpha[r0 * mwu + c0] as u32 * (256 - fx) + alpha[r0 * mwu + c1] as u32 * fx;
            let bot = alpha[r1 * mwu + c0] as u32 * (256 - fx) + alpha[r1 * mwu + c1] as u32 * fx;
            let a = ((top * (256 - fy) + bot * fy + (1 << 15)) >> 16) as u8;
            if a > 0 {
                let i = py as usize * stride + px as usize * 4;
                darken(&mut canvas.data[i..i + 4], a);
            }
        }
    }
}

/// A cheap preview-sized copy (fits `max_w` x `max_h`, never upscales): each output pixel
/// averages a 3 x 3 grid of samples from its cell. Microseconds where `Rgba::thumbnail`
/// (Lanczos) takes tens of ms on a 4K capture, so the editor can refresh it on the UI thread.
pub fn preview_thumbnail(img: &Rgba, max_w: u32, max_h: u32) -> Rgba {
    if img.width == 0 || img.height == 0 {
        return img.clone();
    }
    let s = (max_w as f32 / img.width as f32)
        .min(max_h as f32 / img.height as f32)
        .min(1.0);
    let (w, h) = (
        ((img.width as f32 * s).round() as u32).max(1),
        ((img.height as f32 * s).round() as u32).max(1),
    );
    let (cx, cy) = (img.width as f32 / w as f32, img.height as f32 / h as f32);
    let mut out = Rgba::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0u32; 4];
            for j in 0..3 {
                for i in 0..3 {
                    let sx = ((x as f32 + (i as f32 + 0.5) / 3.0) * cx) as u32;
                    let sy = ((y as f32 + (j as f32 + 0.5) / 3.0) * cy) as u32;
                    let p = img.pixel(sx.min(img.width - 1), sy.min(img.height - 1));
                    for k in 0..4 {
                        acc[k] += p[k] as u32;
                    }
                }
            }
            out.set_pixel(x, y, acc.map(|v| ((v + 4) / 9) as u8));
        }
    }
    out
}

/// The framed image. Returns `img` unchanged when it is empty.
pub fn apply(img: &Rgba, b: &Beautify) -> Rgba {
    if img.width == 0 || img.height == 0 {
        return img.clone();
    }
    let (cw, ch, x, y) = layout(img.width, img.height, b);
    let mut canvas = match b.background {
        BackgroundKind::Solid => Rgba::filled(cw, ch, hex(&b.color, [255, 255, 255, 255])),
        BackgroundKind::Gradient => gradient(
            cw,
            ch,
            hex(&b.gradient_from, [0xff, 0x8a, 0x3d, 255]),
            hex(&b.gradient_to, [0xc4, 0x3d, 0x8f, 255]),
        ),
        BackgroundKind::Blurred => blurred_backdrop(img, cw, ch),
    };
    let radius = b.corner_radius.clamp(0.0, 256.0);
    let opaque = img.data.chunks_exact(4).all(|p| p[3] == 255);
    if b.shadow && b.padding > 0 {
        draw_shadow(
            &mut canvas,
            (x, y, img.width, img.height),
            radius,
            b.padding,
            opaque,
        );
    }
    let mut top = img.clone();
    round_corners(&mut top, radius);
    let (src_stride, dst_stride) = (top.width as usize * 4, cw as usize * 4);
    for (j, row) in top.data.chunks_exact(src_stride).enumerate() {
        let at = (y as usize + j) * dst_stride + x as usize * 4;
        let dst = &mut canvas.data[at..at + src_stride];
        if row.chunks_exact(4).all(|p| p[3] == 255) {
            dst.copy_from_slice(row);
            continue;
        }
        for (d, s) in dst.chunks_exact_mut(4).zip(row.chunks_exact(4)) {
            if s[3] == 0 {
                continue;
            }
            let out = over([d[0], d[1], d[2], d[3]], [s[0], s[1], s[2], s[3]]);
            d.copy_from_slice(&out);
        }
    }
    canvas
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: u32, h: u32) -> Rgba {
        Rgba::filled(w, h, [10, 200, 30, 255])
    }

    #[test]
    fn padding_and_aspect_layout() {
        let mut b = Beautify {
            padding: 20,
            ..Default::default()
        };
        assert_eq!(layout(100, 50, &b), (140, 90, 20, 20));
        b.aspect = Aspect::Wide;
        let (cw, ch, x, y) = layout(100, 50, &b);
        assert_eq!((cw, ch), (160, 90));
        assert_eq!((x, y), (30, 20));
        b.aspect = Aspect::Square;
        assert_eq!(layout(100, 50, &b), (140, 140, 20, 45));
        b.aspect = Aspect::Portrait;
        let (cw, ch, _, _) = layout(100, 50, &b);
        assert_eq!((cw, ch), (140, 175));
        // A tall capture in 16:9 grows sideways.
        b.aspect = Aspect::Wide;
        let (cw, ch, _, _) = layout(50, 300, &b);
        assert_eq!(ch, 340);
        assert_eq!(cw, (340.0f32 * 16.0 / 9.0).round() as u32);
    }

    #[test]
    fn solid_background_with_rounded_capture() {
        let b = Beautify {
            padding: 16,
            background: BackgroundKind::Solid,
            color: "#ffffff".into(),
            corner_radius: 8.0,
            shadow: false,
            ..Default::default()
        };
        let out = apply(&img(64, 32), &b);
        assert_eq!((out.width, out.height), (96, 64));
        assert_eq!(out.pixel(0, 0), [255, 255, 255, 255]);
        // Centre is the capture, the rounded corner shows the background.
        assert_eq!(out.pixel(48, 32), [10, 200, 30, 255]);
        assert_eq!(out.pixel(16, 16), [255, 255, 255, 255]);
        assert!(out.data.chunks_exact(4).all(|p| p[3] == 255));
    }

    #[test]
    fn gradient_runs_corner_to_corner() {
        let b = Beautify {
            padding: 30,
            background: BackgroundKind::Gradient,
            gradient_from: "#000000".into(),
            gradient_to: "#ffffff".into(),
            shadow: false,
            ..Default::default()
        };
        let out = apply(&img(10, 10), &b);
        let (w, h) = (out.width, out.height);
        assert!(out.pixel(0, 0)[0] < 10);
        assert!(out.pixel(w - 1, h - 1)[0] > 245);
        let mid = out.pixel(w - 1, 0)[0];
        assert!((100..=155).contains(&mid), "{mid}");
    }

    #[test]
    fn shadow_darkens_below_the_capture() {
        let mut b = Beautify {
            padding: 40,
            background: BackgroundKind::Solid,
            color: "#ffffff".into(),
            corner_radius: 0.0,
            shadow: false,
            ..Default::default()
        };
        let plain = apply(&img(100, 60), &b);
        b.shadow = true;
        let shadowed = apply(&img(100, 60), &b);
        // Just below the capture's bottom edge, centred.
        let (x, y) = (90, 40 + 60 + 4);
        assert_eq!(plain.pixel(x, y)[0], 255);
        assert!(shadowed.pixel(x, y)[0] < 245, "{:?}", shadowed.pixel(x, y));
        // Far corner stays clean.
        assert!(shadowed.pixel(0, 0)[0] > 250);
        // The capture itself is untouched.
        assert_eq!(shadowed.pixel(90, 70), [10, 200, 30, 255]);
    }

    #[test]
    fn blurred_background_takes_the_capture_colors() {
        let mut src = Rgba::filled(80, 40, [200, 20, 20, 255]);
        for y in 0..40 {
            for x in 40..80 {
                src.set_pixel(x, y, [20, 20, 200, 255]);
            }
        }
        let b = Beautify {
            padding: 30,
            background: BackgroundKind::Blurred,
            shadow: false,
            corner_radius: 0.0,
            ..Default::default()
        };
        let out = apply(&src, &b);
        let left = out.pixel(2, out.height / 2);
        let right = out.pixel(out.width - 3, out.height / 2);
        assert!(
            left[0] > left[2] && right[2] > right[0],
            "{left:?} {right:?}"
        );
        assert!(out.data.chunks_exact(4).all(|p| p[3] == 255));
    }

    #[test]
    fn config_serde_defaults() {
        let b: Beautify = toml::from_str("padding = 10\naspect = \"square\"").unwrap();
        assert_eq!(b.padding, 10);
        assert_eq!(b.aspect, Aspect::Square);
        assert_eq!(b.background, BackgroundKind::Gradient);
        let s = toml::to_string(&Beautify::default()).unwrap();
        assert_eq!(toml::from_str::<Beautify>(&s).unwrap(), Beautify::default());
    }

    #[test]
    fn preview_thumbnail_fits_and_averages() {
        let mut src = Rgba::filled(400, 200, [0, 0, 0, 255]);
        for y in 0..200 {
            for x in (0..400).step_by(2) {
                src.set_pixel(x, y, [255, 255, 255, 255]);
            }
        }
        let t = preview_thumbnail(&src, 100, 100);
        assert_eq!((t.width, t.height), (100, 50));
        // Alternating columns average to mid grey, not aliased black or white.
        let v = t.pixel(50, 25)[0];
        assert!((60..=200).contains(&v), "{v}");
        // Never upscales.
        let small = preview_thumbnail(&img(20, 10), 100, 100);
        assert_eq!((small.width, small.height), (20, 10));
    }

    #[test]
    fn empty_image_is_returned_as_is() {
        let e = Rgba::new(0, 0);
        assert_eq!(apply(&e, &Beautify::default()).width, 0);
    }

    /// `cargo test -p stinkshot-core beautify_perf -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn beautify_perf() {
        for (w, h) in [(1920, 1080), (3840, 2160)] {
            let src = img(w, h);
            for kind in [
                BackgroundKind::Solid,
                BackgroundKind::Gradient,
                BackgroundKind::Blurred,
            ] {
                let b = Beautify {
                    background: kind,
                    ..Default::default()
                };
                let t = std::time::Instant::now();
                let out = apply(&src, &b);
                println!(
                    "{w}x{h} {kind:?}: {:.1} ms -> {}x{}",
                    t.elapsed().as_secs_f64() * 1000.0,
                    out.width,
                    out.height
                );
            }
        }
    }
}
