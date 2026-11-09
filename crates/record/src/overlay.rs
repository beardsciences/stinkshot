//! Recording overlays composited on the CPU with tiny-skia: click ripples, key chip and the
//! webcam bubble.
//!
//! The target pixmap holds BGRA bytes (what capture produces and the NV12 converter reads).
//! tiny-skia thinks in RGBA, so every colour handed to it has red and blue swapped via
//! [`swap_rb`]; the bytes in memory then end up in BGRA order. Frames are opaque, so
//! premultiplied and straight alpha coincide for the base picture.

use std::time::Duration;

use ab_glyph::{Font, FontArc, Glyph, PxScale, ScaleFont, point};
use tiny_skia::{
    Color, FillRule, FilterQuality, Paint, PathBuilder, Pattern, Pixmap, SpreadMode, Stroke,
    Transform,
};

use crate::Corner;
use crate::convert::scale_4ch_crop;

pub const RIPPLE_LIFE: Duration = Duration::from_millis(400);
pub const KEY_HOLD_AFTER_RELEASE: Duration = Duration::from_millis(1200);
const KEY_FADE_OUT: Duration = Duration::from_millis(250);
const KEY_FADE_IN: Duration = Duration::from_millis(80);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Debug)]
struct Ripple {
    x: f32,
    y: f32,
    born: Duration,
    button: Button,
    small: bool,
}

/// Fonts used for the key chip.
#[derive(Clone)]
pub struct OverlayFonts {
    pub bold: FontArc,
}

impl OverlayFonts {
    /// Loaded once per process: `Fonts::load_system` maps (and keeps) the font files, so
    /// calling it per recording leaked two file mappings each time.
    pub fn load() -> Option<OverlayFonts> {
        static CACHE: std::sync::OnceLock<Option<OverlayFonts>> = std::sync::OnceLock::new();
        CACHE.get_or_init(Self::load_uncached).clone()
    }

    /// Prefer the shared `stinkshot_core::fonts::Fonts` loader; fall back to reading Segoe UI
    /// / Arial straight from the Windows font folder.
    fn load_uncached() -> Option<OverlayFonts> {
        let core = std::panic::catch_unwind(stinkshot_core::fonts::Fonts::load_system);
        if let Ok(Ok(f)) = core {
            return Some(OverlayFonts {
                bold: (*f.bold).clone(),
            });
        }
        let dir = std::env::var_os("WINDIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| "C:\\Windows".into());
        let dir = dir.join("Fonts");
        let load = |names: &[&str]| -> Option<FontArc> {
            names.iter().find_map(|n| {
                let bytes = std::fs::read(dir.join(n)).ok()?;
                FontArc::try_from_vec(bytes).ok()
            })
        };
        let bold = load(&["segoeuib.ttf", "arialbd.ttf", "segoeui.ttf", "arial.ttf"])?;
        Some(OverlayFonts { bold })
    }
}

/// Mutable overlay state: live ripples and the key combo currently displayed.
#[derive(Default)]
pub struct OverlayState {
    ripples: Vec<Ripple>,
    held: Vec<(u32, String)>,
    shown_label: String,
    key_changed: Option<Duration>,
    released_at: Option<Duration>,
}

impl OverlayState {
    pub fn new() -> Self {
        Self::default()
    }

    /// `x`,`y` are relative to the recorded region (physical px).
    pub fn mouse_down(&mut self, x: f32, y: f32, button: Button, now: Duration) {
        self.ripples.push(Ripple {
            x,
            y,
            born: now,
            button,
            small: false,
        });
    }

    pub fn mouse_up(&mut self, x: f32, y: f32, button: Button, now: Duration) {
        self.ripples.push(Ripple {
            x,
            y,
            born: now,
            button,
            small: true,
        });
    }

