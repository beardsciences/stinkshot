//! Overlay chrome built from egui widgets: tool bar, action bar, size/preset chips, hint
//! bar, color and width popups. Layout follows docs/design/Main.dc.html. Widgets never
//! mutate overlay state directly; they push `UiAction`s that the overlay applies.

use egui::{
    Align2, Area, Color32, Context, FontId, Frame, Id, Margin, Order, Pos2, Rect, RichText, Sense,
    Vec2,
};

use super::paint::color32;
use super::selection::Ratio;
use super::shapes::Tool;
use crate::state::FinishAction;
use crate::theme::{self, Icon, Palette};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Popup {
    Color,
    Width,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UiAction {
    Tool(Tool),
    Finish(FinishAction),
    Cancel,
    Color([u8; 4]),
    Width(f32),
    Ratio(Option<Ratio>),
    OpenExact,
    ExactSize(i32, i32),
    CloseExact,
    TogglePopup(Popup),
    /// The toolbar grip (see `toolbar.rs`). Positions are egui points.
    Grip(GripEvent),
    /// Find Sensitive Info button: start a scan or close the panel.
    FindSensitive,
    /// Inside the Find Sensitive Info panel.
    Smart(super::smart_redact::SmartAction),
    /// Eyedropper button (or I): pick a color from the image.
    Eyedropper,
    /// A context bar option on the selected arrow or text.
    Shape(super::ops::ShapeOpt),
    /// The zoom chip: back to the fitted view.
    ZoomFit,
    /// Add Regions toggle (multi-region capture).
    ToggleAddRegions,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GripEvent {
    /// Drag started with the pointer at this position.
    Start(Pos2),
    /// Pointer moved to this position while dragging.
    Move(Pos2),
    End,
    /// Double click: dock under the selection again.
    Redock,
}

pub const WIDTH_PRESETS: [f32; 8] = [1.0, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 16.0];

pub const TOOL_GROUPS: [&[(Tool, Icon)]; 2] = [
    &[
        (Tool::Select, Icon::Select),
        (Tool::Arrow, Icon::Arrow),
        (Tool::Line, Icon::Line),
        (Tool::Rect, Icon::Rect),
        (Tool::Ellipse, Icon::Ellipse),
        (Tool::Pen, Icon::Pen),
        (Tool::Highlighter, Icon::Highlighter),
        (Tool::Text, Icon::Text),
        (Tool::Step, Icon::Step),
    ],
    &[
        (Tool::Blur, Icon::Blur),
        (Tool::Pixelate, Icon::Pixelate),
        (Tool::Redact, Icon::Redact),
        (Tool::Magnifier, Icon::Magnifier),
        (Tool::Callout, Icon::Callout),
        (Tool::Spotlight, Icon::Spotlight),
    ],
];

pub struct BarInfo {
    pub tool: Tool,
    pub color: [u8; 4],
    pub width: f32,
    /// The eyedropper is picking.
    pub picking: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct BarLayout {
    pub rect: Rect,
    pub color_btn: Rect,
    pub width_btn: Rect,
}

/// `lifted`: the toolbar is being dragged by its grip (deeper, softer shadow).
fn bar_frame(pal: &Palette, lifted: bool) -> Frame {
    let shadow = if lifted {
        egui::epaint::Shadow {
            offset: [0, 14],
            blur: 36,
            spread: 2,
            color: Color32::from_black_alpha(140),
        }
    } else {
        egui::epaint::Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(115),
        }
    };
    Frame::NONE
        .fill(pal.flyout.gamma_multiply(0.96))
        .stroke(egui::Stroke::new(1.0, pal.stroke))
        .corner_radius(10)
        .inner_margin(Margin::same(5))
        .shadow(shadow)
}

pub const GRIP_TIP: &str = "Drag to Move \u{b7} Double-Click to Dock";

/// Dotted grip at the tool bar's leading edge: drag moves the whole toolbar, double click
/// docks it under the selection again. Move cursor on hover and while dragging.
fn grip(ui: &mut egui::Ui, pal: &Palette, lifted: bool, out: &mut Vec<UiAction>) {
    let (r, resp) = ui.allocate_exact_size(Vec2::new(14.0, 34.0), Sense::click_and_drag());
    let active = resp.hovered() || resp.dragged() || lifted;
    if active {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
    }
    let alpha = if active { 0.75 } else { 0.4 };
    let dot = pal.text.gamma_multiply(alpha);
    // Two columns of three dots, centred.
    for col in [-1.0f32, 1.0] {
        for row in [-1.0f32, 0.0, 1.0] {
            ui.painter().circle_filled(
                Pos2::new(r.center().x + col * 2.5, r.center().y + row * 5.0),
                1.25,
                dot,
            );
        }
    }
    if resp.double_clicked() {
        out.push(UiAction::Grip(GripEvent::Redock));
    } else if resp.drag_started() {
        if let Some(p) = resp.interact_pointer_pos() {
            out.push(UiAction::Grip(GripEvent::Start(p)));
        }
    } else if resp.dragged()
        && let Some(p) = resp.interact_pointer_pos()
    {
        out.push(UiAction::Grip(GripEvent::Move(p)));
    }
    if resp.drag_stopped() {
        out.push(UiAction::Grip(GripEvent::End));
    }
    if !resp.dragged() {
        resp.on_hover_text(GRIP_TIP);
    }
}

fn separator(ui: &mut egui::Ui, pal: &Palette) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(9.0, 34.0), Sense::hover());
    let x = r.center().x;
    ui.painter().line_segment(
        [
            Pos2::new(x, r.center().y - 11.0),
            Pos2::new(x, r.center().y + 11.0),
        ],
        egui::Stroke::new(1.0, pal.text.gamma_multiply(0.12)),
    );
}

