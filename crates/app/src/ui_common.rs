//! Shared helpers for the window surfaces (toast, pin, history, settings, record HUDs).
//!
//! Everything that can be pure is pure and unit tested: placement math, debounce, LRU,
//! throttle, time/size formatting, thumbnail cover math. The few OS bits (async key poll,
//! no-activate style, folder picker) are small wrappers around the `windows` crate.
//!
//! This module is mounted from `toast.rs` with `#[path]` so `main.rs` does not need to change;
//! other files reach it as `crate::toast::ui_common`.
#![allow(dead_code)]

use std::collections::HashMap;
use std::hash::Hash;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use stinkshot_core::{PxRect, Rgba};
use stinkshot_platform::Hwnd;

use crate::state::Shared;
use crate::theme::{self, Palette};

// ---------------------------------------------------------------------------------------
// Colors
// ---------------------------------------------------------------------------------------

pub fn pal(shared: &Shared) -> &'static Palette {
    theme::palette(shared.dark)
}

pub fn accent(shared: &Shared) -> Color32 {
    theme::rgba(shared.accent)
}

/// Accent at 18% alpha (the design's `accent + '2e'` soft fill).
pub fn accent_soft(c: Color32) -> Color32 {
    with_alpha(c, 0x2e)
}

pub fn with_alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// Parse "#rrggbb" / "#rrggbbaa" (leading '#' optional). Independent of core so it can be
/// unit tested here.
pub fn parse_hex(s: &str) -> Option<[u8; 4]> {
    let t = s.trim().trim_start_matches('#');
    if !t.is_ascii() || (t.len() != 6 && t.len() != 8) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&t[i..i + 2], 16).ok();
    let a = if t.len() == 8 { byte(6)? } else { 255 };
    Some([byte(0)?, byte(2)?, byte(4)?, a])
}

pub fn hex_to_color(s: &str) -> Option<Color32> {
    parse_hex(s).map(theme::rgba)
}

/// "#rrggbb" (alpha dropped when opaque).
pub fn color_to_hex(c: Color32) -> String {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    }
}

pub fn rgba_to_color_image(img: &Rgba) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.data)
}

// ---------------------------------------------------------------------------------------
// Placement math (physical pixels)
// ---------------------------------------------------------------------------------------

pub fn px(logical: f32, scale: f32) -> i32 {
    (logical * scale).round() as i32
}

pub fn pts(physical: i32, scale: f32) -> f32 {
    physical as f32 / scale.max(0.1)
}

/// A work area + DPI scale pair (one monitor).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MonArea {
    pub work: PxRect,
    pub scale: f32,
}

impl MonArea {
    pub const FALLBACK: MonArea = MonArea {
        work: PxRect::new(0, 0, 1920, 1040),
        scale: 1.0,
    };
}

/// Monitor under the cursor (work area + scale). Falls back to a 1080p area if the platform
/// layer reports nothing.
pub fn area_under_cursor() -> MonArea {
    let p = stinkshot_platform::input::cursor_pos();
    area_at(p)
}

pub fn area_at(p: stinkshot_core::PxPoint) -> MonArea {
    match stinkshot_platform::monitors::at_point(p) {
        Some(m) => MonArea {
            work: m.work,
            scale: if m.scale > 0.0 { m.scale } else { 1.0 },
        },
        None => stinkshot_platform::monitors::list()
            .into_iter()
            .find(|m| m.primary)
            .map(|m| MonArea {
                work: m.work,
                scale: if m.scale > 0.0 { m.scale } else { 1.0 },
            })
            .unwrap_or(MonArea::FALLBACK),
    }
}

/// MonArea of the monitor containing the center of `r`.
pub fn area_of_rect(r: PxRect) -> MonArea {
    area_at(stinkshot_core::PxPoint {
        x: r.x + r.w / 2,
        y: r.y + r.h / 2,
    })
}

/// Clamp `r` (keeping its size when possible) so it lies inside `bounds`.
pub fn clamp_into(r: PxRect, bounds: PxRect) -> PxRect {
    let w = r.w.min(bounds.w);
    let h = r.h.min(bounds.h);
    let x = r.x.clamp(bounds.x, (bounds.right() - w).max(bounds.x));
    let y = r.y.clamp(bounds.y, (bounds.bottom() - h).max(bounds.y));
    PxRect::new(x, y, w, h)
}

