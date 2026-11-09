//! The animated logo at the top of the About page: the gel orb spinning with its lumps
//! sliding and breathing (one seamless loop every 12 s, 6 s while hovered), a faint bloom
//! and pulse on dark themes. No particles: the user preferred the orb's own motion alone.
//!
//! The orb is ray marched by `stinkshot-logo`, which is far too slow for 60 fps on the CPU,
//! so the loop is pre-rendered into `FRAMES` frames and played back with a crossfade
//! between neighbouring frames (blended on the CPU into one image and uploaded to a single
//! GPU texture: two overlaid textures add up alpha at the soft edges and the shadow).
//!
//! Getting the frames without a visible wait:
//! - A disk cache (`cache_path`) holds the finished loop as PNG frames, keyed by the
//!   renderer's source hash, the pixel size and the supersampling. A warm start decodes
//!   it in playback order, so the orb moves within a frame or two of the page appearing.
//! - The Settings window calls `prewarm` (and the shell calls `prewarm_cache` once, a while
//!   after startup), which loads or renders the loop in the background at low thread
//!   priority on half the cores.
//! - Cold and visible, frames are rendered in playback order from the playhead, each one
//!   split over all the worker threads by rows. When the next frame to show is missing, a
//!   1x-supersampled preview of it (about 5x cheaper) is rendered first; the full-quality
//!   frame replaces it before long. Playback only advances onto frames that exist, so it
//!   never blends toward a frame from elsewhere in the loop.
//!
//! The CPU frames are dropped a minute after the page was last shown (or prewarmed) once
//! nothing is rendering. Nothing animates (and no repaint is requested) unless `show` is
//! called, i.e. while the About page is visible, and not while the window is minimized or
//! covered. With Windows animations off (SPI_GETCLIENTAREAANIMATION) the orb is a still
//! frame and there is no bloom.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::l10n::tr;
use egui::{
    Align2, Color32, ColorImage, FontId, Mesh, Pos2, Rect, Sense, Shape, TextureHandle,
    TextureOptions, Ui, pos2, vec2,
};
use stinkshot_logo::{ABOUT_HALF, Opts, Prepared};

/// Crossfade steps between neighbouring frames (the blend is redone only when the step
/// changes).
const BLEND_STEPS: f64 = 32.0;

/// Frames in one loop (spin + lump cycle), crossfaded on playback.
pub const FRAMES: usize = 96;
/// The loop frame shown still (reduced motion), and where playback starts.
const STILL: usize = 6;
/// Seconds per loop, normally and while the pointer is over the orb.
const LOOP_S: f64 = 12.0;
const HOVER_LOOP_S: f64 = 6.0;
/// Logical size of the square orb view (`ABOUT_HALF` world units across half of it).
pub const VIEW: f32 = 184.0;
/// Height the widget reserves.
pub const HEIGHT: f32 = 192.0;
/// Height of the name, version and tagline under the orb in `header`.
const HEADER_TEXT: f32 = 80.0;
/// CPU frames are freed this long after the page was last shown or prewarmed.
const KEEP: Duration = Duration::from_secs(60);
/// Frames past the playhead kept covered by previews while rendering on screen (`pick`):
/// 500 ms of playback, more than a full frame takes with 3 worker threads.
const PREVIEW_AHEAD: usize = 4;
/// Bumped when the cache file layout changes.
const CACHE_FORMAT: u64 = 1;
const CACHE_MAGIC: &[u8; 8] = b"STKORB1\n";
/// The cache folder is pruned to this many bytes (a loop is about 4 MB at 230 px).
const CACHE_BUDGET: u64 = 48 << 20;

/// Rendered frames shared with the worker threads.
#[derive(Default)]
struct Frames {
    size: u32,
    ss: u32,
    /// One slot per loop frame: a preview or the full-quality frame.
    slots: Vec<Option<Arc<ColorImage>>>,
    /// The slot holds the full-quality frame.
    full: Vec<bool>,
    /// Bumped whenever a slot changes, so the blended texture is redone.
    ver: Vec<u32>,
    full_done: usize,
    /// The frames came from the disk cache.
    from_cache: bool,
    /// A worker is loading or rendering.
    running: bool,
    /// Wall time of the run that completed the loop.
    took: Option<Duration>,
}

impl Frames {
    fn reset(&mut self, size: u32, ss: u32) {
        *self = Frames {
            size,
            ss,
            slots: vec![None; FRAMES],
            full: vec![false; FRAMES],
            ver: vec![0; FRAMES],
            ..Frames::default()
        };
    }