/// Tool bar: grip, design order, a separator between groups, color swatch and width
/// button. `lifted` = the toolbar is being dragged.
pub fn tool_bar(
    ctx: &Context,
    pos: Pos2,
    info: &BarInfo,
    pal: &Palette,
    lifted: bool,
    out: &mut Vec<UiAction>,
) -> BarLayout {
    let mut color_btn = Rect::NOTHING;
    let mut width_btn = Rect::NOTHING;
    let (dark, _) = theme::current(ctx);
    let resp = Area::new(Id::unique("stinkshot-ov-tools"))
        .order(Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            bar_frame(pal, lifted).show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(2.0, 0.0);
                ui.horizontal(|ui| {
                    grip(ui, pal, lifted, out);
                    for (gi, group) in TOOL_GROUPS.iter().enumerate() {
                        if gi > 0 {
                            separator(ui, pal);
                        }
                        for (tool, icon) in group.iter() {
                            if theme::widgets::icon_button(
                                ui,
                                *icon,
                                tool.label(),
                                info.tool == *tool,
                            )
                            .clicked()
                            {
                                out.push(UiAction::Tool(*tool));
                            }
                        }
                    }
                    separator(ui, pal);

                    // Color swatch.
                    let (r, resp) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::click());
                    let resp = resp.on_hover_text("Color (1-8)");
                    color_btn = r;
                    ui.painter().rect_filled(
                        r,
                        6.0,
                        theme::subtle_fill(dark, u8::from(resp.hovered())),
                    );
                    let c = r.center();
                    // Design: 18 px swatch, 2 px gap, 1.5 px ring in the ink colour.
                    ui.painter().circle_filled(
                        c,
                        9.0,
                        color32([info.color[0], info.color[1], info.color[2], 255]),
                    );
                    ui.painter()
                        .circle_stroke(c, 9.0, egui::Stroke::new(1.0, pal.stroke));
                    ui.painter()
                        .circle_stroke(c, 11.75, egui::Stroke::new(1.5, pal.text));
                    if resp.clicked() {
                        out.push(UiAction::TogglePopup(Popup::Color));
                    }
                    let tip = format!("Pick Color from Image ({})", super::keys::EYEDROPPER.label);
                    if theme::widgets::icon_button(ui, Icon::Eyedropper, &tip, info.picking)
                        .clicked()
                    {
                        out.push(UiAction::Eyedropper);
                    }

                    // Stroke width: painted like the icon buttons so hover/press match.
                    let (wr, wb) = ui.allocate_exact_size(Vec2::new(46.0, 34.0), Sense::click());
                    let level = if wb.is_pointer_button_down_on() {
                        2
                    } else {
                        u8::from(wb.hovered())
                    };
                    ui.painter()
                        .rect_filled(wr, 6.0, theme::subtle_fill(dark, level));
                    ui.painter().text(
                        wr.center(),
                        Align2::CENTER_CENTER,
                        format!("{} px", info.width.round() as i32),
                        egui::FontId::proportional(12.0),
                        pal.text,
                    );
                    let wb = wb.on_hover_text("Stroke Width ([ and ])");
                    width_btn = wb.rect;
                    if wb.clicked() {
                        out.push(UiAction::TogglePopup(Popup::Width));
                    }
                });
            });
        });
    BarLayout {
        rect: resp.response.rect,
        color_btn,
        width_btn,
    }
}

/// What the action bar shows.
#[derive(Clone, Copy, Debug)]
pub struct ActionBarState {
    /// An OCR language is installed (otherwise Find Sensitive Info and Copy Text are
    /// disabled and say why).
    pub ocr: bool,
    /// The Find Sensitive Info panel is showing.
    pub smart_open: bool,
    /// The Add Regions toggle and whether it is on (screen overlay only).
    pub add_regions: Option<bool>,
}

