//! After-capture toast (docs/design/Toast.dc.html): bottom-right of the work area of the
//! monitor with the cursor, above the taskbar; topmost, no activation, excluded from
//! capture, auto-dismiss after config.general.toast_seconds (paused while hovered); click
//! on the thumbnail = Edit. Action row: Edit, Copy, Pin, Copy Text for captures and Play,
//! Copy for recordings (Copy puts the image, or the recording file, on the clipboard again,
//! e.g. after a failed copy). Show in Folder is an icon button next to Dismiss so the row
//! keeps four comfortable buttons.

#[path = "ui_common.rs"]
pub mod ui_common;

use std::time::{Duration, Instant};

use egui::{Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};
use stinkshot_core::{PxRect, Rgba};
use stinkshot_history::ItemId;
use stinkshot_platform::Hwnd;

use self::ui_common as uc;
use crate::state::{Command, Shared, Surface, SurfaceStatus};
use crate::theme::{self, Icon};

pub enum ToastKind {
    Capture {
        thumb: Rgba,
        title: String,
        body: String,
        item: Option<ItemId>,
        can_edit: bool,
    },
    Recording {
        thumb: Option<Rgba>,
        title: String,
        body: String,
        item: Option<ItemId>,
    },
    Message {
        title: String,
        body: String,
        error: bool,
    },
}

/// Logical layout constants (design: 380 px card, 12 px padding, 96x60 thumbnail).
const CARD_W: f32 = 380.0;
const PAD: f32 = 12.0;
const THUMB_W: f32 = 96.0;
const THUMB_H: f32 = 60.0;
const BTN_H: f32 = 34.0;
const BTN_GAP: f32 = 6.0;
const DISMISS: f32 = 28.0;
/// Tallest toast (capture with actions); used as the stack pitch.
const SLOT_H: f32 = PAD + THUMB_H + 12.0 + BTN_H + PAD;
const SCREEN_MARGIN: f32 = 16.0;
const STACK_GAP: f32 = 10.0;
/// Entrance: the toast rises this far (logical px, = the stack gap so it never overlaps the
/// toast below) into its slot while its content fades in.
const SLIDE_PX: f32 = 10.0;
const SLIDE_SECS: f32 = 0.18;

/// Ease-out cubic progress 0..=1 of the entrance animation after `secs`.
fn slide_progress(secs: f32) -> f32 {
    let t = (secs / SLIDE_SECS).clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Buttons a toast can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastAction {
    Edit,
    Copy,
    Pin,
    CopyText,
    Folder,
    Play,
}

impl ToastAction {
    fn label(self) -> &'static str {
        match self {
            ToastAction::Edit => "Edit",
            ToastAction::Copy => "Copy",
            ToastAction::Pin => "Pin",
            ToastAction::CopyText => "Copy Text",
            ToastAction::Folder => "Folder",
            ToastAction::Play => "Play",
        }
    }
    fn command(self, id: ItemId) -> Command {
        match self {
            ToastAction::Edit => Command::EditItem(id),
            ToastAction::Copy => Command::CopyItemImage(id),
            ToastAction::Pin => Command::PinItem(id),
            ToastAction::CopyText => Command::CopyItemText(id),
            ToastAction::Folder => Command::RevealItem(id),
            ToastAction::Play => Command::OpenItem(id),
        }
    }

    /// Hover text (Title Case).
    fn tooltip(self, recording: bool) -> &'static str {
        match self {
            ToastAction::Edit => "Open in the Editor",
            ToastAction::Copy if recording => "Copy the File to the Clipboard",
            ToastAction::Copy => "Copy the Image to the Clipboard",
            ToastAction::Pin => "Pin to Screen",
            ToastAction::CopyText => "Copy Recognized Text",
            ToastAction::Folder => "Show in Folder",
            ToastAction::Play => "Play",
        }
    }
}

