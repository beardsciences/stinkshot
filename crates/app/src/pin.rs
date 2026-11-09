//! Pinned capture (docs/design/Pin.dc.html): borderless topmost window showing an image
//! 1:1 at its physical size. Drag moves (our own drag loop in physical px, see `PinDrag`);
//! wheel zooms (10%..800%); Ctrl+wheel opacity
//! (20%..100%); double-click opens in the editor (if from history); Ctrl+C copies; Esc or
//! the close button closes. The toolbar (opacity, edit, copy, close) sits in a strip above
//! the image (Always Show Pin Toolbar, the window grows to fit it, `Chrome`), or with that
//! setting off shows over the image on hover. Hover shows the accent outline. With Show Pins
//! in Task View the pin is a normal app window (Alt+Tab, Task View, taskbar) titled
//! "Pin: <name>", still always on top.
//!
//! The window is transparent (rounded corners + opacity are painted by egui), so the
//! shell's `App::clear_color` must return fully transparent for the pin viewports.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use egui::{
    Align2, Color32, CornerRadius, Event, Key, PointerButton, Rect, Sense, Stroke, StrokeKind,
    pos2, vec2,
};
use stinkshot_core::{PxPoint, PxRect, Rgba};
use stinkshot_history::ItemId;
use stinkshot_platform::Hwnd;

use crate::l10n::tr;
use crate::state::{Command, Shared, Surface, SurfaceStatus};
use crate::theme::{self, Icon};
use crate::toast::ui_common as uc;

pub const ZOOM_MIN: f32 = 0.1;
pub const ZOOM_MAX: f32 = 8.0;
pub const OPACITY_MIN: f32 = 0.2;
pub const OPACITY_MAX: f32 = 1.0;

/// Zoom after `notches` wheel notches (each notch is a 10% multiplicative step).
pub fn zoom_step(zoom: f32, notches: f32) -> f32 {
    (zoom * 1.1_f32.powf(notches)).clamp(ZOOM_MIN, ZOOM_MAX)
}

/// Opacity after `notches` wheel notches (5 points per notch), snapped to whole percent.
pub fn opacity_step(opacity: f32, notches: f32) -> f32 {
    (((opacity + notches * 0.05) * 100.0).round() / 100.0).clamp(OPACITY_MIN, OPACITY_MAX)
}

/// Toolbar opacity button: 100 -> 80 -> 60 -> 40 -> 20 -> 100.
pub fn cycle_opacity(opacity: f32) -> f32 {
    let next = ((opacity * 100.0).round() as i32 - 20).max(0) as f32 / 100.0;
    if next < OPACITY_MIN - 0.001 {
        OPACITY_MAX
    } else {
        next
    }
}

/// 1:1 unless the image would not fit the work area, then the largest zoom that fits.
pub fn initial_zoom(img: (u32, u32), work: PxRect) -> f32 {
    let (w, h) = (img.0.max(1) as f32, img.1.max(1) as f32);
    let fit = (work.w as f32 / w).min(work.h as f32 / h);
    if fit >= 1.0 {
        1.0
    } else {
        fit.clamp(ZOOM_MIN, 1.0)
    }
}

/// Physical window size for an image at `zoom`.
pub fn window_size_px(img: (u32, u32), zoom: f32) -> (i32, i32) {
    (
        ((img.0 as f32 * zoom).round() as i32).max(1),
        ((img.1 as f32 * zoom).round() as i32).max(1),
    )
}

/// New window origin after resizing so the image point under the cursor stays put.
/// `cursor_off` is the cursor offset from the window origin in physical px.
pub fn zoom_anchor(
    pos: (i32, i32),
    old: (i32, i32),
    new: (i32, i32),
    cursor_off: (f32, f32),
) -> (i32, i32) {
    let fx = if old.0 > 0 {
        new.0 as f32 / old.0 as f32
    } else {
        1.0
    };
    let fy = if old.1 > 0 {
        new.1 as f32 / old.1 as f32
    } else {
        1.0
    };
    (
        pos.0 + (cursor_off.0 * (1.0 - fx)).round() as i32,
        pos.1 + (cursor_off.1 * (1.0 - fy)).round() as i32,
    )
}

/// Cursor travel (physical px, either axis) before a press on the pin becomes a drag. Below
/// it the press stays a click, so double-click and the context menu keep working.
pub const DRAG_THRESHOLD_PX: i32 = 4;
/// A double click or toolbar click that lands this soon after a drag ends is ignored.
const DRAG_CLICK_GUARD: Duration = Duration::from_millis(400);

/// True once the cursor moved more than `threshold` px on either axis since the press.
pub fn past_drag_threshold(press: PxPoint, now: PxPoint, threshold: i32) -> bool {
    (now.x - press.x).abs() > threshold || (now.y - press.y).abs() > threshold
}

/// Window rect while dragging: the press-time origin moved by the cursor travel, at the
/// pin's intended physical size (never the size Windows may have applied on a DPI change).
pub fn dragged_rect(start: PxRect, press: PxPoint, now: PxPoint, size: (i32, i32)) -> PxRect {
    PxRect::new(
        start.x + (now.x - press.x),
        start.y + (now.y - press.y),
        size.0,
        size.1,
    )
}