/// Action bar: Add Regions (screen overlay), Find Sensitive Info, OCR, Pin, Save, Cancel,
/// then the accent Copy button.
pub fn action_bar(
    ctx: &Context,
    pos: Pos2,
    pal: &Palette,
    st: ActionBarState,
    lifted: bool,
    out: &mut Vec<UiAction>,
) -> Rect {
    let ActionBarState {
        ocr,
        smart_open,
        add_regions,
    } = st;
    let resp = Area::new(Id::unique("stinkshot-ov-actions"))
        .order(Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            bar_frame(pal, lifted).show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(2.0, 0.0);
                ui.horizontal(|ui| {
                    if let Some(on) = add_regions
                        && theme::widgets::icon_button(
                            ui,
                            Icon::Regions,
                            "Add Regions (Shift+Drag)",
                            on,
                        )
                        .clicked()
                    {
                        out.push(UiAction::ToggleAddRegions);
                    }
                    let smart_tip = super::smart_redact::button_tip(ocr);
                    let r = ui
                        .add_enabled_ui(ocr, |ui| {
                            theme::widgets::icon_button(ui, Icon::Shield, &smart_tip, smart_open)
                        })
                        .inner;
                    if r.on_disabled_hover_text(&smart_tip).clicked() {
                        out.push(UiAction::FindSensitive);
                    }
                    let ocr_tip = if ocr {
                        "Copy Text (Ctrl+T)"
                    } else {
                        "Copy Text Needs an OCR Language in Windows Settings"
                    };
                    let r = ui
                        .add_enabled_ui(ocr, |ui| {
                            theme::widgets::icon_button(ui, Icon::Ocr, ocr_tip, false)
                        })
                        .inner;
                    if r.on_disabled_hover_text(ocr_tip).clicked() {
                        out.push(UiAction::Finish(FinishAction::CopyText));
                    }
                    if theme::widgets::icon_button(ui, Icon::Pin, "Pin to Screen (Ctrl+P)", false)
                        .clicked()
                    {
                        out.push(UiAction::Finish(FinishAction::Pin));
                    }
                    if theme::widgets::icon_button(ui, Icon::Save, "Save As (Ctrl+S)", false)
                        .clicked()
                    {
                        out.push(UiAction::Finish(FinishAction::SaveAs));
                    }
                    if theme::widgets::icon_button(ui, Icon::Close, "Cancel (Esc)", false).clicked()
                    {
                        out.push(UiAction::Cancel);
                    }
                    if theme::widgets::accent_button(ui, "Copy")
                        .on_hover_text("Copy to Clipboard (Enter)")
                        .clicked()
                    {
                        out.push(UiAction::Finish(FinishAction::Copy));
                    }
                });
            });
        });
    resp.response.rect
}

/// Popup anchored under (or above) a toolbar button.
fn popup_area(
    ctx: &Context,
    id: &str,
    anchor: Rect,
    screen: Rect,
    size_hint: Vec2,
    pal: &Palette,
    add: impl FnOnce(&mut egui::Ui),
) {
    let below = anchor.bottom() + 8.0 + size_hint.y <= screen.bottom();
    // Kept on the bar's monitor horizontally too (the bar can be dragged to an edge).
    let x = super::toolbar::popup_x(
        anchor.center().x,
        size_hint.x + 10.0,
        screen.left(),
        screen.right(),
        8.0,
    );
    let (pos, pivot) = if below {
        (Pos2::new(x, anchor.bottom() + 8.0), Align2::CENTER_TOP)
    } else {
        (Pos2::new(x, anchor.top() - 8.0), Align2::CENTER_BOTTOM)
    };
    Area::new(Id::unique(id))
        .order(Order::Foreground)
        .fixed_pos(pos)
        .pivot(pivot)
        .show(ctx, |ui| {
            bar_frame(pal, false).show(ui, add);
        });
}

/// One round swatch button (palette and recent colors).
fn swatch(
    ui: &mut egui::Ui,
    c: [u8; 4],
    current: [u8; 4],
    tip: String,
    pal: &Palette,
    out: &mut Vec<UiAction>,
) {
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::click());
    let resp = resp.on_hover_text(tip);
    if resp.hovered() {
        let (dark, _) = theme::current(ui.ctx());
        ui.painter()
            .rect_filled(r, 6.0, theme::subtle_fill(dark, 1));
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter()
        .circle_filled(r.center(), 9.0, color32([c[0], c[1], c[2], 255]));
    if c[..3] == current[..3] {
        ui.painter()
            .circle_stroke(r.center(), 11.5, egui::Stroke::new(1.5, pal.text));
    } else {
        ui.painter()
            .circle_stroke(r.center(), 9.0, egui::Stroke::new(1.0, pal.stroke));
    }
    if resp.clicked() {
        out.push(UiAction::Color(c));
    }
}

/// Palette popup: 8 swatches from config, then the recent colors (newest first).
#[allow(clippy::too_many_arguments)]
pub fn color_popup(
    ctx: &Context,
    anchor: Rect,
    screen: Rect,
    palette: &[[u8; 4]],
    recent: &[[u8; 4]],
    current: [u8; 4],
    pal: &Palette,
    out: &mut Vec<UiAction>,
) {
    let h = if recent.is_empty() { 44.0 } else { 86.0 };
    popup_area(
        ctx,
        "stinkshot-ov-color",
        anchor,
        screen,
        Vec2::new(8.0 * 34.0, h),
        pal,
        |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(2.0, 4.0);
            ui.horizontal(|ui| {
                for (i, c) in palette.iter().take(8).enumerate() {
                    swatch(ui, *c, current, format!("Color {}", i + 1), pal, out);
                }
            });
            if !recent.is_empty() {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::new(64.0, 34.0), Sense::hover());
                    ui.painter().text(
                        Pos2::new(r.left() + 8.0, r.center().y),
                        Align2::LEFT_CENTER,
                        "Recent",
                        FontId::proportional(12.0),
                        pal.text_secondary,
                    );
                    for c in recent {
                        let hex = super::hover::hex_string([c[0], c[1], c[2]]);
                        swatch(ui, *c, current, format!("Recent Color {hex}"), pal, out);
                    }
                });
            }
        },
    );
}

