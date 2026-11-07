//! Stinkshot logo: a glossy, translucent, lumpy green gel orb. See docs/notes/logo.md.
//!
//! Pure std so the app's `build.rs` can link it (the exe .ico and the pre-rendered tray
//! bitmaps are made at build time) and the About page can render its animation frames on
//! a worker thread. The workspace builds this crate with `opt-level = 3` even in dev
//! profiles, which keeps both fast.
//!
//! The orb is a signed distance field (a core sphere smooth-unioned with eight lumps that
//! breathe and slide as it spins), ray marched with a perspective camera and shaded as a
//! green gel: refraction with Beer-Lambert absorption over the real thickness, an inner
//! glow and a luminous nucleus, a studio environment (elliptical softbox, rim strip, floor
//! bounce) seen in Fresnel reflections, a stable dome specular, crease occlusion tinted
//! deep green, a soft contact shadow and, at tray sizes, a thin dark outline. Output is
//! ACES tone mapped, supersampled in premultiplied linear space and dithered to sRGB8.
//!
//! `motes` gives the About page's particle field (two tilted rings and rising drift
//! motes) projected through the same camera.

use std::f32::consts::TAU;

/// Sizes embedded in the exe icon.
pub const ICO_SIZES: [u32; 8] = [16, 20, 24, 32, 48, 64, 128, 256];

/// Half extent of the view at the target, in world units, for the About page (leaves room
/// for the particle rings).
pub const ABOUT_HALF: f32 = 1.75;

/// Visual radius of the orb in world units (particle orbits are given in these).
pub const ORB_R: f32 = 1.12;

// ------------------------------------------------------------------------ options

/// Everything that varies between icon sizes and the animated About orb.
#[derive(Clone, Copy, Debug)]
pub struct Opts {
    /// Output width and height in pixels.
    pub size: u32,
    /// Animation phase in [0, 1): one full spin plus one lump cycle.
    pub t: f32,
    /// Half extent of the view at the target, world units.
    pub half: f32,
    /// Smooth-union radius (bigger at tray sizes: fewer, softer lumps).
    pub k: f32,
    /// Fine surface displacement amplitude (large sizes only).
    pub disp: f32,
    /// Dark outline width in output pixels (tray sizes), 0 for none.
    pub outline_px: f32,
    /// Contact shadow strength.
    pub shadow: f32,
    /// Supersamples per axis.
    pub ss: u32,
    /// Small-size shading: wider glint, lighter absorption, brighter exposure.
    pub small: bool,
    /// Ray-march steps to the surface.
    pub march_steps: u32,
    /// Steps of the inner-glow integration.
    pub glow_steps: u32,
    /// Nucleus emission multiplier (the About page's idle pulse).
    pub nucleus: f32,
}

impl Opts {
    /// The static logo at one icon size, with size-specific simplification.
    pub fn icon(size: u32) -> Opts {
        let small = size <= 32;
        let (half, k, outline_px, shadow, ss) = match size {
            0..=16 => (1.22, 0.40, 0.85, 0.0, 8),
            17..=20 => (1.24, 0.40, 0.80, 0.0, 8),
            21..=24 => (1.24, 0.40, 0.75, 0.0, 8),
            25..=32 => (1.28, 0.34, 0.70, 0.0, 8),
            33..=48 => (1.40, 0.30, 0.0, 0.35, 8),
            49..=64 => (1.41, 0.30, 0.0, 0.42, 6),
            65..=128 => (1.42, 0.30, 0.0, 0.50, 5),
            _ => (1.42, 0.30, 0.0, 0.55, 5),
        };
        Opts {
            size,
            t: 0.0,
            half,
            k,
            disp: if size >= 128 { 0.02 } else { 0.0 },
            outline_px,
            shadow,
            ss,
            small,
            march_steps: 160,
            glow_steps: if small { 16 } else { 36 },
            nucleus: 1.0,
        }
    }

    /// One frame of the About page orb at phase `t` (the view leaves room for particles).
    /// `ss` is 2 below 2x display scaling and 1 at 2x and up.
    pub fn about(size: u32, t: f32, ss: u32) -> Opts {
        Opts {
            size,
            t,
            half: ABOUT_HALF,
            k: 0.30,
            disp: 0.02,
            outline_px: 0.0,
            shadow: 0.30,
            ss: ss.max(1),
            small: false,
            // 64 steps tore the silhouette where rays skim the creases (about 2500 missed
            // pixels per 32 frames); 256 plus the near-miss rule leaves almost none.
            march_steps: 256,
            glow_steps: 12,
            nucleus: 1.0,
        }
    }
}

// ------------------------------------------------------------------------ math

#[derive(Clone, Copy, Debug, PartialEq)]
struct V(f32, f32, f32);