/// The rect to re-assert when the real window drifted from the intended one: keep where
/// the window is, restore its size. None when it already matches.
pub fn size_fix(actual: PxRect, size: (i32, i32)) -> Option<PxRect> {
    ((actual.w, actual.h) != size).then(|| PxRect::new(actual.x, actual.y, size.0, size.1))
}

/// Our own drag loop. `ViewportCommand::StartDrag` hands the move to the OS, which only
/// started after egui's drag threshold (the window jumped), and across a DPI boundary
/// Windows rescales the window mid-drag, breaking the 1:1 pixel size.
#[derive(Clone, Copy, Debug)]
struct PinDrag {
    /// Global physical cursor position at the press.
    press: PxPoint,
    /// Window rect at the press.
    start: PxRect,
    /// Past the threshold: the window follows the cursor.
    moving: bool,
}

const TOOL_BTN: f32 = 28.0;
const TOOL_PAD: f32 = 3.0;
const TOOL_GAP: f32 = 4.0;
/// Height of the toolbar bar (points).
const BAR_H: f32 = TOOL_PAD * 2.0 + TOOL_BTN;
/// Always-shown toolbar: the strip above the image is the bar plus this gap (points).
const STRIP_GAP: f32 = 4.0;

/// Toolbar bar width for `n` buttons in a row (points).
fn bar_w(n: usize) -> f32 {
    let n = n.max(1) as f32;
    TOOL_PAD * 2.0 + TOOL_BTN * n + TOOL_GAP * (n - 1.0)
}

/// What the always-shown toolbar adds around the image, in physical px: a strip above it,
/// and a minimum window width so the toolbar fits over a narrow image (the image is then
/// centered under it). Zero when the toolbar only shows on hover.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Chrome {
    pub strip: i32,
    pub min_w: i32,
}

impl Chrome {
    pub fn new(always_toolbar: bool, buttons: usize, scale: f32) -> Chrome {
        if !always_toolbar {
            return Chrome::default();
        }
        let s = scale.max(0.1);
        Chrome {
            strip: ((BAR_H + STRIP_GAP) * s).round() as i32,
            min_w: (bar_w(buttons) * s).ceil() as i32,
        }
    }

    /// Window size for the image shown at `img` physical px.
    pub fn window_size(self, img: (i32, i32)) -> (i32, i32) {
        (img.0.max(self.min_w), img.1 + self.strip)
    }

    /// The image's offset inside the window.
    pub fn image_offset(self, img: (i32, i32)) -> (i32, i32) {
        ((self.window_size(img).0 - img.0) / 2, self.strip)
    }

    /// Window origin that puts the image's top left at `img_at`.
    pub fn window_at(self, img_at: (i32, i32), img: (i32, i32)) -> (i32, i32) {
        let off = self.image_offset(img);
        (img_at.0 - off.0, img_at.1 - off.1)
    }
}

/// The window rect after zooming the image from `old_img` to `new_img` px, keeping the image
/// point under the cursor in place. `pos` is the window origin, `cursor` the cursor offset
/// from it (physical px; it may be over the toolbar strip, then the anchor is clamped to the
/// image edge).
pub fn zoomed_rect(
    chrome: Chrome,
    pos: (i32, i32),
    old_img: (i32, i32),
    new_img: (i32, i32),
    cursor: (f32, f32),
) -> PxRect {
    let off = chrome.image_offset(old_img);
    let img_at = (pos.0 + off.0, pos.1 + off.1);
    let in_img = (
        (cursor.0 - off.0 as f32).clamp(0.0, old_img.0 as f32),
        (cursor.1 - off.1 as f32).clamp(0.0, old_img.1 as f32),
    );
    let new_at = zoom_anchor(img_at, old_img, new_img, in_img);
    let (x, y) = chrome.window_at(new_at, new_img);
    let (w, h) = chrome.window_size(new_img);
    PxRect::new(x, y, w, h)
}

/// The window rect after switching chrome (toolbar shown or hidden, DPI change): the image
/// stays where it is on screen.
pub fn rechromed_rect(old: Chrome, new: Chrome, pos: (i32, i32), img: (i32, i32)) -> PxRect {
    let off = old.image_offset(img);
    let (x, y) = new.window_at((pos.0 + off.0, pos.1 + off.1), img);
    let (w, h) = new.window_size(img);
    PxRect::new(x, y, w, h)
}

/// Toolbar buttons, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    Opacity,
    Edit,
    Copy,
    Close,
}

fn tools(has_item: bool) -> Vec<Tool> {
    let mut t = vec![Tool::Opacity];
    if has_item {
        t.push(Tool::Edit);
    }
    t.extend([Tool::Copy, Tool::Close]);
    t
}

