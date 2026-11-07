//! Find Sensitive Info (Smart Redact) in the editor: OCR + `stinkshot_core::sensitive` on
//! the unannotated selection, off the UI thread; a compact review panel lists the findings
//! (kind, masked preview, per-item toggle, hover highlights the spot on the canvas) and
//! Redact Selected adds Blur / Pixelate / Solid Box shapes as one undo step. The chosen style
//! is remembered in `[editor] smart_redact_style`. Spec: docs/notes/smart-redact.md.

use egui::{Align2, Area, Frame, Margin, RichText, ScrollArea, Sense};
use stinkshot_core::sensitive::{self, Finding, TextWord};
use stinkshot_platform::ocr::OcrEngine;

use super::*;
use crate::theme::Palette;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedactStyle {
    Box,
    Blur,
    Pixelate,
}

impl RedactStyle {
    pub const ALL: [RedactStyle; 3] = [RedactStyle::Blur, RedactStyle::Pixelate, RedactStyle::Box];

    pub fn label(self) -> &'static str {
        match self {
            RedactStyle::Box => "Solid Box",
            RedactStyle::Blur => "Blur",
            RedactStyle::Pixelate => "Pixelate",
        }
    }

    pub fn from_config(s: &str) -> RedactStyle {
        match s.trim().to_ascii_lowercase().as_str() {
            "blur" => RedactStyle::Blur,
            "pixelate" => RedactStyle::Pixelate,
            _ => RedactStyle::Box,
        }
    }

    pub fn config_name(self) -> &'static str {
        match self {
            RedactStyle::Box => "box",
            RedactStyle::Blur => "blur",
            RedactStyle::Pixelate => "pixelate",
        }
    }
}

/// How long "No Sensitive Info Found" stays.
const EMPTY_SECS: Duration = Duration::from_millis(2500);
/// Padding around each found word box, as a fraction of its height (at least 2 px), so
/// anti-aliased glyph edges are covered too.
const PAD_FRACTION: f32 = 0.15;

pub struct SmartRedact {
    /// The image that was scanned; findings are dropped when the editor's base changes.
    base: Arc<Rgba>,
    phase: Phase,
}

enum Phase {
    Scanning(Receiver<Result<Vec<Finding>, String>>),
    Review(Review),
    Empty(Instant),
}

pub struct Review {
    pub findings: Vec<Finding>,
    pub on: Vec<bool>,
    pub hovered: Option<usize>,
    pub style: RedactStyle,
}

/// What the panel shows this frame.
pub enum PanelView<'a> {
    Scanning,
    Empty,
    Review(&'a Review),
}

#[derive(Clone, Debug, PartialEq)]
pub enum SmartAction {
    Toggle(usize),
    Hover(Option<usize>),
    Style(RedactStyle),
    Apply,
    Close,
}

/// Grow `r` by the word padding and clip it to `w` x `h`.
pub fn padded(r: PxRect, w: u32, h: u32) -> Option<PxRect> {
    let pad = ((r.h as f32 * PAD_FRACTION).round() as i32).max(2);
    PxRect::new(r.x - pad, r.y - pad, r.w + 2 * pad, r.h + 2 * pad)
        .intersect(&PxRect::new(0, 0, w as i32, h as i32))
}

/// OCR + detection for one image (worker thread).
fn scan(engine: &dyn OcrEngine, img: &Rgba) -> Result<Vec<Finding>, String> {
    let ocr = engine.recognize(img).map_err(|e| format!("{e:#}"))?;
    let lines: Vec<Vec<TextWord>> = ocr
        .lines
        .iter()
        .map(|l| {
            if l.words.is_empty() {
                vec![TextWord {
                    text: l.text.clone(),
                    rect: l.rect,
                }]
            } else {
                l.words
                    .iter()
                    .map(|w| TextWord {
                        text: w.text.clone(),
                        rect: w.rect,
                    })
                    .collect()
            }
        })
        .collect();
    Ok(sensitive::find_in_lines(&lines)
        .into_iter()
        .filter_map(|mut f| {
            f.rect = padded(f.rect, img.width, img.height)?;
            Some(f)
        })
        .collect())
}