/// What the context bar above a selected arrow or text shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ContextBar {
    Text {
        bold: bool,
        pill: bool,
        size: f32,
    },
    Arrow {
        double: bool,
        head: stinkshot_core::annotate::ArrowHead,
        curved: bool,
    },
}

/// A 30x30 option button styled like the icon buttons, its glyph drawn by `glyph`.
fn opt_button(
    ui: &mut egui::Ui,
    tip: &str,
    selected: bool,
    glyph: impl FnOnce(&egui::Painter, Rect, Color32),
) -> egui::Response {
    let (dark, accent) = theme::current(ui.ctx());
    let pal = theme::palette(dark);
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::click());
    let hover = ui
        .ctx()
        .animate_bool_with_time(resp.id.with("hover"), resp.hovered(), 0.1);
    let bg = if selected {
        theme::accent_soft(accent, 56 + (24.0 * hover) as u8)
    } else if resp.is_pointer_button_down_on() {
        theme::subtle_fill(dark, 2)
    } else {
        theme::subtle_fill(dark, 1).gamma_multiply(hover)
    };
    ui.painter().rect_filled(r, 6.0, bg);
    if selected {
        ui.painter().rect_stroke(
            r,
            6.0,
            egui::Stroke::new(1.0, theme::accent_soft(accent, 140)),
            egui::StrokeKind::Inside,
        );
    }
    let ink = if selected {
        theme::accent_fg(accent, dark)
    } else {
        pal.text
    };
    glyph(ui.painter(), r.shrink(7.0), ink);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.on_hover_text(tip)
}

fn arrow_glyph(
    p: &egui::Painter,
    r: Rect,
    c: Color32,
    double: bool,
    head: stinkshot_core::annotate::ArrowHead,
) {
    use stinkshot_core::annotate::ArrowHead;
    let a = Pos2::new(r.left(), r.center().y);
    let b = Pos2::new(r.right(), r.center().y);
    let stroke = egui::Stroke::new(1.6, c);
    let head_at = |tip: Pos2, dir: f32| {
        let back = tip.x - dir * 6.0;
        let (l, rr) = (Pos2::new(back, tip.y - 4.5), Pos2::new(back, tip.y + 4.5));
        match head {
            ArrowHead::Open => {
                p.line_segment([l, tip], stroke);
                p.line_segment([rr, tip], stroke);
            }
            ArrowHead::Triangle => {
                p.add(egui::Shape::convex_polygon(
                    vec![tip, l, rr],
                    c,
                    egui::Stroke::NONE,
                ));
            }
            ArrowHead::Notched => {
                let n = Pos2::new(tip.x - dir * 4.0, tip.y);
                p.add(egui::Shape::convex_polygon(
                    vec![tip, l, n],
                    c,
                    egui::Stroke::NONE,
                ));
                p.add(egui::Shape::convex_polygon(
                    vec![tip, n, rr],
                    c,
                    egui::Stroke::NONE,
                ));
            }
        }
    };
    let (sa, sb) = (
        if double { Pos2::new(a.x + 4.0, a.y) } else { a },
        Pos2::new(b.x - 4.0, b.y),
    );
    p.line_segment([sa, sb], stroke);
    head_at(b, 1.0);
    if double {
        head_at(a, -1.0);
    }
}

