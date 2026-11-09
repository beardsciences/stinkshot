//! Editor Window: the overlay's annotation editor in a normal top-level window (taskbar,
//! Task View, movable, resizable) instead of over a frozen screen. Forgejo #4, #5, #9.
//!
//! The same `Overlay` state machine runs in Annotate mode with `windowed` set. Its canvas is
//! the window's client area (physical px) and the client rect is its only "monitor", so the
//! bars, chips and popups stay inside the window. Each frame the host re-fits the picture
//! when the client size or DPI changed (`fit`), keeping the crop and the annotations, which
//! live in image px. Zoom and pan are the overlay's own (`view.rs`).
//!
//! Window bookkeeping: the window is created hidden on the primary monitor, then cloaked,
//! shown and moved to the monitor under the cursor (Windows rescales it for that monitor's
//! DPI), centred once its size settles, and uncloaked, so it never shows at the wrong size.
//! Closing with unsaved annotations asks first (`confirm`).

use egui::{RichText, Sense, ViewportCommand, vec2};
use stinkshot_core::PxPoint;
use stinkshot_platform::capture::Frozen;

use super::*;
use crate::state::EditorRequest;
use crate::toast::ui_common as uc;

/// Space around the picture, in points: sides, top (hint bar and size chip above the
/// picture) and the gap under the bars.
const SIDE_PT: f32 = 16.0;
const TOP_PT: f32 = 88.0;
const BAR_GAP_PT: f32 = 8.0;
/// Smallest window (client area, points) and the initial size cap (share of the work area).
const MIN_CLIENT_PT: (f32, f32) = (640.0, 420.0);
const MAX_SHARE: f32 = 0.9;
/// Frames the cloaked window waits for its size to settle before it is shown.
const REVEAL_MIN_FRAMES: u8 = 2;
const REVEAL_MAX_FRAMES: u8 = 20;

/// Window title base (not translated: the shell finds windows by title).
pub const TITLE: &str = "Stinkshot Editor"; // l10n-ok

/// Room the bars need under the picture, in points: one row when the tool bar and action
/// bar fit side by side in the client width, else two.
pub(super) fn bars_height_pt(client_w_pt: f32, tool: Vec2, act: Vec2) -> f32 {
    let side_by_side = tool.x + BAR_GAP_PT + act.x + 2.0 * SIDE_PT <= client_w_pt;
    let rows = if side_by_side {
        tool.y.max(act.y)
    } else {
        tool.y + BAR_GAP_PT + act.y
    };
    rows + 2.0 * BAR_GAP_PT
}

/// Fit an `img` px picture into a `client` px window: (canvas px per image px, top-left of
/// the picture). Never scales up; centred in the space left by the margins.
pub(super) fn fit(
    img: (u32, u32),
    client: (i32, i32),
    ppp: f32,
    bottom_pt: f32,
) -> (f32, (f32, f32)) {
    let (iw, ih) = (img.0.max(1) as f32, img.1.max(1) as f32);
    let side = SIDE_PT * ppp;
    let top = TOP_PT * ppp;
    let bottom = bottom_pt * ppp;
    let aw = (client.0 as f32 - 2.0 * side).max(1.0);
    let ah = (client.1 as f32 - top - bottom).max(1.0);
    let s = (aw / iw).min(ah / ih).clamp(0.01, 1.0);
    let (w, h) = ((iw * s).round(), (ih * s).round());
    let x = side + ((aw - w) / 2.0).max(0.0);
    let y = top + ((ah - h) / 2.0).max(0.0);
    (s, (x.round(), y.round()))
}

/// Initial client size in points for an `img` px picture on a monitor with `scale` and a
/// `work` px work area: the picture at 100 % plus the margins, within the size limits.
pub(super) fn initial_client_pt(img: (u32, u32), scale: f32, work: PxRect, bottom_pt: f32) -> Vec2 {
    let s = scale.max(0.1);
    let want_w = img.0 as f32 / s + 2.0 * SIDE_PT;
    let want_h = img.1 as f32 / s + TOP_PT + bottom_pt;
    let max_w = (work.w as f32 / s * MAX_SHARE).max(MIN_CLIENT_PT.0);
    let max_h = (work.h as f32 / s * MAX_SHARE).max(MIN_CLIENT_PT.1);
    vec2(
        want_w.clamp(MIN_CLIENT_PT.0, max_w).round(),
        want_h.clamp(MIN_CLIENT_PT.1, max_h).round(),
    )
}

