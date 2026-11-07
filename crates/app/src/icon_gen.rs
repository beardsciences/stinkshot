//! Application logo bitmaps: the glossy green gel orb from `stinkshot-logo`, rendered once
//! by build.rs (which also writes the exe .ico) and embedded here at the tray sizes
//! (16/20/24/32 px for DPI) and the 64 px window icon. See docs/notes/logo.md.
//!
//! Everything at runtime is cheap 2D work on those bitmaps: the tray's capture "squish and
//! pulse" (resampled about the base, brightened toward the hot core colour), recolouring
//! for a custom accent (hue shift) and the red recording badge.

/// Default accent; the embedded bitmaps are rendered in it.
pub const DEFAULT_ACCENT: [u8; 3] = [0x7f, 0xd3, 0x6a];

/// Pre-rendered static logo bitmaps (straight-alpha RGBA8) from build.rs.
const EMBEDDED: [(u32, &[u8]); 5] = [
    (
        16,
        include_bytes!(concat!(env!("OUT_DIR"), "/logo-16.rgba")),
    ),
    (
        20,
        include_bytes!(concat!(env!("OUT_DIR"), "/logo-20.rgba")),
    ),
    (
        24,
        include_bytes!(concat!(env!("OUT_DIR"), "/logo-24.rgba")),
    ),
    (
        32,
        include_bytes!(concat!(env!("OUT_DIR"), "/logo-32.rgba")),
    ),
    (
        64,
        include_bytes!(concat!(env!("OUT_DIR"), "/logo-64.rgba")),
    ),
];

/// How the orb is posed in one frame. `REST` is the static logo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    /// Horizontal / vertical scale about the orb's base.
    pub scale_x: f32,
    pub scale_y: f32,
    /// 0..1 inner-glow pulse (brightens toward the hot core colour).
    pub glow: f32,
    /// Red recording dot in the lower right.
    pub recording: bool,
}

impl Pose {
    pub const REST: Pose = Pose {
        scale_x: 1.0,
        scale_y: 1.0,
        glow: 0.0,
        recording: false,
    };
}

/// Duration of one frame of the capture animation.
pub const ANIM_FRAME_MS: u64 = 60;

/// Capture-complete animation: a quick squish, a pop with a bright pulse from the core,
/// then a soft wobble back to rest (7 x 60 ms). The orb fills the canvas, so poses never
/// grow past it: the squish widens a little while it flattens, the pop narrows.
pub const ANIM: [Pose; 7] = [
    anim(1.03, 0.88, 0.30),
    anim(0.93, 0.99, 0.75),
    anim(0.97, 1.0, 0.60),
    anim(1.02, 0.95, 0.40),
    anim(0.99, 1.0, 0.24),
    anim(1.0, 0.985, 0.12),
    anim(1.0, 1.0, 0.04),
];

const fn anim(scale_x: f32, scale_y: f32, glow: f32) -> Pose {
    Pose {
        scale_x,
        scale_y,
        glow,
        recording: false,
    }
}

/// Straight-alpha RGBA8 static logo, `size * size * 4` bytes.
pub fn render_rgba(size: u32, accent: [u8; 3]) -> Vec<u8> {
    render_pose(size, accent, Pose::REST)
}

/// The static orb at `size`: embedded when available, otherwise rendered (slow-ish, only
/// for sizes nothing ships with).
fn base(size: u32) -> Vec<u8> {
    EMBEDDED
        .iter()
        .find(|(s, _)| *s == size)
        .map(|(_, px)| px.to_vec())
        .unwrap_or_else(|| stinkshot_logo::render_icon(size))
}

/// Straight-alpha RGBA8 logo in the given pose.
pub fn render_pose(size: u32, accent: [u8; 3], pose: Pose) -> Vec<u8> {
    let mut img = base(size);
    if pose.scale_x != 1.0 || pose.scale_y != 1.0 {
        img = transform(&img, size, pose);
    }
    if pose.glow > 0.0 {
        pulse(&mut img, size, pose.glow);
    }
    if accent != DEFAULT_ACCENT {
        recolor(&mut img, accent);
    }
    if pose.recording {
        badge(&mut img, size);
    }
    img
}