    fn has(&self, i: usize) -> bool {
        self.slots.get(i % FRAMES).is_some_and(|s| s.is_some())
    }

    /// Stores frame `i`; a preview never replaces anything, a full frame replaces a
    /// preview. False when the frames were reset for another size.
    fn put(&mut self, size: u32, ss: u32, i: usize, img: Arc<ColorImage>, full: bool) -> bool {
        if self.size != size || self.ss != ss || i >= self.slots.len() {
            return false;
        }
        if self.full[i] || (!full && self.slots[i].is_some()) {
            return true;
        }
        self.slots[i] = Some(img);
        self.ver[i] = self.ver[i].wrapping_add(1);
        if full {
            self.full[i] = true;
            self.full_done += 1;
        }
        true
    }
}

/// The next frame to produce: (index, supersampling), or None when the loop is complete.
/// On screen, previews (1x supersampling) first for any frame from the playhead to
/// `PREVIEW_AHEAD` frames past it that has nothing yet; then full-quality frames in
/// playback order starting one past the playhead once the frame on screen has something
/// (it is about to be left behind, so it gets its full frame last). The preview window covers more than one full
/// frame's render time even on a slow machine, so playback never waits on a full frame.
fn pick(f: &Frames, hint: usize, want_loop: bool, preview: bool) -> Option<(usize, u32)> {
    if !want_loop {
        return (!f.full[STILL]).then_some((STILL, f.ss));
    }
    if preview && f.ss > 1 {
        for k in 0..=PREVIEW_AHEAD {
            let i = (hint + k) % FRAMES;
            if f.slots[i].is_none() {
                return Some((i, 1));
            }
        }
    }
    // Past the frame on screen only once it has something to show (without previews,
    // e.g. at 1x supersampling, it may still be empty).
    let start = if f.slots[hint % FRAMES].is_some() {
        hint + 1
    } else {
        hint
    };
    (0..FRAMES)
        .map(|k| (start + k) % FRAMES)
        .find(|&i| !f.full[i])
        .map(|i| (i, f.ss))
}

/// Shared with the worker: stop, whether the page is on screen (worker priority and
/// thread count) and the playhead frame (render order).
struct Ctl {
    cancel: AtomicBool,
    visible: AtomicBool,
    hint: AtomicUsize,
}

impl Ctl {
    fn new(visible: bool, hint: usize) -> Arc<Ctl> {
        Arc::new(Ctl {
            cancel: AtomicBool::new(false),
            visible: AtomicBool::new(visible),
            hint: AtomicUsize::new(hint),
        })
    }
}

/// Timing of the About animation, for the logo notes and the example. Times are from the
/// first `show` after the frames were (re)created.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    /// Physical pixel size of the frames and supersampling per axis.
    pub size: u32,
    pub ss: u32,
    /// The loop came from the disk cache.
    pub from_cache: bool,
    /// First orb image on screen, first movement, every frame at full quality.
    pub first_frame: Option<Duration>,
    pub first_motion: Option<Duration>,
    pub all_full: Option<Duration>,
    /// Playback time spent held on the last existing frame after the first movement,
    /// waiting for the next one (stops and starts while the loop renders).
    pub held: Duration,
    /// Wall time of the worker run that completed the loop (render or cache load).
    pub loop_total: Option<Duration>,
    /// CPU time of `show` on the UI thread: last call and running mean / max (ms).
    pub show_ms: f32,
    pub show_mean_ms: f32,
    pub show_max_ms: f32,
    pub calls: u64,
}

struct Anim {
    frames: Arc<Mutex<Frames>>,
    ctl: Arc<Ctl>,
    /// Frame size and supersampling the worker was started for, and whether it does the
    /// whole loop.
    started_for: Option<(u32, u32, bool)>,
    /// The orb texture and what it holds: (frame a, frame b, blend step, versions).
    tex: Option<TextureHandle>,
    tex_key: Option<(usize, usize, u32, u32, u32)>,
    phase: f64,
    rate: f64,
    last_time: Option<f64>,
    reduced: bool,
    reduced_at: Option<Instant>,
    /// The window is unfocused and fully covered by the foreground window; re-checked
    /// every half second.
    hidden: bool,
    hidden_at: Option<Instant>,
    /// Last `show` or `prewarm` (the reaper's clock) and last `show` alone.
    last_wanted: Instant,
    last_shown: Option<Instant>,
    /// First `show` of this set of frames (the stats' clock).
    opened: Option<Instant>,
    /// `prewarm` was asked for this size and starts the worker on its next call.
    prewarm_pending: Option<(u32, u32)>,
    reaper: bool,
    stats: Stats,
}