impl std::ops::Add for V {
    type Output = V;
    fn add(self, o: V) -> V {
        V(self.0 + o.0, self.1 + o.1, self.2 + o.2)
    }
}
impl std::ops::Sub for V {
    type Output = V;
    fn sub(self, o: V) -> V {
        V(self.0 - o.0, self.1 - o.1, self.2 - o.2)
    }
}
impl std::ops::Mul<f32> for V {
    type Output = V;
    fn mul(self, s: f32) -> V {
        V(self.0 * s, self.1 * s, self.2 * s)
    }
}
impl std::ops::Mul<V> for V {
    type Output = V;
    fn mul(self, o: V) -> V {
        V(self.0 * o.0, self.1 * o.1, self.2 * o.2)
    }
}
impl std::ops::Neg for V {
    type Output = V;
    fn neg(self) -> V {
        V(-self.0, -self.1, -self.2)
    }
}

fn dot(a: V, b: V) -> f32 {
    a.0 * b.0 + a.1 * b.1 + a.2 * b.2
}
fn len(a: V) -> f32 {
    dot(a, a).sqrt()
}
fn norm(a: V) -> V {
    a * (1.0 / len(a).max(1e-9))
}
fn cross(a: V, b: V) -> V {
    V(
        a.1 * b.2 - a.2 * b.1,
        a.2 * b.0 - a.0 * b.2,
        a.0 * b.1 - a.1 * b.0,
    )
}
fn vexp(a: V) -> V {
    V(a.0.exp(), a.1.exp(), a.2.exp())
}
fn vmin(a: V, m: f32) -> V {
    V(a.0.min(m), a.1.min(m), a.2.min(m))
}
fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}
fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = clamp01((x - a) / (b - a));
    t * t * (3.0 - 2.0 * t)
}
fn mix(a: V, b: V, t: f32) -> V {
    a * (1.0 - t) + b * t
}
fn reflect(i: V, n: V) -> V {
    i - n * (2.0 * dot(i, n))
}
fn refract(i: V, n: V, eta: f32) -> Option<V> {
    let c = -dot(n, i);
    let k = 1.0 - eta * eta * (1.0 - c * c);
    (k >= 0.0).then(|| i * eta + n * (eta * c - k.sqrt()))
}
fn rot_y(p: V, a: f32) -> V {
    let (s, c) = a.sin_cos();
    V(c * p.0 + s * p.2, p.1, -s * p.0 + c * p.2)
}
fn rot_x(p: V, a: f32) -> V {
    let (s, c) = a.sin_cos();
    V(p.0, c * p.1 - s * p.2, s * p.1 + c * p.2)
}
fn rot_z(p: V, a: f32) -> V {
    let (s, c) = a.sin_cos();
    V(c * p.0 - s * p.1, s * p.0 + c * p.1, p.2)
}
fn smin(a: f32, b: f32, k: f32) -> f32 {
    let h = clamp01(0.5 + 0.5 * (b - a) / k);
    b * (1.0 - h) + a * h - k * h * (1.0 - h)
}
fn srgb_to_lin(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}
fn lin_to_srgb(x: f32) -> f32 {
    let x = clamp01(x);
    if x <= 0.003_130_8 {
        12.92 * x
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    }
}
fn hex(h: u32) -> V {
    V(
        srgb_to_lin(((h >> 16) & 255) as f32 / 255.0),
        srgb_to_lin(((h >> 8) & 255) as f32 / 255.0),
        srgb_to_lin((h & 255) as f32 / 255.0),
    )
}
fn aces(x: f32) -> f32 {
    let (a, b, c, d, e) = (2.51, 0.03, 2.43, 0.59, 0.14);
    clamp01((x * (a * x + b)) / (x * (c * x + d) + e))
}
/// Deterministic hash to [0, 1).
fn hash(mut x: u32) -> f32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

// ------------------------------------------------------------------------ scene

/// (direction, offset, radius, phase) of each lump; offsets are scaled by 0.92.
const LUMPS: [(V, f32, f32, f32); 8] = [
    (V(-0.55, 0.62, 0.55), 0.70, 0.55, 0.0),
    (V(0.68, 0.45, 0.40), 0.68, 0.50, 1.1),
    (V(0.05, -0.30, 1.0), 0.66, 0.52, 2.3),
    (V(-0.80, -0.45, 0.10), 0.68, 0.50, 3.4),
    (V(0.75, -0.50, -0.10), 0.70, 0.48, 4.2),
    (V(-0.10, 0.92, -0.35), 0.66, 0.50, 5.0),
    (V(-0.35, 0.05, -0.95), 0.68, 0.52, 0.7),
    (V(0.45, 0.10, -0.90), 0.66, 0.48, 1.9),
];
const CORE_R: f32 = 0.86;
/// Everything lies inside this sphere (largest lump reach plus displacement and margin).
const BOUND_R: f32 = 1.45;
const EYE: V = V(0.0, 1.0, 6.2);
const TARGET: V = V(0.0, -0.06, 0.0);
const FLOOR_Y: f32 = -1.06;
/// Cap on the transmitted environment: thin lumps seen against the softbox otherwise glow
/// neon lime.
const TRANS_CAP: f32 = 0.45;