/// Button rects in `bar`: left to right; with `close_right` the last one (Close) sits at the
/// right end of a bar wider than the buttons need.
fn button_rects(bar: Rect, n: usize, close_right: bool) -> Vec<Rect> {
    let mut out = Vec::with_capacity(n);
    let mut x = bar.left() + TOOL_PAD;
    for i in 0..n {
        let left = if close_right && n > 1 && i == n - 1 {
            (bar.right() - TOOL_PAD - TOOL_BTN).max(x)
        } else {
            x
        };
        out.push(Rect::from_min_size(
            pos2(left, bar.top() + TOOL_PAD),
            vec2(TOOL_BTN, TOOL_BTN),
        ));
        x = left + TOOL_BTN + TOOL_GAP;
    }
    out
}

/// How a pin is created (from the Pins settings at the time).
#[derive(Clone, Copy, Debug)]
pub struct PinOpts {
    /// A normal app window (Alt+Tab, Task View, taskbar) instead of a tool window.
    pub task_view: bool,
    /// Toolbar always shown in a strip above the image.
    pub always_toolbar: bool,
}

pub struct Pin {
    image: Arc<Rgba>,
    title: String,
    item: Option<ItemId>,
    id: u64,
    zoom: f32,
    opacity: f32,
    area: uc::MonArea,
    /// Where the window is created. The viewport builder always reports this rect: egui
    /// turns any builder change into OuterPosition/InnerSize commands in points, which
    /// winit converts with the window's current scale and which fight our physical moves.
    initial: PxRect,
    /// Intended physical rect (position and window size). Authoritative for the size.
    rect: PxRect,
    /// Toolbar strip around the image (follows the setting and the window's DPI).
    chrome: Chrome,
    always_toolbar: bool,
    /// Taskbar flag the window was created with (the builder must not change afterwards).
    created_task_view: bool,
    /// Task View flag currently applied to the window style.
    task_view: bool,
    drag: Option<PinDrag>,
    last_drag_end: Option<Instant>,
    hwnd: Option<Hwnd>,
    texture: Option<egui::TextureHandle>,
    chip: Option<(String, Instant)>,
    sent_transparent: bool,
    copy_rx: Option<Receiver<Result<(), String>>>,
}

impl Pin {
    /// `at`: physical rect of the image on screen (e.g. where the region was captured);
    /// None = centered on the monitor with the cursor.
    pub fn new(
        image: Rgba,
        title: String,
        item: Option<ItemId>,
        at: Option<PxRect>,
        id: u64,
        opts: PinOpts,
    ) -> Self {
        let area = match at {
            Some(r) => uc::area_of_rect(r),
            None => uc::area_under_cursor(),
        };
        let chrome = Chrome::new(opts.always_toolbar, tools(item.is_some()).len(), area.scale);
        let dims = (image.width, image.height);
        let mut fit_in = area.work;
        fit_in.h = (fit_in.h - chrome.strip).max(1);
        let zoom = initial_zoom(dims, fit_in);
        let img = window_size_px(dims, zoom);
        let (w, h) = chrome.window_size(img);
        let rect = match at {
            Some(r) => {
                let full = PxRect::new(area.work.x, area.work.y, area.work.w, area.work.h);
                let (x, y) = chrome.window_at((r.x, r.y), img);
                uc::clamp_into(PxRect::new(x, y, w, h), full)
            }
            None => uc::centered_in(area.work, w, h),
        };
        Self {
            image: Arc::new(image),
            title,
            item,
            id,
            zoom,
            opacity: 1.0,
            area,
            initial: rect,
            rect,
            chrome,
            always_toolbar: opts.always_toolbar,
            created_task_view: opts.task_view,
            task_view: opts.task_view,
            drag: None,
            last_drag_end: None,
            hwnd: None,
            texture: None,
            chip: None,
            sent_transparent: false,
            copy_rx: None,
        }
    }

    /// Window size at the current zoom and chrome.
    fn win_size(&self, dims: (u32, u32)) -> (i32, i32) {
        self.chrome.window_size(window_size_px(dims, self.zoom))
    }

    fn texture_id(&mut self, ctx: &egui::Context) -> Option<egui::TextureId> {
        if self.texture.is_none() {
            let max = ctx.input(|i| i.max_texture_side) as u32;
            let (w, h) = uc::fit_texture_size(self.image.width, self.image.height, max);
            if self.image.width == 0 || self.image.height == 0 {
                return None;
            }
            let ci = if (w, h) == (self.image.width, self.image.height) {
                uc::rgba_to_color_image(&self.image)
            } else {
                uc::rgba_to_color_image(&self.image.thumbnail(w, h))
            };
            self.texture = Some(ctx.load_texture(
                format!("pin_{}", self.id),
                ci,
                egui::TextureOptions::LINEAR,
            ));
        }
        self.texture.as_ref().map(|t| t.id())
    }

    fn start_copy(&mut self) {
        if self.copy_rx.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let img = Arc::clone(&self.image);
        let ok = uc::spawn_worker("pin-copy", move || {
            let r = stinkshot_platform::clipboard::set_image(&img)
                .map_err(|e| crate::workers::clipboard_text_error(&e));
            let _ = tx.send(r);
        });
        if ok {
            self.copy_rx = Some(rx);
        }
    }