impl Default for Anim {
    fn default() -> Self {
        Anim {
            frames: Arc::default(),
            ctl: Ctl::new(false, STILL),
            started_for: None,
            tex: None,
            tex_key: None,
            hidden: false,
            hidden_at: None,
            phase: STILL as f64 / FRAMES as f64,
            rate: 1.0 / LOOP_S,
            last_time: None,
            reduced: false,
            reduced_at: None,
            last_wanted: Instant::now(),
            last_shown: None,
            opened: None,
            prewarm_pending: None,
            reaper: false,
            stats: Stats::default(),
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

/// Frame size and supersampling for a display scale.
fn frame_size(ppp: f32) -> (u32, u32) {
    let size = (VIEW * ppp).round().max(16.0) as u32;
    let ss = if ppp < 1.5 { 2 } else { 1 };
    (size, ss)
}

/// Loads (or renders) the loop in the background at low priority so the About page's orb
/// moves as soon as it is shown. Cheap enough to call every frame; the Settings window
/// does, at its own display scale.
pub fn prewarm(ctx: &egui::Context) {
    prewarm_at(ctx, ctx.pixels_per_point(), true);
}

/// Renders the loop once in the background when the disk cache has no copy for display
/// scale `ppp`, so even the first visit to the About page is instant. Does nothing when the
/// cache is warm or the frames are already loading.
pub fn prewarm_cache(ctx: &egui::Context, ppp: f32) {
    // Frames already loaded or loading are for whatever is on screen (Settings may be at
    // another display scale than the system's), and that run writes its own cache.
    let busy = handle(ctx)
        .lock()
        .is_ok_and(|a| a.started_for.is_some() || a.prewarm_pending.is_some());
    if busy {
        return;
    }
    let (size, ss) = frame_size(ppp);
    if cache_enabled() && cache_path(size, ss).is_some_and(|p| !p.exists()) {
        prewarm_at(ctx, ppp, false);
    }
}

/// `defer`: start on the next call rather than this one (see below).
fn prewarm_at(ctx: &egui::Context, ppp: f32, defer: bool) {
    let h = handle(ctx);
    let Ok(mut a) = h.lock() else {
        return;
    };
    a.last_wanted = Instant::now();
    if a.last_shown
        .is_none_or(|t| t.elapsed() > Duration::from_secs(1))
    {
        a.ctl.visible.store(false, Ordering::Relaxed);
    }
    if !a.reaper {
        a.reaper = true;
        spawn_reaper(h.clone());
    }
    let (size, ss) = frame_size(ppp);
    if a.started_for.is_some_and(|(s, q, _)| s == size && q == ss) {
        return;
    }
    // Start on the next frame, not this one: when the About page is being opened, its
    // `show` runs later in this frame and starts the worker in the foreground instead.
    if defer && a.prewarm_pending != Some((size, ss)) {
        a.prewarm_pending = Some((size, ss));
        ctx.request_repaint();
        return;
    }
    let want_loop = client_area_animation();
    a.ensure_worker(ctx, size, ss, want_loop, false);
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
        tr!("shell.about.version", v = env!("CARGO_PKG_VERSION")),
        body.clone(),
        secondary,
    );
    painter.text(
        pos2(x, r.top() + 52.0),
        Align2::CENTER_TOP,
        tr!("shell.about.tagline"),
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
    a.last_wanted = Instant::now();
    a.last_shown = Some(Instant::now());
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
    let (size, ss) = frame_size(ctx.pixels_per_point());
    a.ensure_worker(&ctx, size, ss, !reduced, true);
    // After `ensure_worker`, which restarts the clock for a new set of frames.
    let opened = *a.opened.get_or_insert_with(Instant::now);

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

    // Which frames exist decides how far the playhead may go: onto a frame only once it
    // exists, and it holds on the last one rather than blending toward a missing one.
    let n = FRAMES as f64;
    let f0 = a.phase * n;
    let i0 = f0.floor() as usize % FRAMES;
    let (has, all_full, from_cache, took) = match a.frames.lock() {
        Ok(fr) => (
            [fr.has(i0), fr.has(i0 + 1), fr.has(i0 + 2)],
            fr.full_done >= FRAMES,
            fr.from_cache,
            fr.took,
        ),
        Err(_) => return,
    };
    if !reduced && has[0] && has[1] {
        let cap = if has[2] {
            f0.floor() + 2.0 - 1e-6
        } else {
            f0.floor() + 1.0
        };
        let want = f0 + dt * a.rate * n;
        let f1 = want.min(cap);
        if f1 > f0 && a.stats.first_motion.is_none() {
            a.stats.first_motion = Some(opened.elapsed());
        }
        if want > cap && a.stats.first_motion.is_some() {
            a.stats.held += Duration::from_secs_f64(dt);
        }
        a.phase = (f1 / n).fract();
    } else if !reduced && has[0] && a.stats.first_motion.is_some() {
        a.stats.held += Duration::from_secs_f64(dt);
    }
    let f = a.phase * n;
    let i = f.floor() as usize % FRAMES;
    a.ctl.hint.store(i, Ordering::Relaxed);
    a.ctl.visible.store(true, Ordering::Relaxed);

    let painter = ui.painter_at(rect.expand(8.0));

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

    // The orb: the current frame, crossfaded into the next on the CPU.
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    let (fi, step) = if reduced {
        (STILL, 0)
    } else {
        (i, ((f - f.floor()) * BLEND_STEPS).floor() as u32)
    };
    if let Some(t) = a.texture(&ctx, fi, step) {
        painter.image(t, view, uv, Color32::WHITE);
        if a.stats.first_frame.is_none() {
            a.stats.first_frame = Some(opened.elapsed());
        }
    }
    if all_full && a.stats.all_full.is_none() {
        a.stats.all_full = Some(opened.elapsed());
        log::debug!(
            "about logo: {} px ss {}, {} frames {} in {:?}; first image {:?}, first motion \
             {:?}, all full quality {:?} after the page appeared; held {:?}",
            size,
            ss,
            FRAMES,
            if from_cache { "loaded" } else { "rendered" },
            took,
            a.stats.first_frame,
            a.stats.first_motion,
            a.stats.all_full,
            a.stats.held
        );
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
    let ppp = ctx.pixels_per_point();
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
    } else if !reduced || !has[0] {
        ctx.request_repaint();
    } else {
        ctx.request_repaint_after(Duration::from_secs(2));
    }

    let ms = t0.elapsed().as_secs_f32() * 1000.0;
    let s = &mut a.stats;
    s.size = size;
    s.ss = ss;
    s.from_cache = from_cache;
    s.loop_total = took;
    s.calls += 1;
    s.show_ms = ms;
    s.show_mean_ms += (ms - s.show_mean_ms) / s.calls as f32;
    s.show_max_ms = s.show_max_ms.max(ms);
}

impl Anim {
    /// Starts the worker unless one already covers `size`/`ss` (and the whole loop when
    /// `want_loop`). A new size (DPI change) starts over; asking for the loop after only
    /// the still frame keeps what exists.
    fn ensure_worker(
        &mut self,
        ctx: &egui::Context,
        size: u32,
        ss: u32,
        want_loop: bool,
        visible: bool,
    ) {
        let same_size = self
            .started_for
            .is_some_and(|(s, q, _)| s == size && q == ss);
        if same_size && self.started_for.is_some_and(|(_, _, l)| l || !want_loop) {
            return;
        }
        self.ctl.cancel.store(true, Ordering::Relaxed);
        let hint = self.ctl.hint.load(Ordering::Relaxed);
        self.ctl = Ctl::new(visible, hint);
        {
            let Ok(mut f) = self.frames.lock() else {
                return;
            };
            if !same_size {
                f.reset(size, ss);
                self.tex_key = None;
                self.opened = None;
                self.stats = Stats::default();
            }
            f.running = true;
        }
        self.started_for = Some((size, ss, want_loop));
        let frames = self.frames.clone();
        let ctl = self.ctl.clone();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("logo-anim".into())
            .spawn(move || {
                run(&frames, &ctl, &ctx, size, ss, want_loop);
                if let Ok(mut f) = frames.lock()
                    && !ctl.cancel.load(Ordering::Relaxed)
                {
                    f.running = false;
                }
            });
        if let Err(e) = spawned {
            log::warn!("about logo: cannot start render thread: {e}");
            if let Ok(mut f) = self.frames.lock() {
                f.running = false;
            }
        }
    }

    /// The orb texture for frame `i` blended toward frame `i + 1` by `step / BLEND_STEPS`
    /// on the CPU, then uploaded. Falls back to the nearest earlier frame that exists, and
    /// to no blend when `i + 1` is missing. Re-uploads only when what it shows changes.
    fn texture(&mut self, ctx: &egui::Context, i: usize, step: u32) -> Option<egui::TextureId> {
        let (key, a, b) = {
            let f = self.frames.lock().ok()?;
            let ia = (0..FRAMES)
                .map(|k| (i + FRAMES - k) % FRAMES)
                .find(|&j| f.has(j))?;
            let ib = (ia + 1) % FRAMES;
            let (ib, step) = if ia == i && step > 0 && f.has(ib) {
                (ib, step)
            } else {
                (usize::MAX, 0)
            };
            let vb = if ib == usize::MAX { 0 } else { f.ver[ib] };
            let key = (ia, ib, step, f.ver[ia], vb);
            if self.tex_key == Some(key)
                && let Some(t) = &self.tex
            {
                return Some(t.id());
            }
            let a = f.slots[ia].clone()?;
            let b = if ib == usize::MAX {
                None
            } else {
                f.slots[ib].clone()
            };
            (key, a, b)
        };
        let img = match b {
            Some(b) => Arc::new(blend(&a, &b, step as f32 / BLEND_STEPS as f32)),
            None => a,
        };
        match &mut self.tex {
            Some(t) => t.set(img, TextureOptions::LINEAR),
            None => self.tex = Some(ctx.load_texture("about-logo", img, TextureOptions::LINEAR)),
        }
        self.tex_key = Some(key);
        self.tex.as_ref().map(|t| t.id())
    }
}

/// Worker threads: on screen, all cores but one (at most 8); in the background, half the
/// cores (at most 4). `STINKSHOT_LOGO_CORES=<n>` pretends the machine has `n` cores (for
/// measuring slower machines).
fn worker_count(visible: bool) -> usize {
    let cores = std::env::var("STINKSHOT_LOGO_CORES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
        });
    if visible {
        cores.saturating_sub(1).clamp(1, 8)
    } else {
        (cores / 2).clamp(1, 4)
    }
}

/// The worker: the disk cache if it has the loop, otherwise render frame after frame
/// (`pick`), then save the finished loop to the cache.
fn run(
    frames: &Mutex<Frames>,
    ctl: &Ctl,
    ctx: &egui::Context,
    size: u32,
    ss: u32,
    want_loop: bool,
) {
    let start = Instant::now();
    set_priority(ctl);
    let cache = cache_enabled().then(|| cache_path(size, ss)).flatten();
    if let Some(path) = &cache
        && load_cache(frames, ctl, ctx, path, size, ss)
    {
        if let Ok(mut f) = frames.lock()
            && f.full_done >= FRAMES
        {
            f.from_cache = true;
            f.took = Some(start.elapsed());
        }
        log::debug!(
            "about logo: loaded {FRAMES} frames from the cache in {:?}",
            start.elapsed()
        );
        return;
    }
    let mut rendered = 0usize;
    loop {
        if ctl.cancel.load(Ordering::Relaxed) {
            return;
        }
        let hint = ctl.hint.load(Ordering::Relaxed);
        let visible = ctl.visible.load(Ordering::Relaxed);
        let job = match frames.lock() {
            Ok(f) => pick(&f, hint, want_loop, visible),
            Err(_) => return,
        };
        let Some((i, q)) = job else {
            break;
        };
        set_priority(ctl);
        let t = i as f32 / FRAMES as f32;
        let Some(rgba) = render_frame(&Opts::about(size, t, q), worker_count(visible), ctl) else {
            return;
        };
        let img = Arc::new(ColorImage::from_rgba_unmultiplied(
            [size as usize, size as usize],
            &rgba,
        ));
        let Ok(mut f) = frames.lock() else {
            return;
        };
        if ctl.cancel.load(Ordering::Relaxed) || !f.put(size, ss, i, img, q == ss) {
            return;
        }
        rendered += 1;
        if f.full_done >= FRAMES {
            f.took = Some(start.elapsed());
        }
        drop(f);
        ctx.request_repaint();
    }
    let all: Option<Vec<Arc<ColorImage>>> = frames
        .lock()
        .ok()
        .filter(|f| f.full_done >= FRAMES)
        .map(|f| f.slots.iter().flatten().cloned().collect());
    log::debug!(
        "about logo: rendered {rendered} frames at {size} px (ss {ss}) in {:?}",
        start.elapsed()
    );
    if let (Some(path), Some(all)) = (cache, all) {
        // Saving is never urgent.
        ctl.visible.store(false, Ordering::Relaxed);
        set_priority(ctl);
        let t = Instant::now();
        match save_cache(&path, &all, size, ss) {
            Ok(bytes) => log::debug!(
                "about logo: cached {bytes} bytes in {:?} at {}",
                t.elapsed(),
                path.display()
            ),
            Err(e) => log::warn!("about logo: cannot write {}: {e}", path.display()),
        }
    }
}

/// Renders one frame on `threads` threads (this one included), handing out rows one at a
/// time. None when cancelled.
fn render_frame(o: &Opts, threads: usize, ctl: &Ctl) -> Option<Vec<u8>> {
    let p = Prepared::new(o);
    let n = p.size();
    let mut out = vec![0u8; n * n * 4];
    if n == 0 {
        return Some(out);
    }
    let rows: Vec<Mutex<&mut [u8]>> = out.chunks_mut(n * 4).map(Mutex::new).collect();
    let next = AtomicUsize::new(0);
    let work = || {
        loop {
            if ctl.cancel.load(Ordering::Relaxed) {
                return;
            }
            let y = next.fetch_add(1, Ordering::Relaxed);
            let Some(row) = rows.get(y) else {
                return;
            };
            if let Ok(mut row) = row.lock() {
                p.row(y, &mut row);
            }
        }
    };
    std::thread::scope(|s| {
        for _ in 1..threads.max(1) {
            s.spawn(|| {
                set_priority(ctl);
                work();
            });
        }
        work();
    });
    drop(rows);
    (!ctl.cancel.load(Ordering::Relaxed)).then_some(out)
}

/// Below normal while the page is on screen (the UI thread keeps priority), lowest in the
/// background.
fn set_priority(ctl: &Ctl) {
    use windows::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL, THREAD_PRIORITY_LOWEST,
    };
    let p = if ctl.visible.load(Ordering::Relaxed) {
        THREAD_PRIORITY_BELOW_NORMAL
    } else {
        THREAD_PRIORITY_LOWEST
    };
    // SAFETY: changes the calling thread's priority through its pseudo handle.
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), p);
    }
}