/// What Enter / Ctrl+C do while the panel is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelKey {
    /// Enter in the review list: the same as Redact Selected.
    Apply,
    /// Eaten so it cannot copy the unredacted image (Ctrl+C anywhere in the panel, Enter
    /// while scanning or with nothing ticked).
    Swallow,
    /// No panel, or only the "No Sensitive Info Found" notice: the key does its usual job.
    Pass,
}

/// The panel decides Enter and Ctrl+C before the editor's Copy shortcut does, so
/// confirming the dialog never copies the findings in the clear. See smart-redact.md.
pub fn panel_key(view: Option<&PanelView>, enter: bool) -> PanelKey {
    match view {
        None | Some(PanelView::Empty) => PanelKey::Pass,
        Some(PanelView::Review(r)) if enter && r.on.iter().any(|o| *o) => PanelKey::Apply,
        Some(_) => PanelKey::Swallow,
    }
}

/// Smart Redact strength grows with the found text, so large text never stays readable
/// (or brute-forceable from a known format) under the user's lighter tool setting:
/// blur radius at least a third of the box height, pixel blocks at least half of it.
fn strength(rect: PxRect, params: &ToolParams) -> (f32, u32) {
    let h = rect.h.max(0) as f32;
    let radius = params.blur_radius.max(h / 3.0).max(1.0);
    let block = params.pixelate_block.max((h / 2.0).ceil() as u32).max(2);
    (radius, block)
}

/// The shape that redacts `rect` (image px) in `style`.
pub fn redaction_shape(
    style: RedactStyle,
    rect: PxRect,
    params: &ToolParams,
) -> (ShapeKind, Style) {
    let r = RectF::new(rect.x as f32, rect.y as f32, rect.w as f32, rect.h as f32);
    let (radius, block) = strength(rect, params);
    let (tool, kind) = match style {
        RedactStyle::Box => (Tool::Redact, ShapeKind::Redact { rect: r }),
        RedactStyle::Blur => (Tool::Blur, ShapeKind::Blur { rect: r, radius }),
        RedactStyle::Pixelate => (Tool::Pixelate, ShapeKind::Pixelate { rect: r, block }),
    };
    let mut st = shapes::style_for_tool(tool, &params.style);
    if style == RedactStyle::Box {
        // A redaction box is black whatever the annotation color is.
        st.color = [0, 0, 0, 255];
    }
    (kind, st)
}

impl Overlay {
    /// Find Sensitive Info: start a scan, or close the panel when it is already open.
    pub(super) fn toggle_smart_redact(&mut self, ctx: &Context, shared: &Shared) {
        if self.smart.take().is_some() {
            return;
        }
        if !self.annotating() || self.sel.is_none() {
            return;
        }
        let Some(engine) = shared.ocr.clone() else {
            self.flash_notice(NO_OCR_NOTICE);
            return;
        };
        self.commit_text();
        self.popup = None;
        let base = self.ensure_base();
        let (tx, rx) = channel();
        let (img, ctx2) = (base.clone(), ctx.clone());
        let spawned = std::thread::Builder::new()
            .name("smart-redact".into())
            .spawn(move || {
                let r = scan(engine.as_ref(), &img);
                if let Err(e) = &r {
                    log::warn!("smart redact: {e}");
                }
                let _ = tx.send(r);
                ctx2.request_repaint();
            });
        match spawned {
            Ok(_) => {
                self.smart = Some(SmartRedact {
                    base,
                    phase: Phase::Scanning(rx),
                })
            }
            Err(e) => {
                log::error!("smart redact thread: {e}");
                self.flash_notice("Could Not Start Finding Sensitive Info");
            }
        }
    }

