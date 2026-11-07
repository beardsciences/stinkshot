//! Overlay chrome built from egui widgets: tool bar, action bar, size/preset chips, hint
//! bar, color and width popups. Layout follows docs/design/Main.dc.html. Widgets never
//! mutate overlay state directly; they push `UiAction`s that the overlay applies.

use egui::{
    Align2, Area, Color32, Context, Frame, Id, Margin, Order, Pos2, Rect, RichText, Sense, Vec2,
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
}

#[derive(Clone, Copy, Debug)]
pub struct BarLayout {
    pub rect: Rect,
    pub color_btn: Rect,
    pub width_btn: Rect,
}

fn bar_frame(pal: &Palette) -> Frame {
    Frame::NONE
        .fill(pal.flyout.gamma_multiply(0.96))
        .stroke(egui::Stroke::new(1.0, pal.stroke))
        .corner_radius(10)
        .inner_margin(Margin::same(5))
        .shadow(egui::epaint::Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: Color32::from_black_alpha(115),
        })
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

/// Tool bar: design order, a separator between groups, color swatch and width button.
pub fn tool_bar(
    ctx: &Context,
    pos: Pos2,
    info: &BarInfo,
    pal: &Palette,
    accent: Color32,
    out: &mut Vec<UiAction>,
) -> BarLayout {
    let mut color_btn = Rect::NOTHING;
    let mut width_btn = Rect::NOTHING;
    let resp = Area::new(Id::unique("stinkshot-ov-tools"))
        .order(Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            bar_frame(pal).show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(2.0, 0.0);
                ui.horizontal(|ui| {
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
                    let hovered = resp.hovered();
                    if hovered {
                        ui.painter()
                            .rect_filled(r, 6.0, pal.text.gamma_multiply(0.08));
                    }
                    let c = r.center();
                    ui.painter().circle_filled(
                        c,
                        9.0,
                        color32([info.color[0], info.color[1], info.color[2], 255]),
                    );
                    ui.painter()
                        .circle_stroke(c, 10.8, egui::Stroke::new(1.5, pal.text));
                    if resp.clicked() {
                        out.push(UiAction::TogglePopup(Popup::Color));
                    }

                    // Stroke width.
                    let label = RichText::new(format!("{} px", info.width.round() as i32))
                        .size(12.0)
                        .color(pal.text);
                    let wb = ui.add(
                        egui::Button::new(label)
                            .fill(Color32::TRANSPARENT)
                            .stroke(egui::Stroke::NONE)
                            .corner_radius(6)
                            .min_size(Vec2::new(46.0, 34.0)),
                    );
                    let wb = wb.on_hover_text("Stroke Width ([ and ])");
                    width_btn = wb.rect;
                    if wb.clicked() {
                        out.push(UiAction::TogglePopup(Popup::Width));
                    }
                });
            });
        });
    let _ = accent;
    BarLayout {
        rect: resp.response.rect,
        color_btn,
        width_btn,
    }
}

/// Action bar: OCR, Pin, Save, Cancel, then the accent Copy button. `ocr` = an OCR language
/// is installed (otherwise Copy Text is disabled and says why).
pub fn action_bar(
    ctx: &Context,
    pos: Pos2,
    pal: &Palette,
    ocr: bool,
    out: &mut Vec<UiAction>,
) -> Rect {
    let resp = Area::new(Id::unique("stinkshot-ov-actions"))
        .order(Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            bar_frame(pal).show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(2.0, 0.0);
                ui.horizontal(|ui| {
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
    let (pos, pivot) = if below {
        (
            Pos2::new(anchor.center().x, anchor.bottom() + 8.0),
            Align2::CENTER_TOP,
        )
    } else {
        (
            Pos2::new(anchor.center().x, anchor.top() - 8.0),
            Align2::CENTER_BOTTOM,
        )
    };
    Area::new(Id::unique(id))
        .order(Order::Foreground)
        .fixed_pos(pos)
        .pivot(pivot)
        .show(ctx, |ui| {
            bar_frame(pal).show(ui, add);
        });
}

/// Palette popup (8 swatches from config).
pub fn color_popup(
    ctx: &Context,
    anchor: Rect,
    screen: Rect,
    palette: &[[u8; 4]],
    current: [u8; 4],
    pal: &Palette,
    out: &mut Vec<UiAction>,
) {
    popup_area(
        ctx,
        "stinkshot-ov-color",
        anchor,
        screen,
        Vec2::new(8.0 * 34.0, 44.0),
        pal,
        |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(2.0, 0.0);
            ui.horizontal(|ui| {
                for (i, c) in palette.iter().take(8).enumerate() {
                    let (r, resp) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::click());
                    let resp = resp.on_hover_text(format!("Color {}", i + 1));
                    if resp.hovered() {
                        ui.painter()
                            .rect_filled(r, 6.0, pal.text.gamma_multiply(0.08));
                    }
                    ui.painter()
                        .circle_filled(r.center(), 9.0, color32([c[0], c[1], c[2], 255]));
                    if c[..3] == current[..3] {
                        ui.painter().circle_stroke(
                            r.center(),
                            11.5,
                            egui::Stroke::new(1.5, pal.text),
                        );
                    } else {
                        ui.painter().circle_stroke(
                            r.center(),
                            9.0,
                            egui::Stroke::new(1.0, Color32::from_white_alpha(40)),
                        );
                    }
                    if resp.clicked() {
                        out.push(UiAction::Color(*c));
                    }
                }
            });
        },
    );
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
                    if sel {
                        ui.painter()
                            .rect_filled(r, 6.0, accent.gamma_multiply(0.18));
                    } else if resp.hovered() {
                        ui.painter()
                            .rect_filled(r, 6.0, pal.text.gamma_multiply(0.08));
                    }
                    ui.painter().circle_filled(
                        r.center(),
                        (w / 2.0).clamp(1.0, 9.0),
                        if sel { accent } else { pal.text },
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
    pub purpose: Option<&'static str>,
    /// Aspect/exact chips only make sense while the selection can still be edited.
    pub editable: bool,
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
                            .color(pal.text)
                            .monospace(),
                    );
                });
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
                                let color = if active { accent } else { pal.text_secondary };
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
                        ui.label(RichText::new(p).size(12.0).color(accent));
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
                        if let Some(l) = leading {
                            ui.label(RichText::new(l).size(12.0).strong().color(accent));
                        }
                        for (k, v) in items {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                ui.label(RichText::new(*k).size(12.0).strong().color(pal.text));
                                ui.label(RichText::new(*v).size(12.0).color(pal.text_secondary));
                            });
                        }
                    });
                });
        });
}

/// Loupe info plate: coordinates, scale, hex swatch.
pub fn loupe_plate(
    ctx: &Context,
    pos: Pos2,
    xy: (i32, i32),
    scale_pct: i32,
    hex: &str,
    rgb: [u8; 3],
    pal: &Palette,
) {
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
                                    .color(pal.text)
                                    .monospace(),
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
                            ui.label(RichText::new(hex).size(12.0).color(pal.text).monospace());
                            ui.label(
                                RichText::new("C to Copy")
                                    .size(12.0)
                                    .color(pal.text_tertiary),
                            );
                        });
                    });
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