// ------------------------------------------------------------------------ disk cache

/// `STINKSHOT_LOGO_CACHE=off` disables the disk cache (for measuring cold starts).
fn cache_enabled() -> bool {
    !std::env::var("STINKSHOT_LOGO_CACHE").is_ok_and(|v| v.eq_ignore_ascii_case("off"))
}

/// `%LOCALAPPDATA%\Stinkshot\cache` (plus the dev profile suffix).
fn cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty())?;
    Some(
        PathBuf::from(base)
            .join(format!(
                "Stinkshot{}",
                stinkshot_core::config::profile_suffix()
            ))
            .join("cache"),
    )
}

/// The cache file for one frame size: the name changes with the renderer's source, the
/// size, the supersampling, the frame count, the file layout, the app version and the
/// egui / epaint / ecolor versions from Cargo.lock (frames are stored after egui's
/// premultiply, `ColorImage::from_rgba_unmultiplied`).
pub fn cache_path(size: u32, ss: u32) -> Option<PathBuf> {
    let mut h = stinkshot_logo::source_hash();
    h = stinkshot_logo::fnv1a(h, env!("CARGO_PKG_VERSION").as_bytes());
    h = stinkshot_logo::fnv1a(h, env!("STINKSHOT_EGUI_VERSIONS").as_bytes());
    for v in [size as u64, ss as u64, FRAMES as u64, CACHE_FORMAT] {
        h = stinkshot_logo::fnv1a(h, &v.to_le_bytes());
    }
    Some(cache_dir()?.join(format!("about-orb-{size}-{h:016x}.bin")))
}

