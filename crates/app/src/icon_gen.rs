//! Procedural application logo: a glossy 3D green "puff" (a little cloud built from
//! overlapping spheres) with two motion puffs trailing to the left. See docs/notes/logo.md.
//!
//! Pure std so `build.rs` can include it (for the exe .ico), the tray can redraw it at
//! runtime (16/20/24/32 px depending on DPI, plus a short bounce animation and a recording
//! badge) and `examples/logo.rs` can export PNGs.
//!
//! Rendering is a tiny analytic ray tracer: every sphere has a closed-form front height, the
//! cloud surface is a soft maximum of those heights, and its normal is the matching blend of
//! sphere normals. Shading: wrapped key light from the top left, lighter facing-the-viewer
//! core, crease occlusion where lobes meet, crisp Blinn specular, mint rim light from the
//! lower right, soft contact shadow and a thin dark outline for small sizes. Each pixel is
//! supersampled.

/// Sizes embedded in the exe icon.
pub const ICO_SIZES: [u32; 8] = [16, 20, 24, 32, 48, 64, 128, 256];

/// How the cloud is posed in one frame. `REST` is the static logo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    /// Horizontal / vertical scale of the main cloud about its base.
    pub scale_x: f32,
    pub scale_y: f32,
    /// Upward hop, as a fraction of the icon size.
    pub lift: f32,
    /// How far the motion puffs have drifted left (fraction of the icon size); they fade
    /// as they drift.
    pub drift: f32,
    /// Red recording dot in the lower right.
    pub recording: bool,
}

impl Pose {
    pub const REST: Pose = Pose {
        scale_x: 1.0,
        scale_y: 1.0,
        lift: 0.0,
        drift: 0.0,
        recording: false,
    };
}

/// Duration of one frame of the capture animation.
#[allow(dead_code)] // used by the tray, not by build.rs
pub const ANIM_FRAME_MS: u64 = 60;

/// Capture-complete animation: squash, hop with a stretch, land, settle (7 x 60 ms).
#[allow(dead_code)] // used by the tray, not by build.rs
pub const ANIM: [Pose; 7] = [
    anim(1.12, 0.86, 0.0, 0.00),
    anim(0.93, 1.09, 0.05, 0.03),
    anim(0.96, 1.05, 0.07, 0.06),
    anim(1.00, 1.00, 0.03, 0.09),
    anim(1.08, 0.91, 0.0, 0.12),
    anim(0.97, 1.03, 0.0, 0.15),
    anim(1.01, 0.99, 0.0, 0.18),
];

const fn anim(scale_x: f32, scale_y: f32, lift: f32, drift: f32) -> Pose {
    Pose {
        scale_x,
        scale_y,
        lift,
        drift,
        recording: false,
    }
}

/// Straight-alpha RGBA8 static logo, `size * size * 4` bytes.
pub fn render_rgba(size: u32, accent: [u8; 3]) -> Vec<u8> {
    render_pose(size, accent, Pose::REST)
}

/// One lobe of the cloud: an ellipsoid facing the viewer (a sphere when `rx == ry == rz`).
#[derive(Clone, Copy)]
struct Ball {
    x: f32,
    y: f32,
    rx: f32,
    ry: f32,
    rz: f32,
    /// Specular strength: the big top lobe carries the hero highlight.
    gloss: f32,
}

const fn ball(x: f32, y: f32, r: f32, gloss: f32) -> Ball {
    Ball {
        x,
        y,
        rx: r,
        ry: r,
        rz: r,
        gloss,
    }
}

/// A wide, shallow ellipsoid that gives the cloud its flatter base.
const fn base(x: f32, y: f32, rx: f32, ry: f32, rz: f32) -> Ball {
    Ball {
        x,
        y,
        rx,
        ry,
        rz,
        gloss: 0.3,
    }
}

/// Level of detail: tray sizes get a bigger, simpler silhouette with no small parts.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Detail {
    /// 16-24 px: fat silhouette, no motion puffs, no shadow, outline.
    Tiny,
    /// 32-48 px: cloud plus one motion puff.
    Small,
    /// 64 px and up: everything.
    Full,
}