pub fn centered_in(work: PxRect, w: i32, h: i32) -> PxRect {
    PxRect::new(work.x + (work.w - w) / 2, work.y + (work.h - h) / 2, w, h)
}

/// Toast slot: bottom-right of the work area, `margin` from the edges, stacked upward by
/// `stack_index` using a fixed `slot_h` pitch (`gap` between slots). The toast is bottom
/// aligned inside its slot so short message toasts line up with tall capture toasts.
pub fn toast_rect(
    work: PxRect,
    size: (i32, i32),
    slot_h: i32,
    stack_index: usize,
    margin: i32,
    gap: i32,
) -> PxRect {
    let (w, h) = size;
    let x = work.right() - margin - w;
    let bottom = work.bottom() - margin - stack_index as i32 * (slot_h + gap);
    let y = (bottom - h).max(work.y);
    PxRect::new(x.max(work.x), y, w, h)
}

/// HUD under the region (left aligned), above it when there is no room below, and inside the
/// region's bottom edge as a last resort.
pub fn hud_rect(region: PxRect, size: (i32, i32), work: PxRect, gap: i32) -> PxRect {
    let (w, h) = size;
    let x = region.x.clamp(work.x, (work.right() - w).max(work.x));
    let below = region.bottom() + gap;
    let y = if below + h <= work.bottom() {
        below
    } else if region.y - gap - h >= work.y {
        region.y - gap - h
    } else {
        (region.bottom() - gap - h).clamp(work.y, (work.bottom() - h).max(work.y))
    };
    PxRect::new(x, y, w, h)
}

// ---------------------------------------------------------------------------------------
// Debounce / throttle / LRU
// ---------------------------------------------------------------------------------------

/// Trailing-edge debouncer. `now` is passed in so it is testable.
#[derive(Debug)]
pub struct Debounce<T> {
    delay: Duration,
    pending: Option<(T, Instant)>,
}

impl<T> Debounce<T> {
    pub fn new(delay: Duration) -> Self {
        Self {
            delay,
            pending: None,
        }
    }
    pub fn push(&mut self, value: T, now: Instant) {
        self.pending = Some((value, now));
    }
    /// The value, once `delay` has passed since the last push.
    pub fn poll(&mut self, now: Instant) -> Option<T> {
        match &self.pending {
            Some((_, at)) if now.duration_since(*at) >= self.delay => {
                self.pending.take().map(|(v, _)| v)
            }
            _ => None,
        }
    }
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
    /// Time until `poll` would fire (for `request_repaint_after`).
    pub fn remaining(&self, now: Instant) -> Option<Duration> {
        self.pending
            .as_ref()
            .map(|(_, at)| self.delay.saturating_sub(now.duration_since(*at)))
    }
    pub fn cancel(&mut self) {
        self.pending = None;
    }
}

/// Allows an action at most once per `interval`.
#[derive(Debug)]
pub struct Throttle {
    interval: Duration,
    last: Option<Instant>,
}

impl Throttle {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: None,
        }
    }
    /// True (and records `now`) if the action may run.
    pub fn ready(&mut self, now: Instant) -> bool {
        match self.last {
            Some(l) if now.duration_since(l) < self.interval => false,
            _ => {
                self.last = Some(now);
                true
            }
        }
    }
}

/// Small least-recently-used map. `get` and `insert` refresh recency; inserting beyond the
/// capacity evicts the stalest entry. O(n) eviction is fine for a few hundred textures.
#[derive(Debug)]
pub struct Lru<K, V> {
    cap: usize,
    tick: u64,
    map: HashMap<K, (V, u64)>,
}