/// Buttons of the action row (design order). Nothing is actionable without a history item.
pub fn actions_for(kind: &ToastKind) -> Vec<ToastAction> {
    match kind {
        ToastKind::Capture {
            item: Some(_),
            can_edit,
            ..
        } => {
            let mut v = Vec::new();
            if *can_edit {
                v.push(ToastAction::Edit);
            }
            v.extend([ToastAction::Copy, ToastAction::Pin, ToastAction::CopyText]);
            v
        }
        ToastKind::Recording { item: Some(_), .. } => vec![ToastAction::Play, ToastAction::Copy],
        _ => Vec::new(),
    }
}

/// Header icon actions next to Dismiss.
pub fn header_actions_for(kind: &ToastKind) -> Vec<ToastAction> {
    match kind {
        ToastKind::Capture { item: Some(_), .. } | ToastKind::Recording { item: Some(_), .. } => {
            vec![ToastAction::Folder]
        }
        _ => Vec::new(),
    }
}

fn kind_item(kind: &ToastKind) -> Option<ItemId> {
    match kind {
        ToastKind::Capture { item, .. } | ToastKind::Recording { item, .. } => *item,
        ToastKind::Message { .. } => None,
    }
}

/// Line height of a 12 px body row.
const BODY_ROW_H: f32 = 15.0;
/// Rough characters per body row at the card width (12 px proportional font).
const BODY_ROW_CHARS: usize = 46;

/// Body rows a message toast reserves: 2 normally, up to 4 for long errors so the reason is
/// not cut off mid-sentence. 4 rows still fit the stack pitch of a capture toast.
pub fn message_rows(body: &str) -> usize {
    let rows: usize = body
        .lines()
        .map(|l| l.chars().count().div_ceil(BODY_ROW_CHARS).max(1))
        .sum();
    rows.clamp(2, 4)
}

/// Logical window size for a toast.
pub fn logical_size(kind: &ToastKind) -> Vec2 {
    let has_actions = !actions_for(kind).is_empty();
    match kind {
        ToastKind::Message { body, .. } => vec2(
            CARD_W,
            PAD + 16.0 + 3.0 + 20.0 + 3.0 + BODY_ROW_H * message_rows(body) as f32 + PAD,
        ),
        _ => {
            let h = if has_actions {
                SLOT_H
            } else {
                PAD + THUMB_H + PAD
            };
            vec2(CARD_W, h)
        }
    }
}

/// Seconds a toast stays up. Errors linger at least 8 s.
pub fn lifetime(kind: &ToastKind, configured_secs: u32) -> Duration {
    let base = configured_secs.max(1);
    let secs = match kind {
        ToastKind::Message { error: true, .. } => base.max(8),
        _ => base,
    };
    Duration::from_secs(secs as u64)
}

/// Tick the dismiss timer: time only counts down while not hovered.
pub fn tick_remaining(remaining: Duration, dt: Duration, hovered: bool) -> Duration {
    if hovered {
        remaining
    } else {
        remaining.saturating_sub(dt)
    }
}

pub struct Toast {
    kind: ToastKind,
    id: u64,
    stack_index: usize,
    area: uc::MonArea,
    rect: PxRect,
    hwnd: Option<Hwnd>,
    remaining: Option<Duration>,
    last_tick: Instant,
    texture: Option<egui::TextureHandle>,
    /// When the window was created; drives the slide-in. `None` once it has settled.
    entering: Option<Instant>,
}

impl Toast {
    /// `id` makes the title unique so several toasts can coexist; `stack_index` 0 = lowest.
    pub fn new(kind: ToastKind, id: u64, stack_index: usize) -> Self {
        let area = uc::area_under_cursor();
        let mut t = Self {
            kind,
            id,
            stack_index,
            area,
            rect: PxRect::new(0, 0, 0, 0),
            hwnd: None,
            remaining: None,
            last_tick: Instant::now(),
            texture: None,
            entering: None,
        };
        t.rect = t.compute_rect();
        t
    }

    fn compute_rect(&self) -> PxRect {
        let s = self.area.scale;
        let size = logical_size(&self.kind);
        uc::toast_rect(
            self.area.work,
            (uc::px(size.x, s), uc::px(size.y, s)),
            uc::px(SLOT_H, s),
            self.stack_index,
            uc::px(SCREEN_MARGIN, s),
            uc::px(STACK_GAP, s),
        )
    }