    fn apply_rect(&mut self, ctx: &egui::Context, rect: PxRect) {
        self.rect = rect;
        match self.hwnd {
            Some(h) => stinkshot_platform::window_util::set_rect(h, rect),
            None => {
                let s = self.area.scale;
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(
                    uc::pts(rect.w, s),
                    uc::pts(rect.h, s),
                )));
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos2(
                    uc::pts(rect.x, s),
                    uc::pts(rect.y, s),
                )));
            }
        }
    }

    /// Follow the Pins settings and the window's DPI: resize around the image when the
    /// toolbar strip changes, switch the window style when Show Pins in Task View changes.
    fn follow_settings(&mut self, ctx: &egui::Context, shared: &Shared, ppp: f32) {
        let pins = &shared.config.pins;
        self.always_toolbar = pins.always_show_toolbar;
        let chrome = Chrome::new(self.always_toolbar, tools(self.item.is_some()).len(), ppp);
        if chrome != self.chrome && self.drag.is_none() {
            let dims = (self.image.width, self.image.height);
            let img = window_size_px(dims, self.zoom);
            let r = rechromed_rect(self.chrome, chrome, (self.rect.x, self.rect.y), img);
            self.chrome = chrome;
            self.apply_rect(ctx, r);
        }
        if pins.show_in_task_view != self.task_view
            && let Some(h) = self.hwnd
        {
            self.task_view = pins.show_in_task_view;
            use stinkshot_platform::window_util as wu;
            wu::set_tool_window(h, !self.task_view);
            wu::set_topmost(h, true);
        }
    }

    /// Drive the drag loop: start on a primary press on the background, follow the global
    /// cursor in physical px while the button is held, end on release.
    fn update_drag(&mut self, ctx: &egui::Context, press_bg: bool, dims: (u32, u32)) {
        use stinkshot_platform::{input, window_util};
        let Some(h) = self.hwnd else {
            self.drag = None;
            return;
        };
        if press_bg
            && self.drag.is_none()
            && let Some(press) = input::try_cursor_pos()
        {
            let size = self.win_size(dims);
            let at = window_util::get_rect(h).unwrap_or(self.rect);
            self.drag = Some(PinDrag {
                press,
                start: PxRect::new(at.x, at.y, size.0, size.1),
                moving: false,
            });
        }
        let Some(mut d) = self.drag else { return };
        // egui sees the release through winit's mouse capture; the async state also catches
        // a release that never reached the window.
        let held = ctx.input(|i| i.pointer.primary_down()) && input::primary_button_down();
        if !held {
            if d.moving {
                self.last_drag_end = Some(Instant::now());
            }
            self.drag = None;
            return;
        }
        if let Some(now) = input::try_cursor_pos() {
            if !d.moving && past_drag_threshold(d.press, now, DRAG_THRESHOLD_PX) {
                d.moving = true;
            }
            if d.moving {
                let r = dragged_rect(d.start, d.press, now, self.win_size(dims));
                // Also re-assert when Windows rescaled the window on a DPI change.
                let drifted = window_util::get_rect(h).is_some_and(|a| a != r);
                if r != self.rect || drifted {
                    self.rect = r;
                    window_util::set_rect(h, r);
                }
            }
        }
        self.drag = Some(d);
        // Keep polling the cursor while the button is held: there may be no mouse event
        // between frames, and the pin must follow anyway.
        ctx.request_repaint_after(Duration::from_millis(8));
    }

    fn set_chip(&mut self, text: String) {
        self.chip = Some((text, Instant::now()));
    }
}

impl Surface for Pin {
    fn title(&self) -> String {
        format!("StinkShot.Pin.{}", self.id)
    }

    fn viewport_builder(&self, _shared: &Shared) -> egui::ViewportBuilder {
        let s = self.area.scale;
        let r = self.initial;
        egui::ViewportBuilder::default()
            .with_title(self.title())
            .with_inner_size(vec2(uc::pts(r.w, s), uc::pts(r.h, s)))
            .with_position(pos2(uc::pts(r.x, s), uc::pts(r.y, s)))
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_always_on_top()
            .with_taskbar(self.created_task_view)
            .with_active(false)
            .with_drag_and_drop(false)
    }

    fn on_created(&mut self, hwnd: Hwnd, _shared: &Shared) {
        self.hwnd = Some(hwnd);
        use stinkshot_platform::window_util as wu;
        wu::set_tool_window(hwnd, !self.task_view);
        wu::set_topmost(hwnd, true);
        wu::set_rect(hwnd, self.rect);
        // The shell found the window by its unique title; from now on Alt+Tab, Task View and
        // the taskbar show a readable one.
        crate::sysutil::set_window_text(hwnd, &tr!("shell.pin.window_title", name = self.title));
    }

