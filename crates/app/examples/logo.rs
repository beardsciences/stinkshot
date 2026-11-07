//! Exports the logo PNGs: `cargo run -p stinkshot --example logo [-- <gif path>]`.
//!
//! Writes docs/logo-256.png and, in docs/notes/logo/, the icon size sheet, the tray sizes
//! and capture animation on light and dark taskbars, and a strip of the About page
//! animation (orb frames with the particle field composited the way `logo_anim` paints
//! it). With a path argument it also writes an animated GIF of the About loop there.

#[allow(dead_code)]
#[path = "../src/icon_gen.rs"]
mod icon_gen;

use std::path::{Path, PathBuf};

use icon_gen::{ANIM, DEFAULT_ACCENT, Pose, render_pose, render_rgba};
use stinkshot_logo::{
    ABOUT_HALF, ORB_R, Opts, mote_alpha, mote_layers, motes, render, render_icon, trails,
};

/// (width, height, straight RGBA8).
type Img = (u32, u32, Vec<u8>);

const LIGHT: [u8; 3] = [0xf3, 0xf3, 0xf3];
const DARK: [u8; 3] = [0x20, 0x20, 0x20];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let docs = root.join("docs");
    let notes = docs.join("notes").join("logo");
    std::fs::create_dir_all(&notes)?;

    let sq = |s: u32, px: Vec<u8>| (s, s, px);
    save(&docs.join("logo-256.png"), &sq(256, render_icon(256)))?;

    // Every shipped size, 1:1 and (below 64) zoomed, on light and dark.
    let sizes = [256u32, 128, 64, 48, 32, 24, 20, 16];
    let icons: Vec<Img> = sizes.iter().map(|s| sq(*s, render_icon(*s))).collect();
    save(&notes.join("icon-sizes.png"), &sheet(&icons, 256))?;

    // Tray sizes as the app shows them (embedded bitmaps), 1:1 and 4x.
    let tray: Vec<Img> = [16u32, 20, 24, 32]
        .iter()
        .map(|s| sq(*s, render_rgba(*s, DEFAULT_ACCENT)))
        .collect();
    save(&notes.join("tray-sizes.png"), &taskbar_sheet(&tray))?;

    // Capture animation (rest + 7 frames + rest) at 20 px, plus the recording badge.
    let mut frames = vec![sq(20, render_rgba(20, DEFAULT_ACCENT))];
    frames.extend(
        ANIM.iter()
            .map(|p| sq(20, render_pose(20, DEFAULT_ACCENT, *p))),
    );
    frames.push(sq(20, render_rgba(20, DEFAULT_ACCENT)));
    let rec = Pose {
        recording: true,
        ..Pose::REST
    };
    frames.push(sq(20, render_pose(20, DEFAULT_ACCENT, rec)));
    save(&notes.join("tray-animation.png"), &taskbar_sheet(&frames))?;

    // About animation strip: 8 phases on dark (top) and light (bottom).
    let n = 8;
    let (cw, ch) = (300u32, 200u32);
    let mut strip = blank(cw * n, ch * 2, DARK);
    fill(&mut strip, 0, ch, cw * n, ch, LIGHT);
    for i in 0..n {
        let t = i as f32 / n as f32;
        let orb = render(&Opts::about(184, t, 2));
        for (row, dark) in [(0u32, true), (1, false)] {
            let time = t as f64 * 12.0;
            let frame = about_frame(&orb, cw, ch, time, dark);
            blit(&mut strip, &frame, (i * cw) as i32, (row * ch) as i32);
        }
    }
    save(&notes.join("about-animation.png"), &strip)?;

    if let Some(gif_path) = std::env::args().nth(1) {
        write_gif(Path::new(&gif_path), cw, ch)?;
        println!("wrote {gif_path}");
    }
    println!("wrote {}", notes.display());
    Ok(())
}

// ------------------------------------------------------------------- about frame