impl<K: Eq + Hash + Clone, V> Lru<K, V> {
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            tick: 0,
            map: HashMap::new(),
        }
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
    pub fn contains(&self, k: &K) -> bool {
        self.map.contains_key(k)
    }
    pub fn get(&mut self, k: &K) -> Option<&V> {
        self.tick += 1;
        let t = self.tick;
        self.map.get_mut(k).map(|e| {
            e.1 = t;
            &e.0
        })
    }
    /// Returns the evicted entry, if any.
    pub fn insert(&mut self, k: K, v: V) -> Option<(K, V)> {
        self.tick += 1;
        self.map.insert(k, (v, self.tick));
        if self.map.len() <= self.cap {
            return None;
        }
        let stalest = self
            .map
            .iter()
            .min_by_key(|(_, (_, t))| *t)
            .map(|(k, _)| k.clone())?;
        self.map.remove(&stalest).map(|(v, _)| (stalest, v))
    }
    pub fn clear(&mut self) {
        self.map.clear();
    }
}

// ---------------------------------------------------------------------------------------
// Formatting
// ---------------------------------------------------------------------------------------

/// "00:42", or "1:02:03" past an hour.
pub fn fmt_clock(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

/// "00:03.2" (tenths, truncated).
pub fn fmt_tenths(d: Duration) -> String {
    let tenths = d.as_millis() / 100;
    let s = tenths / 10;
    format!("{:02}:{:02}.{}", s / 60, s % 60, tenths % 10)
}

pub fn fmt_bytes(b: u64) -> String {
    const KB: f64 = 1024.0;
    let f = b as f64;
    if f >= KB * KB * KB {
        format!("{:.1} GB", f / (KB * KB * KB))
    } else if f >= KB * KB {
        format!("{:.1} MB", f / (KB * KB))
    } else if f >= KB {
        format!("{:.0} KB", f / KB)
    } else {
        format!("{b} B")
    }
}

/// 6912 -> "6,912".
pub fn fmt_thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// "1280 × 720".
pub fn fmt_dims(w: u32, h: u32) -> String {
    format!(
        "{} \u{d7} {}",
        fmt_thousands(w as u64),
        fmt_thousands(h as u64)
    )
}

// ---------------------------------------------------------------------------------------
// Image math
// ---------------------------------------------------------------------------------------

/// UV rect that makes an image of `img` pixels fill a `bx` x `by` box without distortion
/// ("object-fit: cover"), centered horizontally and aligned to the top vertically so tall
/// scrolling captures show their beginning. Returns (u0, v0, u1, v1).
pub fn cover_uv(img: (f32, f32), bx: f32, by: f32) -> (f32, f32, f32, f32) {
    let (iw, ih) = img;
    if iw <= 0.0 || ih <= 0.0 || bx <= 0.0 || by <= 0.0 {
        return (0.0, 0.0, 1.0, 1.0);
    }
    let img_aspect = iw / ih;
    let box_aspect = bx / by;
    if img_aspect > box_aspect {
        let vis = box_aspect / img_aspect;
        let u0 = (1.0 - vis) / 2.0;
        (u0, 0.0, u0 + vis, 1.0)
    } else {
        let vis = img_aspect / box_aspect;
        (0.0, 0.0, 1.0, vis)
    }
}

/// Size that fits `(w, h)` into `max` x `max` while keeping aspect; unchanged when it fits.
pub fn fit_texture_size(w: u32, h: u32, max: u32) -> (u32, u32) {
    let max = max.max(1);
    if w <= max && h <= max {
        return (w, h);
    }
    let s = (max as f64 / w as f64).min(max as f64 / h as f64);
    (
        ((w as f64 * s).floor() as u32).clamp(1, max),
        ((h as f64 * s).floor() as u32).clamp(1, max),
    )
}

/// Largest rect with the aspect of `(iw, ih)` that fits inside `bounds`, centered.
pub fn contain_rect(bounds: Rect, iw: f32, ih: f32) -> Rect {
    if iw <= 0.0 || ih <= 0.0 {
        return bounds;
    }
    let s = (bounds.width() / iw).min(bounds.height() / ih);
    Rect::from_center_size(bounds.center(), Vec2::new(iw * s, ih * s))
}

// ---------------------------------------------------------------------------------------
// Drawing helpers
// ---------------------------------------------------------------------------------------

pub fn font(size: f32) -> FontId {
    FontId::proportional(size)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BtnKind {
    Neutral,
    Accent,
    Danger,
}

/// Button drawn into an exact rect (toast grids, HUD rows) so layouts are deterministic.
/// Same look as `theme::widgets::button` / `accent_button`: 6 px radius, control fill.
#[allow(clippy::too_many_arguments)]
pub fn rect_button(
    ui: &mut Ui,
    id: impl Hash + std::fmt::Debug,
    rect: Rect,
    text: &str,
    kind: BtnKind,
    enabled: bool,
    p: &Palette,
    accent: Color32,
) -> egui::Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let resp = ui.interact(rect, ui.scope_id().with(id), sense);
    let hovered = enabled && resp.hovered();
    let pressed = enabled && resp.is_pointer_button_down_on();
    let (mut fill, text_col, stroke) = match kind {
        BtnKind::Neutral => (
            p.control,
            if enabled { p.text } else { p.text_tertiary },
            Stroke::new(1.0, p.stroke),
        ),
        BtnKind::Accent => (accent, p.on_accent, Stroke::NONE),
        BtnKind::Danger => (p.danger, Color32::from_rgb(0x1f, 0x06, 0x05), Stroke::NONE),
    };
    if !enabled {
        fill = fill.gamma_multiply(0.6);
    } else if pressed {
        fill = fill.lerp_to_gamma(Color32::BLACK, 0.12);
    } else if hovered {
        fill = fill.lerp_to_gamma(Color32::WHITE, 0.08);
    }
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(6), fill);
    if stroke != Stroke::NONE {
        painter.rect_stroke(rect, CornerRadius::same(6), stroke, StrokeKind::Inside);
    }
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        font(13.0),
        text_col,
    );
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// Run `add` inside a fresh top-level Ui covering the whole viewport, after filling it with
/// `fill`. The surfaces use this instead of panels so layout is fully manual and works with
/// any viewport size.
pub fn root_ui(ctx: &egui::Context, fill: Color32, add: impl FnOnce(&mut Ui, Rect)) {
    let rect = ctx.content_rect();
    let mut ui = Ui::new(
        ctx.clone(),
        egui::Id::unique("stinkshot_root_ui"),
        egui::UiBuilder::new().max_rect(rect),
    );
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, CornerRadius::ZERO, fill);
    }
    add(&mut ui, rect);
}