    /// Pick up scan results; drop everything when the scanned image is no longer the base
    /// (the selection moved or resized).
    pub(super) fn poll_smart_redact(&mut self, shared: &Shared) {
        let Some(s) = self.smart.as_mut() else { return };
        if !self.base.as_ref().is_some_and(|b| Arc::ptr_eq(b, &s.base)) {
            self.smart = None;
            return;
        }
        match &s.phase {
            Phase::Scanning(rx) => match rx.try_recv() {
                Ok(Ok(findings)) if findings.is_empty() => {
                    s.phase = Phase::Empty(Instant::now() + EMPTY_SECS);
                }
                Ok(Ok(findings)) => {
                    s.phase = Phase::Review(Review {
                        on: vec![true; findings.len()],
                        findings,
                        hovered: None,
                        style: RedactStyle::from_config(&shared.config.editor.smart_redact_style),
                    });
                }
                Ok(Err(_)) | Err(TryRecvError::Disconnected) => {
                    self.smart = None;
                    self.flash_notice("Could Not Read the Text in This Image");
                }
                Err(TryRecvError::Empty) => {}
            },
            Phase::Empty(until) => {
                if Instant::now() >= *until {
                    self.smart = None;
                }
            }
            Phase::Review(_) => {}
        }
    }

    pub(super) fn smart_view(&self) -> Option<PanelView<'_>> {
        Some(match &self.smart.as_ref()?.phase {
            Phase::Scanning(_) => PanelView::Scanning,
            Phase::Empty(_) => PanelView::Empty,
            Phase::Review(r) => PanelView::Review(r),
        })
    }

    /// Enter / Ctrl+C while the panel is open; true when the panel handled the key.
    pub(super) fn smart_key(&mut self, shared: &mut Shared, enter: bool) -> bool {
        match panel_key(self.smart_view().as_ref(), enter) {
            PanelKey::Pass => false,
            PanelKey::Swallow => true,
            PanelKey::Apply => {
                self.redact_selected(shared);
                true
            }
        }
    }

    pub(super) fn smart_active(&self) -> bool {
        self.smart.is_some()
    }

    fn review_mut(&mut self) -> Option<&mut Review> {
        match &mut self.smart.as_mut()?.phase {
            Phase::Review(r) => Some(r),
            _ => None,
        }
    }

    pub(super) fn apply_smart_action(&mut self, shared: &mut Shared, a: SmartAction) {
        match a {
            SmartAction::Toggle(i) => {
                if let Some(v) = self.review_mut().and_then(|r| r.on.get_mut(i)) {
                    *v = !*v;
                }
            }
            SmartAction::Hover(h) => {
                if let Some(r) = self.review_mut() {
                    r.hovered = h;
                }
            }
            SmartAction::Style(st) => {
                if let Some(r) = self.review_mut() {
                    r.style = st;
                }
            }
            SmartAction::Close => self.smart = None,
            SmartAction::Apply => self.redact_selected(shared),
        }
    }

    /// Redact Selected: one shape per enabled finding, a single undo step.
    fn redact_selected(&mut self, shared: &mut Shared) {
        let Some(SmartRedact {
            phase: Phase::Review(r),
            ..
        }) = self.smart.take()
        else {
            return;
        };
        let name = r.style.config_name();
        if shared.config.editor.smart_redact_style != name {
            shared.config.editor.smart_redact_style = name.into();
            shared.config_dirty = true;
        }
        let params = self.params();
        let pre = self.doc.clone();
        for (f, on) in r.findings.iter().zip(&r.on) {
            if *on {
                let (kind, style) = redaction_shape(r.style, f.rect, &params);
                self.doc.add(kind, style);
            }
        }
        if self.doc != pre {
            self.undo.push(&pre);
            self.selected = None;
            self.tool = Tool::Select;
        }
    }

    /// Outline every finding on the canvas; the hovered one gets a fill.
    pub(super) fn paint_smart_findings(
        &self,
        painter: &egui::Painter,
        m: ImgToPt,
        accent: Color32,
    ) {
        let Some(PanelView::Review(r)) = self.smart_view() else {
            return;
        };
        for (i, f) in r.findings.iter().enumerate() {
            let rr = m.r(RectF::new(
                f.rect.x as f32,
                f.rect.y as f32,
                f.rect.w as f32,
                f.rect.h as f32,
            ));
            let hovered = r.hovered == Some(i);
            let col = if r.on[i] {
                accent
            } else {
                Color32::from_white_alpha(140)
            };
            if hovered {
                painter.rect_filled(rr, 2.0, col.gamma_multiply(0.35));
            }
            painter.rect_stroke(
                rr.expand(1.0),
                2.0,
                egui::Stroke::new(if hovered { 2.5 } else { 1.5 }, col),
                egui::epaint::StrokeKind::Outside,
            );
        }
    }
}

