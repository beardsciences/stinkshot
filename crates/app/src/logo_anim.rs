//! The animated logo at the top of the About page: the gel orb spinning with its lumps
//! sliding and breathing (one seamless loop every 12 s, 6 s while hovered), a faint bloom
//! and pulse on dark themes. No particles: the user preferred the orb's own motion alone.
//!
//! The orb is ray marched by `stinkshot-logo`, which is far too slow for 60 fps on the CPU,
//! so the loop is pre-rendered once into `FRAMES` frames on worker threads (a still frame
//! first, shown until the loop is ready) and played back with a crossfade between
//! neighbouring frames. The two frames are blended on the CPU into one image and uploaded
//! to a single GPU texture (two overlaid textures add up alpha at the soft edges and the
//! shadow, which pulsed); the CPU frames are dropped a minute after the page was last
//! shown. Nothing animates (and no repaint
//! is requested) unless `show` is called, i.e. while the About page is visible, and not
//! while the window is minimized or covered. With Windows animations off
//! (SPI_GETCLIENTAREAANIMATION) the orb is a still frame and there is no bloom.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::{
    Align2, Color32, ColorImage, FontId, Mesh, Pos2, Rect, Sense, Shape, TextureHandle,
    TextureOptions, Ui, pos2, vec2,
};
use stinkshot_logo::{ABOUT_HALF, Opts};

/// Crossfade steps between neighbouring frames (the blend is redone only when the step
/// changes).
const BLEND_STEPS: f64 = 32.0;

/// Frames in one loop (spin + lump cycle), crossfaded on playback.
pub const FRAMES: usize = 96;
/// Seconds per loop, normally and while the pointer is over the orb.
const LOOP_S: f64 = 12.0;
const HOVER_LOOP_S: f64 = 6.0;
/// Phase of the still frame (reduced motion, and while the loop renders).
const STILL_T: f32 = 0.06;
/// Logical size of the square orb view (`ABOUT_HALF` world units across half of it).
pub const VIEW: f32 = 184.0;
/// Height the widget reserves.
pub const HEIGHT: f32 = 192.0;
/// Height of the name, version and tagline under the orb in `header`.
const HEADER_TEXT: f32 = 80.0;
/// The tagline under the version.
const TAGLINE: &str = "Local-first screenshots and screen recording for Windows";
/// CPU frames are freed this long after the page was last shown.
const KEEP: Duration = Duration::from_secs(60);

/// Rendered frames shared with the worker threads.
#[derive(Default)]
struct Frames {
    size: u32,
    still: Option<Arc<ColorImage>>,
    loop_frames: Vec<Option<Arc<ColorImage>>>,
    done: usize,
    started: Option<Instant>,
    /// Wall time to render the whole loop.
    took: Option<Duration>,
}

/// Timing of the About animation, for the logo notes and the example.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    /// Physical pixel size of the frames and supersampling per axis.
    pub size: u32,
    pub ss: u32,
    /// Wall time of the still frame and the full loop.
    pub still: Option<Duration>,
    pub loop_total: Option<Duration>,
    /// CPU time of `show` on the UI thread: last call and running mean / max (ms).
    pub show_ms: f32,
    pub show_mean_ms: f32,
    pub show_max_ms: f32,
    pub calls: u64,
}

struct Anim {
    frames: Arc<Mutex<Frames>>,
    cancel: Arc<AtomicBool>,
    /// Frame size the workers were started for and whether they render the loop.
    started_for: Option<(u32, bool)>,
    /// The orb texture and what it holds: (frame, blend step), frame `usize::MAX` = still.
    tex: Option<TextureHandle>,
    tex_key: (usize, u32),
    phase: f64,
    rate: f64,
    last_time: Option<f64>,
    reduced: bool,
    reduced_at: Option<Instant>,
    /// The window is unfocused and fully covered by the foreground window; re-checked
    /// every half second.
    hidden: bool,
    hidden_at: Option<Instant>,
    last_shown: Instant,
    reaper: bool,
    stats: Stats,
    still_took: Option<Duration>,
}