/// Loads the cached loop into `frames`, decoding in playback order from the playhead on
/// all workers. True when every frame loaded; a damaged file is deleted.
fn load_cache(
    frames: &Mutex<Frames>,
    ctl: &Ctl,
    ctx: &egui::Context,
    path: &std::path::Path,
    size: u32,
    ss: u32,
) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let Some(blobs) = parse_cache(&bytes, size, ss) else {
        log::warn!("about logo: discarding damaged cache {}", path.display());
        let _ = std::fs::remove_file(path);
        return false;
    };
    let hint = ctl.hint.load(Ordering::Relaxed);
    let next = AtomicUsize::new(0);
    let bad = AtomicBool::new(false);
    let work = || {
        loop {
            if ctl.cancel.load(Ordering::Relaxed) || bad.load(Ordering::Relaxed) {
                return;
            }
            let k = next.fetch_add(1, Ordering::Relaxed);
            if k >= FRAMES {
                return;
            }
            let i = (hint + k) % FRAMES;
            let Some(img) = decode_frame(blobs[i], size) else {
                bad.store(true, Ordering::Relaxed);
                return;
            };
            let Ok(mut f) = frames.lock() else {
                return;
            };
            if ctl.cancel.load(Ordering::Relaxed) || !f.put(size, ss, i, Arc::new(img), true) {
                return;
            }
            drop(f);
            ctx.request_repaint();
        }
    };
    std::thread::scope(|s| {
        for _ in 1..worker_count(ctl.visible.load(Ordering::Relaxed)) {
            s.spawn(|| {
                set_priority(ctl);
                work();
            });
        }
        work();
    });
    if bad.load(Ordering::Relaxed) {
        log::warn!(
            "about logo: discarding undecodable cache {}",
            path.display()
        );
        let _ = std::fs::remove_file(path);
        return false;
    }
    frames.lock().is_ok_and(|f| f.full_done >= FRAMES)
}

