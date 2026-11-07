//! Recording and scrolling HUDs (docs/design/Recording.dc.html, Pin.dc.html):
//! - `Countdown`: big 3-2-1 circle centered on the region; the shell polls `finished()`;
//!   Esc cancels (pushes `Command::RecordCancel`).
//! - `RecordFrame`: dashed red border around the region, click-through, excluded from capture.
//! - `RecordHud`: bar under (or above, if no room) the region: red dot, elapsed, pause/resume,
//!   mic, system audio, webcam, clicks/keys toggles, Stop. Excluded from capture. Toggle
//!   changes are read by the shell each frame via `take_toggle_changes`.
//! - `TrimWindow`: filmstrip (record::thumbnails), draggable in/out handles, preview frame,
//!   Discard / Save GIF / Save MP4 -> `Command::TrimDone`. Discard pushes
//!   `Command::RecordCancel` (documented semantics: the shell deletes the untrimmed source
//!   recording and closes the window), after an in-window confirmation.
//! - `ScrollHud`: "Stitching frame N", W x H, progress, Cancel / Stop here.
//!
//! Countdown, RecordFrame and ScrollHud-free areas rely on transparent windows: the shell's
//! `App::clear_color` must be fully transparent for these viewports.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

use egui::{
    Align, Align2, Color32, CornerRadius, Key, Layout, Rect, Sense, Shape, Stroke, StrokeKind, Ui,
    UiBuilder, pos2, vec2,
};
use stinkshot_core::PxRect;
use stinkshot_platform::Hwnd;
use stinkshot_record::{MediaInfo, Status};

use crate::state::{Command, Shared, Surface, SurfaceStatus};
use crate::theme::{self, Icon};
use crate::toast::ui_common as uc;

fn place<R>(ui: &mut Ui, rect: Rect, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::left_to_right(Align::Center)),
        add,
    )
    .inner
}

// ---------------------------------------------------------------------------------------
// Pure logic
// ---------------------------------------------------------------------------------------

/// The digit to show: seconds left, rounded up (3 for the first second of a 3 s countdown).
pub fn countdown_number(total_secs: u32, elapsed: Duration) -> u32 {
    let left = Duration::from_secs(total_secs as u64).saturating_sub(elapsed);
    let n = left.as_secs() + u64::from(left.subsec_nanos() > 0);
    n as u32
}

/// 0..1 progress through the current countdown second (for the pulse ring).
pub fn countdown_phase(elapsed: Duration) -> f32 {
    elapsed.subsec_nanos() as f32 / 1_000_000_000.0
}

/// Scroll HUD progress bar: stitched height against the configured maximum.
pub fn scroll_progress(height: u32, max_height: u32) -> f32 {
    if max_height == 0 {
        0.0
    } else {
        (height as f32 / max_height as f32).clamp(0.0, 1.0)
    }
}

/// Number of filmstrip thumbnails that fill a strip without distortion.
pub fn filmstrip_count(strip_w: f32, strip_h: f32, aspect: f32) -> usize {
    let cell_w = (strip_h * aspect.max(0.2)).max(8.0);
    ((strip_w / cell_w).ceil() as usize).clamp(3, 24)
}

pub fn x_to_secs(x: f32, left: f32, width: f32, dur: f64) -> f64 {
    if width <= 0.0 {
        0.0
    } else {
        (((x - left) / width).clamp(0.0, 1.0) as f64) * dur
    }
}