struct Scene {
    lumps: [(V, f32); 8],
    core_c: V,
    k: f32,
    disp: f32,
    ang: f32,
    tilt: f32,
    t: f32,
}

impl Scene {
    fn new(o: &Opts) -> Scene {
        let t = o.t;
        let ang = TAU * t;
        let tilt = 0.12 * (TAU * t).sin();
        let mut lumps = [(V(0.0, 0.0, 0.0), 0.0); 8];
        for (out, (dir, off, r, ph)) in lumps.iter_mut().zip(LUMPS) {
            // Lumps breathe (2x per loop) and wander over the surface as the orb spins.
            let wander = V((TAU * t + ph).cos(), (2.0 * TAU * t + 1.7 * ph).sin(), 0.0);
            let d = norm(norm(dir) + wander * 0.12);
            let c = rot_x(rot_y(d * (off * 0.92), ang), tilt);
            *out = (c, r * (1.0 + 0.07 * (2.0 * TAU * t + ph).sin()));
        }
        Scene {
            lumps,
            core_c: rot_y(V(0.06, -0.10, 0.05), ang),
            k: o.k,
            disp: o.disp,
            ang,
            tilt,
            t,
        }
    }

    fn sdf(&self, p: V) -> f32 {
        let mut d = len(p) - CORE_R;
        for &(c, r) in &self.lumps {
            d = smin(d, len(p - c) - r, self.k);
        }
        if self.disp > 0.0 {
            let q = rot_y(rot_x(p, -self.tilt), -self.ang);
            let ph = TAU * self.t;
            d += self.disp
                * ((2.3 * q.0 + 0.7 + ph).sin()
                    * (2.1 * q.1 + 1.3).sin()
                    * (2.5 * q.2 + 0.4 - ph).sin());
        }
        d
    }

    /// Tetrahedral central differences (4 evaluations).
    fn normal(&self, p: V) -> V {
        let e = 0.0015;
        let (a, b, c, d) = (
            V(1.0, -1.0, -1.0),
            V(-1.0, -1.0, 1.0),
            V(-1.0, 1.0, -1.0),
            V(1.0, 1.0, 1.0),
        );
        norm(
            a * self.sdf(p + a * e)
                + b * self.sdf(p + b * e)
                + c * self.sdf(p + c * e)
                + d * self.sdf(p + d * e),
        )
    }
}

// ------------------------------------------------------------------------ lighting

fn key_dir() -> V {
    norm(V(-0.55, 0.75, 0.55))
}

/// Studio environment: elliptical softbox upper left, a broad sheen around it, a thin rim
/// strip on the right, dark-to-grey sky and a faint green floor bounce.
fn env(d: V, small: bool) -> V {
    env_rest(d) + env_softbox(d, small)
}

/// The key softbox and the broad sheen around it.
fn env_softbox(d: V, small: bool) -> V {
    let kd = key_dir();
    let f = dot(d, kd);
    if f <= 0.0 {
        return V(0.0, 0.0, 0.0);
    }
    let kr = norm(cross(V(0.0, 1.0, 0.0), kd));
    let ku = cross(kd, kr);
    let x = dot(d, kr) / f;
    let y = dot(d, ku) / f;
    let e = ((x / 0.34).powi(2) + (y / 0.22).powi(2)).sqrt();
    let m = 1.0 - smoothstep(if small { 0.6 } else { 0.78 }, 1.0, e);
    let sheen = 1.0 - smoothstep(0.6, 2.6, e);
    V(1.0, 1.0, 0.97) * (7.0 * m) + V(0.9, 1.0, 0.95) * (0.9 * sheen)
}

/// Everything but the softbox: sky gradient, rim strip, floor bounce.
fn env_rest(d: V) -> V {
    let up = d.1;
    let sky = mix(
        V(0.01, 0.012, 0.012),
        V(0.32, 0.36, 0.36),
        smoothstep(-0.3, 1.0, up),
    );
    let sd = norm(V(0.95, 0.15, -0.35));
    let strip =
        smoothstep(0.88, 0.97, dot(d, sd)) * (1.0 - smoothstep(0.35, 0.6, (d.1 - 0.1).abs()));
    sky + V(0.85, 1.0, 0.9) * (3.5 * strip) + V(0.12, 0.25, 0.1) * smoothstep(-0.2, -0.9, up)
}

struct Pal {
    base: V,
    deep: V,
    outline: V,
}

fn pal() -> Pal {
    Pal {
        base: hex(0x7fd36a),
        deep: hex(0x06301a),
        outline: hex(0x0f4a14),
    }
}