    pub fn key_down(&mut self, vk: u32, label: &str, now: Duration) {
        if self.held.iter().any(|(v, _)| *v == vk) {
            return; // auto-repeat
        }
        if self.held.is_empty() && self.released_at.is_some() {
            // a fresh combo replaces the lingering one
            self.released_at = None;
        }
        self.held.push((vk, label.to_string()));
        self.held.sort_by_key(|(_, l)| modifier_rank(l));
        self.shown_label = combo_text(self.held.iter().map(|(_, l)| l.as_str()));
        if self.key_changed.is_none() || self.held.len() == 1 {
            self.key_changed = Some(now);
        }
    }

    pub fn key_up(&mut self, vk: u32, now: Duration) {
        let before = self.held.len();
        self.held.retain(|(v, _)| *v != vk);
        if before != 0 && self.held.is_empty() {
            self.released_at = Some(now);
        }
    }

    /// Clear held keys (used on pause/resume so a stuck key does not linger).
    pub fn release_all(&mut self, now: Duration) {
        if !self.held.is_empty() {
            self.held.clear();
            self.released_at = Some(now);
        }
    }

    pub fn clear(&mut self) {
        self.ripples.clear();
        self.held.clear();
        self.released_at = None;
        self.shown_label.clear();
    }

    /// Drop expired ripples.
    pub fn prune(&mut self, now: Duration) {
        self.ripples
            .retain(|r| now.saturating_sub(r.born) < RIPPLE_LIFE);
    }

    /// True when something is (or just was) drawn, so unchanged frames can be reused.
    pub fn is_active(&self, now: Duration, clicks: bool, keys: bool) -> bool {
        (clicks
            && self
                .ripples
                .iter()
                .any(|r| now.saturating_sub(r.born) < RIPPLE_LIFE))
            || (keys && self.chip_alpha(now) > 0.0)
    }

    /// Opacity (0..=1) of the key chip at `now`.
    pub fn chip_alpha(&self, now: Duration) -> f32 {
        if self.shown_label.is_empty() {
            return 0.0;
        }
        let fade_in = match self.key_changed {
            Some(t) => {
                (now.saturating_sub(t).as_secs_f32() / KEY_FADE_IN.as_secs_f32()).clamp(0.0, 1.0)
            }
            None => 1.0,
        };
        if !self.held.is_empty() {
            return fade_in;
        }
        match self.released_at {
            Some(r) => {
                let since = now.saturating_sub(r);
                if since >= KEY_HOLD_AFTER_RELEASE {
                    0.0
                } else {
                    let fade_start = KEY_HOLD_AFTER_RELEASE - KEY_FADE_OUT;
                    let a = if since > fade_start {
                        1.0 - (since - fade_start).as_secs_f32() / KEY_FADE_OUT.as_secs_f32()
                    } else {
                        1.0
                    };
                    a.min(fade_in)
                }
            }
            None => 0.0,
        }
    }

    /// Draw everything that is currently live.
    pub fn render(
        &self,
        pix: &mut Pixmap,
        now: Duration,
        accent: [u8; 4],
        clicks: bool,
        keys: bool,
        fonts: Option<&OverlayFonts>,
    ) {
        let s = ui_scale(pix.height());
        if clicks {
            for r in &self.ripples {
                let age = now.saturating_sub(r.born);
                if age >= RIPPLE_LIFE {
                    continue;
                }
                let t = age.as_secs_f32() / RIPPLE_LIFE.as_secs_f32();
                draw_ripple(pix, r, t, accent, s);
            }
        }
        if keys {
            let a = self.chip_alpha(now);
            if a > 0.0
                && let Some(f) = fonts
            {
                draw_chip(pix, &self.shown_label, a, f, s);
            }
        }
    }
}

/// Scale factor for overlay sizes relative to a 1080p-tall frame.
pub fn ui_scale(frame_h: u32) -> f32 {
    (frame_h as f32 / 1080.0).clamp(0.8, 2.0)
}

fn modifier_rank(label: &str) -> u8 {
    match label.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => 0,
        "alt" => 1,
        "shift" => 2,
        "win" | "windows" | "meta" => 3,
        _ => 4,
    }
}