/// Bilinear resample about the base of the orb (premultiplied, so edges stay clean).
fn transform(src: &[u8], size: u32, pose: Pose) -> Vec<u8> {
    let n = size as usize;
    let s = size as f32;
    let (ax, ay) = (0.5 * s, 0.9 * s);
    let fetch = |x: i32, y: i32| -> [f32; 4] {
        if x < 0 || y < 0 || x >= n as i32 || y >= n as i32 {
            return [0.0; 4];
        }
        let i = (y as usize * n + x as usize) * 4;
        let a = src[i + 3] as f32 / 255.0;
        [
            src[i] as f32 * a,
            src[i + 1] as f32 * a,
            src[i + 2] as f32 * a,
            a,
        ]
    };
    let mut out = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            // Output pixel centre back into the source.
            let fx = ax + (x as f32 + 0.5 - ax) / pose.scale_x - 0.5;
            let fy = ay + (y as f32 + 0.5 - ay) / pose.scale_y - 0.5;
            let (x0, y0) = (fx.floor(), fy.floor());
            let (tx, ty) = (fx - x0, fy - y0);
            let (x0, y0) = (x0 as i32, y0 as i32);
            let mut acc = [0.0f32; 4];
            for (dx, dy, w) in [
                (0, 0, (1.0 - tx) * (1.0 - ty)),
                (1, 0, tx * (1.0 - ty)),
                (0, 1, (1.0 - tx) * ty),
                (1, 1, tx * ty),
            ] {
                let p = fetch(x0 + dx, y0 + dy);
                for c in 0..4 {
                    acc[c] += p[c] * w;
                }
            }
            if acc[3] <= 0.0 {
                continue;
            }
            let i = (y * n + x) * 4;
            for c in 0..3 {
                out[i + c] = (acc[c] / acc[3]).round().clamp(0.0, 255.0) as u8;
            }
            out[i + 3] = (acc[3] * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// Brightens the body toward the hot core colour (#d4ff86), strongest in the middle.
fn pulse(img: &mut [u8], size: u32, glow: f32) {
    let n = size as usize;
    let hot = [0xd4 as f32, 0xff as f32, 0x86 as f32];
    let c = n as f32 * 0.5;
    for y in 0..n {
        for x in 0..n {
            let i = (y * n + x) * 4;
            if img[i + 3] == 0 {
                continue;
            }
            let d = ((x as f32 + 0.5 - c).powi(2) + (y as f32 + 0.5 - c * 1.05).powi(2)).sqrt()
                / (n as f32 * 0.5);
            let w = (glow * (1.0 - d * 0.8)).clamp(0.0, 1.0) * 0.6;
            for k in 0..3 {
                let v = img[i + k] as f32;
                img[i + k] = (v + (hot[k].max(v) - v) * w).round() as u8;
            }
        }
    }
}

/// Shifts hue (and scales saturation) from the default accent to `accent`; lightness, and
/// with it the glossy shading, is kept.
fn recolor(img: &mut [u8], accent: [u8; 3]) {
    let (h0, s0, _) = hsl(DEFAULT_ACCENT.map(|c| c as f32 / 255.0));
    let (h1, s1, _) = hsl(accent.map(|c| c as f32 / 255.0));
    let dh = h1 - h0;
    let ks = if s0 > 0.0 { s1 / s0 } else { 1.0 };
    for p in img.chunks_exact_mut(4) {
        if p[3] == 0 {
            continue;
        }
        let (h, s, l) = hsl([p[0], p[1], p[2]].map(|c| c as f32 / 255.0));
        let rgb = from_hsl((h + dh).rem_euclid(1.0), (s * ks).clamp(0.0, 1.0), l);
        for k in 0..3 {
            p[k] = (rgb[k] * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
}

fn hsl(c: [f32; 3]) -> (f32, f32, f32) {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let l = (max + min) * 0.5;
    let d = max - min;
    if d <= 1e-6 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs()).max(1e-6);
    let h = if max == c[0] {
        ((c[1] - c[2]) / d).rem_euclid(6.0)
    } else if max == c[1] {
        (c[2] - c[0]) / d + 2.0
    } else {
        (c[0] - c[1]) / d + 4.0
    };
    (h / 6.0, s.min(1.0), l)
}

fn from_hsl(h: f32, s: f32, l: f32) -> [f32; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h * 6.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c * 0.5;
    [r + m, g + m, b + m]
}

/// Red recording dot with a white ring in the lower right, supersampled 4x4.
fn badge(img: &mut [u8], size: u32) {
    let n = size as usize;
    let s = size as f32;
    // Stays inside the canvas: centre + radius + ring <= 1.
    let (cx, cy, r) = if size <= 24 {
        (0.74, 0.74, 0.2)
    } else {
        (0.78, 0.78, 0.17)
    };
    let ring = (1.0 / s).max(0.035);
    const SS: usize = 4;
    for y in 0..n {
        for x in 0..n {
            let mut acc = [0.0f32; 4];
            for sy in 0..SS {
                for sx in 0..SS {
                    let px = (x as f32 + (sx as f32 + 0.5) / SS as f32) / s;
                    let py = (y as f32 + (sy as f32 + 0.5) / SS as f32) / s;
                    let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
                    let c = if d < r {
                        // Slight top-lit gloss on the dot.
                        let t = ((py - (cy - r)) / (2.0 * r)).clamp(0.0, 1.0);
                        [1.0 - 0.14 * t, 0.42 - 0.27 * t, 0.42 - 0.24 * t]
                    } else if d < r + ring {
                        [1.0, 1.0, 1.0]
                    } else {
                        continue;
                    };
                    for k in 0..3 {
                        acc[k] += c[k] * 255.0;
                    }
                    acc[3] += 1.0;
                }
            }
            if acc[3] == 0.0 {
                continue;
            }
            let a = acc[3] / (SS * SS) as f32;
            let i = (y * n + x) * 4;
            let da = img[i + 3] as f32 / 255.0;
            let oa = a + da * (1.0 - a);
            for k in 0..3 {
                let src = acc[k] / acc[3];
                let v = (src * a + img[i + k] as f32 * da * (1.0 - a)) / oa.max(1e-6);
                img[i + k] = v.round().clamp(0.0, 255.0) as u8;
            }
            img[i + 3] = (oa * 255.0).round() as u8;
        }
    }
}

/// Smallest tray size that is not blurry at the given system DPI.
pub fn tray_size_for_dpi(dpi: u32) -> u32 {
    let want = (16 * dpi.max(96)).div_ceil(96);
    [16u32, 20, 24, 32]
        .into_iter()
        .find(|s| *s >= want)
        .unwrap_or(32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(img: &[u8], size: usize, x: usize, y: usize) -> [u8; 4] {
        let i = (y * size + x) * 4;
        [img[i], img[i + 1], img[i + 2], img[i + 3]]
    }

    #[test]
    fn embedded_bitmaps_are_green_orbs() {
        for (size, data) in EMBEDDED {
            let n = size as usize;
            assert_eq!(data.len(), n * n * 4, "{size}");
            assert_eq!(px(data, n, 0, 0)[3], 0, "{size}");
            assert_eq!(px(data, n, n - 1, 0)[3], 0, "{size}");
            let c = px(data, n, n / 2, n / 2);
            assert!(c[3] >= 250 && c[1] > c[0] && c[1] > c[2], "{size}: {c:?}");
        }
    }

    #[test]
    fn embedded_bitmaps_match_the_renderer() {
        // build.rs output and the shared renderer agree (the bitmaps are not stale).
        assert_eq!(
            render_rgba(16, DEFAULT_ACCENT),
            stinkshot_logo::render_icon(16)
        );
    }

    #[test]
    fn recording_badge_is_red() {
        let pose = Pose {
            recording: true,
            ..Pose::REST
        };
        let img = render_pose(32, DEFAULT_ACCENT, pose);
        let p = px(&img, 32, 26, 26);
        assert!(p[0] > 180 && p[1] < 120 && p[3] == 255, "{p:?}");
    }

    #[test]
    fn animation_squishes_pulses_and_settles() {
        let rest = render_rgba(20, DEFAULT_ACCENT);
        let squash = render_pose(20, DEFAULT_ACCENT, ANIM[0]);
        assert_ne!(rest, squash);
        // The pulse frame's body is brighter on average.
        let luma = |img: &[u8]| -> f32 {
            let body: Vec<&[u8]> = img.chunks(4).filter(|p| p[3] > 200).collect();
            body.iter()
                .map(|p| p[0] as f32 + p[1] as f32 + p[2] as f32)
                .sum::<f32>()
                / body.len() as f32
        };
        let pop = render_pose(20, DEFAULT_ACCENT, ANIM[1]);
        assert!(luma(&pop) > luma(&rest));
        let last = ANIM[ANIM.len() - 1];
        assert!((last.scale_x - 1.0).abs() < 0.01 && last.glow < 0.1);
    }

    #[test]
    fn custom_accent_recolors() {
        let accent = [0x4c, 0xc2, 0xff];
        let blue = render_rgba(24, accent);
        // The body's mean colour lands near the accent's hue.
        let body: Vec<&[u8]> = blue.chunks(4).filter(|p| p[3] > 200).collect();
        let mean: [f32; 3] = std::array::from_fn(|k| {
            body.iter().map(|p| p[k] as f32 / 255.0).sum::<f32>() / body.len() as f32
        });
        let (h, _, _) = hsl(mean);
        let (ha, _, _) = hsl(accent.map(|c| c as f32 / 255.0));
        let d = (h - ha).abs().min(1.0 - (h - ha).abs());
        assert!(d < 0.08, "hue {h} vs accent {ha}");
        // Alpha is untouched.
        let green = render_rgba(24, DEFAULT_ACCENT);
        assert!(
            blue.chunks(4)
                .zip(green.chunks(4))
                .all(|(a, b)| a[3] == b[3])
        );
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