/// Tone-mapped linear colour and coverage of one camera ray.
fn shade_ray(sc: &Scene, o: &Opts, pl: &Pal, ro: V, rd: V, px_world: f32) -> ([f32; 3], f32) {
    let ow = o.outline_px * px_world;
    // Bounding sphere: rays that miss it can only see the shadow.
    let b = dot(ro, rd);
    let cterm = dot(ro, ro) - (BOUND_R + ow) * (BOUND_R + ow);
    let h = b * b - cterm;
    let mut hit = None;
    let mut dmin = f32::MAX;
    if h > 0.0 {
        let sq = h.sqrt();
        let (t0, t1) = (-b - sq, -b + sq);
        let mut t = t0.max(0.0);
        let mut d = f32::MAX;
        let mut exhausted = true;
        for _ in 0..o.march_steps {
            d = sc.sdf(ro + rd * t);
            dmin = dmin.min(d);
            if d < 0.0005 {
                hit = Some(t);
                exhausted = false;
                break;
            }
            t += d * 0.95;
            if t > t1 {
                exhausted = false;
                break;
            }
        }
        // A ray skimming a crease can run out of steps while still crawling along the
        // surface. Within half a pixel of it, that is a hit, not a hole in the silhouette.
        if exhausted && d < 0.5 * px_world {
            hit = Some(t);
        }
    }
    let Some(t) = hit else {
        if ow > 0.0 && dmin < ow {
            let c = pl.outline;
            return ([c.0, c.1, c.2], 1.0);
        }
        if o.shadow > 0.0 && rd.1 < 0.0 {
            let q = ro + rd * ((FLOOR_Y - ro.1) / rd.1);
            let r2 = q.0 * q.0 + q.2 * q.2;
            let a = clamp01(o.shadow * (0.55 * (-r2 / 0.30).exp() + 0.6 * (-r2 / 0.06).exp()));
            return ([0.0, 0.004, 0.001], a);
        }
        return ([0.0; 3], 0.0);
    };

    let pos = ro + rd * t;
    let n = sc.normal(pos);
    let v = -rd;
    let ndv = clamp01(dot(n, v));
    let fres = 0.04 + 0.96 * (1.0 - ndv).powi(5);

    // Ambient occlusion in the creases between lumps.
    let mut occ = 0.0;
    for i in 1..=5 {
        let hh = 0.05 * i as f32;
        occ += (hh - sc.sdf(pos + n * hh)).max(0.0) / i as f32;
    }
    let ao = clamp01(1.0 - 3.2 * occ);

    // Refract into the gel and march to the far side for the real thickness.
    let rin = refract(rd, n, 1.0 / 1.42).unwrap_or(rd);
    let pe = pos - n * 0.003;
    let mut s = 0.02;
    for _ in 0..48 {
        let d = -sc.sdf(pe + rin * s);
        if d < 0.001 {
            break;
        }
        s += d.max(0.006);
        if s > 3.2 {
            break;
        }
    }
    let thick = s;
    let ne = sc.normal(pe + rin * thick);
    let rout = refract(rin, -ne, 1.42).unwrap_or_else(|| reflect(rin, -ne));
    let sigma = if o.small {
        V(3.0, 0.75, 3.9)
    } else {
        V(3.4, 0.85, 4.2)
    };
    // Transmitted environment, capped so bright env does not punch neon patches through.
    let through = vmin(env(rout, o.small), TRANS_CAP) * vexp(-sigma * thick);

    // Inner glow and the luminous nucleus, integrated along the refracted path.
    let steps = o.glow_steps.max(1);
    let ds = thick / steps as f32;
    let mut glow = V(0.0, 0.0, 0.0);
    let mut core = V(0.0, 0.0, 0.0);
    let core_r = if o.small { 0.38 } else { 0.36 };
    let glow_c = V(0.05, -0.30, 0.15);
    for i in 0..steps {
        let ss = (i as f32 + 0.5) * ds;
        let q = pe + rin * ss;
        let gc = q - glow_c;
        let g = (-dot(gc, gc) * 2.0).exp();
        glow = glow + vexp(-sigma * (ss * 0.6)) * (g * ds);
        let dist = len(q - sc.core_c);
        let dc = (dist - core_r).max(0.0);
        let cg = (-dc * dc * 26.0).exp() * if dist < core_r { 1.6 } else { 1.0 };
        core = core + vexp(-sigma * (ss * 0.45)) * (cg * ds);
    }
    let core_gain = if o.small { 1.0 } else { 1.15 };
    let glow =
        glow * V(0.36, 1.0, 0.22) * 0.75 + core * V(0.85, 1.0, 0.30) * (core_gain * o.nucleus);

    let l = key_dir();
    let wrap = clamp01((dot(n, l) + 0.5) / 1.5);
    let diffuse = pl.base * (0.12 * wrap);
    let caustic = V(0.45, 1.0, 0.35)
        * (0.55 * smoothstep(0.1, -0.9, n.1) * smoothstep(0.0, 0.6, ndv))
        * if o.small { 0.6 } else { 1.0 };
    let mut body = (through + glow + diffuse + caustic * pl.base * 1.4) * (0.35 + 0.65 * ao);
    // Creases go rich deep green rather than grey-black.
    body = mix(body, pl.deep * 0.6, (1.0 - ao) * 0.45);

    // Specular and the softbox reflection use a normal blended toward the orb's sphere
    // normal: one stable dome glint instead of streaks along the lumps as it spins.
    let ns = norm(mix(n, norm(pos), 0.5));
    let refl =
        (env_rest(reflect(rd, n)) + env_softbox(reflect(rd, ns), o.small)) * (0.25 + 0.75 * ao);

    let hv = norm(l + v);
    let ndh = clamp01(dot(n, hv));
    let nsh = clamp01(dot(ns, hv));
    // The crisp glint leans further toward the sphere normal so it stays a round dot.
    let ng = norm(mix(n, norm(pos), 0.78));
    let ngh = clamp01(dot(ng, hv));
    let spec = if o.small {
        0.08 * ndh.powi(16) + 3.0 * ngh.powf(90.0)
    } else {
        0.08 * ndh.powi(16) + 0.15 * nsh.powf(60.0) + 4.0 * ngh.powf(700.0)
    };
    let rim = V(0.55, 1.0, 0.6) * ((1.0 - ndv).powf(2.5) * 0.35 * ao);

    let col = body * (1.0 - fres) + refl * fres + V(1.0, 1.0, 1.0) * spec + rim;
    let ex = if o.small { 1.2 } else { 1.0 };
    ([aces(col.0 * ex), aces(col.1 * ex), aces(col.2 * ex)], 1.0)
}