/// Small floating bar of options for the selected arrow or text (or the text being typed),
/// above `anchor` (the shape's screen rect, points), below it when there is no room.
pub fn context_bar(
    ctx: &Context,
    anchor: Rect,
    screen: Rect,
    bar: ContextBar,
    pal: &Palette,
    out: &mut Vec<UiAction>,
) {
    use super::ops::ShapeOpt;
    use stinkshot_core::annotate::ArrowHead;
    let (w, h) = match bar {
        ContextBar::Text { .. } => (4.0 * 32.0 + 40.0, 40.0),
        ContextBar::Arrow { curved, .. } => {
            ((5.0 + f32::from(u8::from(curved))) * 32.0 + 22.0, 40.0)
        }
    };
    let above = anchor.top() - 10.0 - h >= screen.top() + 4.0;
    let y = if above {
        anchor.top() - 10.0 - h
    } else {
        (anchor.bottom() + 10.0).min(screen.bottom() - h - 4.0)
    };
    let x = (anchor.center().x - w / 2.0).clamp(
        screen.left() + 4.0,
        (screen.right() - w - 4.0).max(screen.left() + 4.0),
    );
    Area::new(Id::unique("stinkshot-ov-context"))
        .order(Order::Foreground)
        .fixed_pos(Pos2::new(x, y))
        .show(ctx, |ui| {
            Frame::NONE
                .fill(pal.flyout.gamma_multiply(0.97))
                .stroke(egui::Stroke::new(1.0, pal.stroke))
                .corner_radius(8)
                .inner_margin(Margin::same(4))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 4],
                    blur: 14,
                    spread: 0,
                    color: Color32::from_black_alpha(90),
                })
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(2.0, 0.0);
                    ui.horizontal(|ui| match bar {
                        ContextBar::Text { bold, pill, size } => {
                            let tip = format!(
                                "Smaller Text ({} px)",
                                super::shapes::step_text_size(size, false).round()
                            );
                            if opt_button(ui, &tip, false, |p, r, c| {
                                p.text(
                                    r.center(),
                                    Align2::CENTER_CENTER,
                                    "A\u{2212}",
                                    FontId::proportional(13.0),
                                    c,
                                );
                            })
                            .clicked()
                            {
                                out.push(UiAction::Shape(ShapeOpt::TextSmaller));
                            }
                            let (r, _) =
                                ui.allocate_exact_size(Vec2::new(30.0, 30.0), Sense::hover());
                            ui.painter().text(
                                r.center(),
                                Align2::CENTER_CENTER,
                                format!("{}", size.round() as i32),
                                FontId::proportional(12.0),
                                pal.text_secondary,
                            );
                            let tip = format!(
                                "Larger Text ({} px)",
                                super::shapes::step_text_size(size, true).round()
                            );
                            if opt_button(ui, &tip, false, |p, r, c| {
                                p.text(
                                    r.center(),
                                    Align2::CENTER_CENTER,
                                    "A+",
                                    FontId::proportional(15.0),
                                    c,
                                );
                            })
                            .clicked()
                            {
                                out.push(UiAction::Shape(ShapeOpt::TextLarger));
                            }
                            let tip = format!("Bold ({})", super::keys::BOLD.label);
                            if opt_button(ui, &tip, bold, |p, r, c| {
                                p.text(
                                    r.center(),
                                    Align2::CENTER_CENTER,
                                    "B",
                                    theme::semibold(15.0),
                                    c,
                                );
                            })
                            .clicked()
                            {
                                out.push(UiAction::Shape(ShapeOpt::Bold(!bold)));
                            }
                            if opt_button(ui, "Background Pill", pill, |p, r, c| {
                                let pr = Rect::from_center_size(
                                    r.center(),
                                    Vec2::new(r.width(), r.height() * 0.62),
                                );
                                p.rect_filled(pr, pr.height() / 2.0, c);
                                p.text(
                                    pr.center(),
                                    Align2::CENTER_CENTER,
                                    "T",
                                    theme::semibold(10.0),
                                    pal.flyout,
                                );
                            })
                            .clicked()
                            {
                                out.push(UiAction::Shape(ShapeOpt::Pill(!pill)));
                            }
                        }
                        ContextBar::Arrow {
                            double,
                            head,
                            curved,
                        } => {
                            if opt_button(ui, "One Head", !double, |p, r, c| {
                                arrow_glyph(p, r, c, false, head)
                            })
                            .clicked()
                            {
                                out.push(UiAction::Shape(ShapeOpt::Double(false)));
                            }
                            if opt_button(ui, "Heads at Both Ends", double, |p, r, c| {
                                arrow_glyph(p, r, c, true, head)
                            })
                            .clicked()
                            {
                                out.push(UiAction::Shape(ShapeOpt::Double(true)));
                            }
                            separator(ui, pal);
                            for (h, tip) in [
                                (ArrowHead::Notched, "Notched Head"),
                                (ArrowHead::Triangle, "Triangle Head"),
                                (ArrowHead::Open, "Open Head"),
                            ] {
                                if opt_button(ui, tip, head == h, |p, r, c| {
                                    arrow_glyph(p, r, c, false, h)
                                })
                                .clicked()
                                {
                                    out.push(UiAction::Shape(ShapeOpt::Head(h)));
                                }
                            }
                            if curved
                                && opt_button(
                                    ui,
                                    "Straighten (Double-Click the Curve Handle)",
                                    false,
                                    |p, r, c| {
                                        p.line_segment(
                                            [r.left_bottom(), r.right_top()],
                                            egui::Stroke::new(1.6, c),
                                        );
                                    },
                                )
                                .clicked()
                            {
                                out.push(UiAction::Shape(ShapeOpt::Straighten));
                            }
                        }
                    });
                });
        });
}

/// Stroke width popup with preset dots.
pub fn width_popup(
    ctx: &Context,
    anchor: Rect,
    screen: Rect,
    current: f32,
    pal: &Palette,
    accent: Color32,
    out: &mut Vec<UiAction>,
) {
    popup_area(
        ctx,
        "stinkshot-ov-width",
        anchor,
        screen,
        Vec2::new(8.0 * 34.0, 44.0),
        pal,
        |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(2.0, 0.0);
            ui.horizontal(|ui| {
                for w in WIDTH_PRESETS {
                    let (r, resp) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::click());
                    let resp = resp.on_hover_text(format!("{} px", w as i32));
                    let sel = (w - current).abs() < 0.5;
                    let (dark, _) = theme::current(ui.ctx());
                    if sel {
                        ui.painter()
                            .rect_filled(r, 6.0, theme::accent_soft(accent, 56));
                    } else if resp.hovered() {
                        ui.painter()
                            .rect_filled(r, 6.0, theme::subtle_fill(dark, 1));
                    }
                    ui.painter().circle_filled(
                        r.center(),
                        (w / 2.0).clamp(1.0, 9.0),
                        if sel {
                            theme::accent_fg(accent, dark)
                        } else {
                            pal.text
                        },
                    );
                    if resp.clicked() {
                        out.push(UiAction::Width(w));
                    }
                }
            });
        },
    );
}