pub fn secs_to_x(t: f64, left: f32, width: f32, dur: f64) -> f32 {
    if dur <= 0.0 {
        left
    } else {
        left + ((t / dur).clamp(0.0, 1.0) as f32) * width
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    In,
    Out,
    Playhead,
}

/// Smallest kept span.
pub fn min_keep(dur: f64) -> f64 {
    (dur / 2.0).min(0.2)
}

/// Move a handle to `t` seconds, keeping `0 <= start <= end <= dur` and the minimum span.
/// Returns the new (start, end).
pub fn drag_handle(handle: Handle, t: f64, start: f64, end: f64, dur: f64) -> (f64, f64) {
    let gap = min_keep(dur);
    match handle {
        Handle::In => ((t.clamp(0.0, (end - gap).max(0.0))), end),
        Handle::Out => (start, t.clamp((start + gap).min(dur), dur)),
        Handle::Playhead => (start, end),
    }
}

/// Time shown in the preview frame for the current interaction.
pub fn preview_time(active: Handle, start: f64, end: f64, playhead: f64, dur: f64) -> f64 {
    match active {
        Handle::In => start,
        // The last decodable frame is slightly before the end timestamp.
        Handle::Out => (end - 0.05).clamp(0.0, dur),
        Handle::Playhead => playhead.clamp(start, end),
    }
}

fn secs(d: f64) -> Duration {
    Duration::from_secs_f64(d.max(0.0))
}

// ---------------------------------------------------------------------------------------
// Countdown
// ---------------------------------------------------------------------------------------

const COUNTDOWN_WIN: f32 = 220.0;
const COUNTDOWN_R: f32 = 80.0;

pub struct Countdown {
    seconds: u32,
    start: Instant,
    rect: PxRect,
    scale: f32,
    esc_was_down: bool,
}

impl Countdown {
    pub fn new(seconds: u32, region: PxRect) -> Self {
        let area = uc::area_of_rect(region);
        let side = uc::px(COUNTDOWN_WIN, area.scale);
        let cx = region.x + region.w / 2;
        let cy = region.y + region.h / 2;
        let rect = uc::clamp_into(
            PxRect::new(cx - side / 2, cy - side / 2, side, side),
            area.work,
        );
        Self {
            seconds,
            start: Instant::now(),
            rect,
            scale: area.scale,
            // The Esc that confirmed the region may still be held: only a fresh press counts.
            esc_was_down: uc::async_key_down(uc::VK_ESCAPE),
        }
    }

    pub fn finished(&self) -> bool {
        self.start.elapsed() >= Duration::from_secs(self.seconds as u64)
    }
}

impl Surface for Countdown {
    fn title(&self) -> String {
        "StinkShot.Countdown".into()
    }

    fn viewport_builder(&self, _shared: &Shared) -> egui::ViewportBuilder {
        egui::ViewportBuilder::default()
            .with_title(self.title())
            .with_inner_size(vec2(
                uc::pts(self.rect.w, self.scale),
                uc::pts(self.rect.h, self.scale),
            ))
            .with_position(pos2(
                uc::pts(self.rect.x, self.scale),
                uc::pts(self.rect.y, self.scale),
            ))
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_active(false)
            .with_mouse_passthrough(true)
            .with_drag_and_drop(false)
    }

    fn on_created(&mut self, hwnd: Hwnd, _shared: &Shared) {
        uc::apply_hud_style(hwnd, self.rect, true, true);
    }

    fn show(&mut self, ctx: &egui::Context, shared: &mut Shared) -> SurfaceStatus {
        // Esc works without focus: the window never takes it from the app being recorded.
        let esc_down = uc::async_key_down(uc::VK_ESCAPE);
        let esc = (esc_down && !self.esc_was_down) || ctx.input(|i| i.key_pressed(Key::Escape));
        self.esc_was_down = esc_down;
        if esc {
            shared.outbox.push(Command::RecordCancel);
            return SurfaceStatus::Close;
        }
        if self.finished() {
            return SurfaceStatus::Close;
        }
        ctx.request_repaint();

        let elapsed = self.start.elapsed();
        let n = countdown_number(self.seconds, elapsed);
        let phase = countdown_phase(elapsed);
        let p = uc::pal(shared);
        let accent = uc::accent(shared);
        uc::root_ui(ctx, Color32::TRANSPARENT, |ui, rect| {
            let center = pos2(rect.center().x, rect.top() + 100.0);
            let painter = ui.painter();
            // Pulse ring expanding each second.
            let t = phase;
            painter.circle_stroke(
                center,
                COUNTDOWN_R + 4.0 + 18.0 * t,
                Stroke::new(2.0, uc::with_alpha(accent, ((1.0 - t) * 110.0) as u8)),
            );
            painter.circle_filled(
                center,
                COUNTDOWN_R,
                Color32::from_rgba_unmultiplied(20, 20, 20, 191),
            );
            painter.circle_stroke(center, COUNTDOWN_R - 1.5, Stroke::new(3.0, accent));
            painter.text(
                center,
                Align2::CENTER_CENTER,
                n.to_string(),
                uc::font(84.0),
                Color32::WHITE,
            );
            painter.text(
                pos2(rect.center().x, rect.bottom() - 14.0),
                Align2::CENTER_CENTER,
                "Esc to cancel",
                uc::font(12.0),
                p.text_secondary,
            );
        });
        SurfaceStatus::Open
    }
}

// ---------------------------------------------------------------------------------------
// RecordFrame
// ---------------------------------------------------------------------------------------

const FRAME_MARGIN: f32 = 4.0;

pub struct RecordFrame {
    scale: f32,
    rect: PxRect,
}

impl RecordFrame {
    pub fn new(region: PxRect) -> Self {
        let scale = uc::area_of_rect(region).scale;
        let m = uc::px(FRAME_MARGIN, scale);
        let rect = PxRect::new(
            region.x - m,
            region.y - m,
            region.w + 2 * m,
            region.h + 2 * m,
        );
        Self { scale, rect }
    }
}

impl Surface for RecordFrame {
    fn title(&self) -> String {
        "StinkShot.RecordFrame".into()
    }

    fn viewport_builder(&self, _shared: &Shared) -> egui::ViewportBuilder {
        egui::ViewportBuilder::default()
            .with_title(self.title())
            .with_inner_size(vec2(
                uc::pts(self.rect.w, self.scale),
                uc::pts(self.rect.h, self.scale),
            ))
            .with_position(pos2(
                uc::pts(self.rect.x, self.scale),
                uc::pts(self.rect.y, self.scale),
            ))
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_active(false)
            .with_mouse_passthrough(true)
            .with_drag_and_drop(false)
    }

    fn on_created(&mut self, hwnd: Hwnd, _shared: &Shared) {
        uc::apply_hud_style(hwnd, self.rect, true, true);
    }

    fn show(&mut self, ctx: &egui::Context, shared: &mut Shared) -> SurfaceStatus {
        let p = uc::pal(shared);
        uc::root_ui(ctx, Color32::TRANSPARENT, |ui, rect| {
            // The window is the region plus a small margin, so the dashed line hugs the
            // outside of the captured pixels.
            let r = rect.shrink(FRAME_MARGIN / 2.0);
            let pts = [
                r.left_top(),
                r.right_top(),
                r.right_bottom(),
                r.left_bottom(),
                r.left_top(),
            ];
            let shapes = Shape::dashed_line(&pts, Stroke::new(2.0, p.danger), 8.0, 6.0);
            ui.painter().extend(shapes);
        });
        SurfaceStatus::Open
    }
}

// ---------------------------------------------------------------------------------------
// RecordHud
// ---------------------------------------------------------------------------------------

/// Live toggles the HUD exposes; the shell applies changes to the Recorder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HudToggles {
    pub mic: bool,
    pub system: bool,
    pub webcam: bool,
    pub overlays: bool,
}

const HUD_W: f32 = 352.0;
const HUD_H: f32 = 72.0;
const HUD_PAD: f32 = 5.0;
const HUD_BTN: f32 = 34.0;
const HUD_GAP: f32 = 4.0;

pub struct RecordHud {
    summary: String,
    toggles: HudToggles,
    changed: bool,
    status: Option<Status>,
    scale: f32,
    rect: PxRect,
}

impl RecordHud {
    /// `summary` e.g. "MP4 · NVENC H.264 · 60 fps · mic + system".
    pub fn new(region: PxRect, summary: String, toggles: HudToggles) -> Self {
        let area = uc::area_of_rect(region);
        let size = (uc::px(HUD_W, area.scale), uc::px(HUD_H, area.scale));
        let rect = uc::hud_rect(region, size, area.work, uc::px(10.0, area.scale));
        Self {
            summary,
            toggles,
            changed: false,
            status: None,
            scale: area.scale,
            rect,
        }
    }

    pub fn update_status(&mut self, status: Status) {
        self.status = Some(status);
    }

    /// Some(new) if the user changed a toggle since the last call.
    pub fn take_toggle_changes(&mut self) -> Option<HudToggles> {
        if std::mem::take(&mut self.changed) {
            Some(self.toggles)
        } else {
            None
        }
    }
}

impl Surface for RecordHud {
    fn title(&self) -> String {
        "StinkShot.RecordHud".into()
    }