/// One About page frame: back motes, bloom, the orb, the pulse, front motes, over the
/// page colour. Mirrors `logo_anim::show` at 1x.
fn about_frame(orb: &[u8], w: u32, h: u32, time: f64, dark: bool) -> Img {
    let view = 184.0f32;
    let mut img = blank(w, h, if dark { DARK } else { LIGHT });
    let (ox, oy) = ((w as f32 - view) / 2.0, (h as f32 - view) / 2.0);
    let ms = motes(time, ABOUT_HALF);
    let tr = trails(time, ABOUT_HALF);
    let coverage = |x: f32, y: f32| -> f32 {
        let (px, py) = ((x * 184.0) as i32, (y * 184.0) as i32);
        if !(0..184).contains(&px) || !(0..184).contains(&py) {
            return 0.0;
        }
        orb[((py * 184 + px) * 4 + 3) as usize] as f32 / 255.0
    };
    let draw_motes = |img: &mut Img, front: bool| {
        let trail = if front { &tr[..] } else { &[][..] };
        for m in trail
            .iter()
            .chain(ms.iter().filter(|m| (m.z >= 0.0) == front))
        {
            let (cx, cy) = (ox + m.x * view, oy + m.y * view);
            let halo = if front { 0.5 } else { 1.0 };
            for (c, a, add) in mote_layers(m, dark, coverage(m.x, m.y), 0.8) {
                if a <= 0.003 {
                    continue;
                }
                let c = c.map(|v| v as f32);
                splat(img, cx, cy, m.r * 6.0, c, add, |d| {
                    a * mote_alpha(d, m.r, halo)
                });
            }
        }
    };
    draw_motes(&mut img, false);
    if dark {
        let r = view * 0.5 * ORB_R * 1.25 / ABOUT_HALF;
        splat(
            &mut img,
            w as f32 / 2.0,
            h as f32 / 2.0,
            r,
            [127.0, 211.0, 106.0],
            true,
            |d| {
                let f = d / r;
                0.18 * (-f * f * 3.0).exp() * (1.0 - f)
            },
        );
    }
    blit(&mut img, &(184, 184, orb.to_vec()), ox as i32, oy as i32);
    let pulse = 0.12 * (std::f64::consts::TAU * time / 3.0).sin() as f32;
    if pulse > 0.0 {
        let r = view * 0.26;
        let k = pulse * if dark { 0.9 } else { 0.6 };
        let cy = h as f32 / 2.0 + view * 0.04;
        splat(
            &mut img,
            w as f32 / 2.0,
            cy,
            r,
            [212.0, 255.0, 134.0],
            dark,
            |d| {
                let f = d / r;
                k * (-f * f * 3.0).exp() * (1.0 - f)
            },
        );
    }
    draw_motes(&mut img, true);
    img
}