/// A title no other open Editor Window has: `base`, then `base (2)`, `base (3)`, ...
pub fn unique_title(base: &str, taken: &[String]) -> String {
    if !taken.iter().any(|t| t == base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base} ({n})"))
        .find(|t| !taken.iter().any(|x| x == t))
        .unwrap_or_else(|| base.to_string())
}

/// Title for a capture: "name - Stinkshot Editor" for a history item, else the plain name.
pub fn base_title(name: Option<&str>) -> String {
    match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => format!("{n} - {TITLE}"),
        None => TITLE.to_string(),
    }
}

/// Whether finishing with `action` keeps the editor open: only an Editor Window saving back
/// to the history item it edits (Ctrl+S / Save); everything else closes it, like the
/// full-screen editor.
pub(super) fn stays_open(
    windowed: bool,
    action: FinishAction,
    editing: Option<stinkshot_history::ItemId>,
) -> bool {
    windowed && editing.is_some() && action == FinishAction::Save
}

/// `rect` (outer, px) centred in `work`, kept inside it.
pub(super) fn centred_in(rect: PxRect, work: PxRect) -> PxRect {
    let w = rect.w.min(work.w);
    let h = rect.h.min(work.h);
    PxRect::new(work.x + (work.w - w) / 2, work.y + (work.h - h) / 2, w, h)
}

pub struct EditorWindow {
    ov: Overlay,
    title: String,
    item: Option<stinkshot_history::ItemId>,
    /// Where the window goes: the monitor under the cursor when it opened.
    target_work: PxRect,
    target_scale: f32,
    /// Primary monitor scale (winit creates the window there first).
    create_scale: f32,
    init_client_pt: Vec2,
    hwnd: Option<Hwnd>,
    /// Cloaked until its size settles: frames presented and the last outer rect seen.
    reveal: Option<(u8, Option<PxRect>)>,
    /// Client size (px) and scale the picture was last fitted for.
    laid: Option<((i32, i32), f32, f32)>,
    /// The Discard Changes prompt is showing.
    confirm: bool,
}

impl EditorWindow {
    pub fn new(req: EditorRequest, title: String, shared: &Shared) -> Self {
        let EditorRequest {
            base,
            doc,
            mode,
            editing,
            source,
            name: _,
            rect,
        } = req;
        let carried = editing.is_none()
            && doc
                .shapes
                .iter()
                .any(|s| !matches!(s.kind, ShapeKind::Cursor { .. }));
        let img = (base.width, base.height);
        let cursor = stinkshot_platform::input::cursor_pos();
        let mons = monitors::list();
        let mon = mons
            .iter()
            .find(|m| m.rect.contains(cursor))
            .or_else(|| mons.iter().find(|m| m.primary))
            .or(mons.first());
        let (target_work, target_scale) = mon.map_or((PxRect::new(0, 0, 1280, 800), 1.0), |m| {
            (m.work, m.scale.max(0.1))
        });
        let create_scale = mons
            .iter()
            .find(|m| m.primary)
            .map_or(1.0, |m| m.scale)
            .max(0.1);
        // No frozen screen: a 1 px stand-in (never drawn in a window).
        let frozen = Frozen {
            bounds: PxRect::new(0, 0, 1, 1),
            image: Rgba::new(1, 1),
            cursor: PxPoint { x: 0, y: 0 },
            cursor_shot: None,
            cursor_drawn: false,
        };
        let req = OverlayRequest {
            mode: OverlayMode::Annotate {
                base,
                doc,
                mode,
                editing,
                source,
            },
            frozen,
            windows: Vec::new(),
            layers: Vec::new(),
            last_region: None,
            notice: None,
            from_tray_click: false,
        };
        let mut ov = Overlay::new(req, shared);
        ov.windowed = true;
        ov.fixed_rect = rect;
        ov.carried_edits = carried;
        ov.bar_origin = None;
        let bars = bars_height_pt(f32::INFINITY, ov.bar_sizes.0, ov.bar_sizes.1);
        let init_client_pt = initial_client_pt(img, target_scale, target_work, bars);
        EditorWindow {
            ov,
            title,
            item: editing,
            target_work,
            target_scale,
            create_scale,
            init_client_pt,
            hwnd: None,
            reveal: None,
            laid: None,
            confirm: false,
        }
    }