pub struct ChipState<'a> {
    pub size: (i32, i32),
    pub ratio: Option<Ratio>,
    pub exact_open: bool,
    /// True on the first frame the exact editor is shown (requests focus).
    pub exact_just_opened: bool,
    pub exact_text: &'a mut String,
    pub purpose: Option<&'a str>,
    /// Aspect/exact chips only make sense while the selection can still be edited.
    pub editable: bool,
    /// Image zoom in percent while the view is zoomed or panned; clicking it fits again.
    pub zoom: Option<i32>,
}

const PRESETS: [(&str, Option<Ratio>); 4] = [
    ("Free", None),
    ("16:9", Some((16, 9))),
    ("4:3", Some((4, 3))),
    ("1:1", Some((1, 1))),
];

/// Size label and presets chip above the selection (design: "760 x 420" + "Free . 16:9 . 1:1 . Exact").
pub fn size_chips(
    ctx: &Context,
    pos: Pos2,
    st: &mut ChipState,
    pal: &Palette,
    accent: Color32,
    out: &mut Vec<UiAction>,
) {
    Area::new(Id::unique("stinkshot-ov-chips"))
        .order(Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 0.0);
            let accent_fg = theme::accent_fg(accent, theme::current(ctx).0);
            ui.horizontal(|ui| {
                let chip = |pal: &Palette| {
                    Frame::NONE
                        .fill(pal.flyout.gamma_multiply(0.94))
                        .stroke(egui::Stroke::new(1.0, pal.stroke))
                        .corner_radius(6)
                        .inner_margin(Margin::symmetric(9, 4))
                };
                chip(pal).show(ui, |ui| {
                    ui.label(
                        RichText::new(format!("{} \u{d7} {}", st.size.0, st.size.1))
                            .size(12.0)
                            .color(pal.text),
                    );
                });
                if let Some(z) = st.zoom {
                    let r = chip(pal)
                        .show(ui, |ui| {
                            ui.label(RichText::new(format!("{z}%")).size(12.0).color(accent_fg));
                        })
                        .response
                        .interact(Sense::click())
                        .on_hover_text(format!(
                            "Zoom \u{b7} Click to Fit ({})",
                            super::keys::ZOOM_FIT.label
                        ));
                    if r.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if r.clicked() {
                        out.push(UiAction::ZoomFit);
                    }
                }
                if st.editable {
                    chip(pal).show(ui, |ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(4.0, 0.0);
                        ui.horizontal(|ui| {
                            if st.exact_open {
                                let te = egui::TextEdit::singleline(st.exact_text)
                                    .desired_width(92.0)
                                    .hint_text("1280 \u{d7} 720")
                                    .font(egui::FontId::proportional(12.0));
                                let resp = ui.add(te);
                                if st.exact_just_opened {
                                    resp.request_focus();
                                }
                                let enter = resp.lost_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                if enter {
                                    match super::selection::parse_exact_size(st.exact_text) {
                                        Some((w, h)) => out.push(UiAction::ExactSize(w, h)),
                                        None => out.push(UiAction::CloseExact),
                                    }
                                } else if resp.lost_focus() {
                                    out.push(UiAction::CloseExact);
                                }
                                return;
                            }
                            for (i, (label, ratio)) in PRESETS.iter().enumerate() {
                                if i > 0 {
                                    ui.label(
                                        RichText::new("\u{b7}").size(12.0).color(pal.text_tertiary),
                                    );
                                }
                                let active = st.ratio == *ratio;
                                let color = if active {
                                    accent_fg
                                } else {
                                    pal.text_secondary
                                };
                                let r = ui.add(
                                    egui::Label::new(RichText::new(*label).size(12.0).color(color))
                                        .sense(Sense::click()),
                                );
                                if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                    out.push(UiAction::Ratio(*ratio));
                                }
                            }
                            ui.label(RichText::new("\u{b7}").size(12.0).color(pal.text_tertiary));
                            let r = ui.add(
                                egui::Label::new(
                                    RichText::new("Exact").size(12.0).color(pal.text_secondary),
                                )
                                .sense(Sense::click()),
                            );
                            if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                out.push(UiAction::OpenExact);
                            }
                        });
                    });
                }
                if let Some(p) = st.purpose {
                    chip(pal).show(ui, |ui| {
                        ui.label(RichText::new(p).size(12.0).color(accent_fg));
                    });
                }
            });
        });
}