/// Draws a radial splat of colour `c` with per-distance alpha, additive or normal over.
fn splat(
    img: &mut Img,
    cx: f32,
    cy: f32,
    radius: f32,
    c: [f32; 3],
    add: bool,
    a: impl Fn(f32) -> f32,
) {
    let (w, h) = (img.0 as i32, img.1 as i32);
    let x0 = ((cx - radius).floor() as i32).max(0);
    let x1 = ((cx + radius).ceil() as i32).min(w - 1);
    let y0 = ((cy - radius).floor() as i32).max(0);
    let y1 = ((cy + radius).ceil() as i32).min(h - 1);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            if d > radius {
                continue;
            }
            let al = a(d).clamp(0.0, 1.0);
            let i = ((y * w + x) * 4) as usize;
            for (px, ck) in img.2[i..i + 3].iter_mut().zip(c) {
                let dst = *px as f32;
                let v = if add {
                    dst + ck * al
                } else {
                    dst + (ck - dst) * al
                };
                *px = v.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

fn write_gif(path: &Path, w: u32, h: u32) -> Result<(), Box<dyn std::error::Error>> {
    // 72 phases of the 12 s loop played at 0.1 s per frame (1.67x speed).
    let frames = 72;
    let file = std::fs::File::create(path)?;
    let mut enc = gif::Encoder::new(std::io::BufWriter::new(file), w as u16, h as u16, &[])?;
    enc.set_repeat(gif::Repeat::Infinite)?;
    for i in 0..frames {
        let t = i as f32 / frames as f32;
        let orb = render(&Opts::about(184, t, 2));
        let mut img = about_frame(&orb, w, h, t as f64 * 12.0, true);
        let mut f = gif::Frame::from_rgba_speed(w as u16, h as u16, &mut img.2, 10);
        f.delay = 10;
        enc.write_frame(&f)?;
    }
    Ok(())
}

// ------------------------------------------------------------------- helpers

fn blank(w: u32, h: u32, c: [u8; 3]) -> Img {
    let mut img = (w, h, vec![0u8; (w * h * 4) as usize]);
    fill(&mut img, 0, 0, w, h, c);
    img
}

fn fill(img: &mut Img, x0: u32, y0: u32, w: u32, h: u32, c: [u8; 3]) {
    let iw = img.0;
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            let i = ((y * iw + x) * 4) as usize;
            img.2[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
}

/// Straight-alpha source-over of `src` onto `dst` at an offset.
fn blit(dst: &mut Img, src: &Img, ox: i32, oy: i32) {
    let (dw, dh) = (dst.0 as i32, dst.1 as i32);
    for y in 0..src.1 as i32 {
        for x in 0..src.0 as i32 {
            let (dx, dy) = (x + ox, y + oy);
            if dx < 0 || dy < 0 || dx >= dw || dy >= dh {
                continue;
            }
            let si = ((y * src.0 as i32 + x) * 4) as usize;
            let di = ((dy * dw + dx) * 4) as usize;
            let sa = src.2[si + 3] as f32 / 255.0;
            let da = dst.2[di + 3] as f32 / 255.0;
            let oa = sa + da * (1.0 - sa);
            if oa <= 0.0 {
                continue;
            }
            for c in 0..3 {
                let v = (src.2[si + c] as f32 * sa + dst.2[di + c] as f32 * da * (1.0 - sa)) / oa;
                dst.2[di + c] = v.round() as u8;
            }
            dst.2[di + 3] = (oa * 255.0).round() as u8;
        }
    }
}

/// Upscales with nearest neighbour (for inspecting small sizes).
fn zoom(img: &Img, k: u32) -> Img {
    let (w, h) = (img.0 * k, img.1 * k);
    let mut out = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let si = (((y / k) * img.0 + x / k) * 4) as usize;
            let di = ((y * w + x) * 4) as usize;
            out[di..di + 4].copy_from_slice(&img.2[si..si + 4]);
        }
    }
    (w, h, out)
}

/// Images side by side on light (top) and dark (bottom) panels, each at 1:1 and, below
/// 64 px, also zoomed to about 128 px.
fn sheet(imgs: &[Img], cell_h: u32) -> Img {
    let pad = 20u32;
    let zoomed = |i: &Img| (i.0 < 64).then(|| zoom(i, (128 / i.0).max(1)));
    let w: u32 = pad
        + imgs
            .iter()
            .map(|i| i.0 + pad + zoomed(i).map_or(0, |z| z.0 + pad))
            .sum::<u32>();
    let row = cell_h + 2 * pad;
    let mut out = blank(w, row * 2, LIGHT);
    fill(&mut out, 0, row, w, row, DARK);
    for r in 0..2u32 {
        let mut x = pad;
        for img in imgs {
            let y = r * row + pad + (cell_h - img.1) / 2;
            blit(&mut out, img, x as i32, y as i32);
            x += img.0 + pad;
            if let Some(z) = zoomed(img) {
                let y = r * row + pad + (cell_h - z.1) / 2;
                blit(&mut out, &z, x as i32, y as i32);
                x += z.0 + pad;
            }
        }
    }
    out
}

/// Small images on light and dark taskbar colours at 1:1 and 4x.
fn taskbar_sheet(imgs: &[Img]) -> Img {
    let pad = 8u32;
    let s = imgs.iter().map(|i| i.0).max().unwrap_or(16);
    let w = imgs.len() as u32 * (s * 4 + pad) + pad;
    let row = s * 4 + s + 3 * pad;
    let mut out = blank(w, row * 2, LIGHT);
    fill(&mut out, 0, row, w, row, DARK);
    for r in 0..2u32 {
        for (i, img) in imgs.iter().enumerate() {
            let x = pad + i as u32 * (s * 4 + pad);
            let y = r * row + pad;
            blit(&mut out, img, x as i32, y as i32);
            blit(&mut out, &zoom(img, 4), x as i32, (y + s + pad) as i32);
        }
    }
    out
}

fn save(path: &Path, img: &Img) -> Result<(), Box<dyn std::error::Error>> {
    image::save_buffer(path, &img.2, img.0, img.1, image::ExtendedColorType::Rgba8)?;
    Ok(())
}