pub const NO_OCR_NOTICE: &str = "Find Sensitive Info Needs an OCR Language in Windows Settings";

/// Tooltip for the toolbar button.
pub fn button_tip(ocr: bool) -> String {
    if ocr {
        format!("Find Sensitive Info ({})", keys::FIND_SENSITIVE.label)
    } else {
        format!("{NO_OCR_NOTICE}\n{}", crate::workers::NO_OCR_HELP)
    }
}

const PANEL_W: f32 = 300.0;

/// The panel's size estimate (for placing it before it is drawn), egui points.
pub fn panel_height(view: &PanelView) -> f32 {
    match view {
        PanelView::Scanning | PanelView::Empty => 44.0,
        PanelView::Review(r) => 120.0 + 34.0 * r.findings.len().min(6) as f32,
    }
}

/// The review panel at `pos` (its top-right corner when `right_aligned`).
pub fn panel(
    ctx: &Context,
    pos: Pos2,
    pivot: Align2,
    view: &PanelView,
    pal: &Palette,
    accent: Color32,
    out: &mut Vec<SmartAction>,
) {
    let (dark, _) = theme::current(ctx);
    Area::new(egui::Id::unique("stinkshot-ov-smart"))
        .order(Order::Foreground)
        .fixed_pos(pos)
        .pivot(pivot)
        .show(ctx, |ui| {
            Frame::NONE
                .fill(pal.flyout.gamma_multiply(0.97))
                .stroke(egui::Stroke::new(1.0, pal.stroke))
                .corner_radius(10)
                .inner_margin(Margin::symmetric(12, 10))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 8],
                    blur: 24,
                    spread: 0,
                    color: Color32::from_black_alpha(115),
                })
                .show(ui, |ui| match view {
                    PanelView::Scanning => {
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new().size(16.0).color(accent));
                            ui.label(
                                RichText::new("Finding Sensitive Info\u{2026}")
                                    .size(13.0)
                                    .color(pal.text),
                            );
                        });
                    }
                    PanelView::Empty => {
                        ui.horizontal(|ui| {
                            let (icon, _) =
                                ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                            theme::widgets::paint_icon(
                                ui.painter(),
                                icon,
                                theme::Icon::Shield,
                                pal.text_secondary,
                            );
                            ui.label(
                                RichText::new("No Sensitive Info Found")
                                    .size(13.0)
                                    .color(pal.text),
                            );
                        });
                    }
                    PanelView::Review(r) => review(ui, r, pal, accent, dark, out),
                });
        });
}