/// Paint a 1px hairline card border + fill (used for cards inside windows).
pub fn card(ui: &Ui, rect: Rect, radius: u8, fill: Color32, stroke: Color32) {
    ui.painter()
        .rect_filled(rect, CornerRadius::same(radius), fill);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(radius),
        Stroke::new(1.0, stroke),
        StrokeKind::Inside,
    );
}

/// Wheel notches from a raw egui mouse wheel event (lines = 1 notch, points ~ 50 px/notch).
pub fn wheel_notches(unit: egui::MouseWheelUnit, delta_y: f32) -> f32 {
    match unit {
        egui::MouseWheelUnit::Line => delta_y,
        egui::MouseWheelUnit::Point => delta_y / 50.0,
        egui::MouseWheelUnit::Page => delta_y * 10.0,
    }
}

/// Sum of vertical wheel notches this frame and whether Ctrl was held. Reads the raw
/// events because egui turns Ctrl+wheel into a zoom gesture and removes it from the
/// smooth scroll delta.
pub fn frame_wheel(ctx: &egui::Context) -> (f32, bool) {
    ctx.input(|i| {
        let mut n = 0.0;
        let mut ctrl = i.modifiers.ctrl;
        for e in &i.raw.events {
            if let egui::Event::MouseWheel {
                unit,
                delta,
                modifiers,
                ..
            } = e
            {
                n += wheel_notches(*unit, delta.y);
                ctrl |= modifiers.ctrl;
            }
        }
        (n, ctrl)
    })
}

// ---------------------------------------------------------------------------------------
// Threads
// ---------------------------------------------------------------------------------------

/// Spawn a named worker; failure to spawn is logged and reported as `false`.
pub fn spawn_worker(name: &str, f: impl FnOnce() + Send + 'static) -> bool {
    match std::thread::Builder::new().name(name.to_string()).spawn(f) {
        Ok(_) => true,
        Err(e) => {
            log::error!("could not start worker {name}: {e}");
            false
        }
    }
}

// ---------------------------------------------------------------------------------------
// Windows glue
// ---------------------------------------------------------------------------------------