// ------------------------------------------------------------------------ camera

struct Camera {
    fwd: V,
    right: V,
    up: V,
    scale: f32,
    dist: f32,
}

fn camera(half: f32) -> Camera {
    let fwd = norm(TARGET - EYE);
    let right = norm(cross(fwd, V(0.0, 1.0, 0.0)));
    let up = cross(right, fwd);
    let dist = len(TARGET - EYE);
    Camera {
        fwd,
        right,
        up,
        scale: half / dist,
        dist,
    }
}

// ------------------------------------------------------------------------ render

/// Straight-alpha sRGB RGBA8, `size * size * 4` bytes, using every core.
pub fn render(o: &Opts) -> Vec<u8> {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    render_threads(o, threads)
}

/// Like `render`, on at most `threads` threads (1 renders on the calling thread).
pub fn render_threads(o: &Opts, threads: usize) -> Vec<u8> {
    let size = o.size as usize;
    let mut out = vec![0u8; size * size * 4];
    if size == 0 {
        return out;
    }
    let sc = Scene::new(o);
    let pl = pal();
    let threads = threads.clamp(1, size);
    if threads == 1 {
        for (y, row) in out.chunks_mut(size * 4).enumerate() {
            render_row(&sc, o, &pl, y, row);
        }
        return out;
    }
    // Interleaved rows balance the empty top/bottom against the busy middle.
    let mut rows: Vec<(usize, &mut [u8])> = out.chunks_mut(size * 4).enumerate().collect();
    let mut buckets: Vec<Vec<(usize, &mut [u8])>> = (0..threads).map(|_| Vec::new()).collect();
    for (i, r) in rows.drain(..).enumerate() {
        buckets[i % threads].push(r);
    }
    std::thread::scope(|s| {
        for bucket in buckets {
            let (sc, pl) = (&sc, &pl);
            s.spawn(move || {
                for (y, row) in bucket {
                    render_row(sc, o, pl, y, row);
                }
            });
        }
    });
    out
}

fn render_row(sc: &Scene, o: &Opts, pl: &Pal, y: usize, row: &mut [u8]) {
    let size = o.size as f32;
    let cam = camera(o.half);
    let px_world = 2.0 * o.half / size;
    let n = o.ss.max(1);
    let inv = 1.0 / (n * n) as f32;
    for x in 0..o.size as usize {
        let mut acc = [0.0f32; 4];
        for sy in 0..n {
            for sx in 0..n {
                let u = ((x as f32 + (sx as f32 + 0.5) / n as f32) / size) * 2.0 - 1.0;
                let v = 1.0 - ((y as f32 + (sy as f32 + 0.5) / n as f32) / size) * 2.0;
                let rd = norm(cam.fwd + cam.right * (u * cam.scale) + cam.up * (v * cam.scale));
                let (c, a) = shade_ray(sc, o, pl, EYE, rd, px_world);
                for k in 0..3 {
                    acc[k] += c[k] * a;
                }
                acc[3] += a;
            }
        }
        let a = acc[3] * inv;
        let i = x * 4;
        if a <= 0.0 {
            row[i..i + 4].fill(0);
            continue;
        }
        let dither = hash((y as u32).wrapping_mul(7919) ^ (x as u32).wrapping_mul(104_729)) - 0.5;
        for k in 0..3 {
            let lin = (acc[k] * inv) / a;
            row[i + k] = (lin_to_srgb(lin) * 255.0 + 0.5 + dither).clamp(0.0, 255.0) as u8;
        }
        row[i + 3] = (clamp01(a) * 255.0 + 0.5 + dither).clamp(0.0, 255.0) as u8;
    }
}