    fn show(&mut self, ctx: &egui::Context, shared: &mut Shared) -> SurfaceStatus {
        if !self.sent_transparent {
            ctx.send_viewport_cmd(egui::ViewportCommand::Transparent(true));
            self.sent_transparent = true;
        }

        // Copy result.
        if let Some(rx) = &self.copy_rx {
            match rx.try_recv() {
                Ok(res) => {
                    self.copy_rx = None;
                    let cmd = match res {
                        Ok(()) => Command::Notify {
                            title: tr!("shell.pin.copied").into(),
                            body: tr!("shell.pin.ready", name = self.title),
                            error: false,
                        },
                        Err(e) => Command::Notify {
                            title: tr!("shell.pin.copy_failed").into(),
                            body: e,
                            error: true,
                        },
                    };
                    shared.outbox.push(cmd);
                }
                Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(50)),
                Err(TryRecvError::Disconnected) => self.copy_rx = None,
            }
        }

        let ppp = ctx
            .input(|i| i.viewport().native_pixels_per_point)
            .unwrap_or_else(|| ctx.pixels_per_point());
        let dims = (self.image.width, self.image.height);
        self.follow_settings(ctx, shared, ppp);
        // Outside a drag, follow where the window really is (Windows may move it on a DPI or
        // display change) but keep the intended 1:1 size: a move onto a monitor with another
        // scale makes Windows (WM_DPICHANGED) rescale the window, so put the size back.
        if self.drag.is_none()
            && let Some(h) = self.hwnd
            && let Some(actual) = stinkshot_platform::window_util::get_rect(h)
        {
            let size = self.win_size(dims);
            self.rect = PxRect::new(actual.x, actual.y, size.0, size.1);
            if let Some(fix) = size_fix(actual, size) {
                stinkshot_platform::window_util::set_rect(h, fix);
            }
        }

        // Wheel: zoom, Ctrl+wheel: opacity.
        let hovering = ctx.input(|i| i.pointer.hover_pos());
        let (notches, ctrl) = uc::frame_wheel(ctx);
        if notches != 0.0 && hovering.is_some() && self.drag.is_none() {
            if ctrl {
                let o = opacity_step(self.opacity, notches);
                if (o - self.opacity).abs() > f32::EPSILON {
                    self.opacity = o;
                    self.set_chip(tr!("shell.pin.opacity", pct = (o * 100.0).round() as i32));
                }
            } else {
                let z = zoom_step(self.zoom, notches);
                if (z - self.zoom).abs() > f32::EPSILON {
                    let old = window_size_px(dims, self.zoom);
                    let new = window_size_px(dims, z);
                    let win = self.win_size(dims);
                    let cursor = hovering
                        .map(|p| (p.x * ppp, p.y * ppp))
                        .unwrap_or((win.0 as f32 / 2.0, win.1 as f32 / 2.0));
                    let r = zoomed_rect(self.chrome, (self.rect.x, self.rect.y), old, new, cursor);
                    self.zoom = z;
                    self.apply_rect(ctx, r);
                    self.set_chip(format!("{}%", (z * 100.0).round() as i32));
                }
            }
        }

        // Keys.
        let (esc, copy) = ctx.input(|i| {
            let esc = i.key_pressed(Key::Escape);
            let copy = i.events.iter().any(|e| matches!(e, Event::Copy))
                || (i.modifiers.command && i.key_pressed(Key::C));
            (esc, copy)
        });
        let mut close = esc;
        let mut want_copy = copy;
        let mut edit = false;
        let mut reset_zoom = false;
        let tex = self.texture_id(ctx);
        let p = uc::pal(shared);
        let accent = uc::accent(shared);
        let opacity = self.opacity;
        let mut cycle = false;
        let mut press_bg = false;
        let item = self.item;
        let recent_drag = self.drag.is_some_and(|d| d.moving)
            || self
                .last_drag_end
                .is_some_and(|t| t.elapsed() < DRAG_CLICK_GUARD);
        let chip = self
            .chip
            .as_ref()
            .filter(|(_, t)| t.elapsed() < Duration::from_millis(900))
            .map(|(s, _)| s.clone());
        if self
            .chip
            .as_ref()
            .is_some_and(|(_, t)| t.elapsed() >= Duration::from_millis(900))
        {
            self.chip = None;
        }
        if chip.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let img_px = window_size_px(dims, self.zoom);
        let img_off = self.chrome.image_offset(img_px);
        let strip = self.chrome.strip > 0;
        let tool_list = tools(item.is_some());

        uc::root_ui(ctx, Color32::TRANSPARENT, |ui, rect| {
            let radius = CornerRadius::same(8);
            let img_rect = Rect::from_min_size(
                rect.min + vec2(img_off.0 as f32 / ppp, img_off.1 as f32 / ppp),
                vec2(img_px.0 as f32 / ppp, img_px.1 as f32 / ppp),
            )
            .intersect(rect);
            let tint = Color32::from_white_alpha((opacity * 255.0).round() as u8);
            if let Some(id) = tex {
                ui.painter().add(
                    egui::epaint::RectShape::filled(img_rect, radius, tint)
                        .with_texture(id, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))),
                );
            } else {
                ui.painter()
                    .rect_filled(img_rect, radius, p.layer.gamma_multiply(opacity));
            }