/// Top-center hint bar; `items` are (key, action) pairs with the key emphasised.
pub fn hint_bar(
    ctx: &Context,
    center_top: Pos2,
    leading: Option<&str>,
    items: &[(&str, &str)],
    pal: &Palette,
    accent: Color32,
) {
    Area::new(Id::unique("stinkshot-ov-hint"))
        .order(Order::Foreground)
        .fixed_pos(center_top)
        .pivot(Align2::CENTER_TOP)
        .interactable(false)
        .show(ctx, |ui| {
            Frame::NONE
                .fill(pal.flyout.gamma_multiply(0.9))
                .stroke(egui::Stroke::new(1.0, pal.stroke))
                .corner_radius(8)
                .inner_margin(Margin::symmetric(12, 6))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(14.0, 0.0);
                    ui.horizontal(|ui| {
                        let accent_fg = theme::accent_fg(accent, theme::current(ui.ctx()).0);
                        if let Some(l) = leading {
                            ui.label(
                                RichText::new(l)
                                    .font(theme::semibold(12.0))
                                    .color(accent_fg),
                            );
                        }
                        for (k, v) in items {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                ui.label(
                                    RichText::new(*k)
                                        .font(theme::semibold(12.0))
                                        .color(pal.text),
                                );
                                ui.label(RichText::new(*v).size(12.0).color(pal.text_secondary));
                            });
                        }
                    });
                });
        });
}

/// Loupe info plate: coordinates, scale, hex swatch. `flash` = (text, ok) right after C:
/// "Copied #CE9178" replaces the "C to Copy" hint for a moment.
#[allow(clippy::too_many_arguments)]
pub fn loupe_plate(
    ctx: &Context,
    pos: Pos2,
    xy: (i32, i32),
    scale_pct: i32,
    hex: &str,
    rgb: [u8; 3],
    flash: Option<(&str, bool)>,
    pal: &Palette,
    accent: Color32,
) {
    let (dark, _) = theme::current(ctx);
    Area::new(Id::unique("stinkshot-ov-loupe-plate"))
        .order(Order::Tooltip)
        .fixed_pos(pos)
        .interactable(false)
        .show(ctx, |ui| {
            Frame::NONE
                .fill(pal.flyout.gamma_multiply(0.94))
                .stroke(egui::Stroke::new(1.0, pal.stroke))
                .corner_radius(8)
                .inner_margin(Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(0.0, 3.0);
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            ui.label(
                                RichText::new(format!("X {} \u{b7} Y {}", xy.0, xy.1))
                                    .size(12.0)
                                    .color(pal.text),
                            );
                            ui.label(
                                RichText::new(format!("@ {scale_pct}%"))
                                    .size(12.0)
                                    .color(pal.text_tertiary),
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                            ui.painter().rect_filled(
                                r,
                                2.0,
                                Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
                            );
                            match flash {
                                Some((text, ok)) => {
                                    let col = if ok {
                                        theme::accent_fg(accent, dark)
                                    } else {
                                        pal.danger
                                    };
                                    ui.label(
                                        RichText::new(text).font(theme::semibold(12.0)).color(col),
                                    );
                                }
                                None => {
                                    ui.label(RichText::new(hex).size(12.0).color(pal.text));
                                    ui.label(
                                        RichText::new("C to Copy")
                                            .size(12.0)
                                            .color(pal.text_tertiary),
                                    );
                                }
                            }
                        });
                    });
                });
        });
}

/// Element label chip next to the hover highlight (same look as the size chip).
pub fn element_chip(ctx: &Context, pos: Pos2, text: &str, pal: &Palette) {
    Area::new(Id::unique("stinkshot-ov-element"))
        .order(Order::Foreground)
        .fixed_pos(pos)
        .interactable(false)
        .show(ctx, |ui| {
            Frame::NONE
                .fill(pal.flyout.gamma_multiply(0.94))
                .stroke(egui::Stroke::new(1.0, pal.stroke))
                .corner_radius(6)
                .inner_margin(Margin::symmetric(9, 4))
                .show(ui, |ui| {
                    ui.label(RichText::new(text).size(12.0).color(pal.text));
                });
        });
}

/// Columns per row for groups of the given widths: all in one row when they fit in `max_w`,
/// else half per row (2x2 for four groups), else one per row.
pub fn sheet_columns(widths: &[f32], gap: f32, max_w: f32) -> usize {
    let n = widths.len().max(1);
    let fits = |cols: usize| {
        widths.chunks(cols).all(|row| {
            row.iter().sum::<f32>() + gap * (row.len().saturating_sub(1)) as f32 <= max_w
        })
    };
    let half = n.div_ceil(2);
    if fits(n) {
        n
    } else if half < n && fits(half) {
        half
    } else {
        1
    }
}