    /// The history item this window edits, if any.
    pub fn item(&self) -> Option<stinkshot_history::ItemId> {
        self.item
    }

    /// Re-fit the picture to the client area when its size or DPI changed, and keep the
    /// canvas origin on the client's screen position (pins open where the picture was).
    fn sync_layout(&mut self, ctx: &Context) {
        let ppp = ctx.pixels_per_point();
        let r = ctx.content_rect();
        let client = (
            (r.width() * ppp).round().max(1.0) as i32,
            (r.height() * ppp).round().max(1.0) as i32,
        );
        let origin = ctx.input(|i| i.viewport().inner_rect).map_or((0, 0), |r| {
            (
                (r.min.x * ppp).round() as i32,
                (r.min.y * ppp).round() as i32,
            )
        });
        let ov = &mut self.ov;
        ov.bounds = PxRect::new(origin.0, origin.1, client.0, client.1);
        let bottom = bars_height_pt(client.0 as f32 / ppp, ov.bar_sizes.0, ov.bar_sizes.1);
        if self.laid == Some((client, ppp, bottom)) {
            return;
        }
        self.laid = Some((client, ppp, bottom));
        let Some(orig) = ov.orig.clone() else { return };
        let (scale, at) = fit((orig.width, orig.height), client, ppp, bottom);
        ov.scale = scale;
        ov.img_origin = at;
        ov.monitors = vec![(PxRect::new(0, 0, client.0, client.1), ppp)];
        ov.cursor_local = (client.0 / 2, client.1 / 2);
        ov.sel = Some(ov.sel_from_crop());
        if !ov.view.is_identity() {
            let v = ov.view;
            ov.set_view(v);
        }
    }

    /// Cloaked reveal: centre on the target monitor once Windows has rescaled the window for
    /// its DPI, then show it.
    fn settle(&mut self, ctx: &Context) {
        let (Some(hwnd), Some((frames, last))) = (self.hwnd, self.reveal) else {
            return;
        };
        let now = window_util::get_rect(hwnd);
        let stable = now.is_some() && now == last;
        if (frames >= REVEAL_MIN_FRAMES && stable) || frames >= REVEAL_MAX_FRAMES {
            if let Some(r) = now {
                let want = centred_in(r, self.target_work);
                if want != r {
                    window_util::set_rect(hwnd, want);
                }
            }
            self.reveal = None;
            crate::sysutil::set_cloaked(hwnd, false);
            window_util::force_foreground(hwnd);
        } else {
            self.reveal = Some((frames, now));
            ctx.request_repaint();
        }
    }

    fn discard_prompt(&mut self, ctx: &Context, shared: &mut Shared) {
        let p = uc::pal(shared);
        let accent = Color32::from_rgba_unmultiplied(
            shared.accent[0],
            shared.accent[1],
            shared.accent[2],
            255,
        );
        let modal = egui::Modal::new(egui::Id::unique("editor_window_discard")).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.label(
                RichText::new(tr!("overlay.window.discard_title"))
                    .font(theme::semibold(15.0))
                    .color(p.text),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new(tr!("overlay.window.discard_body"))
                    .size(12.0)
                    .color(p.text_secondary),
            );
            ui.add_space(14.0);
            let (mut keep, mut discard) = (false, false);
            ui.horizontal(|ui| {
                let (r1, _) = ui.allocate_exact_size(vec2(164.0, 34.0), Sense::hover());
                keep = uc::rect_button(
                    ui,
                    "editor_keep",
                    r1,
                    tr!("overlay.window.keep"),
                    uc::BtnKind::Neutral,
                    true,
                    p,
                    accent,
                )
                .clicked();
                let (r2, _) = ui.allocate_exact_size(vec2(164.0, 34.0), Sense::hover());
                discard = uc::rect_button(
                    ui,
                    "editor_discard",
                    r2,
                    tr!("overlay.window.discard"),
                    uc::BtnKind::Danger,
                    true,
                    p,
                    accent,
                )
                .clicked();
            });
            (keep, discard)
        });
        let (keep, discard) = modal.inner;
        if discard {
            self.confirm = false;
            self.ov.cancel(shared);
        } else if keep || modal.should_close() {
            self.confirm = false;
        }
    }
}

