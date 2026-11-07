//! Exports the logo PNGs: `cargo run -p stinkshot --example logo`.
//!
//! Writes docs/logo-256.png, the concept comparison sheets in docs/notes/logo/ and the
//! tray animation / recording badge strips. The shipped logo is concept A ("Puff"), drawn by
//! `src/icon_gen.rs`; concepts B and C are composed here on top of it for comparison only.

#[allow(dead_code)]
#[path = "../src/icon_gen.rs"]
mod icon_gen;

use std::path::{Path, PathBuf};

use icon_gen::{ANIM, Pose, render_pose, render_rgba};

const ACCENT: [u8; 3] = [0x7f, 0xd3, 0x6a];

type Img = (u32, Vec<u8>);
type Concept = fn(u32) -> Img;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let docs = root.join("docs");
    let notes = docs.join("notes").join("logo");
    std::fs::create_dir_all(&notes)?;

    save(&docs.join("logo-256.png"), &(256, render_rgba(256, ACCENT)))?;

    let concepts: [(&str, Concept); 3] = [
        ("a-puff", concept_a),
        ("b-tile", concept_b),
        ("c-lens", concept_c),
    ];
    for (name, f) in concepts {
        let sheet = sheet(&[f(256), f(32), f(16)]);
        save(&notes.join(format!("concept-{name}.png")), &sheet)?;
    }

    // Tray sizes of the chosen logo, on light and dark taskbar colors, 1:1 and 4x zoomed.
    let sizes = [16u32, 20, 24, 32];
    let tray: Vec<Img> = sizes
        .iter()
        .map(|s| (*s, render_rgba(*s, ACCENT)))
        .collect();
    save(&notes.join("tray-sizes.png"), &taskbar_sheet(&tray))?;

    // Capture animation (rest + 7 frames + rest) at 20 px, plus the recording badge.
    let mut frames = vec![(20, render_rgba(20, ACCENT))];
    frames.extend(ANIM.iter().map(|p| (20, render_pose(20, ACCENT, *p))));
    frames.push((
        20,
        render_pose(
            20,
            ACCENT,
            Pose {
                recording: true,
                ..Pose::REST
            },
        ),
    ));
    save(&notes.join("tray-animation.png"), &taskbar_sheet(&frames))?;
    let mut big = vec![(96, render_rgba(96, ACCENT))];
    big.extend(ANIM.iter().map(|p| (96, render_pose(96, ACCENT, *p))));
    big.push((
        96,
        render_pose(
            96,
            ACCENT,
            Pose {
                recording: true,
                ..Pose::REST
            },
        ),
    ));
    save(&notes.join("animation-96.png"), &sheet(&big))?;
    println!("wrote {}", notes.display());
    Ok(())
}

// ------------------------------------------------------------------- concepts

/// A: free-standing glossy puff with trailing motion puffs (the shipped logo).
fn concept_a(size: u32) -> Img {
    (size, render_rgba(size, ACCENT))
}

/// B: the puff on a dark rounded "app tile" with viewfinder corner brackets.
fn concept_b(size: u32) -> Img {
    let mut img = shape(size, |x, y| {
        let d = rounded_rect(x, y, 0.5, 0.5, 0.47, 0.47, 0.2);
        (d < 0.0).then(|| {
            let t = y.clamp(0.0, 1.0);
            (mix([0.17, 0.22, 0.19], [0.08, 0.11, 0.09], t), 1.0)
        })
    });
    let br = shape(size, |x, y| {
        let (w, len) = (0.035f32.max(1.2 / size as f32), 0.14);
        let mut hit = false;
        for (cx, cy, sx, sy) in [
            (0.17, 0.17, 1.0, 1.0),
            (0.83, 0.17, -1.0, 1.0),
            (0.17, 0.83, 1.0, -1.0),
            (0.83, 0.83, -1.0, -1.0),
        ] {
            let (dx, dy) = ((x - cx) * sx, (y - cy) * sy);
            hit |= (0.0..len).contains(&dx) && (0.0..w).contains(&dy);
            hit |= (0.0..len).contains(&dy) && (0.0..w).contains(&dx);
        }
        hit.then_some(([0.65, 0.9, 0.59], 0.9))
    });
    over_img(&mut img, &br, 0, 0);
    let inner = (size as f32 * 0.78).round() as u32;
    let cloud = render_rgba(inner, ACCENT);
    let off = ((size - inner) / 2) as i32;
    over_img(&mut img, &(inner, cloud), off, off + (size as i32 / 40));
    img
}