/// F1 / ? cheat sheet: the overlay's shortcuts in columns sized to their text, centered on the
/// monitor. Four columns wrap to two rows when the monitor (`max_w` logical px) is too narrow.
pub fn shortcut_sheet(
    ctx: &Context,
    center: Pos2,
    max_w: f32,
    groups: &super::keys::Sheet,
    pal: &Palette,
    accent: Color32,
) {
    const KEY_GAP: f32 = 10.0;
    const COL_GAP: f32 = 18.0;
    const MARGIN_X: i8 = 22;
    let key_font = theme::semibold(12.0);
    let what_font = FontId::proportional(12.0);
    Area::new(Id::unique("stinkshot-ov-shortcuts"))
        .order(Order::Tooltip)
        .fixed_pos(center)
        .pivot(Align2::CENTER_CENTER)
        .interactable(false)
        .show(ctx, |ui| {
            // Column widths from the text itself: (key column, whole column) per group.
            let measure = |t: &str, f: &FontId| {
                ui.painter()
                    .layout_no_wrap(t.to_owned(), f.clone(), pal.text)
                    .size()
                    .x
                    .ceil()
            };
            let cols: Vec<(f32, f32)> = groups
                .iter()
                .map(|(title, rows)| {
                    let key_w = rows
                        .iter()
                        .map(|(k, _)| measure(k, &key_font))
                        .fold(0.0_f32, f32::max);
                    let what_w = rows
                        .iter()
                        .map(|(_, w)| measure(w, &what_font))
                        .fold(0.0_f32, f32::max);
                    let title_w = measure(title, &theme::semibold(13.0));
                    (key_w, (key_w + KEY_GAP + what_w).max(title_w))
                })
                .collect();
            let widths: Vec<f32> = cols.iter().map(|c| c.1).collect();
            let room = max_w - 32.0 - 2.0 * MARGIN_X as f32;
            let per_row = sheet_columns(&widths, COL_GAP, room);
            Frame::NONE
                .fill(pal.flyout.gamma_multiply(0.97))
                .stroke(egui::Stroke::new(1.0, pal.stroke))
                .corner_radius(10)
                .inner_margin(Margin::symmetric(MARGIN_X, 18))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 8],
                    blur: 24,
                    spread: 0,
                    color: Color32::from_black_alpha(115),
                })
                .show(ui, |ui| {
                    let accent_fg = theme::accent_fg(accent, theme::current(ui.ctx()).0);
                    ui.spacing_mut().item_spacing = Vec2::new(0.0, 6.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Keyboard Shortcuts")
                                .font(theme::semibold(15.0))
                                .color(pal.text),
                        );
                        ui.add_space(14.0);
                        ui.label(
                            RichText::new("F1, ? or Esc to Close")
                                .size(12.0)
                                .color(pal.text_tertiary),
                        );
                    });
                    ui.add_space(8.0);
                    let indexed: Vec<usize> = (0..groups.len()).collect();
                    for (row_i, row) in indexed.chunks(per_row).enumerate() {
                        if row_i > 0 {
                            ui.add_space(12.0);
                        }
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(COL_GAP, 0.0);
                            for &gi in row {
                                let (title, rows) = &groups[gi];
                                let (key_w, col_w) = cols[gi];
                                ui.vertical(|ui| {
                                    ui.set_width(col_w);
                                    ui.spacing_mut().item_spacing = Vec2::new(0.0, 5.0);
                                    ui.label(
                                        RichText::new(*title)
                                            .font(theme::semibold(13.0))
                                            .color(accent_fg),
                                    );
                                    ui.add_space(2.0);
                                    for (keys, what) in rows {
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = KEY_GAP;
                                            let (r, _) = ui.allocate_exact_size(
                                                Vec2::new(key_w, 16.0),
                                                Sense::hover(),
                                            );
                                            ui.painter().text(
                                                r.left_center(),
                                                Align2::LEFT_CENTER,
                                                keys,
                                                key_font.clone(),
                                                pal.text,
                                            );
                                            ui.label(
                                                RichText::new(*what)
                                                    .font(what_font.clone())
                                                    .color(pal.text_secondary),
                                            );
                                        });
                                    }
                                });
                            }
                        });
                    }
                });
        });
}

/// Small centered pill ("Saving...") shown while the final flatten runs.
pub fn busy_pill(ctx: &Context, center: Pos2, pal: &Palette) {
    Area::new(Id::unique("stinkshot-ov-busy"))
        .order(Order::Tooltip)
        .fixed_pos(center)
        .pivot(Align2::CENTER_CENTER)
        .interactable(false)
        .show(ctx, |ui| {
            Frame::NONE
                .fill(pal.flyout.gamma_multiply(0.96))
                .stroke(egui::Stroke::new(1.0, pal.stroke))
                .corner_radius(8)
                .inner_margin(Margin::symmetric(14, 8))
                .show(ui, |ui| {
                    ui.label(RichText::new("Finishing...").size(13.0).color(pal.text));
                });
        });
}

#[cfg(test)]
mod sheet_tests {
    use super::sheet_columns;

    #[test]
    fn sheet_wraps_to_two_rows_on_narrow_monitors() {
        let w = [236.0, 236.0, 236.0, 236.0];
        // 1920 px at 100 %: one row.
        assert_eq!(sheet_columns(&w, 18.0, 1920.0 - 76.0), 4);
        // 1280 px at 125 % or 1920 px at 200 %: 2x2.
        assert_eq!(sheet_columns(&w, 18.0, 1024.0 - 76.0), 2);
        assert_eq!(sheet_columns(&w, 18.0, 960.0 - 76.0), 2);
        // Absurdly narrow: one column.
        assert_eq!(sheet_columns(&w, 18.0, 300.0), 1);
        // The region-only sheet has two groups.
        assert_eq!(sheet_columns(&w[..2], 18.0, 960.0 - 76.0), 2);
    }
}