impl Default for Anim {
    fn default() -> Self {
        Anim {
            frames: Arc::default(),
            cancel: Arc::new(AtomicBool::new(false)),
            started_for: None,
            tex: None,
            tex_key: (usize::MAX - 1, 0),
            hidden: false,
            hidden_at: None,
            phase: 0.0,
            rate: 1.0 / LOOP_S,
            last_time: None,
            reduced: false,
            reduced_at: None,
            last_shown: Instant::now(),
            reaper: false,
            stats: Stats::default(),
            still_took: None,
        }
    }
}

type Handle = Arc<Mutex<Anim>>;

fn handle(ctx: &egui::Context) -> Handle {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_insert_with(egui::Id::unique("stinkshot_logo_anim"), Handle::default)
            .clone()
    })
}

/// Latest timing numbers (None before the first `show`).
#[allow(dead_code)] // read by examples/about_anim.rs
pub fn stats(ctx: &egui::Context) -> Option<Stats> {
    let h = handle(ctx);
    let a = h.lock().ok()?;
    (a.stats.calls > 0).then_some(a.stats)
}

/// The About page header: the animated orb, with the app name, version and tagline
/// centred under it.
pub fn header(ui: &mut Ui, title: FontId, body: FontId, text: Color32, secondary: Color32) {
    show(ui);
    // The orb's view has empty space under the contact shadow; pull the text up into it.
    ui.add_space(-12.0);
    let w = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(w, HEADER_TEXT), Sense::hover());
    let x = r.center().x;
    let painter = ui.painter();
    painter.text(
        pos2(x, r.top()),
        Align2::CENTER_TOP,
        "Stinkshot",
        title,
        text,
    );
    painter.text(
        pos2(x, r.top() + 32.0),
        Align2::CENTER_TOP,
        format!("Version {}", env!("CARGO_PKG_VERSION")),
        body.clone(),
        secondary,
    );
    painter.text(
        pos2(x, r.top() + 52.0),
        Align2::CENTER_TOP,
        TAGLINE,
        body,
        secondary,
    );
}