impl Surface for EditorWindow {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn viewport_builder(&self, _shared: &Shared) -> egui::ViewportBuilder {
        // Created hidden on the primary monitor (winit's default), sized in points so the
        // DPI rescale on the move to the target monitor lands on the wanted size there.
        let s = self.create_scale;
        let w = self.target_work;
        egui::ViewportBuilder::default()
            .with_title(self.title())
            .with_inner_size(self.init_client_pt)
            .with_min_inner_size(vec2(MIN_CLIENT_PT.0, MIN_CLIENT_PT.1))
            .with_position(Pos2::new(w.x as f32 / s, w.y as f32 / s))
            .with_resizable(true)
            .with_visible(false)
    }

    fn on_created(&mut self, hwnd: Hwnd, shared: &Shared) {
        self.hwnd = Some(hwnd);
        window_util::apply_mica(hwnd, shared.dark);
        crate::sysutil::set_cloaked(hwnd, true);
        crate::sysutil::show_window(hwnd);
        // Onto the target monitor; Windows then rescales it for that monitor's DPI.
        if let Some(r) = window_util::get_rect(hwnd) {
            let k = self.target_scale / self.create_scale;
            let (w, h) = (
                (r.w as f32 * k).round() as i32,
                (r.h as f32 * k).round() as i32,
            );
            let at = centred_in(PxRect::new(0, 0, w, h), self.target_work);
            window_util::set_rect(hwnd, PxRect::new(at.x, at.y, r.w, r.h));
        }
        self.reveal = Some((0, None));
    }

    fn after_paint(&mut self) {
        if let Some((frames, _)) = &mut self.reveal {
            *frames = frames.saturating_add(1);
        }
    }