fn review(
    ui: &mut egui::Ui,
    r: &Review,
    pal: &Palette,
    accent: Color32,
    dark: bool,
    out: &mut Vec<SmartAction>,
) {
    ui.set_width(PANEL_W);
    ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
    let count = r.on.iter().filter(|o| **o).count();
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!(
                "Found {} {}",
                r.findings.len(),
                if r.findings.len() == 1 {
                    "Item"
                } else {
                    "Items"
                }
            ))
            .font(theme::semibold(13.0))
            .color(pal.text),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let close = ui.add_sized(
                Vec2::splat(22.0),
                egui::Label::new("").sense(Sense::click()),
            );
            theme::widgets::paint_icon(
                ui.painter(),
                close.rect.shrink(4.0),
                theme::Icon::Close,
                if close.hovered() {
                    pal.text
                } else {
                    pal.text_secondary
                },
            );
            if close.on_hover_text("Close (Esc)").clicked() {
                out.push(SmartAction::Close);
            }
        });
    });

    let mut hovered = None;
    ScrollArea::vertical()
        .max_height(34.0 * 6.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(0.0, 2.0);
            for (i, f) in r.findings.iter().enumerate() {
                let (rect, resp) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), Sense::click());
                if resp.hovered() {
                    hovered = Some(i);
                    ui.painter()
                        .rect_filled(rect, 6.0, theme::subtle_fill(dark, 1));
                }
                // Checkbox.
                let cb = egui::Rect::from_center_size(
                    egui::pos2(rect.left() + 14.0, rect.center().y),
                    Vec2::splat(16.0),
                );
                if r.on[i] {
                    ui.painter().rect_filled(cb, 4.0, accent);
                    let p = |x: f32, y: f32| egui::pos2(cb.left() + x, cb.top() + y);
                    ui.painter().line(
                        vec![p(4.0, 8.5), p(7.0, 11.5), p(12.0, 5.0)],
                        egui::Stroke::new(1.8, pal.on_accent),
                    );
                } else {
                    ui.painter().rect_stroke(
                        cb,
                        4.0,
                        egui::Stroke::new(1.0, pal.text_secondary),
                        egui::epaint::StrokeKind::Inside,
                    );
                }
                ui.painter().text(
                    egui::pos2(cb.right() + 10.0, rect.center().y),
                    Align2::LEFT_CENTER,
                    f.kind.label(),
                    egui::FontId::proportional(12.5),
                    pal.text,
                );
                ui.painter().text(
                    egui::pos2(rect.right() - 8.0, rect.center().y),
                    Align2::RIGHT_CENTER,
                    &f.preview,
                    egui::FontId::monospace(12.0),
                    pal.text_secondary,
                );
                if resp.clicked() {
                    out.push(SmartAction::Toggle(i));
                }
            }
        });
    if hovered != r.hovered {
        out.push(SmartAction::Hover(hovered));
    }

    // Style segmented choice.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(RichText::new("Style").size(12.0).color(pal.text_secondary));
        ui.add_space(4.0);
        for st in RedactStyle::ALL {
            let active = r.style == st;
            let text = RichText::new(st.label()).size(12.0).color(if active {
                theme::accent_fg(accent, dark)
            } else {
                pal.text
            });
            let resp = ui.add(
                egui::Button::new(text)
                    .fill(if active {
                        theme::accent_soft(accent, 56)
                    } else {
                        theme::subtle_fill(dark, 1)
                    })
                    .corner_radius(6),
            );
            if resp.clicked() {
                out.push(SmartAction::Style(st));
            }
        }
    });

    ui.add_space(2.0);
    ui.horizontal(|ui| {
        let label = if count == 1 {
            "Redact 1 Selected".to_string()
        } else {
            format!("Redact {count} Selected")
        };
        let resp = ui.add_enabled_ui(count > 0, |ui| theme::widgets::accent_button(ui, &label));
        if resp.inner.clicked() {
            out.push(SmartAction::Apply);
        }
        if theme::widgets::button(ui, "Cancel").clicked() {
            out.push(SmartAction::Close);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_round_trips_through_config() {
        for st in RedactStyle::ALL {
            assert_eq!(RedactStyle::from_config(st.config_name()), st);
        }
        assert_eq!(RedactStyle::from_config(""), RedactStyle::Box);
        assert_eq!(RedactStyle::from_config("BLUR"), RedactStyle::Blur);
    }

    #[test]
    fn padding_grows_and_clips() {
        let r = padded(PxRect::new(10, 10, 50, 20), 100, 100).unwrap();
        assert_eq!(r, PxRect::new(7, 7, 56, 26));
        let edge = padded(PxRect::new(0, 0, 10, 10), 100, 100).unwrap();
        assert_eq!((edge.x, edge.y), (0, 0));
        assert!(padded(PxRect::new(200, 200, 10, 10), 100, 100).is_none());
    }

    #[test]
    fn shapes_match_the_style() {
        let params = ToolParams {
            style: Style {
                color: [255, 0, 0, 255],
                width: 4.0,
                fill: None,
                shadow: true,
            },
            font_size: 20.0,
            blur_radius: 12.0,
            pixelate_block: 10,
        };
        let r = PxRect::new(1, 2, 30, 40);
        let (k, s) = redaction_shape(RedactStyle::Box, r, &params);
        assert!(
            matches!(k, ShapeKind::Redact { rect } if rect == RectF::new(1.0, 2.0, 30.0, 40.0))
        );
        assert_eq!(s.color, [0, 0, 0, 255]);
        assert!(!s.shadow);
        let (k, _) = redaction_shape(RedactStyle::Blur, r, &params);
        // 40 px tall: radius at least 40 / 3, more than the tool's 12.
        assert!(matches!(k, ShapeKind::Blur { radius, .. } if (radius - 40.0 / 3.0).abs() < 1e-4));
        let (k, _) = redaction_shape(RedactStyle::Pixelate, r, &params);
        // 40 px tall: blocks at least 20 px, more than the tool's 10.
        assert!(matches!(k, ShapeKind::Pixelate { block: 20, .. }));
    }

    #[test]
    fn strength_scales_with_text_height() {
        let params = ToolParams {
            style: Style {
                color: [255, 0, 0, 255],
                width: 4.0,
                fill: None,
                shadow: false,
            },
            font_size: 20.0,
            blur_radius: 4.0,
            pixelate_block: 4,
        };
        // Small text keeps the tool setting; tall text gets coarser cells and a wider blur.
        assert_eq!(strength(PxRect::new(0, 0, 80, 9), &params), (4.0, 5));
        let (radius, block) = strength(PxRect::new(0, 0, 300, 42), &params);
        assert_eq!(block, 21);
        assert!((radius - 14.0).abs() < 1e-4);
        let (k, _) = redaction_shape(RedactStyle::Blur, PxRect::new(0, 0, 300, 42), &params);
        assert!(matches!(k, ShapeKind::Blur { radius, .. } if radius >= 14.0));
    }

    #[test]
    fn enter_redacts_and_copy_is_held_while_the_panel_is_open() {
        let finding = Finding {
            kind: sensitive::SensitiveKind::Email,
            preview: "masked".into(),
            rect: PxRect::new(0, 0, 10, 10),
        };
        let review = |on: bool| Review {
            findings: vec![finding.clone()],
            on: vec![on],
            hovered: None,
            style: RedactStyle::Box,
        };
        let r = review(true);
        let view = PanelView::Review(&r);
        assert_eq!(panel_key(Some(&view), true), PanelKey::Apply);
        assert_eq!(panel_key(Some(&view), false), PanelKey::Swallow);
        let none = review(false);
        let view = PanelView::Review(&none);
        assert_eq!(panel_key(Some(&view), true), PanelKey::Swallow);
        assert_eq!(
            panel_key(Some(&PanelView::Scanning), true),
            PanelKey::Swallow
        );
        assert_eq!(
            panel_key(Some(&PanelView::Scanning), false),
            PanelKey::Swallow
        );
        assert_eq!(panel_key(Some(&PanelView::Empty), true), PanelKey::Pass);
        assert_eq!(panel_key(None, true), PanelKey::Pass);
    }
}