    /// The toast's rect `t` of the way through its entrance (starts `SLIDE_PX` lower).
    fn entrance_rect(&self, t: f32) -> PxRect {
        let dy = uc::px(SLIDE_PX * (1.0 - t), self.area.scale);
        PxRect::new(self.rect.x, self.rect.y + dy, self.rect.w, self.rect.h)
    }

    fn thumb_texture(&mut self, ctx: &egui::Context) -> Option<egui::TextureId> {
        if self.texture.is_none() {
            let img = match &self.kind {
                ToastKind::Capture { thumb, .. } => Some(thumb),
                ToastKind::Recording { thumb, .. } => thumb.as_ref(),
                ToastKind::Message { .. } => None,
            }?;
            // 2x the displayed size is plenty and keeps the upload small.
            let small = if img.width > 384 || img.height > 240 {
                img.thumbnail(384, 240)
            } else {
                img.clone()
            };
            if small.width == 0 || small.height == 0 {
                return None;
            }
            self.texture = Some(ctx.load_texture(
                format!("toast_thumb_{}", self.id),
                uc::rgba_to_color_image(&small),
                egui::TextureOptions::LINEAR,
            ));
        }
        self.texture.as_ref().map(|t| t.id())
    }

    fn texture_dims(&self) -> (f32, f32) {
        self.texture
            .as_ref()
            .map(|t| (t.size()[0] as f32, t.size()[1] as f32))
            .unwrap_or((1.0, 1.0))
    }
}

impl Surface for Toast {
    fn title(&self) -> String {
        format!("StinkShot.Toast.{}", self.id)
    }