/// C: the puff with a glossy camera lens set into its face.
fn concept_c(size: u32) -> Img {
    let mut img = (size, render_rgba(size, ACCENT));
    let (cx, cy) = if size <= 24 {
        (0.52, 0.52)
    } else {
        (0.585, 0.53)
    };
    let r = if size <= 24 { 0.2 } else { 0.15 };
    let lens = shape(size, |x, y| {
        let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
        if d > r {
            return None;
        }
        if d > r * 0.78 {
            return Some(([0.12, 0.2, 0.14], 1.0));
        }
        let t = ((y - (cy - r)) / (2.0 * r)).clamp(0.0, 1.0);
        let mut c = mix([0.25, 0.42, 0.55], [0.03, 0.06, 0.1], t);
        let gx = x - (cx - r * 0.28);
        let gy = y - (cy - r * 0.3);
        if (gx * gx + gy * gy).sqrt() < r * 0.2 {
            c = [0.95, 0.98, 1.0];
        }
        Some((c, 1.0))
    });
    over_img(&mut img, &lens, 0, 0);
    img
}

// ------------------------------------------------------------------- helpers

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn rounded_rect(x: f32, y: f32, cx: f32, cy: f32, hx: f32, hy: f32, r: f32) -> f32 {
    let qx = (x - cx).abs() - (hx - r);
    let qy = (y - cy).abs() - (hy - r);
    (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r
}

/// Rasterizes a shape function (unit coords -> color, alpha) with 6x6 supersampling.
fn shape(size: u32, f: impl Fn(f32, f32) -> Option<([f32; 3], f32)>) -> Img {
    let n = size as usize;
    let mut out = vec![0u8; n * n * 4];
    const SS: usize = 6;
    for y in 0..n {
        for x in 0..n {
            let mut acc = [0.0f32; 4];
            for sy in 0..SS {
                for sx in 0..SS {
                    let px = (x as f32 + (sx as f32 + 0.5) / SS as f32) / size as f32;
                    let py = (y as f32 + (sy as f32 + 0.5) / SS as f32) / size as f32;
                    if let Some((c, a)) = f(px, py) {
                        for i in 0..3 {
                            acc[i] += c[i] * a;
                        }
                        acc[3] += a;
                    }
                }
            }
            let t = (SS * SS) as f32;
            if acc[3] > 0.0 {
                let i = (y * n + x) * 4;
                for c in 0..3 {
                    out[i + c] = (acc[c] / acc[3] * 255.0).round().clamp(0.0, 255.0) as u8;
                }
                out[i + 3] = (acc[3] / t * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    (size, out)
}

/// Straight-alpha source-over of `src` onto `dst` at an offset.
fn over_img(dst: &mut Img, src: &Img, ox: i32, oy: i32) {
    over_rect(&mut dst.1, dst.0, dst.0, src, ox, oy);
}

fn over_rect(dst: &mut [u8], dw: u32, dh: u32, src: &Img, ox: i32, oy: i32) {
    let (s, data) = (src.0 as i32, &src.1);
    for y in 0..s {
        for x in 0..s {
            let (dx, dy) = (x + ox, y + oy);
            if dx < 0 || dy < 0 || dx >= dw as i32 || dy >= dh as i32 {
                continue;
            }
            let si = ((y * s + x) * 4) as usize;
            let di = ((dy * dw as i32 + dx) * 4) as usize;
            let sa = data[si + 3] as f32 / 255.0;
            let da = dst[di + 3] as f32 / 255.0;
            let oa = sa + da * (1.0 - sa);
            if oa <= 0.0 {
                continue;
            }
            for c in 0..3 {
                let v = (data[si + c] as f32 * sa + dst[di + c] as f32 * da * (1.0 - sa)) / oa;
                dst[di + c] = v.round() as u8;
            }
            dst[di + 3] = (oa * 255.0).round() as u8;
        }
    }
}

fn fill(dst: &mut [u8], dw: u32, x0: u32, y0: u32, w: u32, h: u32, c: [u8; 3]) {
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            let i = ((y * dw + x) * 4) as usize;
            dst[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
}

/// Upscales with nearest neighbor (for inspecting small sizes).
fn zoom(img: &Img, k: u32) -> Img {
    let (s, d) = (img.0, &img.1);
    let n = s * k;
    let mut out = vec![0u8; (n * n * 4) as usize];
    for y in 0..n {
        for x in 0..n {
            let si = (((y / k) * s + x / k) * 4) as usize;
            let di = ((y * n + x) * 4) as usize;
            out[di..di + 4].copy_from_slice(&d[si..si + 4]);
        }
    }
    (n, out)
}

/// Images side by side on light and dark panels (top: light, bottom: dark), each image
/// shown at 1:1 and, below 64 px, also zoomed to 128 px.
fn sheet(imgs: &[Img]) -> (u32, u32, Vec<u8>) {
    let pad = 16u32;
    let cell = |i: &Img| if i.0 < 64 { i.0 + pad + 128 } else { i.0 };
    let w: u32 = imgs.iter().map(|i| cell(i) + pad).sum::<u32>() + pad;
    let ch = imgs.iter().map(|i| i.0.max(128)).max().unwrap_or(128) + 2 * pad;
    let h = ch * 2;
    let mut out = vec![0u8; (w * h * 4) as usize];
    fill(&mut out, w, 0, 0, w, ch, [0xf3, 0xf3, 0xf3]);
    fill(&mut out, w, 0, ch, w, ch, [0x20, 0x20, 0x20]);
    for row in 0..2u32 {
        let mut x = pad;
        for img in imgs {
            let y = row * ch + pad;
            over_rect(&mut out, w, h, img, x as i32, y as i32);
            if img.0 < 64 {
                let z = zoom(img, 128 / img.0.max(1));
                over_rect(&mut out, w, h, &z, (x + img.0 + pad) as i32, y as i32);
            }
            x += cell(img) + pad;
        }
    }
    (w, h, out)
}

/// Small images on light and dark taskbar colors at 1:1 and 4x.
fn taskbar_sheet(imgs: &[Img]) -> (u32, u32, Vec<u8>) {
    let pad = 8u32;
    let s = imgs.iter().map(|i| i.0).max().unwrap_or(16);
    let w = imgs.len() as u32 * (s * 4 + pad) + pad;
    let row = s * 4 + s + 3 * pad;
    let h = row * 2;
    let mut out = vec![0u8; (w * h * 4) as usize];
    fill(&mut out, w, 0, 0, w, row, [0xee, 0xee, 0xee]);
    fill(&mut out, w, 0, row, w, row, [0x1c, 0x1c, 0x1c]);
    for r in 0..2u32 {
        for (i, img) in imgs.iter().enumerate() {
            let x = pad + i as u32 * (s * 4 + pad);
            let y = r * row + pad;
            over_rect(&mut out, w, h, img, x as i32, y as i32);
            let z = zoom(img, 4);
            over_rect(&mut out, w, h, &z, x as i32, (y + s + pad) as i32);
        }
    }
    (w, h, out)
}

fn save(path: &Path, img: &dyn ToPng) -> Result<(), Box<dyn std::error::Error>> {
    let (w, h, data) = img.parts();
    image::save_buffer(path, data, w, h, image::ExtendedColorType::Rgba8)?;
    Ok(())
}

trait ToPng {
    fn parts(&self) -> (u32, u32, &[u8]);
}

impl ToPng for Img {
    fn parts(&self) -> (u32, u32, &[u8]) {
        (self.0, self.0, &self.1)
    }
}

impl ToPng for (u32, u32, Vec<u8>) {
    fn parts(&self) -> (u32, u32, &[u8]) {
        (self.0, self.1, &self.2)
    }
}