fn detail_for(size: u32) -> Detail {
    match size {
        0..=24 => Detail::Tiny,
        25..=48 => Detail::Small,
        _ => Detail::Full,
    }
}

const CLOUD_TINY: [Ball; 4] = [
    base(0.50, 0.69, 0.41, 0.17, 0.12),
    ball(0.27, 0.57, 0.19, 0.45),
    ball(0.52, 0.40, 0.255, 0.8),
    ball(0.77, 0.585, 0.18, 0.45),
];
const CLOUD_SMALL: [Ball; 4] = [
    base(0.585, 0.67, 0.33, 0.13, 0.10),
    ball(0.385, 0.57, 0.17, 0.45),
    ball(0.60, 0.43, 0.225, 1.0),
    ball(0.80, 0.59, 0.155, 0.45),
];
const CLOUD_FULL: [Ball; 4] = [
    base(0.59, 0.655, 0.32, 0.125, 0.105),
    ball(0.375, 0.545, 0.17, 0.45),
    ball(0.59, 0.405, 0.225, 1.0),
    ball(0.81, 0.56, 0.15, 0.45),
];

struct Puff {
    b: Ball,
    alpha: f32,
}

struct Layout {
    cloud: &'static [Ball],
    puffs: Vec<Puff>,
    shadow: bool,
    /// Anchor of the squash/stretch (base of the cloud).
    anchor: (f32, f32),
    /// Outline width in pixels and opacity.
    outline_px: f32,
    outline_alpha: f32,
    /// Soft-max sharpness in unit coordinates: larger blends the lobes more.
    k: f32,
    /// Overall specular strength.
    spec: f32,
}

fn layout(d: Detail) -> Layout {
    match d {
        Detail::Tiny => Layout {
            cloud: &CLOUD_TINY,
            puffs: Vec::new(),
            shadow: false,
            anchor: (0.52, 0.86),
            outline_px: 1.0,
            outline_alpha: 0.85,
            k: 0.05,
            spec: 0.7,
        },
        Detail::Small => Layout {
            cloud: &CLOUD_SMALL,
            puffs: vec![Puff {
                b: ball(0.115, 0.715, 0.075, 0.5),
                alpha: 0.95,
            }],
            shadow: false,
            anchor: (0.585, 0.81),
            outline_px: 1.0,
            outline_alpha: 0.75,
            k: 0.045,
            spec: 0.9,
        },
        Detail::Full => Layout {
            cloud: &CLOUD_FULL,
            puffs: vec![
                Puff {
                    b: ball(0.16, 0.675, 0.072, 0.5),
                    alpha: 0.95,
                },
                Puff {
                    b: ball(0.06, 0.74, 0.04, 0.5),
                    alpha: 0.75,
                },
            ],
            shadow: true,
            anchor: (0.59, 0.785),
            outline_px: 0.0,
            outline_alpha: 0.0,
            k: 0.045,
            spec: 1.0,
        },
    }
}

type Rgb = [f32; 3];

struct Palette {
    deeper: Rgb,
    shadow: Rgb,
    base: Rgb,
    light: Rgb,
    rim: Rgb,
    outline: Rgb,
}

impl Palette {
    /// Derived from the accent so a custom accent recolors the logo. For the default
    /// #7fd36a: shadow is #3f8f3a and light is about #a6e596.
    fn new(accent: [u8; 3]) -> Palette {
        let a = [
            accent[0] as f32 / 255.0,
            accent[1] as f32 / 255.0,
            accent[2] as f32 / 255.0,
        ];
        let mul = |m: Rgb| [a[0] * m[0], a[1] * m[1], a[2] * m[2]];
        Palette {
            deeper: mul([0.30, 0.47, 0.38]),
            shadow: mul([0.50, 0.68, 0.55]),
            base: a,
            light: mix(a, [1.0, 1.0, 0.94], 0.36),
            rim: mix(a, [0.85, 1.0, 0.95], 0.65),
            outline: mul([0.24, 0.38, 0.30]),
        }
    }
}