/// Cache layout: magic, frame count, size, supersampling (u32 LE each), then per frame a
/// u32 LE length and a PNG of the premultiplied RGBA (stored as is, so a load is exact).
fn encode_cache(all: &[Arc<ColorImage>], size: u32, ss: u32) -> Option<Vec<u8>> {
    use image::ImageEncoder;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    let mut out = Vec::with_capacity(all.len() * 64 * 1024);
    out.extend_from_slice(CACHE_MAGIC);
    for v in [all.len() as u32, size, ss] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    let mut png = Vec::new();
    for img in all {
        if img.size != [size as usize, size as usize] {
            return None;
        }
        let raw: Vec<u8> = img.pixels.iter().flat_map(|p| p.to_array()).collect();
        png.clear();
        PngEncoder::new_with_quality(&mut png, CompressionType::Fast, FilterType::Adaptive)
            .write_image(&raw, size, size, image::ExtendedColorType::Rgba8)
            .ok()?;
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&png);
    }
    Some(out)
}

/// The per-frame PNGs of a cache file, or None when it does not match `size` and `ss`.
fn parse_cache(bytes: &[u8], size: u32, ss: u32) -> Option<Vec<&[u8]>> {
    let u32_at = |at: usize| -> Option<u32> {
        Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
    };
    if bytes.get(..8)? != CACHE_MAGIC
        || u32_at(8)? as usize != FRAMES
        || u32_at(12)? != size
        || u32_at(16)? != ss
    {
        return None;
    }
    let mut at = 20;
    let mut blobs = Vec::with_capacity(FRAMES);
    for _ in 0..FRAMES {
        let len = u32_at(at)? as usize;
        blobs.push(bytes.get(at + 4..at + 4 + len)?);
        at += 4 + len;
    }
    (at == bytes.len()).then_some(blobs)
}