            // Toolbar: always shown in the strip above the image, or (setting off) over the
            // image's top right while hovered, when the image is big enough for it.
            let n = tool_list.len();
            let (bar, bar_shown) = if strip {
                (
                    Rect::from_min_size(rect.min, vec2(rect.width(), BAR_H)),
                    true,
                )
            } else {
                let w = bar_w(n);
                let tb = Rect::from_min_size(
                    pos2(
                        (img_rect.right() - 6.0 - w).max(img_rect.left()),
                        img_rect.top() + 6.0,
                    ),
                    vec2(w, BAR_H),
                );
                let fits = img_rect.width() >= w + 12.0 && img_rect.height() >= BAR_H + 12.0;
                (tb, fits)
            };
            let buttons = button_rects(bar, n, strip);

            let bg = ui.interact(rect, ui.scope_id().with("pin_bg"), Sense::click_and_drag());
            // egui reports the background as hovered under the toolbar buttons too (a click
            // widget over a drag widget hits both), so presses on the buttons are kept out of
            // the drag explicitly. The strip around them drags the pin like a title bar.
            let press = ui.input(|i| {
                i.pointer
                    .button_pressed(PointerButton::Primary)
                    .then(|| i.pointer.press_origin())
                    .flatten()
            });
            let on_button = |pos: egui::Pos2| bar_shown && buttons.iter().any(|b| b.contains(pos));
            if bg.hovered() && press.is_some_and(|pos| !on_button(pos)) {
                press_bg = true;
            }
            let on_image = ui
                .input(|i| i.pointer.interact_pos())
                .is_some_and(|pos| img_rect.contains(pos));
            if bg.double_clicked() && on_image && item.is_some() && !recent_drag {
                edit = true;
            }
            bg.context_menu(|ui| {
                crate::theme::menu_rows(ui);
                if ui.button(tr!("shell.pin.copy_image")).clicked() {
                    want_copy = true;
                    ui.close();
                }
                if item.is_some() && ui.button(tr!("shell.pin.open_editor")).clicked() {
                    edit = true;
                    ui.close();
                }
                if ui.button(tr!("shell.pin.reset_zoom")).clicked() {
                    reset_zoom = true;
                    ui.close();
                }
                if ui.button(tr!("shell.pin.close")).clicked() {
                    close = true;
                    ui.close();
                }
            });

            let hovered = bg.hovered() || ui.rect_contains_pointer(rect);
            if hovered {
                ui.painter().rect_stroke(
                    img_rect,
                    radius,
                    Stroke::new(1.5, accent),
                    StrokeKind::Inside,
                );
            }

            if bar_shown && (strip || hovered) {
                if strip {
                    // The strip is window, not overlay: opaque across its full width and
                    // down to the image, so nothing behind the pin shows through.
                    let strip_rect = Rect::from_min_max(rect.min, pos2(rect.max.x, img_rect.top()));
                    ui.painter().rect_filled(
                        strip_rect,
                        CornerRadius {
                            nw: 6,
                            ne: 6,
                            sw: 0,
                            se: 0,
                        },
                        Color32::from_rgb(32, 32, 32),
                    );
                } else {
                    let bar_fill = Color32::from_rgba_unmultiplied(32, 32, 32, 230);
                    ui.painter()
                        .rect_filled(bar, CornerRadius::same(6), bar_fill);
                }
                let icon_col = Color32::from_rgb(0xe6, 0xe6, 0xe6);
                for (tool, r) in tool_list.iter().zip(buttons.iter().copied()) {
                    let resp = ui.interact(
                        r,
                        ui.scope_id().with(("pin_tool", *tool as u8)),
                        Sense::click(),
                    );
                    if resp.hovered() {
                        ui.painter().rect_filled(
                            r,
                            CornerRadius::same(4),
                            Color32::from_white_alpha(28),
                        );
                    }
                    let icon = |icon: Icon| {
                        theme::widgets::paint_icon(
                            ui.painter(),
                            Rect::from_center_size(r.center(), vec2(14.0, 14.0)),
                            icon,
                            icon_col,
                        )
                    };
                    let tip = match tool {
                        Tool::Opacity => {
                            ui.painter().text(
                                r.center(),
                                Align2::CENTER_CENTER,
                                format!("{}%", (opacity * 100.0).round() as i32),
                                uc::font(11.0),
                                icon_col,
                            );
                            tr!("shell.pin.opacity.tip")
                        }
                        Tool::Edit => {
                            icon(Icon::Edit);
                            tr!("shell.pin.edit.tip")
                        }
                        Tool::Copy => {
                            icon(Icon::Copy);
                            tr!("shell.pin.copy.tip")
                        }
                        Tool::Close => {
                            icon(Icon::Close);
                            tr!("shell.pin.close.tip")
                        }
                    };
                    if resp.on_hover_text(tip).clicked() && !recent_drag {
                        match tool {
                            Tool::Opacity => cycle = true,
                            Tool::Edit => edit = true,
                            Tool::Copy => want_copy = true,
                            Tool::Close => close = true,
                        }
                    }
                }
            }