/// Straight-alpha RGBA8 logo in the given pose.
pub fn render_pose(size: u32, accent: [u8; 3], pose: Pose) -> Vec<u8> {
    let n = size as usize;
    let mut out = vec![0u8; n * n * 4];
    let pal = Palette::new(accent);
    let lay = layout(detail_for(size));
    let ss: usize = match size {
        0..=32 => 8,
        33..=64 => 5,
        _ => 3,
    };
    let inv = 1.0 / size as f32;
    let outline_w = lay.outline_px * inv;
    for y in 0..n {
        for x in 0..n {
            let mut acc = [0.0f32; 4];
            for sy in 0..ss {
                for sx in 0..ss {
                    let px = (x as f32 + (sx as f32 + 0.5) / ss as f32) * inv;
                    let py = (y as f32 + (sy as f32 + 0.5) / ss as f32) * inv;
                    let s = sample(px, py, &lay, &pal, pose, outline_w, size);
                    for c in 0..4 {
                        acc[c] += s[c];
                    }
                }
            }
            let total = (ss * ss) as f32;
            let a = acc[3] / total;
            if a <= 0.0 {
                continue;
            }
            let i = (y * n + x) * 4;
            for c in 0..3 {
                let v = (acc[c] / total) / a;
                out[i + c] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
            out[i + 3] = (a.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    out
}

/// Premultiplied RGBA of one sub-sample.
fn sample(
    px: f32,
    py: f32,
    lay: &Layout,
    pal: &Palette,
    pose: Pose,
    outline_w: f32,
    size: u32,
) -> [f32; 4] {
    let mut acc = [0.0f32; 4];

    if lay.shadow {
        let spread = (1.0 - pose.lift * 2.5).max(0.3);
        let (cx, cy) = (lay.anchor.0, 0.825);
        let (rx, ry) = (0.30 * spread * pose.scale_x, 0.045 * spread);
        let e = ((px - cx) / rx).powi(2) + ((py - cy) / ry).powi(2);
        let a = 0.34 * (-2.2 * e).exp() * spread;
        if a > 0.002 {
            over(&mut acc, pal.deeper, a);
        }
    }

    for p in &lay.puffs {
        // Puffs rise up and to the left, grow a little and fade as they dissipate.
        let fade = (1.0 - pose.drift * 7.0).clamp(0.0, 1.0);
        if fade <= 0.0 {
            continue;
        }
        let grow = 1.0 + pose.drift * 1.5;
        let b = Ball {
            x: p.b.x - pose.drift * 0.35,
            y: p.b.y - pose.drift,
            rx: p.b.rx * grow,
            ry: p.b.ry * grow,
            rz: p.b.rz * grow,
            ..p.b
        };
        blob(
            &mut acc,
            px,
            py,
            std::slice::from_ref(&b),
            lay,
            pal,
            p.alpha * fade,
            outline_w,
        );
    }

    // Main cloud: squash/stretch about its base, hop upward.
    let (ax, ay) = lay.anchor;
    let lx = ax + (px - ax) / pose.scale_x;
    let ly = ay + (py + pose.lift - ay) / pose.scale_y;
    blob(&mut acc, lx, ly, lay.cloud, lay, pal, 1.0, outline_w);

    if pose.recording {
        badge(&mut acc, px, py, size);
    }
    acc
}

#[allow(clippy::too_many_arguments)]
fn blob(
    acc: &mut [f32; 4],
    px: f32,
    py: f32,
    balls: &[Ball],
    lay: &Layout,
    pal: &Palette,
    alpha: f32,
    outline_w: f32,
) {
    match surface(balls, px, py, lay.k) {
        Some(s) => over(acc, shade(&s, pal, lay.spec), alpha),
        None => {
            if outline_w > 0.0 {
                let d = balls
                    .iter()
                    .map(|b| {
                        let (u, v) = ((px - b.x) / b.rx, (py - b.y) / b.ry);
                        ((u * u + v * v).sqrt() - 1.0) * b.rx.min(b.ry)
                    })
                    .fold(f32::MAX, f32::min);
                if d < outline_w {
                    over(acc, pal.outline, lay.outline_alpha * alpha);
                }
            }
        }
    }
}

struct Surf {
    n: [f32; 3],
    /// 0 where one lobe dominates, 1 in the crease where two meet at equal height.
    crease: f32,
    gloss: f32,
}

/// Front height of an ellipsoid lobe at a point, if the point is inside its outline.
fn height(b: &Ball, px: f32, py: f32) -> Option<f32> {
    let (u, v) = ((px - b.x) / b.rx, (py - b.y) / b.ry);
    let q = 1.0 - u * u - v * v;
    (q > 0.0).then(|| b.rz * q.sqrt())
}

fn surface(balls: &[Ball], px: f32, py: f32, k: f32) -> Option<Surf> {
    let m = balls
        .iter()
        .filter_map(|b| height(b, px, py))
        .fold(f32::MIN, f32::max);
    if m == f32::MIN {
        return None;
    }
    // Soft maximum of the lobe heights; the normal and gloss blend with the same weights.
    let (mut sum, mut gloss) = (0.0, 0.0);
    let mut n = [0.0f32; 3];
    for b in balls {
        let Some(h) = height(b, px, py) else {
            continue;
        };
        let w = ((h - m) / k).exp();
        let bn = normalize([
            (px - b.x) / (b.rx * b.rx),
            (py - b.y) / (b.ry * b.ry),
            h / (b.rz * b.rz),
        ]);
        sum += w;
        gloss += w * b.gloss;
        for i in 0..3 {
            n[i] += w * bn[i];
        }
    }
    Some(Surf {
        n: normalize(n),
        crease: (sum.ln() / std::f32::consts::LN_2).clamp(0.0, 1.0),
        gloss: gloss / sum,
    })
}

fn shade(s: &Surf, pal: &Palette, spec: f32) -> Rgb {
    let n = s.n;
    // Key light from the top left, a bit toward the viewer (y points down).
    let l = normalize([-0.5, -0.8, 0.75]);
    let ndl = dot(n, l);
    let d = ((ndl + 0.45) / 1.45).clamp(0.0, 1.0);
    let mut c = if d < 0.35 {
        mix(pal.deeper, pal.shadow, d / 0.35)
    } else if d < 0.72 {
        mix(pal.shadow, pal.base, (d - 0.35) / 0.37)
    } else {
        mix(pal.base, pal.light, (d - 0.72) / 0.28)
    };
    // Subsurface-ish glow where the surface faces the viewer.
    c = mix(c, pal.light, 0.22 * smoothstep(0.55, 1.0, n[2]));
    // Crease occlusion where lobes meet.
    c = scale(c, 1.0 - 0.16 * s.crease);
    // Rim light from the lower right.
    let side = (n[0] * 0.6 + n[1] * 0.8).max(0.0);
    let rim = (1.0 - n[2]).powf(2.2) * side;
    c = mix(c, pal.rim, (0.55 * rim).min(0.6));
    // Crisp specular, strongest on the hero lobe.
    let h = normalize([l[0], l[1], l[2] + 1.0]);
    let sp = dot(n, h).max(0.0).powf(40.0);
    let sheen = dot(n, h).max(0.0).powf(6.0) * 0.12;
    let crisp = (smoothstep(0.45, 0.7, sp) * 0.85 + sp * 0.15 + sheen) * s.gloss * spec;
    mix(c, [1.0, 1.0, 1.0], crisp.min(1.0))
}

fn badge(acc: &mut [f32; 4], px: f32, py: f32, size: u32) {
    // Stays inside the canvas: center + radius + ring <= 1.
    let (cx, cy, r) = if size <= 24 {
        (0.74, 0.74, 0.2)
    } else {
        (0.78, 0.78, 0.17)
    };
    let ring = (1.0 / size as f32).max(0.035);
    let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
    if d < r + ring {
        over(acc, [1.0, 1.0, 1.0], 1.0);
    }
    if d < r {
        // Slight top-lit gloss on the dot.
        let t = ((py - (cy - r)) / (2.0 * r)).clamp(0.0, 1.0);
        let red = mix([1.0, 0.42, 0.42], [0.86, 0.15, 0.18], t);
        over(acc, red, 1.0);
    }
}

fn over(acc: &mut [f32; 4], c: Rgb, a: f32) {
    let a = a.clamp(0.0, 1.0);
    for i in 0..3 {
        acc[i] = c[i] * a + acc[i] * (1.0 - a);
    }
    acc[3] = a + acc[3] * (1.0 - a);
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn scale(a: Rgb, s: f32) -> Rgb {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt().max(1e-6);
    [a[0] / l, a[1] / l, a[2] / l]
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
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
    let sizes = ICO_SIZES;
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

    const ACCENT: [u8; 3] = [0x7f, 0xd3, 0x6a];

    fn px(img: &[u8], size: usize, x: usize, y: usize) -> [u8; 4] {
        let i = (y * size + x) * 4;
        [img[i], img[i + 1], img[i + 2], img[i + 3]]
    }

    #[test]
    fn logo_has_expected_geometry() {
        for size in ICO_SIZES {
            let img = render_rgba(size, ACCENT);
            let n = size as usize;
            assert_eq!(img.len(), n * n * 4);
            // Corners are transparent, the cloud body is opaque and green.
            assert_eq!(px(&img, n, 0, 0)[3], 0, "{size}");
            assert_eq!(px(&img, n, n - 1, 0)[3], 0, "{size}");
            let c = px(&img, n, n * 55 / 100, n * 58 / 100);
            assert_eq!(c[3], 255, "{size}");
            assert!(c[1] > c[0] && c[1] > c[2], "{size}: {c:?}");
        }
    }

    #[test]
    fn logo_is_lit_from_the_top_left() {
        let n = 256usize;
        let img = render_rgba(256, ACCENT);
        let luma = |p: [u8; 4]| p[0] as u32 + p[1] as u32 + p[2] as u32;
        // Upper-left of the big lobe is brighter than its lower-right underside.
        let top = px(&img, n, 120, 80);
        let under = px(&img, n, 185, 190);
        assert_eq!(top[3], 255);
        assert_eq!(under[3], 255);
        assert!(luma(top) > luma(under) + 60, "{top:?} vs {under:?}");
    }

    #[test]
    fn tray_sizes_cover_most_of_the_canvas() {
        // Small sizes use the fat silhouette: at least 45% of pixels mostly opaque.
        for size in [16u32, 20, 24] {
            let img = render_rgba(size, ACCENT);
            let solid = img.chunks(4).filter(|p| p[3] > 200).count();
            assert!(
                solid * 100 >= (size * size) as usize * 45,
                "{size}: {solid}"
            );
        }
    }

    #[test]
    fn recording_badge_is_red() {
        let pose = Pose {
            recording: true,
            ..Pose::REST
        };
        let img = render_pose(32, ACCENT, pose);
        let p = px(&img, 32, 26, 26);
        assert!(p[0] > 180 && p[1] < 120 && p[3] == 255, "{p:?}");
    }

    #[test]
    fn animation_changes_and_settles() {
        let rest = render_rgba(20, ACCENT);
        let squash = render_pose(20, ACCENT, ANIM[0]);
        assert_ne!(rest, squash);
        let last = ANIM[ANIM.len() - 1];
        assert!((last.scale_x - 1.0).abs() < 0.05 && last.lift == 0.0);
    }

    #[test]
    fn ico_header_is_valid() {
        let ico = ico_bytes([1, 2, 3]);
        assert_eq!(&ico[0..4], &[0, 0, 1, 0]);
        assert_eq!(u16::from_le_bytes([ico[4], ico[5]]), ICO_SIZES.len() as u16);
        // Directory entries list every size; 256 is stored as 0.
        let widths: Vec<u8> = (0..ICO_SIZES.len()).map(|i| ico[6 + 16 * i]).collect();
        assert_eq!(widths, vec![16, 20, 24, 32, 48, 64, 128, 0]);
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