/// The static logo at one icon size.
pub fn render_icon(size: u32) -> Vec<u8> {
    render(&Opts::icon(size))
}

// ------------------------------------------------------------------------ ico

/// A multi-resolution .ico (BMP/DIB frames, no PNG dependency) with every `ICO_SIZES` size.
pub fn ico_bytes() -> Vec<u8> {
    ico_from(&ICO_SIZES.map(|s| (s, render_icon(s))))
}

/// Builds an .ico from already rendered straight-alpha RGBA frames.
pub fn ico_from(frames: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let dibs: Vec<Vec<u8>> = frames.iter().map(|(s, px)| dib_frame(*s, px)).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    out.extend_from_slice(&(frames.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * frames.len() as u32;
    for ((s, _), f) in frames.iter().zip(&dibs) {
        let b = if *s >= 256 { 0 } else { *s as u8 };
        out.extend_from_slice(&[b, b, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bpp
        out.extend_from_slice(&(f.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += f.len() as u32;
    }
    for f in dibs {
        out.extend_from_slice(&f);
    }
    out
}

fn dib_frame(size: u32, rgba: &[u8]) -> Vec<u8> {
    let n = size as usize;
    let mut out = Vec::with_capacity(40 + n * n * 5);
    // BITMAPINFOHEADER; height is doubled to cover the AND mask.
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(size as i32).to_le_bytes());
    out.extend_from_slice(&((size * 2) as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&((n * n * 4) as u32).to_le_bytes());
    out.extend_from_slice(&[0u8; 16]);
    // Pixels, bottom-up, BGRA.
    for y in (0..n).rev() {
        for x in 0..n {
            let i = (y * n + x) * 4;
            out.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
        }
    }
    // AND mask (all zero: alpha decides), rows padded to 32 bits.
    let row_bytes = n.div_ceil(32) * 4;
    out.extend(std::iter::repeat_n(0u8, row_bytes * n));
    out
}

// ------------------------------------------------------------------------ particles

/// One glowing mote of the About page particle field, in frame coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mote {
    /// Position in [0, 1] frame units (0, 0 = top left of the `half`-extent view; motes
    /// may fall slightly outside).
    pub x: f32,
    pub y: f32,
    /// View depth relative to the orb centre plane, world units; positive is in front.
    pub z: f32,
    /// Core radius in pixels at 1x for a 1x frame of `ref_size` logical pixels.
    pub r: f32,
    /// Opacity in [0, 1] before the layer opacity (envelope times twinkle).
    pub alpha: f32,
    /// 0..1 mix between the two particle colours.
    pub hue: f32,
}

/// Motes on ring A, ring B and drifting, in that order in `motes`.
pub const RING_A: usize = 28;
pub const RING_B: usize = 20;
pub const DRIFT: usize = 16;
/// Orbit radii in orb radii: far enough out that the rings clear the orb.
const RING_A_R: f32 = 1.52;
const RING_B_R: f32 = 1.68;
/// Seconds of orbit behind each front ring mote drawn as its trail, and samples in it.
pub const TRAIL_S: f64 = 0.42;
pub const TRAIL_SAMPLES: usize = 16;

/// The particle field at `time` seconds: ring A (`RING_A` motes, 1 rev / 8 s), ring B
/// (`RING_B` motes, counter-rotating, 1 rev / 13 s), both well outside the orb, and
/// `DRIFT` motes rising from below it, projected through the logo camera for a view of
/// half extent `half`. The order is stable, so index `i` is the same mote at every time.
pub fn motes(time: f64, half: f32) -> Vec<Mote> {
    field(time, half, true)
}

/// Short fading trails behind the ring motes in front of the orb (so the rings read as
/// spinning): `TRAIL_SAMPLES` small, dim motes per ring mote along the last `TRAIL_S`
/// seconds of its orbit, tapering away from the head. Paint them with the front motes,
/// before the heads.
pub fn trails(time: f64, half: f32) -> Vec<Mote> {
    let heads = field(time, half, false);
    let mut out = Vec::with_capacity(RING_A * TRAIL_SAMPLES);
    for k in 1..=TRAIL_SAMPLES {
        let f = k as f32 / (TRAIL_SAMPLES + 1) as f32;
        let past = field(time - TRAIL_S * f as f64, half, false);
        for (h, p) in heads.iter().zip(&past) {
            if h.z < 0.0 || p.z < 0.0 {
                continue;
            }
            let fade = (1.0 - f).powf(1.6);
            out.push(Mote {
                r: p.r * (0.75 - 0.4 * f),
                alpha: (h.alpha * 0.26 * fade).clamp(0.0, 1.0),
                ..*p
            });
        }
    }
    out
}

/// The field at `time`; without `drift` only the ring motes, which is all trails need.
fn field(time: f64, half: f32, drift: bool) -> Vec<Mote> {
    let cam = camera(half);
    let mut out = Vec::with_capacity(RING_A + RING_B + DRIFT);
    let twinkle = |i: u32| {
        let hz = 1.3 + hash(i ^ 0x51ed);
        0.75 + 0.25 * ((time * hz as f64).fract() as f32 * TAU).sin()
    };
    let mut push = |p: V, i: u32, env: f32| {
        let d = p - EYE;
        // Everything is within a couple of units of the target, far in front of the eye;
        // never skip a mote, so indices stay stable.
        let z = dot(d, cam.fwd).max(0.1);
        let u = dot(d, cam.right) / z / cam.scale;
        let v = dot(d, cam.up) / z / cam.scale;
        let z_view = cam.dist - z;
        let r0 = 1.3 + 0.9 * hash(i ^ 0x9e37);
        out.push(Mote {
            x: (u + 1.0) * 0.5,
            y: (1.0 - v) * 0.5,
            z: z_view,
            r: r0 * (1.0 + 0.30 * z_view).max(0.4),
            alpha: (0.85 * env * twinkle(i)).clamp(0.0, 1.0),
            hue: hash(i ^ 0x2545),
        });
    };
    // Ring A: evenly spread with a little jitter, so it reads as a ring.
    let a_ang = TAU as f64 * time / 8.0;
    for i in 0..RING_A as u32 {
        let r = ORB_R * RING_A_R * (1.0 + 0.04 * (2.0 * hash(i ^ 0xa11) - 1.0));
        let jitter = 0.35 * (hash(i ^ 0xa12) - 0.5);
        let th = (a_ang % TAU as f64) as f32 + (i as f32 + jitter) * TAU / RING_A as f32;
        let p = V(r * th.cos(), 0.0, r * th.sin());
        push(rot_z(rot_x(p, 0.38), -0.20), i, 1.0);
    }
    // Ring B, counter-rotating.
    let b_ang = -(TAU as f64) * time / 13.0;
    for i in 0..RING_B as u32 {
        let r = ORB_R * RING_B_R * (1.0 + 0.04 * (2.0 * hash(i ^ 0xb22) - 1.0));
        let jitter = 0.35 * (hash(i ^ 0xb23) - 0.5);
        let th = (b_ang % TAU as f64) as f32 + (i as f32 + jitter) * TAU / RING_B as f32 + 0.5;
        let p = V(r * th.cos(), 0.0, r * th.sin());
        push(rot_x(p, -0.22), 100 + i, 1.0);
    }
    if !drift {
        return out;
    }
    // Drift motes: staggered lifetimes, respawned at a new angle each cycle.
    for i in 0..DRIFT as u32 {
        let life_s = 5.0 + 2.0 * hash(i ^ 0xd33) as f64;
        let x = time / life_s + i as f64 / DRIFT as f64;
        let cycle = x.floor() as u32;
        let life = x.fract() as f32;
        let seed = i.wrapping_mul(977) ^ cycle.wrapping_mul(7919);
        let ang = hash(seed) * TAU;
        let rad = 0.9 + 0.4 * hash(seed ^ 0x77);
        let wob = 0.04 * ((TAU as f64 * time / 3.0) as f32 + i as f32).sin();
        let y = -0.6 + 0.12 * life * life_s as f32;
        let p = V(rad * ang.cos() + wob, y, rad * ang.sin());
        let env = smoothstep(0.0, 0.15, life) * (1.0 - smoothstep(0.6, 1.0, life));
        push(p, 200 + i, env);
    }
    out
}

/// Shared with the About page so particle colours match the shader palette.
pub const PARTICLE_DARK: [[u8; 3]; 2] = [[0x7f, 0xd3, 0x6a], [0xd4, 0xff, 0x86]];
pub const PARTICLE_LIGHT: [[u8; 3]; 2] = [[0x2a, 0x9a, 0x4c], [0x14, 0x6e, 0x34]];

/// One painted layer of a mote: colour (sRGB), opacity, additive blending.
pub type MoteLayer = ([u8; 3], f32, bool);

/// How to paint a mote: dark themes add the bright palette; light themes lay the darker
/// palette over the page, except where a front mote crosses the orb (`coverage` = orb
/// alpha under it), where the dark palette would read as specks of dirt, so it fades into
/// the bright additive palette and reads as a highlight. `layer` is the layer opacity.
pub fn mote_layers(m: &Mote, dark: bool, coverage: f32, layer: f32) -> [MoteLayer; 2] {
    let pick = |pal: [[u8; 3]; 2]| -> [u8; 3] {
        std::array::from_fn(|k| {
            (pal[0][k] as f32 + (pal[1][k] as f32 - pal[0][k] as f32) * m.hue).round() as u8
        })
    };
    let a = (m.alpha * layer).clamp(0.0, 1.0);
    if dark {
        return [(pick(PARTICLE_DARK), a, true), ([0; 3], 0.0, true)];
    }
    let c = if m.z >= 0.0 {
        coverage.clamp(0.0, 1.0)
    } else {
        0.0
    };
    [
        (pick(PARTICLE_LIGHT), a * (1.0 - c), false),
        (pick(PARTICLE_DARK), a * c, true),
    ]
}

/// Mote sprite: a core disc plus a wide halo, `a = exp(-d^2/2r^2) + 0.35 exp(-d^2/8r^2)`,
/// normalised so the centre is 1. `d` and `r` share units.
pub fn mote_alpha(d: f32, r: f32, halo: f32) -> f32 {
    let r2 = (r * r).max(1e-6);
    let a = (-d * d / (2.0 * r2)).exp() + 0.35 * halo * (-d * d / (8.0 * r2)).exp();
    a / (1.0 + 0.35 * halo)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(img: &[u8], size: usize, x: usize, y: usize) -> [u8; 4] {
        let i = (y * size + x) * 4;
        [img[i], img[i + 1], img[i + 2], img[i + 3]]
    }

    #[test]
    fn small_icons_are_green_orbs_with_clear_corners() {
        for size in [16u32, 20, 24, 32, 48] {
            let n = size as usize;
            let img = render_icon(size);
            assert_eq!(img.len(), n * n * 4);
            for (x, y) in [(0, 0), (n - 1, 0), (0, n - 1), (n - 1, n - 1)] {
                assert_eq!(px(&img, n, x, y)[3], 0, "{size} corner");
            }
            let c = px(&img, n, n / 2, n / 2);
            assert!(c[3] >= 250, "{size}: {c:?}");
            assert!(c[1] > c[0] && c[1] > c[2], "{size}: {c:?}");
        }
    }

    #[test]
    fn tray_sizes_fill_the_canvas() {
        for size in [16u32, 20, 24] {
            let img = render_icon(size);
            let solid = img.chunks(4).filter(|p| p[3] > 200).count();
            assert!(
                solid * 100 >= (size * size) as usize * 50,
                "{size}: {solid}"
            );
        }
    }

    #[test]
    fn animation_frames_differ() {
        let a = render(&Opts::about(48, 0.0, 1));
        let b = render(&Opts::about(48, 0.25, 1));
        assert_ne!(a, b);
    }

    #[test]
    fn threaded_render_matches_single_thread() {
        let o = Opts::icon(20);
        assert_eq!(render_threads(&o, 1), render_threads(&o, 5));
    }

    #[test]
    fn ico_header_lists_every_size() {
        let frames: Vec<(u32, Vec<u8>)> = [16u32, 256]
            .into_iter()
            .map(|s| (s, vec![0u8; (s * s * 4) as usize]))
            .collect();
        let ico = ico_from(&frames);
        assert_eq!(&ico[0..4], &[0, 0, 1, 0]);
        assert_eq!(u16::from_le_bytes([ico[4], ico[5]]), 2);
        assert_eq!(ico[6], 16);
        assert_eq!(ico[6 + 16], 0);
    }

    #[test]
    fn motes_orbit_and_fade() {
        let a = motes(0.0, ABOUT_HALF);
        let b = motes(1.0, ABOUT_HALF);
        assert_eq!(a.len(), RING_A + RING_B + DRIFT);
        assert_ne!(a[0], b[0]);
        // Some motes pass in front of the orb and some behind it.
        assert!(a.iter().any(|m| m.z > 0.0) && a.iter().any(|m| m.z < 0.0));
        assert!(a.iter().all(|m| (0.0..=1.0).contains(&m.alpha)));
        // The rings run outside the view horizontally but stay near it.
        assert!(
            a.iter()
                .all(|m| (-0.25..1.25).contains(&m.x) && (-0.1..1.1).contains(&m.y))
        );
    }

    #[test]
    fn rings_clear_the_orb() {
        // Ring motes beside the orb (near its centre plane) sit outside its silhouette.
        let orb_frac = ORB_R / ABOUT_HALF * 0.5;
        for m in &motes(2.0, ABOUT_HALF)[..RING_A + RING_B] {
            if m.z.abs() < 0.3 {
                let d = ((m.x - 0.5).powi(2) + (m.y - 0.5).powi(2)).sqrt();
                assert!(d > orb_frac * 1.2, "{m:?}");
            }
        }
    }

    #[test]
    fn front_ring_motes_have_trails() {
        let t = trails(3.0, ABOUT_HALF);
        assert!(!t.is_empty());
        assert!(t.iter().all(|m| m.z >= 0.0 && m.alpha < 0.3));
    }
}