            if let Some(text) = &chip {
                let galley =
                    ui.painter()
                        .layout_no_wrap(text.clone(), uc::font(12.0), Color32::WHITE);
                let size = galley.size() + vec2(16.0, 8.0);
                let r = Rect::from_min_size(
                    pos2(
                        img_rect.left() + 8.0,
                        (img_rect.bottom() - 8.0 - size.y).max(img_rect.top()),
                    ),
                    size,
                );
                ui.painter().rect_filled(
                    r,
                    CornerRadius::same(6),
                    Color32::from_rgba_unmultiplied(20, 20, 20, 220),
                );
                ui.painter()
                    .galley(r.min + vec2(8.0, 4.0), galley, Color32::WHITE);
            }
        });
        self.update_drag(ctx, press_bg, dims);

        if cycle {
            self.opacity = cycle_opacity(self.opacity);
            self.set_chip(tr!(
                "shell.pin.opacity",
                pct = (self.opacity * 100.0).round() as i32
            ));
        }
        if reset_zoom && (self.zoom - 1.0).abs() > f32::EPSILON {
            let old = window_size_px(dims, self.zoom);
            let new = window_size_px(dims, 1.0);
            let off = self.chrome.image_offset(old);
            let center = (
                off.0 as f32 + old.0 as f32 / 2.0,
                off.1 as f32 + old.1 as f32 / 2.0,
            );
            let r = zoomed_rect(self.chrome, (self.rect.x, self.rect.y), old, new, center);
            self.zoom = 1.0;
            self.apply_rect(ctx, r);
            self.set_chip("100%".into());
        }
        if want_copy {
            self.start_copy();
        }
        if edit && let Some(id) = self.item {
            shared.outbox.push(Command::EditItem(id));
        }
        if close {
            SurfaceStatus::Close
        } else {
            SurfaceStatus::Open
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_is_clamped_and_multiplicative() {
        assert!((zoom_step(1.0, 1.0) - 1.1).abs() < 1e-6);
        assert!((zoom_step(1.0, -1.0) - 1.0 / 1.1).abs() < 1e-6);
        assert_eq!(zoom_step(7.9, 5.0), ZOOM_MAX);
        assert_eq!(zoom_step(0.11, -10.0), ZOOM_MIN);
    }

    #[test]
    fn opacity_snaps_and_clamps() {
        assert_eq!(opacity_step(1.0, -1.0), 0.95);
        assert_eq!(opacity_step(1.0, 3.0), 1.0);
        assert_eq!(opacity_step(0.25, -5.0), 0.2);
    }

    #[test]
    fn opacity_button_cycles() {
        let mut o = 1.0;
        let mut seen = vec![];
        for _ in 0..5 {
            o = cycle_opacity(o);
            seen.push((o * 100.0).round() as i32);
        }
        assert_eq!(seen, vec![80, 60, 40, 20, 100]);
    }

    #[test]
    fn initial_zoom_is_one_to_one_unless_too_big() {
        let work = PxRect::new(0, 0, 1920, 1040);
        assert_eq!(initial_zoom((800, 600), work), 1.0);
        assert_eq!(initial_zoom((1920, 1040), work), 1.0);
        let z = initial_zoom((1280, 30000), work);
        assert!((z - 1040.0 / 30000.0).abs() < 1e-6 || z == ZOOM_MIN);
        assert!((ZOOM_MIN..1.0).contains(&z));
        let z = initial_zoom((3840, 2160), work);
        assert!((z - 1040.0 / 2160.0).abs() < 1e-6);
    }

    #[test]
    fn window_size_follows_zoom() {
        assert_eq!(window_size_px((800, 600), 1.0), (800, 600));
        assert_eq!(window_size_px((800, 600), 0.5), (400, 300));
        assert_eq!(window_size_px((1, 1), 0.1), (1, 1));
    }

    #[test]
    fn drag_threshold_is_per_axis_and_exclusive() {
        let p = PxPoint { x: 100, y: 100 };
        assert!(!past_drag_threshold(p, p, DRAG_THRESHOLD_PX));
        assert!(!past_drag_threshold(
            p,
            PxPoint { x: 104, y: 96 },
            DRAG_THRESHOLD_PX
        ));
        assert!(past_drag_threshold(
            p,
            PxPoint { x: 105, y: 100 },
            DRAG_THRESHOLD_PX
        ));
        assert!(past_drag_threshold(
            p,
            PxPoint { x: 100, y: 95 },
            DRAG_THRESHOLD_PX
        ));
    }

    #[test]
    fn drag_moves_by_cursor_travel_and_keeps_size() {
        let start = PxRect::new(3500, 200, 600, 400);
        let press = PxPoint { x: 3600, y: 300 };
        // Across the 3840 boundary onto the 100% monitor: same offset, same physical size,
        // whatever size Windows applied on the DPI change.
        let now = PxPoint { x: 4100, y: 350 };
        assert_eq!(
            dragged_rect(start, press, now, (600, 400)),
            PxRect::new(4000, 250, 600, 400)
        );
        // Moving back to the press point returns exactly to the start (no drift).
        assert_eq!(dragged_rect(start, press, press, (600, 400)), start);
        // Negative virtual coordinates (monitor left of the primary).
        assert_eq!(
            dragged_rect(start, press, PxPoint { x: -100, y: 0 }, (600, 400)),
            PxRect::new(-200, -100, 600, 400)
        );
    }

    #[test]
    fn size_fix_restores_intended_size_only() {
        // Windows rescaled a 600x400 pin by 1.5 when it moved onto the 150% monitor.
        let actual = PxRect::new(100, 50, 900, 600);
        assert_eq!(
            size_fix(actual, (600, 400)),
            Some(PxRect::new(100, 50, 600, 400))
        );
        assert_eq!(size_fix(PxRect::new(100, 50, 600, 400), (600, 400)), None);
    }

    #[test]
    fn anchor_keeps_point_under_cursor() {
        // Cursor at the window center doubles size: origin moves up-left by half the old size.
        assert_eq!(
            zoom_anchor((100, 100), (200, 100), (400, 200), (100.0, 50.0)),
            (0, 50)
        );
        // Cursor at the origin: origin does not move.
        assert_eq!(
            zoom_anchor((100, 100), (200, 100), (400, 200), (0.0, 0.0)),
            (100, 100)
        );
        // Shrinking with cursor at the far corner moves origin toward it.
        assert_eq!(
            zoom_anchor((0, 0), (400, 200), (200, 100), (400.0, 200.0)),
            (200, 100)
        );
    }

    #[test]
    fn chrome_adds_a_strip_and_a_minimum_width() {
        assert_eq!(Chrome::new(false, 4, 1.5), Chrome::default());
        let c = Chrome::new(true, 4, 1.0);
        assert_eq!(c.strip, (BAR_H + STRIP_GAP).round() as i32);
        assert_eq!(c.min_w, bar_w(4).ceil() as i32);
        // A wide image: the window is the image plus the strip, image right under it.
        assert_eq!(c.window_size((800, 600)), (800, 600 + c.strip));
        assert_eq!(c.image_offset((800, 600)), (0, c.strip));
        // A tiny image: the window is as wide as the toolbar, the image centered under it.
        let (w, h) = c.window_size((20, 10));
        assert_eq!((w, h), (c.min_w, 10 + c.strip));
        assert_eq!(c.image_offset((20, 10)), ((c.min_w - 20) / 2, c.strip));
        // Scales with DPI.
        let c2 = Chrome::new(true, 4, 2.0);
        assert!(c2.strip >= c.strip * 2 - 1 && c2.min_w >= c.min_w * 2 - 1);
        // Placing the window so the image lands on the captured rect.
        assert_eq!(c.window_at((100, 200), (800, 600)), (100, 200 - c.strip));
    }

    #[test]
    fn zoom_with_chrome_anchors_on_the_image() {
        let c = Chrome::new(true, 4, 1.0);
        // Cursor at the image center: the image center stays put.
        let pos = (100, 100);
        let img = (400, 200);
        let cur = (200.0, c.strip as f32 + 100.0);
        let r = zoomed_rect(c, pos, img, (800, 400), cur);
        let off = c.image_offset((800, 400));
        assert_eq!((r.x + off.0, r.y + off.1), (100 - 200, 100 + c.strip - 100));
        assert_eq!((r.w, r.h), c.window_size((800, 400)));
        // Cursor over the strip: anchored at the image's top edge, so the image top stays.
        let r = zoomed_rect(c, pos, img, (800, 400), (0.0, 5.0));
        assert_eq!(r.y, 100);
        // No chrome: the same as the plain anchor.
        let r = zoomed_rect(Chrome::default(), pos, img, (800, 400), (200.0, 100.0));
        assert_eq!(
            (r.x, r.y),
            zoom_anchor(pos, img, (800, 400), (200.0, 100.0))
        );
    }

    #[test]
    fn toggling_the_toolbar_keeps_the_image_in_place() {
        let on = Chrome::new(true, 4, 1.0);
        let off = Chrome::default();
        let img = (30, 30);
        let shown = rechromed_rect(off, on, (500, 500), img);
        let o = on.image_offset(img);
        assert_eq!((shown.x + o.0, shown.y + o.1), (500, 500));
        let hidden = rechromed_rect(on, off, (shown.x, shown.y), img);
        assert_eq!(hidden, PxRect::new(500, 500, 30, 30));
    }

    #[test]
    fn toolbar_buttons_fit_and_close_sits_right() {
        assert_eq!(
            tools(true),
            vec![Tool::Opacity, Tool::Edit, Tool::Copy, Tool::Close]
        );
        assert_eq!(tools(false), vec![Tool::Opacity, Tool::Copy, Tool::Close]);
        let bar = Rect::from_min_size(pos2(0.0, 0.0), vec2(bar_w(4), BAR_H));
        let b = button_rects(bar, 4, true);
        assert_eq!(b.len(), 4);
        assert!(b.iter().all(|r| bar.contains_rect(*r)));
        for w in b.windows(2) {
            assert!(w[0].right() <= w[1].left());
        }
        let wide = Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, BAR_H));
        let b = button_rects(wide, 4, true);
        assert!((b[3].right() - (600.0 - TOOL_PAD)).abs() < 1e-3);
        assert!((b[2].right() - (TOOL_PAD + 3.0 * TOOL_BTN + 2.0 * TOOL_GAP)).abs() < 1e-3);
    }
}