/// Draws the animated logo, centred in the available width, `HEIGHT` tall.
pub fn show(ui: &mut Ui) {
    let t0 = Instant::now();
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), HEIGHT), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    let ctx = ui.ctx().clone();
    let h = handle(&ctx);
    let Ok(mut a) = h.lock() else {
        return;
    };
    a.last_shown = Instant::now();
    if !a.reaper {
        a.reaper = true;
        spawn_reaper(h.clone());
    }

    // Windows "Show animations" setting, re-read every couple of seconds.
    if a.reduced_at
        .is_none_or(|t| t.elapsed() > Duration::from_secs(2))
    {
        a.reduced = !client_area_animation();
        a.reduced_at = Some(Instant::now());
    }
    let reduced = a.reduced;
    let dark = ui.visuals().dark_mode;
    let ppp = ctx.pixels_per_point();
    let size = (VIEW * ppp).round().max(16.0) as u32;
    let ss = if ppp < 1.5 { 2 } else { 1 };
    a.ensure_workers(&ctx, size, ss, !reduced);

    let view = Rect::from_center_size(rect.center(), vec2(VIEW, VIEW));
    let now = ui.input(|i| i.time);
    let dt = a.last_time.map_or(0.0, |t| (now - t).clamp(0.0, 0.1));
    a.last_time = Some(now);

    // Playhead: eases toward double speed while hovered.
    let target = if resp.hovered() {
        1.0 / HOVER_LOOP_S
    } else {
        1.0 / LOOP_S
    };
    let k = 1.0 - (-dt / 0.13).exp();
    a.rate += (target - a.rate) * k;
    let ready = {
        let f = a.frames.lock().map(|f| f.done >= FRAMES).unwrap_or(false);
        f && !reduced
    };
    if ready {
        a.phase = (a.phase + dt * a.rate).fract();
    }

    let painter = ui.painter_at(rect.expand(8.0));
    let f = a.phase * FRAMES as f64;
    let i0 = f.floor() as usize % FRAMES;

    // Behind the orb: a soft bloom on dark themes.
    if dark && !reduced {
        radial(
            &painter,
            view.center(),
            VIEW * 0.5 * (stinkshot_logo::ORB_R * 1.25 / ABOUT_HALF),
            Color32::from_rgb(0x7f, 0xd3, 0x6a),
            0.18,
            true,
        );
    }

    // The orb: current frame, crossfaded into the next on the CPU.
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    let key = if ready {
        (i0, ((f - f.floor()) * BLEND_STEPS).floor() as u32)
    } else {
        (usize::MAX, 0)
    };
    if let Some(t) = a.texture(&ctx, key) {
        painter.image(t, view, uv, Color32::WHITE);
    }

    // Idle pulse of the nucleus (the frames are fixed, so it is a soft overlay).
    if !reduced {
        let pulse = 0.12 * (std::f64::consts::TAU * now / 3.0).sin() as f32;
        if pulse > 0.0 {
            radial(
                &painter,
                view.center() + vec2(0.0, VIEW * 0.04),
                VIEW * 0.26,
                Color32::from_rgb(0xd4, 0xff, 0x86),
                pulse * if dark { 0.9 } else { 0.6 },
                dark,
            );
        }
    }

    // No repaints while nobody can see them: minimized, or unfocused under a window that
    // covers this one. Restoring or uncovering the window repaints it anyway (the OS sends
    // a resize or a paint); the slow poll is a backstop.
    let (minimized, focused, occluded) = ctx.input(|i| {
        let v = i.viewport();
        (
            v.minimized.unwrap_or(false),
            v.focused.unwrap_or(true),
            v.occluded.unwrap_or(false),
        )
    });
    if focused {
        a.hidden = false;
    } else if a
        .hidden_at
        .is_none_or(|t| t.elapsed() > Duration::from_millis(500))
    {
        a.hidden = ctx.input(|i| {
            let v = i.viewport();
            let ppp = v.native_pixels_per_point.unwrap_or(ppp);
            v.outer_rect
                .or(v.inner_rect)
                .is_some_and(|r| covered(r, ppp))
        });
        a.hidden_at = Some(Instant::now());
    }
    if minimized {
        // Nothing: restoring the window repaints it.
    } else if occluded || (!focused && a.hidden) {
        ctx.request_repaint_after(Duration::from_secs(1));
    } else if !reduced {
        ctx.request_repaint();
    } else {
        ctx.request_repaint_after(Duration::from_secs(2));
    }

    let ms = t0.elapsed().as_secs_f32() * 1000.0;
    let s = &mut a.stats;
    s.calls += 1;
    s.show_ms = ms;
    s.show_mean_ms += (ms - s.show_mean_ms) / s.calls as f32;
    s.show_max_ms = s.show_max_ms.max(ms);
}

impl Anim {
    /// Starts (or restarts, on a DPI change) the render workers.
    fn ensure_workers(&mut self, ctx: &egui::Context, size: u32, ss: u32, want_loop: bool) {
        let have = self.started_for;
        let need_restart = match have {
            None => true,
            Some((s, looped)) => s != size || (want_loop && !looped),
        };
        if !need_restart {
            self.update_stats(size, ss);
            return;
        }
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        let keep_still = have.is_some_and(|(s, _)| s == size);
        {
            let mut f = match self.frames.lock() {
                Ok(f) => f,
                Err(_) => return,
            };
            if !keep_still {
                *f = Frames::default();
                f.size = size;
                self.tex_key = (usize::MAX - 1, 0);
                self.still_took = None;
            }
            f.loop_frames = vec![None; FRAMES];
            f.done = 0;
            f.took = None;
        }
        self.started_for = Some((size, want_loop));
        let frames = self.frames.clone();
        let cancel = self.cancel.clone();
        let ctx = ctx.clone();
        let still_needed = !keep_still;
        let spawned = std::thread::Builder::new()
            .name("logo-anim".into())
            .spawn(move || render_all(frames, cancel, ctx, size, ss, still_needed, want_loop));
        if let Err(e) = spawned {
            log::warn!("about logo: cannot start render thread: {e}");
        }
        self.update_stats(size, ss);
    }