    fn viewport_builder(&self, _shared: &Shared) -> egui::ViewportBuilder {
        let s = self.area.scale;
        let size = logical_size(&self.kind);
        egui::ViewportBuilder::default()
            .with_title(self.title())
            .with_inner_size(size)
            .with_position(pos2(uc::pts(self.rect.x, s), uc::pts(self.rect.y, s)))
            .with_decorations(false)
            .with_resizable(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_active(false)
            .with_drag_and_drop(false)
    }

    fn on_created(&mut self, hwnd: Hwnd, shared: &Shared) {
        self.hwnd = Some(hwnd);
        // Rounded corners via DWM; the client area is opaque flyout color.
        stinkshot_platform::window_util::apply_mica(hwnd, shared.dark);
        uc::apply_hud_style(hwnd, self.entrance_rect(0.0), true, true);
        self.last_tick = Instant::now();
        self.entering = Some(Instant::now());
    }

    fn show(&mut self, ctx: &egui::Context, shared: &mut Shared) -> SurfaceStatus {
        let now = Instant::now();
        let hovered = ctx.input(|i| i.pointer.hover_pos().is_some());
        let total = lifetime(&self.kind, shared.config.general.toast_seconds);
        let remaining = self.remaining.get_or_insert(total);
        *remaining = tick_remaining(*remaining, now.duration_since(self.last_tick), hovered);
        self.last_tick = now;
        let expired = remaining.is_zero();
        ctx.request_repaint_after(Duration::from_millis(100));

        // Slide up into the slot and fade the content in; never longer than ~180 ms.
        let mut fade = 1.0;
        if let Some(t0) = self.entering {
            let t = slide_progress(t0.elapsed().as_secs_f32());
            if let Some(hwnd) = self.hwnd {
                stinkshot_platform::window_util::set_rect(hwnd, self.entrance_rect(t));
            }
            if t >= 1.0 {
                self.entering = None;
            } else {
                ctx.request_repaint();
            }
            fade = 0.35 + 0.65 * t;
        }

        let p = uc::pal(shared);
        let accent = uc::accent(shared);
        let tex = self.thumb_texture(ctx);
        let (tw, th) = self.texture_dims();
        let actions = actions_for(&self.kind);
        let header_actions = header_actions_for(&self.kind);
        let ocr_available = shared.ocr.is_some();
        let item = kind_item(&self.kind);
        let is_recording = matches!(self.kind, ToastKind::Recording { .. });
        let can_edit = matches!(self.kind, ToastKind::Capture { can_edit: true, .. });

        let (title, body, error) = match &self.kind {
            ToastKind::Capture { title, body, .. } | ToastKind::Recording { title, body, .. } => {
                (title.clone(), body.clone(), false)
            }
            ToastKind::Message { title, body, error } => (title.clone(), body.clone(), *error),
        };
        let is_message = matches!(self.kind, ToastKind::Message { .. });

        let mut close = expired;
        let mut command: Option<Command> = None;

        uc::root_ui(ctx, p.flyout, |ui, rect| {
            ui.set_opacity(fade);
            ui.painter().rect_stroke(
                rect,
                CornerRadius::ZERO,
                Stroke::new(1.0, p.stroke),
                StrokeKind::Inside,
            );
            let inner = rect.shrink(PAD);

            // Thumbnail.
            let mut text_x = inner.left();
            if let Some(tex_id) = tex {
                let thumb_rect = Rect::from_min_size(inner.min, vec2(THUMB_W, THUMB_H));
                let (u0, v0, u1, v1) = uc::cover_uv((tw, th), THUMB_W, THUMB_H);
                let painter = ui.painter();
                painter.rect_filled(thumb_rect, CornerRadius::same(6), p.layer);
                painter.add(
                    egui::epaint::RectShape::filled(
                        thumb_rect,
                        CornerRadius::same(6),
                        Color32::WHITE,
                    )
                    .with_texture(tex_id, Rect::from_min_max(pos2(u0, v0), pos2(u1, v1))),
                );
                painter.rect_stroke(
                    thumb_rect,
                    CornerRadius::same(6),
                    Stroke::new(1.0, p.stroke),
                    StrokeKind::Inside,
                );
                let r = ui.interact(thumb_rect, ui.scope_id().with("thumb"), Sense::click());
                if r.clicked()
                    && let Some(id) = item
                {
                    if can_edit {
                        command = Some(Command::EditItem(id));
                    } else if is_recording {
                        command = Some(Command::OpenItem(id));
                    }
                    close = command.is_some() || close;
                }
                if item.is_some() && (can_edit || is_recording) {
                    r.on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text(if can_edit { "Edit" } else { "Play" });
                }
                text_x = thumb_rect.right() + 12.0;
            }

            // Dismiss.
            let dismiss_rect = Rect::from_min_size(
                pos2(inner.right() - DISMISS + 4.0, inner.top() - 4.0),
                vec2(DISMISS, DISMISS),
            );
            let dr = ui.interact(dismiss_rect, ui.scope_id().with("dismiss"), Sense::click());
            if dr.hovered() {
                ui.painter().rect_filled(
                    dismiss_rect,
                    CornerRadius::same(6),
                    theme::subtle_fill(shared.dark, 1),
                );
            }
            theme::widgets::paint_icon(
                ui.painter(),
                Rect::from_center_size(dismiss_rect.center(), vec2(14.0, 14.0)),
                Icon::Close,
                p.text_secondary,
            );
            if dr.on_hover_text("Dismiss").clicked() {
                close = true;
            }

            // Header icon actions (Show in Folder), right to left from Dismiss.
            let mut header_left = dismiss_rect.left();
            for a in header_actions.iter().rev() {
                let r = Rect::from_min_size(
                    pos2(header_left - DISMISS - 2.0, dismiss_rect.top()),
                    vec2(DISMISS, DISMISS),
                );
                header_left = r.left();
                let resp = ui.interact(r, ui.scope_id().with(a.label()), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(
                        r,
                        CornerRadius::same(6),
                        theme::subtle_fill(
                            shared.dark,
                            if resp.is_pointer_button_down_on() {
                                2
                            } else {
                                1
                            },
                        ),
                    );
                }
                theme::widgets::paint_icon(
                    ui.painter(),
                    Rect::from_center_size(r.center(), vec2(15.0, 15.0)),
                    Icon::Folder,
                    p.text_secondary,
                );
                let resp = resp
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text(a.tooltip(is_recording));
                if resp.clicked()
                    && let Some(id) = item
                {
                    command = Some(a.command(id));
                    close = true;
                }
            }

            // Text column.
            let text_w = (header_left - 4.0 - text_x).max(40.0);
            let top = inner.top();
            let painter = ui.painter();
            let sq = Rect::from_min_size(pos2(text_x, top + 1.0), vec2(14.0, 14.0));
            let (sq_col, sq_r) = if error {
                (p.danger, CornerRadius::same(7))
            } else {
                (accent, CornerRadius::same(4))
            };
            painter.rect_filled(sq, sq_r, sq_col);
            painter.text(
                pos2(text_x + 20.0, top + 8.0),
                Align2::LEFT_CENTER,
                "StinkShot",
                uc::font(12.0),
                p.text_secondary,
            );
            let title_g = painter.layout(title, theme::semibold(14.0), p.text, text_w);
            // Single-line title with ellipsis.
            let title_g = if title_g.rows.len() > 1 {
                let mut job = egui::text::LayoutJob::single_section(
                    title_g.text().to_string(),
                    egui::TextFormat::simple(theme::semibold(14.0), p.text),
                );
                job.wrap = egui::text::TextWrapping {
                    max_width: text_w,
                    max_rows: 1,
                    break_anywhere: true,
                    overflow_character: Some('\u{2026}'),
                };
                painter.layout_job(job)
            } else {
                title_g
            };
            painter.galley(pos2(text_x, top + 20.0), title_g, p.text);
            let body_rows = if is_message { message_rows(&body) } else { 1 };
            let mut job = egui::text::LayoutJob::single_section(
                body,
                egui::TextFormat::simple(uc::font(12.0), p.text_tertiary),
            );
            job.wrap = egui::text::TextWrapping {
                max_width: text_w,
                max_rows: body_rows,
                break_anywhere: false,
                overflow_character: Some('\u{2026}'),
            };
            let body_g = painter.layout_job(job);
            painter.galley(pos2(text_x, top + 20.0 + 19.0), body_g, p.text_tertiary);

            // Action row.
            if !actions.is_empty() {
                let row_top = inner.top() + THUMB_H + 12.0;
                let n = actions.len() as f32;
                let w = (inner.width() - BTN_GAP * (n - 1.0)) / n;
                for (i, a) in actions.iter().enumerate() {
                    let x = inner.left() + i as f32 * (w + BTN_GAP);
                    let r = Rect::from_min_size(pos2(x, row_top), vec2(w, BTN_H));
                    let enabled = match a {
                        ToastAction::CopyText => ocr_available,
                        _ => true,
                    };
                    let resp = uc::rect_button(
                        ui,
                        a.label(),
                        r,
                        a.label(),
                        uc::BtnKind::Neutral,
                        enabled,
                        p,
                        accent,
                    );
                    let resp = if !enabled {
                        resp.on_disabled_hover_text(
                            "Copy Text Needs an OCR Language in Windows Settings",
                        )
                    } else {
                        resp.on_hover_text(a.tooltip(is_recording))
                    };
                    if resp.clicked()
                        && let Some(id) = item
                    {
                        command = Some(a.command(id));
                        close = true;
                    }
                }
            }
        });

        if let Some(c) = command {
            shared.outbox.push(c);
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

    #[test]
    fn long_messages_get_more_rows_but_fit_the_slot() {
        assert_eq!(message_rows("short"), 2);
        let long = "x".repeat(150);
        assert_eq!(message_rows(&long), 4);
        assert_eq!(message_rows(&"y".repeat(100)), 3);
        let k = ToastKind::Message {
            title: "t".into(),
            body: long,
            error: true,
        };
        assert!(logical_size(&k).y <= SLOT_H);
    }
    use super::*;

    fn cap(item: Option<ItemId>, can_edit: bool) -> ToastKind {
        ToastKind::Capture {
            thumb: Rgba::new(1, 1),
            title: "t".into(),
            body: "b".into(),
            item,
            can_edit,
        }
    }

    #[test]
    fn actions_follow_design_order() {
        assert_eq!(
            actions_for(&cap(Some(1), true)),
            vec![
                ToastAction::Edit,
                ToastAction::Copy,
                ToastAction::Pin,
                ToastAction::CopyText,
            ]
        );
        assert_eq!(
            actions_for(&cap(Some(1), false)),
            vec![ToastAction::Copy, ToastAction::Pin, ToastAction::CopyText]
        );
        assert_eq!(
            header_actions_for(&cap(Some(1), true)),
            vec![ToastAction::Folder]
        );
        assert!(header_actions_for(&cap(None, true)).is_empty());
        assert!(actions_for(&cap(None, true)).is_empty());
        let rec = ToastKind::Recording {
            thumb: None,
            title: String::new(),
            body: String::new(),
            item: Some(3),
        };
        assert_eq!(
            actions_for(&rec),
            vec![ToastAction::Play, ToastAction::Copy]
        );
        assert_eq!(header_actions_for(&rec), vec![ToastAction::Folder]);
        let msg = ToastKind::Message {
            title: String::new(),
            body: String::new(),
            error: false,
        };
        assert!(actions_for(&msg).is_empty());
    }

    #[test]
    fn actions_push_expected_commands() {
        assert!(matches!(ToastAction::Edit.command(7), Command::EditItem(7)));
        assert!(matches!(
            ToastAction::Copy.command(7),
            Command::CopyItemImage(7)
        ));
        assert!(matches!(ToastAction::Pin.command(7), Command::PinItem(7)));
        assert!(matches!(
            ToastAction::CopyText.command(7),
            Command::CopyItemText(7)
        ));
        assert!(matches!(
            ToastAction::Folder.command(7),
            Command::RevealItem(7)
        ));
        assert!(matches!(ToastAction::Play.command(7), Command::OpenItem(7)));
    }

    #[test]
    fn action_row_never_gets_cramped() {
        // Four buttons at most, each wide enough for "Copy Text" at 12 px (~60 pt + padding).
        let most = actions_for(&cap(Some(1), true)).len() as f32;
        assert!(most <= 4.0);
        let w = (CARD_W - 2.0 * PAD - BTN_GAP * (most - 1.0)) / most;
        assert!(w >= 80.0, "{w}");
    }

    #[test]
    fn timer_pauses_while_hovered() {
        let r = Duration::from_secs(6);
        assert_eq!(tick_remaining(r, Duration::from_secs(1), true), r);
        assert_eq!(
            tick_remaining(r, Duration::from_secs(1), false),
            Duration::from_secs(5)
        );
        assert_eq!(
            tick_remaining(Duration::from_millis(100), Duration::from_secs(1), false),
            Duration::ZERO
        );
    }

    #[test]
    fn errors_stay_longer() {
        let err = ToastKind::Message {
            title: String::new(),
            body: String::new(),
            error: true,
        };
        let ok = ToastKind::Message {
            title: String::new(),
            body: String::new(),
            error: false,
        };
        assert_eq!(lifetime(&err, 3), Duration::from_secs(8));
        assert_eq!(lifetime(&ok, 3), Duration::from_secs(3));
        assert_eq!(lifetime(&ok, 0), Duration::from_secs(1));
    }

    #[test]
    fn entrance_eases_out_and_settles() {
        assert_eq!(slide_progress(0.0), 0.0);
        assert_eq!(slide_progress(SLIDE_SECS), 1.0);
        assert_eq!(slide_progress(10.0), 1.0);
        // Ease-out: more than half way at the half-time mark.
        assert!(slide_progress(SLIDE_SECS / 2.0) > 0.8);
    }

    #[test]
    fn sizes_are_stack_compatible() {
        assert!(logical_size(&cap(Some(1), true)).y <= SLOT_H + 0.01);
        let msg = ToastKind::Message {
            title: String::new(),
            body: String::new(),
            error: false,
        };
        assert!(logical_size(&msg).y <= SLOT_H);
    }
}