pub fn combo_text<'a>(labels: impl Iterator<Item = &'a str>) -> String {
    labels.collect::<Vec<_>>().join(" + ")
}

/// Geometry of a ripple at normalized age `t` in 0..1: (radius, ring alpha, fill alpha).
pub fn ripple_metrics(t: f32, s: f32, small: bool) -> (f32, f32, f32) {
    let e = 1.0 - (1.0 - t).powi(3); // ease out
    let (r0, r1) = if small { (6.0, 22.0) } else { (10.0, 44.0) };
    let radius = (r0 + (r1 - r0) * e) * s;
    let fade = 1.0 - t;
    let k = if small { 0.6 } else { 1.0 };
    (radius, 0.9 * fade * k, 0.3 * fade * k)
}

/// Chip rectangle (x, y, w, h) centered at the bottom of the frame.
pub fn chip_rect(
    text_w: f32,
    text_h: f32,
    frame_w: f32,
    frame_h: f32,
    s: f32,
) -> (f32, f32, f32, f32) {
    let pad_x = 20.0 * s;
    let pad_y = 10.0 * s;
    let w = (text_w + 2.0 * pad_x).min(frame_w);
    let h = text_h + 2.0 * pad_y;
    let x = (frame_w - w) / 2.0;
    let y = frame_h - h - 48.0 * s;
    (x, y.max(0.0), w, h)
}

/// Top-left origin of the webcam bubble for a corner.
pub fn webcam_origin(corner: Corner, d: i32, fw: i32, fh: i32, margin: i32) -> (i32, i32) {
    let left = margin.min((fw - d).max(0));
    let top = margin.min((fh - d).max(0));
    let right = (fw - d - margin).max(0);
    let bottom = (fh - d - margin).max(0);
    match corner {
        Corner::TopLeft => (left, top),
        Corner::TopRight => (right, top),
        Corner::BottomLeft => (left, bottom),
        Corner::BottomRight => (right, bottom),
    }
}

fn swap_rb(c: [u8; 4], alpha_mul: f32) -> Color {
    Color::from_rgba8(
        c[2],
        c[1],
        c[0],
        (c[3] as f32 * alpha_mul.clamp(0.0, 1.0)).round() as u8,
    )
}

fn paint_of(color: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(color);
    p.anti_alias = true;
    p
}