    fn update_stats(&mut self, size: u32, ss: u32) {
        self.stats.size = size;
        self.stats.ss = ss;
        if let Ok(f) = self.frames.lock() {
            self.stats.loop_total = f.took;
            if let (Some(start), None) = (f.started, self.still_took)
                && f.still.is_some()
            {
                self.still_took = Some(start.elapsed());
            }
        }
        self.stats.still = self.still_took;
    }

    /// The orb texture for `key` = (frame, blend step) (`usize::MAX` = still): frame `i`
    /// blended toward frame `i + 1` by `step / BLEND_STEPS` on the CPU, then uploaded.
    /// Re-uploads only when the key changes.
    fn texture(&mut self, ctx: &egui::Context, key: (usize, u32)) -> Option<egui::TextureId> {
        if self.tex_key == key
            && let Some(t) = &self.tex
        {
            return Some(t.id());
        }
        let img = {
            let f = self.frames.lock().ok()?;
            let (idx, step) = key;
            if idx == usize::MAX {
                f.still.clone()?
            } else {
                let a = f.loop_frames.get(idx)?.clone()?;
                let b = f.loop_frames.get((idx + 1) % FRAMES)?.clone()?;
                drop(f);
                Arc::new(blend(&a, &b, step as f32 / BLEND_STEPS as f32))
            }
        };
        match &mut self.tex {
            Some(t) => t.set(img, TextureOptions::LINEAR),
            None => self.tex = Some(ctx.load_texture("about-logo", img, TextureOptions::LINEAR)),
        }
        self.tex_key = key;
        self.tex.as_ref().map(|t| t.id())
    }
}

/// Worker: still frame on every core, then the loop frames, one per thread.
fn render_all(
    frames: Arc<Mutex<Frames>>,
    cancel: Arc<AtomicBool>,
    ctx: egui::Context,
    size: u32,
    ss: u32,
    still: bool,
    want_loop: bool,
) {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let start = Instant::now();
    if let Ok(mut f) = frames.lock() {
        f.started = Some(start);
    }
    let to_image = |rgba: Vec<u8>| {
        Arc::new(ColorImage::from_rgba_unmultiplied(
            [size as usize, size as usize],
            &rgba,
        ))
    };
    if still {
        let img = to_image(stinkshot_logo::render_threads(
            &Opts::about(size, STILL_T, ss),
            cores,
        ));
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        if let Ok(mut f) = frames.lock() {
            f.still = Some(img);
        }
        ctx.request_repaint();
    }
    if !want_loop {
        return;
    }
    // Leave a core for the UI.
    let workers = cores.saturating_sub(1).clamp(1, 8);
    let next = AtomicUsize::new(0);
    let loop_start = Instant::now();
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= FRAMES {
                        return;
                    }
                    let t = i as f32 / FRAMES as f32;
                    let img =
                        to_image(stinkshot_logo::render_threads(&Opts::about(size, t, ss), 1));
                    let Ok(mut f) = frames.lock() else {
                        return;
                    };
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    f.loop_frames[i] = Some(img);
                    f.done += 1;
                    if f.done == FRAMES {
                        f.took = Some(loop_start.elapsed());
                        log::debug!(
                            "about logo: {FRAMES} frames at {size} px (ss {ss}) in {:?}",
                            loop_start.elapsed()
                        );
                        ctx.request_repaint();
                    }
                }
            });
        }
    });
}

/// `a` crossfaded toward `b` by `mix`. Both are premultiplied, so a per-channel lerp is
/// the correct mix: coverage never exceeds either frame's, unlike stacking two textures.
fn blend(a: &ColorImage, b: &ColorImage, mix: f32) -> ColorImage {
    if mix <= 0.0 || a.size != b.size {
        return a.clone();
    }
    let m = (mix.clamp(0.0, 1.0) * 256.0) as u32;
    let l = |x: u8, y: u8| ((x as u32 * (256 - m) + y as u32 * m + 128) >> 8) as u8;
    let pixels = a
        .pixels
        .iter()
        .zip(&b.pixels)
        .map(|(p, q)| {
            Color32::from_rgba_premultiplied(
                l(p.r(), q.r()),
                l(p.g(), q.g()),
                l(p.b(), q.b()),
                l(p.a(), q.a()),
            )
        })
        .collect();
    ColorImage {
        size: a.size,
        source_size: a.source_size,
        pixels,
    }
}