#[cfg(windows)]
mod sys {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    use windows::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, WS_EX_NOACTIVATE,
    };

    pub fn key_down(vk: u16) -> bool {
        // High bit set = currently down.
        unsafe { (GetAsyncKeyState(vk as i32) as u16) & 0x8000 != 0 }
    }

    pub fn set_no_activate(hwnd: isize, on: bool) {
        let h = HWND(hwnd as *mut core::ffi::c_void);
        unsafe {
            let cur = GetWindowLongPtrW(h, GWL_EXSTYLE);
            let flag = WS_EX_NOACTIVATE.0 as isize;
            let new = if on { cur | flag } else { cur & !flag };
            if new != cur {
                SetWindowLongPtrW(h, GWL_EXSTYLE, new);
            }
        }
    }

    pub fn pick_folder(
        owner: Option<isize>,
        start: Option<&std::path::Path>,
    ) -> Option<std::path::PathBuf> {
        use windows::Win32::System::Com::{
            CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
            CoTaskMemFree, CoUninitialize,
        };
        use windows::Win32::UI::Shell::{
            FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog, IShellItem,
            SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
        };
        use windows::core::HSTRING;

        unsafe {
            let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let inited = init.is_ok();
            let result = (|| -> windows::core::Result<Option<std::path::PathBuf>> {
                let dlg: IFileOpenDialog =
                    CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
                let opts = dlg.GetOptions()?;
                dlg.SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM)?;
                if let Some(dir) = start.filter(|d| d.is_dir())
                    && let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(
                        &HSTRING::from(dir.as_os_str()),
                        None,
                    )
                {
                    let _ = dlg.SetFolder(&item);
                }
                let owner = owner.map(|h| HWND(h as *mut core::ffi::c_void));
                if dlg.Show(owner).is_err() {
                    // Cancelled (HRESULT_FROM_WIN32(ERROR_CANCELLED)) or failed to show.
                    return Ok(None);
                }
                let item = dlg.GetResult()?;
                let name = item.GetDisplayName(SIGDN_FILESYSPATH)?;
                let path = name.to_string().ok().map(std::path::PathBuf::from);
                CoTaskMemFree(Some(name.0 as *const core::ffi::c_void));
                Ok(path)
            })();
            if inited {
                CoUninitialize();
            }
            match result {
                Ok(p) => p,
                Err(e) => {
                    log::warn!("folder picker failed: {e}");
                    None
                }
            }
        }
    }
}

#[cfg(not(windows))]
mod sys {
    pub fn key_down(_vk: u16) -> bool {
        false
    }
    pub fn set_no_activate(_hwnd: isize, _on: bool) {}
    pub fn pick_folder(
        _owner: Option<isize>,
        _start: Option<&std::path::Path>,
    ) -> Option<std::path::PathBuf> {
        None
    }
}

pub const VK_ESCAPE: u16 = 0x1B;
pub const VK_RETURN: u16 = 0x0D;
pub const VK_SNAPSHOT: u16 = 0x2C;
pub const VK_PAUSE: u16 = 0x13;
pub const VK_SCROLL: u16 = 0x91;
pub const VK_LWIN: u16 = 0x5B;
pub const VK_RWIN: u16 = 0x5C;

/// Physical key state independent of window focus.
pub fn async_key_down(vk: u16) -> bool {
    sys::key_down(vk)
}

/// WS_EX_NOACTIVATE: clicking the window never takes focus from the app being captured.
pub fn set_no_activate(hwnd: Hwnd, on: bool) {
    sys::set_no_activate(hwnd, on);
}

/// Blocking native folder picker. Call from a worker thread, never from the UI thread.
pub fn pick_folder(owner: Option<Hwnd>, start: Option<&std::path::Path>) -> Option<PathBuf> {
    sys::pick_folder(owner, start)
}