fn decode_frame(png: &[u8], size: u32) -> Option<ColorImage> {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .ok()?
        .into_rgba8();
    if img.width() != size || img.height() != size {
        return None;
    }
    Some(ColorImage::from_rgba_premultiplied(
        [size as usize, size as usize],
        img.as_raw(),
    ))
}

/// Writes the cache atomically (temp file, then rename) and prunes the folder to
/// `CACHE_BUDGET`, oldest first. Returns the file's size.
fn save_cache(
    path: &std::path::Path,
    all: &[Arc<ColorImage>],
    size: u32,
    ss: u32,
) -> std::io::Result<u64> {
    let bytes = encode_cache(all, size, ss)
        .ok_or_else(|| std::io::Error::other("frames do not match the cache size"))?;
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("no cache folder"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    prune_cache(dir, path);
    Ok(bytes.len() as u64)
}

/// Deletes older `about-orb-*` files past the budget, never `keep`.
fn prune_cache(dir: &std::path::Path, keep: &std::path::Path) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = rd
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("about-orb-"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    let mut total = 0u64;
    for (_, len, p) in files {
        total += len;
        if total > CACHE_BUDGET && p != keep {
            let _ = std::fs::remove_file(&p);
        }
    }
}

// ------------------------------------------------------------------------ drawing

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

/// Frees the frames and textures once the About page has been neither shown nor
/// prewarmed for `KEEP` and nothing is loading or rendering (so a render finishes and
/// reaches the disk cache).
fn spawn_reaper(h: Handle) {
    let spawned = std::thread::Builder::new()
        .name("logo-anim-reaper".into())
        .spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(10));
                let Ok(mut a) = h.lock() else {
                    return;
                };
                // Off screen: finish any render in the background.
                if a.last_shown
                    .is_none_or(|t| t.elapsed() > Duration::from_secs(1))
                {
                    a.ctl.visible.store(false, Ordering::Relaxed);
                }
                let running = a.frames.lock().is_ok_and(|f| f.running);
                if a.last_wanted.elapsed() > KEEP && !running {
                    a.ctl.cancel.store(true, Ordering::Relaxed);
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

    fn img(v: u8) -> Arc<ColorImage> {
        Arc::new(ColorImage::new(
            [2, 2],
            vec![Color32::from_rgba_premultiplied(v, v / 2, 0, v); 4],
        ))
    }

    #[test]
    fn render_order_previews_the_playhead_then_follows_it() {
        let mut f = Frames::default();
        f.reset(2, 2);
        // Visible and empty: cheap previews from the playhead to PREVIEW_AHEAD past it.
        for i in 10..=10 + PREVIEW_AHEAD {
            assert_eq!(pick(&f, 10, true, true), Some((i, 1)));
            assert!(f.put(2, 2, i, img(1), false));
        }
        // Then full quality in playback order from one past the playhead.
        assert_eq!(pick(&f, 10, true, true), Some((11, 2)));
        assert!(f.put(2, 2, 11, img(2), true));
        // The playhead moved on: the new edge of the window gets a preview first.
        assert_eq!(pick(&f, 11, true, true), Some((11 + PREVIEW_AHEAD, 1)));
        assert!(f.put(2, 2, 11 + PREVIEW_AHEAD, img(1), false));
        assert_eq!(pick(&f, 11, true, true), Some((12, 2)));
        // The frame on screen is done last of all.
        for i in (0..FRAMES).filter(|&i| i != 10 && i != 11) {
            f.put(2, 2, i, img(2), true);
        }
        assert_eq!(pick(&f, 10, true, true), Some((10, 2)));
        assert!(f.put(2, 2, 10, img(2), true));
        assert!(f.full[10] && f.ver[10] == 2);
        // A late preview never replaces a full frame.
        assert!(f.put(2, 2, 10, img(3), false));
        assert_eq!(f.slots[10].as_ref().map(|s| s.pixels[0].r()), Some(2));
        // Without supersampling there are no previews: the empty frame on screen comes first.
        let mut f = Frames::default();
        f.reset(2, 1);
        assert_eq!(pick(&f, 10, true, true), Some((10, 1)));
        assert!(f.put(2, 1, 10, img(2), true));
        assert_eq!(pick(&f, 10, true, true), Some((11, 1)));
        // In the background: no previews, from the playhead, wrapping around the loop.
        let mut f = Frames::default();
        f.reset(2, 2);
        f.put(2, 2, 95, img(2), true);
        assert_eq!(pick(&f, 94, true, false), Some((94, 2)));
        assert_eq!(pick(&f, 95, true, false), Some((0, 2)));
        // Still only: just the still frame.
        assert_eq!(pick(&f, 40, false, true), Some((STILL, 2)));
        // Frames for another size are refused.
        assert!(!f.put(4, 2, 3, img(1), true));
        for i in 0..FRAMES {
            f.put(2, 2, i, img(9), true);
        }
        assert_eq!(f.full_done, FRAMES);
        assert_eq!(pick(&f, 0, true, true), None);
    }

    #[test]
    fn cache_key_covers_the_egui_versions() {
        let v = env!("STINKSHOT_EGUI_VERSIONS");
        for name in ["ecolor ", "egui ", "epaint "] {
            assert!(v.contains(name), "{v}");
        }
    }

    #[test]
    fn cache_round_trips_exactly_and_rejects_mismatches() {
        let all: Vec<Arc<ColorImage>> = (0..FRAMES).map(|i| img(i as u8 * 2)).collect();
        let bytes = encode_cache(&all, 2, 2).expect("encode");
        let blobs = parse_cache(&bytes, 2, 2).expect("parse");
        assert_eq!(blobs.len(), FRAMES);
        for (blob, want) in blobs.iter().zip(&all) {
            assert_eq!(decode_frame(blob, 2).expect("decode").pixels, want.pixels);
        }
        assert!(parse_cache(&bytes, 3, 2).is_none());
        assert!(parse_cache(&bytes, 2, 1).is_none());
        assert!(parse_cache(&bytes[..bytes.len() - 1], 2, 2).is_none());
        assert!(encode_cache(&all, 3, 2).is_none());
    }

    #[test]
    fn cache_name_tracks_size_and_quality() {
        if cache_dir().is_none() {
            return;
        }
        let a = cache_path(230, 2).expect("path");
        assert_ne!(Some(&a), cache_path(276, 1).as_ref());
        assert_ne!(Some(&a), cache_path(230, 1).as_ref());
        assert_eq!(Some(a), cache_path(230, 2));
    }
}