/// True when the foreground window (another window: this one is unfocused) covers `rect`,
/// this window's rectangle in points. Cheap: one foreground lookup and one rectangle.
fn covered(rect: Rect, ppp: f32) -> bool {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect, IsIconic};
    // SAFETY: plain queries on a window handle; a stale handle just fails.
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() || IsIconic(fg).as_bool() {
            return false;
        }
        let mut r = RECT::default();
        if GetWindowRect(fg, &mut r).is_err() {
            return false;
        }
        // A couple of pixels of slack for rounding.
        r.left as f32 <= rect.min.x * ppp + 2.0
            && r.top as f32 <= rect.min.y * ppp + 2.0
            && r.right as f32 >= rect.max.x * ppp - 2.0
            && r.bottom as f32 >= rect.max.y * ppp - 2.0
    }
}

/// Frees the frames and textures once the About page has been out of sight for `KEEP`.
fn spawn_reaper(h: Handle) {
    let spawned = std::thread::Builder::new()
        .name("logo-anim-reaper".into())
        .spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(10));
                let Ok(mut a) = h.lock() else {
                    return;
                };
                if a.last_shown.elapsed() > KEEP {
                    a.cancel.store(true, Ordering::Relaxed);
                    *a = Anim::default();
                    return;
                }
            }
        });
    if let Err(e) = spawned {
        log::debug!("about logo: no reaper thread: {e}");
    }
}

/// A soft radial glow (gaussian-ish falloff) as a triangle fan.
fn radial(painter: &egui::Painter, c: Pos2, radius: f32, color: Color32, alpha: f32, add: bool) {
    const RINGS: usize = 8;
    const SEG: usize = 48;
    let col = |a: f32| {
        let a = a.clamp(0.0, 1.0);
        if add {
            Color32::from_rgba_premultiplied(
                (color.r() as f32 * a) as u8,
                (color.g() as f32 * a) as u8,
                (color.b() as f32 * a) as u8,
                0,
            )
        } else {
            Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), (a * 255.0) as u8)
        }
    };
    let mut mesh = Mesh::default();
    mesh.colored_vertex(c, col(alpha));
    for ring in 1..=RINGS {
        let f = ring as f32 / RINGS as f32;
        let a = alpha * (-f * f * 3.0).exp() * (1.0 - f);
        for s in 0..SEG {
            let th = s as f32 / SEG as f32 * std::f32::consts::TAU;
            mesh.colored_vertex(c + vec2(th.cos(), th.sin()) * radius * f, col(a));
        }
    }
    for s in 0..SEG as u32 {
        let n = (s + 1) % SEG as u32;
        mesh.add_triangle(0, 1 + s, 1 + n);
    }
    for ring in 1..RINGS as u32 {
        let (i0, i1) = (1 + (ring - 1) * SEG as u32, 1 + ring * SEG as u32);
        for s in 0..SEG as u32 {
            let n = (s + 1) % SEG as u32;
            mesh.add_triangle(i0 + s, i1 + s, i1 + n);
            mesh.add_triangle(i0 + s, i1 + n, i0 + n);
        }
    }
    painter.add(Shape::mesh(mesh));
}

/// Windows "Show animations in Windows" (SPI_GETCLIENTAREAANIMATION); true when unknown.
fn client_area_animation() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    use windows::core::BOOL;
    let mut on = BOOL(1);
    // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL into the pointer we pass.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut on as *mut BOOL as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    ok.is_err() || on.as_bool()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossfade_never_adds_up_alpha() {
        // A half-transparent edge pixel in both frames stays half transparent mid-fade
        // (two stacked textures would reach 75%).
        let px = Color32::from_rgba_premultiplied(40, 80, 20, 128);
        let a = ColorImage::new([2, 1], vec![px, Color32::TRANSPARENT]);
        let b = ColorImage::new([2, 1], vec![px, px]);
        let m = blend(&a, &b, 0.5);
        assert_eq!(m.pixels[0], px);
        assert_eq!(m.pixels[1].a(), 64);
        assert_eq!(blend(&a, &b, 0.0).pixels, a.pixels);
    }
}