    fn show(&mut self, ctx: &Context, shared: &mut Shared) -> SurfaceStatus {
        self.settle(ctx);
        self.sync_layout(ctx);

        if !self.ov.closed && ctx.input(|i| i.viewport().close_requested()) {
            if self.ov.finishing.is_some() {
                // Copy / Save / Pin in progress: it closes the window when done.
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            } else if self.ov.has_unsaved_edits() {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                self.confirm = true;
            } else {
                self.ov.cancel(shared);
            }
        }
        let before = shared.outbox.len();
        self.ov.ensure_base_texture(ctx);
        let mut status = if self.confirm && !self.ov.closed {
            // The prompt owns the keyboard (Esc keeps editing): no events reach the editor.
            let keep = ctx.input_mut(|i| std::mem::take(&mut i.events));
            let s = self.ov.frame(ctx, shared);
            ctx.input_mut(|i| i.events = keep);
            s
        } else {
            self.ov.frame(ctx, shared)
        };
        if self.confirm && status == SurfaceStatus::Open {
            self.discard_prompt(ctx, shared);
            if self.ov.closed {
                status = SurfaceStatus::Close;
            }
        }
        // Closing a window is not a cancelled screen capture (the shell would drop an OCR
        // overlay's frozen screen).
        let tail = shared.outbox.split_off(before);
        shared.outbox.extend(
            tail.into_iter()
                .filter(|c| !matches!(c, Command::Cancelled)),
        );
        if self.reveal.is_some() {
            ctx.request_repaint();
        }
        status
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_pictures_stay_at_100_percent_and_centred() {
        let (s, (x, y)) = fit((400, 300), (1600, 1000), 1.0, 70.0);
        assert_eq!(s, 1.0);
        // Centred horizontally; vertically between the top margin and the bars.
        assert_eq!(x, ((1600.0 - 400.0) / 2.0f32).round());
        let ah = 1000.0 - TOP_PT - 70.0;
        assert_eq!(y, (TOP_PT + (ah - 300.0) / 2.0).round());
    }

    #[test]
    fn large_pictures_scale_down_to_fit_inside_the_margins() {
        let (s, (x, y)) = fit((3840, 2160), (1600, 1000), 1.5, 70.0);
        let side = SIDE_PT * 1.5;
        let aw = 1600.0 - 2.0 * side;
        let ah = 1000.0 - TOP_PT * 1.5 - 70.0 * 1.5;
        assert!((s - (aw / 3840.0).min(ah / 2160.0)).abs() < 1e-6);
        assert!(x >= side.floor() && y >= (TOP_PT * 1.5).floor());
        assert!(x + 3840.0 * s <= 1600.0 - side + 1.0);
        assert!(y + 2160.0 * s <= 1000.0 - 70.0 * 1.5 + 1.0);
    }

    #[test]
    fn a_tiny_window_never_panics_or_goes_negative() {
        let (s, (x, y)) = fit((1000, 1000), (10, 10), 2.0, 70.0);
        assert!(s > 0.0 && s <= 1.0);
        assert!(x >= 0.0 && y >= 0.0);
        let (s, _) = fit((0, 0), (800, 600), 1.0, 70.0);
        assert!(s > 0.0);
    }

    #[test]
    fn bars_take_two_rows_when_they_do_not_fit_side_by_side() {
        let tool = vec2(760.0, 46.0);
        let act = vec2(330.0, 46.0);
        let one = bars_height_pt(1400.0, tool, act);
        let two = bars_height_pt(900.0, tool, act);
        assert_eq!(one, 46.0 + 2.0 * BAR_GAP_PT);
        assert_eq!(two, 46.0 * 2.0 + BAR_GAP_PT + 2.0 * BAR_GAP_PT);
    }

    #[test]
    fn initial_size_is_the_picture_at_100_percent_within_limits() {
        let work = PxRect::new(0, 0, 3840, 2100);
        // 1200 x 800 px at 150 %: 800 x 533 pt plus margins.
        let v = initial_client_pt((1200, 800), 1.5, work, 62.0);
        assert_eq!(v.x, (800.0 + 2.0 * SIDE_PT).round());
        assert_eq!(v.y, (800.0f32 / 1.5 + TOP_PT + 62.0).round());
        // Tiny pictures get the minimum, huge ones the work-area cap.
        let small = initial_client_pt((50, 40), 1.0, work, 62.0);
        assert_eq!((small.x, small.y), MIN_CLIENT_PT);
        let big = initial_client_pt((20000, 20000), 1.0, work, 62.0);
        assert_eq!(big.x, (3840.0 * MAX_SHARE).round());
        assert_eq!(big.y, (2100.0 * MAX_SHARE).round());
    }

    #[test]
    fn titles_are_unique_among_open_windows() {
        assert_eq!(base_title(None), TITLE);
        assert_eq!(base_title(Some("  ")), TITLE);
        assert_eq!(
            base_title(Some("Stinkshot_2026-11-09.png")),
            "Stinkshot_2026-11-09.png - Stinkshot Editor"
        );
        let taken = vec![TITLE.to_string(), format!("{TITLE} (2)")];
        assert_eq!(unique_title(TITLE, &taken), format!("{TITLE} (3)"));
        assert_eq!(unique_title("a.png - x", &taken), "a.png - x");
    }

    #[test]
    fn only_save_back_to_the_item_keeps_the_window_open() {
        assert!(stays_open(true, FinishAction::Save, Some(3)));
        assert!(!stays_open(false, FinishAction::Save, Some(3)));
        assert!(!stays_open(true, FinishAction::Save, None));
        for a in [
            FinishAction::SaveCopy,
            FinishAction::SaveAs,
            FinishAction::Copy,
            FinishAction::Pin,
            FinishAction::CopyText,
        ] {
            assert!(!stays_open(true, a, Some(3)), "{a:?}");
        }
    }

    #[test]
    fn centring_keeps_the_window_inside_the_work_area() {
        let work = PxRect::new(1920, 0, 2560, 1400);
        assert_eq!(
            centred_in(PxRect::new(0, 0, 1000, 600), work),
            PxRect::new(1920 + 780, 400, 1000, 600)
        );
        let r = centred_in(PxRect::new(0, 0, 4000, 3000), work);
        assert_eq!(r, work);
    }
}