fn draw_ripple(pix: &mut Pixmap, r: &Ripple, t: f32, accent: [u8; 4], s: f32) {
    let (radius, ring_a, fill_a) = ripple_metrics(t, s, r.small);
    let col = match r.button {
        Button::Left => accent,
        Button::Right => mix_white(accent, 0.55),
        Button::Middle => mix_white(accent, 0.85),
    };
    let Some(path) = PathBuilder::from_circle(r.x, r.y, radius) else {
        return;
    };
    if !(r.button == Button::Right) {
        pix.fill_path(
            &path,
            &paint_of(swap_rb(col, fill_a)),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    let stroke = Stroke {
        width: (if r.button == Button::Right { 5.0 } else { 3.0 }) * s,
        ..Stroke::default()
    };
    pix.stroke_path(
        &path,
        &paint_of(swap_rb(col, ring_a)),
        &stroke,
        Transform::identity(),
        None,
    );
}

fn mix_white(c: [u8; 4], k: f32) -> [u8; 4] {
    let m = |v: u8| (v as f32 + (255.0 - v as f32) * k).round() as u8;
    [m(c[0]), m(c[1]), m(c[2]), c[3]]
}

fn rounded_rect_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.min(w / 2.0).min(h / 2.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish()
}

/// Width of `text` in px at `size` using kerning and advances.
pub fn text_width(font: &FontArc, text: &str, size: f32) -> f32 {
    let sf = font.as_scaled(PxScale::from(size));
    let mut w = 0.0;
    let mut prev = None;
    for ch in text.chars() {
        let id = sf.glyph_id(ch);
        if let Some(p) = prev {
            w += sf.kern(p, id);
        }
        w += sf.h_advance(id);
        prev = Some(id);
    }
    w
}

fn draw_chip(pix: &mut Pixmap, text: &str, alpha: f32, fonts: &OverlayFonts, s: f32) {
    let size = 22.0 * s;
    let font = &fonts.bold;
    let sf = font.as_scaled(PxScale::from(size));
    let tw = text_width(font, text, size);
    let th = sf.ascent() - sf.descent();
    let (fw, fh) = (pix.width() as f32, pix.height() as f32);
    let (x, y, w, h) = chip_rect(tw, th, fw, fh, s);
    if let Some(path) = rounded_rect_path(x, y, w, h, 10.0 * s) {
        pix.fill_path(
            &path,
            &paint_of(swap_rb([20, 20, 24, 255], 0.86 * alpha)),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        let stroke = Stroke {
            width: 1.0 * s,
            ..Stroke::default()
        };
        pix.stroke_path(
            &path,
            &paint_of(swap_rb([255, 255, 255, 255], 0.18 * alpha)),
            &stroke,
            Transform::identity(),
            None,
        );
    }
    let mut cx = x + (w - tw) / 2.0;
    let baseline = y + (h - th) / 2.0 + sf.ascent();
    let mut prev = None;
    let (pw, ph) = (pix.width() as i32, pix.height() as i32);
    let data = pix.data_mut();
    for ch in text.chars() {
        let id = sf.glyph_id(ch);
        if let Some(p) = prev {
            cx += sf.kern(p, id);
        }
        let glyph = Glyph {
            id,
            scale: PxScale::from(size),
            position: point(cx, baseline),
        };
        if let Some(og) = font.outline_glyph(glyph) {
            let b = og.px_bounds();
            og.draw(|gx, gy, cov| {
                let px = b.min.x as i32 + gx as i32;
                let py = b.min.y as i32 + gy as i32;
                if px < 0 || py < 0 || px >= pw || py >= ph {
                    return;
                }
                let a = (cov * alpha).clamp(0.0, 1.0);
                let i = ((py * pw + px) * 4) as usize;
                for c in 0..3 {
                    data[i + c] = (data[i + c] as f32 * (1.0 - a) + 255.0 * a).round() as u8;
                }
                data[i + 3] = 255;
            });
        }
        cx += sf.h_advance(id);
        prev = Some(id);
    }
}

/// One webcam picture, BGRA top-down, alpha forced opaque.
#[derive(Clone)]
pub struct WebcamFrame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// Draw `frame` as a circle of diameter `d` with its top-left at (`x`,`y`), plus a white 2 px ring.
/// The source is center-cropped to a square.
pub fn draw_webcam_bubble(pix: &mut Pixmap, frame: &WebcamFrame, x: i32, y: i32, d: u32, s: f32) {
    if d == 0 || frame.width == 0 || frame.height == 0 {
        return;
    }
    let side = frame.width.min(frame.height) as usize;
    let cx = (frame.width as usize - side) / 2;
    let cy = (frame.height as usize - side) / 2;
    let scaled = scale_4ch_crop(
        &frame.bgra,
        frame.width as usize,
        frame.height as usize,
        (cx, cy, side, side),
        d as usize,
        d as usize,
    );
    let Some(src) = tiny_skia::PixmapRef::from_bytes(&scaled, d, d) else {
        return;
    };
    let mut paint = Paint {
        shader: Pattern::new(
            src,
            SpreadMode::Pad,
            FilterQuality::Nearest,
            1.0,
            Transform::from_translate(x as f32, y as f32),
        ),
        anti_alias: true,
        ..Paint::default()
    };
    paint.anti_alias = true;
    let r = d as f32 / 2.0;
    let (ccx, ccy) = (x as f32 + r, y as f32 + r);
    if let Some(path) = PathBuilder::from_circle(ccx, ccy, r - 1.0) {
        pix.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    if let Some(path) = PathBuilder::from_circle(ccx, ccy, r - 1.0 * s.max(1.0)) {
        let stroke = Stroke {
            width: 2.0,
            ..Stroke::default()
        };
        pix.stroke_path(
            &path,
            &paint_of(swap_rb([255, 255, 255, 255], 1.0)),
            &stroke,
            Transform::identity(),
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    #[test]
    fn ripple_grows_and_fades() {
        let (r0, ra0, _) = ripple_metrics(0.0, 1.0, false);
        let (r1, ra1, _) = ripple_metrics(0.5, 1.0, false);
        let (r2, ra2, _) = ripple_metrics(1.0, 1.0, false);
        assert!(r0 < r1 && r1 < r2);
        assert!(ra0 > ra1 && ra1 > ra2);
        assert!(ra2.abs() < 1e-6);
        let (rs, _, _) = ripple_metrics(1.0, 1.0, true);
        assert!(rs < r2);
        let (r_big, _, _) = ripple_metrics(1.0, 2.0, false);
        assert!((r_big - 2.0 * r2).abs() < 1e-4);
    }

    #[test]
    fn ripples_expire() {
        let mut st = OverlayState::new();
        st.mouse_down(10.0, 10.0, Button::Left, ms(100));
        assert!(st.is_active(ms(300), true, false));
        assert!(!st.is_active(ms(300), false, false));
        assert!(!st.is_active(ms(501), true, false));
        st.prune(ms(501));
        assert!(st.ripples.is_empty());
    }

    #[test]
    fn chip_combo_orders_modifiers_first() {
        let mut st = OverlayState::new();
        st.key_down(0x50, "P", ms(0));
        st.key_down(0x11, "Ctrl", ms(10));
        st.key_down(0x10, "Shift", ms(20));
        assert_eq!(st.shown_label, "Ctrl + Shift + P");
    }

    #[test]
    fn chip_visibility_window() {
        let mut st = OverlayState::new();
        st.key_down(0x41, "A", ms(1000));
        assert_eq!(st.chip_alpha(ms(1000)), 0.0); // fade-in starts at zero
        assert!(st.chip_alpha(ms(1100)) > 0.99);
        st.key_up(0x41, ms(1500));
        assert!(st.chip_alpha(ms(2000)) > 0.99);
        // fading between 2.45 s and 2.7 s
        let mid = st.chip_alpha(ms(2575));
        assert!(mid > 0.0 && mid < 1.0, "mid={mid}");
        assert_eq!(st.chip_alpha(ms(2700)), 0.0);
        assert!(!st.is_active(ms(2800), false, true));
    }

    #[test]
    fn chip_auto_repeat_ignored_and_new_combo_replaces() {
        let mut st = OverlayState::new();
        st.key_down(0x41, "A", ms(0));
        st.key_down(0x41, "A", ms(30));
        assert_eq!(st.shown_label, "A");
        st.key_up(0x41, ms(100));
        st.key_down(0x42, "B", ms(200));
        assert_eq!(st.shown_label, "B");
        assert!(st.chip_alpha(ms(400)) > 0.99);
    }

    #[test]
    fn chip_rect_centered_above_bottom() {
        let (x, y, w, h) = chip_rect(100.0, 20.0, 1000.0, 600.0, 1.0);
        assert!((x + w / 2.0 - 500.0).abs() < 0.01);
        assert!(y + h < 600.0 && y > 400.0);
        let (x, _, w, _) = chip_rect(5000.0, 20.0, 300.0, 600.0, 1.0);
        assert_eq!((x, w), (0.0, 300.0));
    }

    #[test]
    fn webcam_origin_corners() {
        assert_eq!(webcam_origin(Corner::TopLeft, 100, 1000, 800, 20), (20, 20));
        assert_eq!(
            webcam_origin(Corner::TopRight, 100, 1000, 800, 20),
            (880, 20)
        );
        assert_eq!(
            webcam_origin(Corner::BottomLeft, 100, 1000, 800, 20),
            (20, 680)
        );
        assert_eq!(
            webcam_origin(Corner::BottomRight, 100, 1000, 800, 20),
            (880, 680)
        );
        // bubble larger than the frame stays at 0
        assert_eq!(
            webcam_origin(Corner::BottomRight, 500, 300, 300, 20),
            (0, 0)
        );
    }

    #[test]
    fn ui_scale_clamps() {
        assert_eq!(ui_scale(100), 0.8);
        assert_eq!(ui_scale(1080), 1.0);
        assert_eq!(ui_scale(4000), 2.0);
    }

    fn solid_pixmap(w: u32, h: u32, bgr: [u8; 3]) -> Pixmap {
        let mut p = Pixmap::new(w, h).unwrap();
        for px in p.data_mut().chunks_exact_mut(4) {
            px.copy_from_slice(&[bgr[0], bgr[1], bgr[2], 255]);
        }
        p
    }

    #[test]
    fn ripple_renders_in_bgra_order() {
        let mut pix = solid_pixmap(200, 200, [0, 0, 0]);
        let mut st = OverlayState::new();
        st.mouse_down(100.0, 100.0, Button::Left, ms(0));
        // accent is pure red (RGBA); in BGRA memory the red byte is index 2
        st.render(&mut pix, ms(40), [255, 0, 0, 255], true, false, None);
        let c = &pix.data()[((100 * 200 + 100) * 4) as usize..][..4];
        assert!(c[2] > 20, "red channel should be lit: {c:?}");
        assert_eq!(c[0], 0, "blue channel untouched: {c:?}");
        assert_eq!(c[3], 255);
    }

    #[test]
    fn nothing_drawn_when_disabled() {
        let mut pix = solid_pixmap(64, 64, [10, 20, 30]);
        let before = pix.data().to_vec();
        let mut st = OverlayState::new();
        st.mouse_down(32.0, 32.0, Button::Left, ms(0));
        st.render(&mut pix, ms(40), [255, 0, 0, 255], false, false, None);
        assert_eq!(pix.data(), &before[..]);
    }

    #[test]
    fn webcam_bubble_circle_and_ring() {
        let mut pix = solid_pixmap(200, 200, [0, 0, 0]);
        // pure green frame (BGRA)
        let mut bgra = vec![0u8; 80 * 60 * 4];
        for p in bgra.chunks_exact_mut(4) {
            p.copy_from_slice(&[0, 255, 0, 255]);
        }
        let frame = WebcamFrame {
            width: 80,
            height: 60,
            bgra,
        };
        draw_webcam_bubble(&mut pix, &frame, 50, 50, 100, 1.0);
        let at = |x: usize, y: usize| pix.data()[(y * 200 + x) * 4..][..4].to_vec();
        // center is green
        let c = at(100, 100);
        assert!(c[1] > 240 && c[0] < 10 && c[2] < 10, "{c:?}");
        // corner of the bounding square stays black (outside the circle)
        assert_eq!(at(52, 52)[..3], [0, 0, 0]);
        // ring is white near the edge
        let ring = at(100, 50 + 1);
        assert!(ring[0] > 200 && ring[1] > 200 && ring[2] > 200, "{ring:?}");
    }

    #[test]
    fn chip_text_renders_pixels_when_font_available() {
        let Some(fonts) = OverlayFonts::load() else {
            return;
        };
        let mut pix = solid_pixmap(600, 300, [0, 0, 0]);
        let mut st = OverlayState::new();
        st.key_down(0x11, "Ctrl", ms(0));
        st.key_down(0x43, "C", ms(0));
        st.render(
            &mut pix,
            ms(200),
            [255, 0, 0, 255],
            false,
            true,
            Some(&fonts),
        );
        let lit = pix
            .data()
            .chunks_exact(4)
            .filter(|p| p[0] > 200 && p[1] > 200 && p[2] > 200)
            .count();
        assert!(lit > 50, "expected white glyph pixels, got {lit}");
    }
}
