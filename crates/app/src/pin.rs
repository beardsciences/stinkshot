//! Pinned capture (docs/design/Pin.dc.html): borderless topmost window showing an image
//! 1:1 at its physical size. Drag moves (our own drag loop in physical px, see `PinDrag`);
//! wheel zooms (10%..800%); Ctrl+wheel opacity
//! (20%..100%); double-click opens in the editor (if from history); Ctrl+C copies; Esc or
//! the close button closes; hover shows the mini toolbar (opacity, copy, close) and the
//! accent outline.
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
    /// Intended physical rect (position and image size x zoom). Authoritative for the size.
    rect: PxRect,
    drag: Option<PinDrag>,
    last_drag_end: Option<Instant>,
    hwnd: Option<Hwnd>,
    texture: Option<egui::TextureHandle>,
    chip: Option<(String, Instant)>,
    sent_transparent: bool,
    copy_rx: Option<Receiver<Result<(), String>>>,
}

impl Pin {
    /// `at`: initial physical rect (e.g. where the region was captured); None = centered
    /// on the monitor with the cursor.
    pub fn new(
        image: Rgba,
        title: String,
        item: Option<ItemId>,
        at: Option<PxRect>,
        id: u64,
    ) -> Self {
        let area = match at {
            Some(r) => uc::area_of_rect(r),
            None => uc::area_under_cursor(),
        };
        let dims = (image.width, image.height);
        let zoom = initial_zoom(dims, area.work);
        let (w, h) = window_size_px(dims, zoom);
        let rect = match at {
            Some(r) => {
                let full = PxRect::new(area.work.x, area.work.y, area.work.w, area.work.h);
                uc::clamp_into(PxRect::new(r.x, r.y, w, h), full)
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
            drag: None,
            last_drag_end: None,
            hwnd: None,
            texture: None,
            chip: None,
            sent_transparent: false,
            copy_rx: None,
        }
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
            let size = window_size_px(dims, self.zoom);
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
                let r = dragged_rect(d.start, d.press, now, window_size_px(dims, self.zoom));
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
            .with_taskbar(false)
            .with_active(false)
            .with_drag_and_drop(false)
    }

    fn on_created(&mut self, hwnd: Hwnd, _shared: &Shared) {
        self.hwnd = Some(hwnd);
        use stinkshot_platform::window_util as wu;
        wu::set_tool_window(hwnd, true);
        wu::set_topmost(hwnd, true);
        wu::set_rect(hwnd, self.rect);
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
        // Outside a drag, follow where the window really is (Windows may move it on a DPI or
        // display change) but keep the intended 1:1 size: a move onto a monitor with another
        // scale makes Windows (WM_DPICHANGED) rescale the window, so put the size back.
        if self.drag.is_none()
            && let Some(h) = self.hwnd
            && let Some(actual) = stinkshot_platform::window_util::get_rect(h)
        {
            let size = window_size_px(dims, self.zoom);
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
                    let old = (self.rect.w, self.rect.h);
                    let new = window_size_px(dims, z);
                    let cursor = hovering
                        .map(|p| (p.x * ppp, p.y * ppp))
                        .unwrap_or((old.0 as f32 / 2.0, old.1 as f32 / 2.0));
                    let (nx, ny) = zoom_anchor((self.rect.x, self.rect.y), old, new, cursor);
                    self.zoom = z;
                    self.apply_rect(ctx, PxRect::new(nx, ny, new.0, new.1));
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
        let zoom_pct = (self.zoom * 100.0).round() as i32;
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

        uc::root_ui(ctx, Color32::TRANSPARENT, |ui, rect| {
            let radius = CornerRadius::same(8);
            let tint = Color32::from_white_alpha((opacity * 255.0).round() as u8);
            if let Some(id) = tex {
                ui.painter().add(
                    egui::epaint::RectShape::filled(rect, radius, tint)
                        .with_texture(id, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))),
                );
            } else {
                ui.painter()
                    .rect_filled(rect, radius, p.layer.gamma_multiply(opacity));
            }

            // Mini toolbar, top right (shown while hovered, when the pin is big enough).
            let tb_w = TOOL_PAD * 2.0 + TOOL_BTN * 3.0 + TOOL_GAP * 2.0;
            let tb_h = TOOL_PAD * 2.0 + TOOL_BTN;
            let tb = Rect::from_min_size(
                pos2(
                    (rect.right() - 6.0 - tb_w).max(rect.left()),
                    rect.top() + 6.0,
                ),
                vec2(tb_w, tb_h),
            );
            let tb_fits = rect.width() >= tb_w + 12.0 && rect.height() >= tb_h + 12.0;

            let bg = ui.interact(rect, ui.scope_id().with("pin_bg"), Sense::click_and_drag());
            // egui reports the background as hovered under the toolbar buttons too (a click
            // widget over a drag widget hits both), so presses inside the toolbar are kept out
            // of the drag explicitly.
            let press = ui.input(|i| {
                i.pointer
                    .button_pressed(PointerButton::Primary)
                    .then(|| i.pointer.press_origin())
                    .flatten()
            });
            if bg.hovered() && press.is_some_and(|pos| !(tb_fits && tb.contains(pos))) {
                press_bg = true;
            }
            if bg.double_clicked() && item.is_some() && !recent_drag {
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
                    rect,
                    radius,
                    Stroke::new(1.5, accent),
                    StrokeKind::Inside,
                );

                if tb_fits {
                    let bar_fill = Color32::from_rgba_unmultiplied(32, 32, 32, 230);
                    ui.painter()
                        .rect_filled(tb, CornerRadius::same(6), bar_fill);
                    let mut x = tb.left() + TOOL_PAD;
                    for slot in 0..3 {
                        let r = Rect::from_min_size(
                            pos2(x, tb.top() + TOOL_PAD),
                            vec2(TOOL_BTN, TOOL_BTN),
                        );
                        x += TOOL_BTN + TOOL_GAP;
                        let resp =
                            ui.interact(r, ui.scope_id().with(("pin_tool", slot)), Sense::click());
                        if resp.hovered() {
                            ui.painter().rect_filled(
                                r,
                                CornerRadius::same(4),
                                Color32::from_white_alpha(28),
                            );
                        }
                        let icon_col = Color32::from_rgb(0xe6, 0xe6, 0xe6);
                        match slot {
                            0 => {
                                ui.painter().text(
                                    r.center(),
                                    Align2::CENTER_CENTER,
                                    format!("{}%", (opacity * 100.0).round() as i32),
                                    uc::font(11.0),
                                    icon_col,
                                );
                                if resp.on_hover_text(tr!("shell.pin.opacity.tip")).clicked()
                                    && !recent_drag
                                {
                                    cycle = true;
                                }
                            }
                            1 => {
                                theme::widgets::paint_icon(
                                    ui.painter(),
                                    Rect::from_center_size(r.center(), vec2(14.0, 14.0)),
                                    Icon::Copy,
                                    icon_col,
                                );
                                if resp.on_hover_text(tr!("shell.pin.copy.tip")).clicked()
                                    && !recent_drag
                                {
                                    want_copy = true;
                                }
                            }
                            _ => {
                                theme::widgets::paint_icon(
                                    ui.painter(),
                                    Rect::from_center_size(r.center(), vec2(14.0, 14.0)),
                                    Icon::Close,
                                    icon_col,
                                );
                                if resp.on_hover_text(tr!("shell.pin.close.tip")).clicked()
                                    && !recent_drag
                                {
                                    close = true;
                                }
                            }
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
                        rect.left() + 8.0,
                        (rect.bottom() - 8.0 - size.y).max(rect.top()),
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
        let _ = zoom_pct;
        self.update_drag(ctx, press_bg, dims);

        if cycle {
            self.opacity = cycle_opacity(self.opacity);
            self.set_chip(tr!(
                "shell.pin.opacity",
                pct = (self.opacity * 100.0).round() as i32
            ));
        }
        if reset_zoom && (self.zoom - 1.0).abs() > f32::EPSILON {
            let old = (self.rect.w, self.rect.h);
            let new = window_size_px(dims, 1.0);
            let center = (old.0 as f32 / 2.0, old.1 as f32 / 2.0);
            let (nx, ny) = zoom_anchor((self.rect.x, self.rect.y), old, new, center);
            self.zoom = 1.0;
            self.apply_rect(ctx, PxRect::new(nx, ny, new.0, new.1));
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
}