    fn viewport_builder(&self, _shared: &Shared) -> egui::ViewportBuilder {
        egui::ViewportBuilder::default()
            .with_title(self.title())
            .with_inner_size(vec2(HUD_W, HUD_H))
            .with_position(pos2(
                uc::pts(self.rect.x, self.scale),
                uc::pts(self.rect.y, self.scale),
            ))
            .with_decorations(false)
            .with_resizable(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_active(false)
            .with_drag_and_drop(false)
    }

    fn on_created(&mut self, hwnd: Hwnd, shared: &Shared) {
        stinkshot_platform::window_util::apply_mica(hwnd, shared.dark);
        uc::apply_hud_style(hwnd, self.rect, true, true);
    }

    fn show(&mut self, ctx: &egui::Context, shared: &mut Shared) -> SurfaceStatus {
        let p = uc::pal(shared);
        let accent = uc::accent(shared);
        let paused = self.status.as_ref().is_some_and(|s| s.paused);
        let elapsed = self.status.as_ref().map(|s| s.elapsed).unwrap_or_default();
        let error = self.status.as_ref().and_then(|s| s.error.clone());
        let dropped = self.status.as_ref().map(|s| s.dropped).unwrap_or(0);
        let time = ctx.input(|i| i.time);
        if !paused {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let mut toggles = self.toggles;
        let mut outbox: Vec<Command> = Vec::new();
        let summary = self.summary.clone();

        uc::root_ui(ctx, p.flyout, |ui, rect| {
            ui.painter().rect_stroke(
                rect,
                CornerRadius::same(10),
                Stroke::new(1.0, p.stroke),
                StrokeKind::Inside,
            );
            // Drag anywhere on the bar background to move it out of the way.
            let bg = ui.interact(rect, ui.scope_id().with("hud_bg"), Sense::click_and_drag());
            if bg.drag_started_by(egui::PointerButton::Primary) {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }

            let row = Rect::from_min_size(
                rect.min + vec2(HUD_PAD, HUD_PAD),
                vec2(rect.width() - 2.0 * HUD_PAD, HUD_BTN),
            );
            let mid_y = row.center().y;
            let mut x = row.left();

            // Record dot + elapsed.
            let blink = if paused {
                0.35
            } else {
                0.55 + 0.45 * ((time * 3.0).sin() as f32 * 0.5 + 0.5)
            };
            ui.painter().circle_filled(
                pos2(x + 11.0, mid_y),
                5.0,
                if paused {
                    p.text_tertiary
                } else {
                    uc::with_alpha(p.danger, (blink * 255.0) as u8)
                },
            );
            x += 22.0;
            let galley = ui.painter().layout_no_wrap(
                uc::fmt_clock(elapsed),
                uc::font(14.0),
                if paused { p.text_secondary } else { p.text },
            );
            let tw = galley.size().x.max(40.0);
            ui.painter()
                .galley(pos2(x, mid_y - galley.size().y / 2.0), galley, p.text);
            x += tw + 10.0;

            let btn = |x: f32| Rect::from_min_size(pos2(x, row.top()), vec2(HUD_BTN, HUD_BTN));

            // Pause / resume.
            let r = btn(x);
            let (icon, tip) = if paused {
                (Icon::Play, "Resume")
            } else {
                (Icon::Pause, "Pause")
            };
            if place(ui, r, |ui| {
                theme::widgets::icon_button(ui, icon, tip, false)
            })
            .clicked()
            {
                outbox.push(if paused {
                    Command::RecordResume
                } else {
                    Command::RecordPause
                });
            }
            x += HUD_BTN + HUD_GAP;

            // Toggles.
            let items: [(Icon, &str, &mut bool); 4] = [
                (Icon::Mic, "Microphone", &mut toggles.mic),
                (Icon::Speaker, "System audio", &mut toggles.system),
                (Icon::Webcam, "Webcam", &mut toggles.webcam),
                (Icon::Clicks, "Show clicks and keys", &mut toggles.overlays),
            ];
            for (icon, name, on) in items {
                let r = btn(x);
                let tip = format!("{name} {}", if *on { "on" } else { "off" });
                let resp = place(ui, r, |ui| theme::widgets::icon_button(ui, icon, &tip, *on));
                if !*on {
                    let c = r.center();
                    ui.painter().line_segment(
                        [c + vec2(-7.0, 7.0), c + vec2(7.0, -7.0)],
                        Stroke::new(1.8, p.text_tertiary),
                    );
                }
                if resp.clicked() {
                    *on = !*on;
                }
                x += HUD_BTN + HUD_GAP;
            }

            // Stop.
            let stop =
                Rect::from_min_max(pos2(x + 2.0, row.top()), pos2(row.right(), row.bottom()));
            if uc::rect_button(
                ui,
                "stop",
                stop,
                "Stop",
                uc::BtnKind::Danger,
                true,
                p,
                accent,
            )
            .clicked()
            {
                outbox.push(Command::RecordStop);
            }

            // Summary / error line.
            let line_y = row.bottom() + 8.0;
            let (text, col) = match &error {
                Some(e) => (format!("Recording error: {e}"), p.danger),
                None if dropped > 0 => (
                    format!("{summary} \u{b7} {dropped} dropped"),
                    p.text_secondary,
                ),
                None => (summary, p.text_secondary),
            };
            let mut job = egui::text::LayoutJob::single_section(
                text,
                egui::TextFormat::simple(uc::font(12.0), col),
            );
            job.wrap = egui::text::TextWrapping {
                max_width: row.width() - 6.0,
                max_rows: 1,
                break_anywhere: true,
                overflow_character: Some('\u{2026}'),
            };
            let g = ui.painter().layout_job(job);
            ui.painter().galley(pos2(row.left() + 6.0, line_y), g, col);
        });

        if toggles != self.toggles {
            self.toggles = toggles;
            self.changed = true;
        }
        shared.outbox.extend(outbox);
        SurfaceStatus::Open
    }
}

// ---------------------------------------------------------------------------------------
// TrimWindow
// ---------------------------------------------------------------------------------------

const TRIM_W: f32 = 560.0;
const TRIM_H: f32 = 580.0;
const TRIM_MARGIN: f32 = 20.0;
const STRIP_H: f32 = 56.0;
const HANDLE_W: f32 = 8.0;

pub struct TrimWindow {
    source: PathBuf,
    info: MediaInfo,
    prefer_gif: bool,
    dur: f64,
    start: f64,
    end: f64,
    playhead: f64,
    active: Handle,
    dragging: Option<Handle>,
    thumb_rx: Option<Receiver<Vec<stinkshot_core::Rgba>>>,
    thumbs: Vec<egui::TextureHandle>,
    preview_tx: Option<Sender<Duration>>,
    preview_rx: Option<Receiver<stinkshot_core::Rgba>>,
    preview: Option<egui::TextureHandle>,
    preview_dims: (f32, f32),
    wanted: Duration,
    requested: Option<Duration>,
    throttle: uc::Throttle,
    workers_started: bool,
    progress: Option<f32>,
    submitted: bool,
    confirm_discard: bool,
    rect: PxRect,
    scale: f32,
}

impl TrimWindow {
    /// `prefer_gif`: recording was started as GIF, so "Save GIF" is the primary button.
    pub fn new(source: PathBuf, info: MediaInfo, prefer_gif: bool) -> Self {
        let area = uc::area_under_cursor();
        let dur = info.duration.as_secs_f64().max(0.1);
        let size = (uc::px(TRIM_W, area.scale), uc::px(TRIM_H, area.scale));
        let rect = uc::clamp_into(uc::centered_in(area.work, size.0, size.1), area.work);
        Self {
            source,
            info,
            prefer_gif,
            dur,
            start: 0.0,
            end: dur,
            playhead: 0.0,
            active: Handle::Playhead,
            dragging: None,
            thumb_rx: None,
            thumbs: Vec::new(),
            preview_tx: None,
            preview_rx: None,
            preview: None,
            preview_dims: (16.0, 9.0),
            wanted: Duration::ZERO,
            requested: None,
            throttle: uc::Throttle::new(Duration::from_millis(66)),
            workers_started: false,
            progress: None,
            submitted: false,
            confirm_discard: false,
            rect,
            scale: area.scale,
        }
    }

    /// Shell reports export progress (0..1) while a TrimDone job runs; None = idle.
    pub fn set_progress(&mut self, p: Option<f32>) {
        if self.progress.is_some() && p.is_none() {
            // Job finished (or failed): allow another save if the shell keeps the window.
            self.submitted = false;
        }
        self.progress = p.map(|v| v.clamp(0.0, 1.0));
    }

    fn start_workers(&mut self, ctx: &egui::Context) {
        self.workers_started = true;
        let aspect = if self.info.height > 0 {
            self.info.width as f32 / self.info.height as f32
        } else {
            16.0 / 9.0
        };
        self.preview_dims = (
            self.info.width.max(1) as f32,
            self.info.height.max(1) as f32,
        );
        let count = filmstrip_count(TRIM_W - 2.0 * TRIM_MARGIN, STRIP_H, aspect);

        let (ttx, trx) = channel();
        let src = self.source.clone();
        let c = ctx.clone();
        if uc::spawn_worker("trim-filmstrip", move || {
            match stinkshot_record::thumbnails(&src, count, (STRIP_H * 2.0) as u32) {
                Ok(v) => {
                    let _ = ttx.send(v);
                }
                Err(e) => log::warn!("filmstrip failed: {e:#}"),
            }
            c.request_repaint();
        }) {
            self.thumb_rx = Some(trx);
        }

        let (ptx, prx_in) = channel::<Duration>();
        let (ftx, frx) = channel();
        let src = self.source.clone();
        let c = ctx.clone();
        if uc::spawn_worker("trim-preview", move || {
            // Latest request wins: drain whatever queued up while the last decode ran.
            while let Ok(mut at) = prx_in.recv() {
                while let Ok(newer) = prx_in.try_recv() {
                    at = newer;
                }
                match stinkshot_record::frame_at(&src, at, 540) {
                    Ok(img) => {
                        if ftx.send(img).is_err() {
                            break;
                        }
                        c.request_repaint();
                    }
                    Err(e) => log::debug!("preview frame at {at:?} failed: {e:#}"),
                }
            }
        }) {
            self.preview_tx = Some(ptx);
            self.preview_rx = Some(frx);
            self.wanted = Duration::ZERO;
            self.requested = None;
        }
    }

    fn pump_workers(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.thumb_rx {
            match rx.try_recv() {
                Ok(v) => {
                    self.thumbs = v
                        .iter()
                        .enumerate()
                        .filter(|(_, img)| img.width > 0 && img.height > 0)
                        .map(|(i, img)| {
                            ctx.load_texture(
                                format!("trim_thumb_{i}"),
                                uc::rgba_to_color_image(img),
                                egui::TextureOptions::LINEAR,
                            )
                        })
                        .collect();
                    self.thumb_rx = None;
                }
                Err(TryRecvError::Disconnected) => self.thumb_rx = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(rx) = &self.preview_rx {
            // Keep only the newest decoded frame.
            let mut latest = None;
            while let Ok(img) = rx.try_recv() {
                latest = Some(img);
            }
            if let Some(img) = latest
                && img.width > 0
                && img.height > 0
            {
                self.preview_dims = (img.width as f32, img.height as f32);
                let ci = uc::rgba_to_color_image(&img);
                match &mut self.preview {
                    Some(t) => t.set(ci, egui::TextureOptions::LINEAR),
                    None => {
                        self.preview =
                            Some(ctx.load_texture("trim_preview", ci, egui::TextureOptions::LINEAR))
                    }
                }
            }
        }
        // Throttled, latest-wins preview requests (trailing edge included).
        if let Some(tx) = &self.preview_tx
            && self.requested != Some(self.wanted)
        {
            if self.throttle.ready(Instant::now()) {
                if tx.send(self.wanted).is_ok() {
                    self.requested = Some(self.wanted);
                }
            } else {
                ctx.request_repaint_after(Duration::from_millis(70));
            }
        }
    }

    fn save(&mut self, shared: &mut Shared, gif: bool) {
        shared.outbox.push(Command::TrimDone {
            start: secs(self.start),
            end: secs(self.end),
            as_gif: gif,
            as_mp4: !gif,
        });
        self.submitted = true;
    }
}

impl Surface for TrimWindow {
    fn title(&self) -> String {
        "StinkShot Trim".into()
    }

    fn viewport_builder(&self, _shared: &Shared) -> egui::ViewportBuilder {
        egui::ViewportBuilder::default()
            .with_title(self.title())
            .with_inner_size(vec2(TRIM_W, TRIM_H))
            .with_min_inner_size(vec2(460.0, 470.0))
            .with_position(pos2(
                uc::pts(self.rect.x, self.scale),
                uc::pts(self.rect.y, self.scale),
            ))
            .with_resizable(true)
    }

    fn on_created(&mut self, hwnd: Hwnd, shared: &Shared) {
        stinkshot_platform::window_util::apply_mica(hwnd, shared.dark);
        stinkshot_platform::window_util::set_rect(hwnd, self.rect);
        stinkshot_platform::window_util::force_foreground(hwnd);
    }

    fn show(&mut self, ctx: &egui::Context, shared: &mut Shared) -> SurfaceStatus {
        if !self.workers_started {
            self.start_workers(ctx);
        }
        self.pump_workers(ctx);

        let p = uc::pal(shared);
        let accent = uc::accent(shared);

        // Closing the window is a discard: ask first.
        if ctx.input(|i| i.viewport().close_requested()) && !self.submitted {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_discard = true;
        }

        // Keyboard.
        if !self.confirm_discard && !ctx.egui_wants_keyboard_input() {
            let (esc, enter, left, right, shift) = ctx.input(|i| {
                (
                    i.key_pressed(Key::Escape),
                    i.key_pressed(Key::Enter),
                    i.key_pressed(Key::ArrowLeft),
                    i.key_pressed(Key::ArrowRight),
                    i.modifiers.shift,
                )
            });
            if esc && !self.submitted {
                self.confirm_discard = true;
            }
            if enter && !self.submitted && self.progress.is_none() {
                let gif = self.prefer_gif;
                self.save(shared, gif);
            }
            let step = if shift { 1.0 } else { 0.1 };
            let delta = match (left, right) {
                (true, false) => -step,
                (false, true) => step,
                _ => 0.0,
            };
            if delta != 0.0 {
                match self.active {
                    Handle::In => {
                        (self.start, self.end) = drag_handle(
                            Handle::In,
                            self.start + delta,
                            self.start,
                            self.end,
                            self.dur,
                        )
                    }
                    Handle::Out => {
                        (self.start, self.end) = drag_handle(
                            Handle::Out,
                            self.end + delta,
                            self.start,
                            self.end,
                            self.dur,
                        )
                    }
                    Handle::Playhead => {
                        self.playhead = (self.playhead + delta).clamp(self.start, self.end)
                    }
                }
            }
        }

        let busy = self.submitted || self.progress.is_some();
        let mut save_gif = false;
        let mut save_mp4 = false;
        let mut ask_discard = false;
        let file_name = self
            .source
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let dur = self.dur;
        let (mut start, mut end, mut playhead, mut active) =
            (self.start, self.end, self.playhead, self.active);
        let mut dragging = self.dragging;
        let preview_tex = self.preview.as_ref().map(|t| t.id());
        let preview_dims = self.preview_dims;
        let thumbs: Vec<(egui::TextureId, [usize; 2])> =
            self.thumbs.iter().map(|t| (t.id(), t.size())).collect();
        let progress = self.progress;
        let prefer_gif = self.prefer_gif;
        let time = ctx.input(|i| i.time);
        if busy {
            ctx.request_repaint_after(Duration::from_millis(50));
        }

        uc::root_ui(ctx, p.bg, |ui, rect| {
            let inner = rect.shrink(TRIM_MARGIN);
            let painter = ui.painter().clone();

            // Title.
            painter.text(
                inner.left_top() + vec2(0.0, 2.0),
                Align2::LEFT_TOP,
                "Trim recording",
                uc::font(14.0),
                p.text,
            );
            painter.text(
                inner.right_top() + vec2(0.0, 3.0),
                Align2::RIGHT_TOP,
                format!("{} \u{b7} {}", file_name, uc::fmt_clock(secs(dur))),
                uc::font(12.0),
                p.text_tertiary,
            );

            // Bottom stack, measured from the bottom edge.
            let buttons = Rect::from_min_size(
                pos2(inner.left(), inner.bottom() - 34.0),
                vec2(inner.width(), 34.0),
            );
            let status_row = Rect::from_min_size(
                pos2(inner.left(), buttons.top() - 10.0 - 18.0),
                vec2(inner.width(), 18.0),
            );
            let labels = Rect::from_min_size(
                pos2(inner.left(), status_row.top() - 6.0 - 16.0),
                vec2(inner.width(), 16.0),
            );
            let strip = Rect::from_min_size(
                pos2(inner.left(), labels.top() - 8.0 - STRIP_H),
                vec2(inner.width(), STRIP_H),
            );
            let preview_area = Rect::from_min_max(
                pos2(inner.left(), inner.top() + 28.0),
                pos2(inner.right(), strip.top() - 14.0),
            );

            // Preview.
            painter.rect_filled(
                preview_area,
                CornerRadius::same(8),
                Color32::from_rgb(0x14, 0x14, 0x14),
            );
            if let Some(id) = preview_tex {
                let r = uc::contain_rect(preview_area.shrink(1.0), preview_dims.0, preview_dims.1);
                painter.image(
                    id,
                    r,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            } else {
                painter.text(
                    preview_area.center(),
                    Align2::CENTER_CENTER,
                    "Loading preview",
                    uc::font(12.0),
                    p.text_tertiary,
                );
            }
            painter.rect_stroke(
                preview_area,
                CornerRadius::same(8),
                Stroke::new(1.0, p.stroke),
                StrokeKind::Inside,
            );
            let shown = preview_time(active, start, end, playhead, dur);
            let tag = uc::fmt_tenths(secs(shown));
            let tg = painter.layout_no_wrap(tag, uc::font(12.0), Color32::WHITE);
            let tr = Rect::from_min_size(
                preview_area.left_bottom() + vec2(8.0, -8.0 - tg.size().y - 6.0),
                tg.size() + vec2(12.0, 6.0),
            );
            painter.rect_filled(
                tr,
                CornerRadius::same(4),
                Color32::from_rgba_unmultiplied(0, 0, 0, 170),
            );
            painter.galley(tr.min + vec2(6.0, 3.0), tg, Color32::WHITE);

            // Filmstrip.
            painter.rect_filled(
                strip,
                CornerRadius::same(6),
                Color32::from_rgb(0x1e, 0x1e, 0x1e),
            );
            if !thumbs.is_empty() {
                let n = thumbs.len() as f32;
                let cell_w = strip.width() / n;
                for (i, (id, size)) in thumbs.iter().enumerate() {
                    let cell = Rect::from_min_size(
                        pos2(strip.left() + i as f32 * cell_w, strip.top()),
                        vec2(cell_w - 1.0, strip.height()),
                    );
                    let (u0, v0, u1, v1) = uc::cover_uv(
                        (size[0] as f32, size[1] as f32),
                        cell.width(),
                        cell.height(),
                    );
                    painter.image(
                        *id,
                        cell,
                        Rect::from_min_max(pos2(u0, v0), pos2(u1, v1)),
                        Color32::WHITE,
                    );
                }
            } else {
                painter.text(
                    strip.center(),
                    Align2::CENTER_CENTER,
                    "Reading frames",
                    uc::font(12.0),
                    p.text_tertiary,
                );
            }
            let xs = secs_to_x(start, strip.left(), strip.width(), dur);
            let xe = secs_to_x(end, strip.left(), strip.width(), dur);
            let dim = Color32::from_rgba_unmultiplied(0, 0, 0, 153);
            painter.rect_filled(
                Rect::from_min_max(strip.left_top(), pos2(xs, strip.bottom())),
                CornerRadius::ZERO,
                dim,
            );
            painter.rect_filled(
                Rect::from_min_max(pos2(xe, strip.top()), strip.right_bottom()),
                CornerRadius::ZERO,
                dim,
            );
            // Selection top/bottom rails.
            painter.rect_filled(
                Rect::from_min_max(pos2(xs, strip.top()), pos2(xe, strip.top() + 2.0)),
                CornerRadius::ZERO,
                accent,
            );
            painter.rect_filled(
                Rect::from_min_max(pos2(xs, strip.bottom() - 2.0), pos2(xe, strip.bottom())),
                CornerRadius::ZERO,
                accent,
            );

            // Playhead.
            let px = secs_to_x(playhead.clamp(start, end), strip.left(), strip.width(), dur);
            painter.line_segment(
                [pos2(px, strip.top() - 3.0), pos2(px, strip.bottom() + 3.0)],
                Stroke::new(2.0, Color32::WHITE),
            );

            // Interaction: strip scrub, then handles on top.
            let scrub = ui.interact(
                strip,
                ui.scope_id().with("trim_scrub"),
                Sense::click_and_drag(),
            );
            if (scrub.clicked() || scrub.dragged())
                && dragging.is_none()
                && let Some(pos) = scrub.interact_pointer_pos()
            {
                playhead = x_to_secs(pos.x, strip.left(), strip.width(), dur).clamp(start, end);
                active = Handle::Playhead;
            }
            for (handle, x) in [(Handle::In, xs), (Handle::Out, xe)] {
                let hr = Rect::from_center_size(
                    pos2(x, strip.center().y),
                    vec2(HANDLE_W, strip.height()),
                );
                let hit = hr.expand2(vec2(6.0, 0.0));
                let resp = ui.interact(
                    hit,
                    ui.scope_id().with(("trim_handle", handle as u8)),
                    Sense::drag(),
                );
                let lit = resp.hovered() || resp.dragged();
                painter.rect_filled(
                    hr,
                    CornerRadius::same(3),
                    if lit {
                        accent.lerp_to_gamma(Color32::WHITE, 0.25)
                    } else {
                        accent
                    },
                );
                // Grip notch.
                painter.line_segment(
                    [
                        pos2(x, strip.center().y - 7.0),
                        pos2(x, strip.center().y + 7.0),
                    ],
                    Stroke::new(1.5, p.on_accent),
                );
                if resp.hovered() || resp.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                }
                if resp.drag_started() {
                    dragging = Some(handle);
                    active = handle;
                }
                if resp.dragged()
                    && let Some(pos) = resp.interact_pointer_pos()
                {
                    let t = x_to_secs(pos.x, strip.left(), strip.width(), dur);
                    (start, end) = drag_handle(handle, t, start, end, dur);
                    playhead = playhead.clamp(start, end);
                }
                if resp.drag_stopped() {
                    dragging = None;
                }
            }

            // Time labels.
            painter.text(
                labels.left_center(),
                Align2::LEFT_CENTER,
                uc::fmt_tenths(secs(start)),
                uc::font(12.0),
                p.text_tertiary,
            );
            painter.text(
                labels.center(),
                Align2::CENTER_CENTER,
                format!("keep {}", uc::fmt_tenths(secs(end - start))),
                uc::font(12.0),
                p.text_secondary,
            );
            painter.text(
                labels.right_center(),
                Align2::RIGHT_CENTER,
                uc::fmt_tenths(secs(end)),
                uc::font(12.0),
                p.text_tertiary,
            );

            // Status / progress row.
            if let Some(pr) = progress {
                let bar = Rect::from_min_size(
                    status_row.left_center() + vec2(0.0, -2.0),
                    vec2(status_row.width() - 64.0, 4.0),
                );
                painter.rect_filled(
                    bar,
                    CornerRadius::same(2),
                    Color32::from_rgb(0x3a, 0x3a, 0x3a),
                );
                painter.rect_filled(
                    Rect::from_min_size(bar.min, vec2(bar.width() * pr, 4.0)),
                    CornerRadius::same(2),
                    accent,
                );
                painter.text(
                    status_row.right_center(),
                    Align2::RIGHT_CENTER,
                    format!("{}%", (pr * 100.0).round() as i32),
                    uc::font(12.0),
                    p.text_secondary,
                );
            } else if busy {
                // Waiting for the first progress report: indeterminate sweep.
                let bar = Rect::from_min_size(
                    status_row.left_center() + vec2(0.0, -2.0),
                    vec2(status_row.width(), 4.0),
                );
                painter.rect_filled(
                    bar,
                    CornerRadius::same(2),
                    Color32::from_rgb(0x3a, 0x3a, 0x3a),
                );
                let t = ((time * 0.8) % 1.0) as f32;
                let w = bar.width() * 0.25;
                let x0 = bar.left() + (bar.width() + w) * t - w;
                let seg = Rect::from_min_max(
                    pos2(x0.max(bar.left()), bar.top()),
                    pos2((x0 + w).min(bar.right()), bar.bottom()),
                );
                if seg.width() > 0.0 {
                    painter.rect_filled(seg, CornerRadius::same(2), accent);
                }
            } else if end - start > 30.0 {
                painter.text(
                    status_row.left_center(),
                    Align2::LEFT_CENTER,
                    "Long clips make very large GIFs. MP4 is smaller.",
                    uc::font(12.0),
                    p.text_tertiary,
                );
            }

            // Buttons.
            let w = (buttons.width() - 12.0) / 3.0;
            let cell = |i: f32| {
                Rect::from_min_size(
                    pos2(buttons.left() + i * (w + 6.0), buttons.top()),
                    vec2(w, 34.0),
                )
            };
            let (gif_kind, mp4_kind) = if prefer_gif {
                (uc::BtnKind::Accent, uc::BtnKind::Neutral)
            } else {
                (uc::BtnKind::Neutral, uc::BtnKind::Accent)
            };
            if uc::rect_button(
                ui,
                "discard",
                cell(0.0),
                "Discard",
                uc::BtnKind::Neutral,
                !busy,
                p,
                accent,
            )
            .clicked()
            {
                ask_discard = true;
            }
            if uc::rect_button(ui, "gif", cell(1.0), "Save GIF", gif_kind, !busy, p, accent)
                .clicked()
            {
                save_gif = true;
            }
            if uc::rect_button(ui, "mp4", cell(2.0), "Save MP4", mp4_kind, !busy, p, accent)
                .clicked()
            {
                save_mp4 = true;
            }
        });

        self.start = start;
        self.end = end;
        self.playhead = playhead;
        self.active = active;
        self.dragging = dragging;
        self.wanted = secs(preview_time(
            self.active,
            self.start,
            self.end,
            self.playhead,
            self.dur,
        ));

        if ask_discard {
            self.confirm_discard = true;
        }
        if save_gif {
            self.save(shared, true);
        }
        if save_mp4 {
            self.save(shared, false);
        }

        let mut status = SurfaceStatus::Open;
        if self.confirm_discard {
            let modal = egui::Modal::new(egui::Id::unique("trim_discard_modal")).show(ctx, |ui| {
                ui.set_width(340.0);
                ui.label(
                    egui::RichText::new("Discard this recording?")
                        .size(15.0)
                        .color(p.text),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("It has not been saved yet. This cannot be undone.")
                        .size(12.0)
                        .color(p.text_secondary),
                );
                ui.add_space(14.0);
                let mut keep = false;
                let mut discard = false;
                ui.horizontal(|ui| {
                    let (r1, _) = ui.allocate_exact_size(vec2(160.0, 34.0), Sense::hover());
                    keep = uc::rect_button(
                        ui,
                        "keep",
                        r1,
                        "Keep editing",
                        uc::BtnKind::Neutral,
                        true,
                        p,
                        accent,
                    )
                    .clicked();
                    let (r2, _) = ui.allocate_exact_size(vec2(160.0, 34.0), Sense::hover());
                    discard = uc::rect_button(
                        ui,
                        "really_discard",
                        r2,
                        "Discard",
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
                // RecordCancel semantics here: the shell drops the untrimmed recording.
                shared.outbox.push(Command::RecordCancel);
                status = SurfaceStatus::Close;
            } else if keep || modal.should_close() {
                self.confirm_discard = false;
            }
        }
        status
    }
}

// ---------------------------------------------------------------------------------------
// ScrollHud
// ---------------------------------------------------------------------------------------

const SCROLL_W: f32 = 300.0;
const SCROLL_H: f32 = 98.0;

pub struct ScrollHud {
    frames: usize,
    width: u32,
    height: u32,
    scale: f32,
    rect: PxRect,
    esc_was_down: bool,
}

impl ScrollHud {
    pub fn new(region: PxRect) -> Self {
        let area = uc::area_of_rect(region);
        let size = (uc::px(SCROLL_W, area.scale), uc::px(SCROLL_H, area.scale));
        let rect = uc::hud_rect(region, size, area.work, uc::px(10.0, area.scale));
        Self {
            frames: 0,
            width: 0,
            height: 0,
            scale: area.scale,
            rect,
            esc_was_down: uc::async_key_down(uc::VK_ESCAPE),
        }
    }

    pub fn update(&mut self, frames: usize, width: u32, height: u32) {
        self.frames = frames;
        self.width = width;
        self.height = height;
    }
}

impl Surface for ScrollHud {
    fn title(&self) -> String {
        "StinkShot.ScrollHud".into()
    }

    fn viewport_builder(&self, _shared: &Shared) -> egui::ViewportBuilder {
        egui::ViewportBuilder::default()
            .with_title(self.title())
            .with_inner_size(vec2(SCROLL_W, SCROLL_H))
            .with_position(pos2(
                uc::pts(self.rect.x, self.scale),
                uc::pts(self.rect.y, self.scale),
            ))
            .with_decorations(false)
            .with_resizable(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_active(false)
            .with_drag_and_drop(false)
    }

    fn on_created(&mut self, hwnd: Hwnd, shared: &Shared) {
        stinkshot_platform::window_util::apply_mica(hwnd, shared.dark);
        uc::apply_hud_style(hwnd, self.rect, true, true);
    }

    fn show(&mut self, ctx: &egui::Context, shared: &mut Shared) -> SurfaceStatus {
        let p = uc::pal(shared);
        let accent = uc::accent(shared);
        let max_h = shared.config.capture.scroll_max_height;
        let progress = scroll_progress(self.height, max_h);
        let (frames, width, height) = (self.frames, self.width, self.height);
        let mut cancel = false;
        let mut stop = false;

        // Esc cancels even though this window never takes focus.
        let esc_down = uc::async_key_down(uc::VK_ESCAPE);
        if esc_down && !self.esc_was_down {
            cancel = true;
        }
        self.esc_was_down = esc_down;
        ctx.request_repaint_after(Duration::from_millis(100));

        uc::root_ui(ctx, p.flyout, |ui, rect| {
            ui.painter().rect_stroke(
                rect,
                CornerRadius::same(10),
                Stroke::new(1.0, p.stroke),
                StrokeKind::Inside,
            );
            let inner = rect.shrink(12.0);
            let painter = ui.painter().clone();
            painter.text(
                inner.left_top(),
                Align2::LEFT_TOP,
                format!("Stitching frame {frames}"),
                uc::font(13.0),
                p.text,
            );
            painter.text(
                inner.right_top(),
                Align2::RIGHT_TOP,
                uc::fmt_dims(width, height),
                uc::font(13.0),
                p.text_tertiary,
            );
            let bar = Rect::from_min_size(
                pos2(inner.left(), inner.top() + 28.0),
                vec2(inner.width(), 4.0),
            );
            painter.rect_filled(
                bar,
                CornerRadius::same(2),
                Color32::from_rgb(0x3a, 0x3a, 0x3a),
            );
            painter.rect_filled(
                Rect::from_min_size(bar.min, vec2(bar.width() * progress, 4.0)),
                CornerRadius::same(2),
                accent,
            );
            let row_top = inner.bottom() - 32.0;
            let w = (inner.width() - 6.0) / 2.0;
            let b1 = Rect::from_min_size(pos2(inner.left(), row_top), vec2(w, 32.0));
            let b2 = Rect::from_min_size(pos2(inner.left() + w + 6.0, row_top), vec2(w, 32.0));
            if uc::rect_button(
                ui,
                "scroll_cancel",
                b1,
                "Cancel",
                uc::BtnKind::Neutral,
                true,
                p,
                accent,
            )
            .clicked()
            {
                cancel = true;
            }
            if uc::rect_button(
                ui,
                "scroll_stop",
                b2,
                "Stop here",
                uc::BtnKind::Accent,
                true,
                p,
                accent,
            )
            .clicked()
            {
                stop = true;
            }
        });

        if cancel {
            shared.outbox.push(Command::ScrollCancel);
            return SurfaceStatus::Close;
        }
        if stop {
            shared.outbox.push(Command::ScrollStop);
            return SurfaceStatus::Close;
        }
        SurfaceStatus::Open
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdown_digits() {
        let d = |ms| Duration::from_millis(ms);
        assert_eq!(countdown_number(3, d(0)), 3);
        assert_eq!(countdown_number(3, d(999)), 3);
        assert_eq!(countdown_number(3, d(1000)), 2);
        assert_eq!(countdown_number(3, d(2500)), 1);
        assert_eq!(countdown_number(3, d(3000)), 0);
        assert_eq!(countdown_number(3, d(9000)), 0);
        assert_eq!(countdown_number(0, d(0)), 0);
    }

    #[test]
    fn countdown_phase_wraps_each_second() {
        assert!((countdown_phase(Duration::from_millis(250)) - 0.25).abs() < 1e-3);
        assert!((countdown_phase(Duration::from_millis(1750)) - 0.75).abs() < 1e-3);
    }

    #[test]
    fn scroll_progress_clamps() {
        assert_eq!(scroll_progress(0, 30000), 0.0);
        assert!((scroll_progress(15000, 30000) - 0.5).abs() < 1e-6);
        assert_eq!(scroll_progress(40000, 30000), 1.0);
        assert_eq!(scroll_progress(100, 0), 0.0);
    }

    #[test]
    fn filmstrip_fills_the_strip() {
        // 520 px strip, 56 px tall, 16:9 -> ~100 px cells -> 6 thumbnails.
        assert_eq!(filmstrip_count(520.0, 56.0, 16.0 / 9.0), 6);
        assert_eq!(filmstrip_count(10.0, 56.0, 1.0), 3);
        assert_eq!(filmstrip_count(5000.0, 10.0, 1.0), 24);
    }

    #[test]
    fn time_x_mapping_roundtrips() {
        let (left, w, dur) = (20.0, 500.0, 38.3);
        assert_eq!(secs_to_x(0.0, left, w, dur), 20.0);
        assert_eq!(secs_to_x(dur, left, w, dur), 520.0);
        let t = x_to_secs(secs_to_x(12.5, left, w, dur), left, w, dur);
        assert!((t - 12.5).abs() < 1e-3);
        assert_eq!(x_to_secs(-100.0, left, w, dur), 0.0);
        assert_eq!(x_to_secs(9999.0, left, w, dur), dur);
        assert_eq!(x_to_secs(10.0, left, 0.0, dur), 0.0);
    }

    #[test]
    fn handles_cannot_cross_or_leave_the_clip() {
        let dur = 10.0;
        // In handle stops short of the out handle by the minimum span.
        let (s, e) = drag_handle(Handle::In, 9.9, 1.0, 5.0, dur);
        assert!((s - 4.8).abs() < 1e-9 && e == 5.0);
        // Out handle stops past the in handle.
        let (s, e) = drag_handle(Handle::Out, 0.0, 2.0, 5.0, dur);
        assert_eq!(s, 2.0);
        assert!((e - 2.2).abs() < 1e-9);
        // Clamped to the clip bounds.
        assert_eq!(drag_handle(Handle::In, -3.0, 1.0, 5.0, dur), (0.0, 5.0));
        assert_eq!(drag_handle(Handle::Out, 99.0, 1.0, 5.0, dur), (1.0, 10.0));
        // Playhead drag leaves the range alone.
        assert_eq!(
            drag_handle(Handle::Playhead, 3.0, 1.0, 5.0, dur),
            (1.0, 5.0)
        );
    }

    #[test]
    fn tiny_clips_still_have_a_valid_span() {
        assert!((min_keep(0.3) - 0.15).abs() < 1e-9);
        let (s, e) = drag_handle(Handle::In, 0.3, 0.0, 0.3, 0.3);
        assert!(s <= e - 0.15 + 1e-9);
    }

    #[test]
    fn preview_follows_the_active_handle() {
        assert_eq!(preview_time(Handle::In, 3.2, 38.3, 10.0, 40.0), 3.2);
        assert!((preview_time(Handle::Out, 3.2, 38.3, 10.0, 40.0) - 38.25).abs() < 1e-9);
        assert_eq!(preview_time(Handle::Playhead, 3.2, 38.3, 1.0, 40.0), 3.2);
        assert_eq!(preview_time(Handle::Playhead, 3.2, 38.3, 20.0, 40.0), 20.0);
    }

    #[test]
    fn toggle_changes_are_reported_once() {
        let t = HudToggles {
            mic: true,
            system: true,
            webcam: false,
            overlays: true,
        };
        let mut hud = RecordHud {
            summary: String::new(),
            toggles: t,
            changed: false,
            status: None,
            scale: 1.0,
            rect: PxRect::new(0, 0, 1, 1),
        };
        assert_eq!(hud.take_toggle_changes(), None);
        hud.toggles.mic = false;
        hud.changed = true;
        let got = hud.take_toggle_changes();
        assert_eq!(got.map(|g| g.mic), Some(false));
        assert_eq!(hud.take_toggle_changes(), None);
    }
}