/// Apply the common "HUD style" window flags: topmost, tool window (no taskbar / Alt+Tab),
/// optional exclusion from capture, optional no-activate, and an exact physical rect.
pub fn apply_hud_style(hwnd: Hwnd, rect: PxRect, exclude_capture: bool, no_activate: bool) {
    use stinkshot_platform::window_util as wu;
    wu::set_tool_window(hwnd, true);
    wu::set_topmost(hwnd, true);
    if exclude_capture {
        wu::exclude_from_capture(hwnd, true);
    }
    if no_activate {
        set_no_activate(hwnd, true);
    }
    wu::set_rect(hwnd, rect);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        assert_eq!(parse_hex("#7fd36a"), Some([0x7f, 0xd3, 0x6a, 255]));
        assert_eq!(parse_hex("7FD36A80"), Some([0x7f, 0xd3, 0x6a, 0x80]));
        assert_eq!(parse_hex("#zzz"), None);
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("#ééé"), None);
        let c = hex_to_color("#ff8a3d").unwrap();
        assert_eq!(color_to_hex(c), "#ff8a3d");
    }

    #[test]
    fn toast_stacks_upward_in_work_area() {
        let work = PxRect::new(0, 0, 1920, 1040);
        let a = toast_rect(work, (400, 140), 140, 0, 16, 10);
        assert_eq!((a.x, a.y), (1920 - 16 - 400, 1040 - 16 - 140));
        let b = toast_rect(work, (400, 140), 140, 1, 16, 10);
        assert_eq!(b.y, a.y - 150);
        // Short message toast is bottom aligned in the same slot.
        let m = toast_rect(work, (400, 80), 140, 0, 16, 10);
        assert_eq!(m.bottom(), a.bottom());
        // Never leaves the top of the work area.
        let far = toast_rect(work, (400, 140), 140, 50, 16, 10);
        assert_eq!(far.y, 0);
    }

    #[test]
    fn toast_respects_offset_work_area() {
        // Secondary monitor to the left with a taskbar on top.
        let work = PxRect::new(-1920, 40, 1920, 1040);
        let r = toast_rect(work, (400, 140), 140, 0, 16, 10);
        assert_eq!(r.right(), 0 - 16);
        assert_eq!(r.bottom(), 40 + 1040 - 16);
    }

    #[test]
    fn hud_below_then_above_then_inside() {
        let work = PxRect::new(0, 0, 1920, 1040);
        let below = hud_rect(PxRect::new(100, 100, 500, 300), (370, 70), work, 12);
        assert_eq!((below.x, below.y), (100, 412));
        let above = hud_rect(PxRect::new(100, 700, 500, 300), (370, 70), work, 12);
        assert_eq!(above.y, 700 - 12 - 70);
        let inside = hud_rect(PxRect::new(0, 0, 1920, 1040), (370, 70), work, 12);
        assert_eq!(inside.bottom(), 1040 - 12);
        // Right edge clamp.
        let clamped = hud_rect(PxRect::new(1800, 100, 100, 100), (370, 70), work, 12);
        assert_eq!(clamped.right(), 1920);
    }

    #[test]
    fn clamp_into_keeps_size() {
        let b = PxRect::new(0, 0, 1000, 800);
        assert_eq!(
            clamp_into(PxRect::new(900, 700, 300, 300), b),
            PxRect::new(700, 500, 300, 300)
        );
        assert_eq!(
            clamp_into(PxRect::new(-50, -50, 100, 100), b),
            PxRect::new(0, 0, 100, 100)
        );
        assert_eq!(clamp_into(PxRect::new(0, 0, 2000, 100), b).w, 1000);
    }

    #[test]
    fn debounce_fires_once_after_delay() {
        let t0 = Instant::now();
        let mut d = Debounce::new(Duration::from_millis(150));
        d.push("a".to_string(), t0);
        assert_eq!(d.poll(t0 + Duration::from_millis(100)), None);
        // A newer push restarts the timer.
        d.push("ab".to_string(), t0 + Duration::from_millis(120));
        assert_eq!(d.poll(t0 + Duration::from_millis(200)), None);
        assert_eq!(
            d.remaining(t0 + Duration::from_millis(200)),
            Some(Duration::from_millis(70))
        );
        assert_eq!(
            d.poll(t0 + Duration::from_millis(271)),
            Some("ab".to_string())
        );
        assert_eq!(d.poll(t0 + Duration::from_secs(5)), None);
        assert!(!d.is_pending());
    }

    #[test]
    fn throttle_limits_rate() {
        let t0 = Instant::now();
        let mut th = Throttle::new(Duration::from_millis(66));
        assert!(th.ready(t0));
        assert!(!th.ready(t0 + Duration::from_millis(10)));
        assert!(th.ready(t0 + Duration::from_millis(70)));
    }

    #[test]
    fn lru_evicts_least_recent() {
        let mut l: Lru<u32, &str> = Lru::new(2);
        assert!(l.insert(1, "a").is_none());
        assert!(l.insert(2, "b").is_none());
        assert_eq!(l.get(&1), Some(&"a")); // 1 is now fresher than 2
        let evicted = l.insert(3, "c");
        assert_eq!(evicted, Some((2, "b")));
        assert!(l.contains(&1) && l.contains(&3) && !l.contains(&2));
        assert_eq!(l.len(), 2);
    }

    #[test]
    fn lru_overwrite_does_not_grow() {
        let mut l: Lru<u32, u32> = Lru::new(2);
        l.insert(1, 1);
        l.insert(1, 2);
        l.insert(2, 3);
        assert_eq!(l.len(), 2);
        assert_eq!(l.get(&1), Some(&2));
    }

    #[test]
    fn time_formats() {
        assert_eq!(fmt_clock(Duration::from_secs(42)), "00:42");
        assert_eq!(fmt_clock(Duration::from_secs(3725)), "1:02:05");
        assert_eq!(fmt_tenths(Duration::from_millis(3250)), "00:03.2");
        assert_eq!(fmt_tenths(Duration::from_millis(61_990)), "01:01.9");
        assert_eq!(fmt_tenths(Duration::ZERO), "00:00.0");
    }

    #[test]
    fn size_formats() {
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(2048), "2 KB");
        assert_eq!(fmt_bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(fmt_bytes(2_254_857_831), "2.1 GB");
        assert_eq!(fmt_thousands(6912), "6,912");
        assert_eq!(fmt_thousands(12), "12");
        assert_eq!(fmt_thousands(1_000_000), "1,000,000");
        assert_eq!(fmt_dims(1280, 6912), "1,280 \u{d7} 6,912");
    }

    #[test]
    fn cover_uv_crops_wide_and_tall() {
        // Wide image in a squarer box: crop left/right equally.
        let (u0, v0, u1, v1) = cover_uv((200.0, 100.0), 100.0, 100.0);
        assert!((u0 - 0.25).abs() < 1e-6 && (u1 - 0.75).abs() < 1e-6 && v0 == 0.0 && v1 == 1.0);
        // Tall image: keep the top.
        let (u0, v0, u1, v1) = cover_uv((100.0, 400.0), 100.0, 100.0);
        assert!(u0 == 0.0 && u1 == 1.0 && v0 == 0.0 && (v1 - 0.25).abs() < 1e-6);
        // Same aspect: whole image.
        assert_eq!(cover_uv((50.0, 25.0), 100.0, 50.0), (0.0, 0.0, 1.0, 1.0));
        // Degenerate input is safe.
        assert_eq!(cover_uv((0.0, 0.0), 10.0, 10.0), (0.0, 0.0, 1.0, 1.0));
    }

    #[test]
    fn texture_fit() {
        assert_eq!(fit_texture_size(800, 600, 2048), (800, 600));
        let (w, h) = fit_texture_size(1280, 30000, 8192);
        assert!(w <= 8192 && h <= 8192 && h == 8192);
        assert_eq!(w, (1280.0 * 8192.0 / 30000.0_f64).floor() as u32);
    }

    #[test]
    fn contain_rect_letterboxes() {
        let b = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(400.0, 200.0));
        let r = contain_rect(b, 100.0, 100.0);
        assert_eq!(r.size(), Vec2::new(200.0, 200.0));
        assert_eq!(r.center(), b.center());
    }

    #[test]
    fn wheel_units() {
        assert_eq!(wheel_notches(egui::MouseWheelUnit::Line, 2.0), 2.0);
        assert_eq!(wheel_notches(egui::MouseWheelUnit::Point, 100.0), 2.0);
        assert_eq!(wheel_notches(egui::MouseWheelUnit::Page, 1.0), 10.0);
    }
}
