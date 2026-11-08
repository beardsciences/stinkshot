//! Shared look for the overlay's floating panels (Codes and Text, Beautify, the color
//! picker): flyout frame, header with a close button, text chips for small choices.

use egui::{Color32, Frame, Margin, RichText, Sense, Vec2};

use crate::theme::{self, Palette};

pub fn panel_frame(pal: &Palette) -> Frame {
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
}

/// Title on the left, a close glyph on the right. True when close was clicked.
pub fn header(ui: &mut egui::Ui, title: &str, pal: &Palette) -> bool {
    let mut close = false;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(title)
                .font(theme::semibold(13.0))
                .color(pal.text),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui.add_sized(
                Vec2::splat(22.0),
                egui::Label::new("").sense(Sense::click()),
            );
            theme::widgets::paint_icon(
                ui.painter(),
                r.rect.shrink(4.0),
                theme::Icon::Close,
                if r.hovered() {
                    pal.text
                } else {
                    pal.text_secondary
                },
            );
            if r.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            close = r.on_hover_text("Close (Esc)").clicked();
        });
    });
    close
}

/// A small selectable text chip (segmented choices).
pub fn chip(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    let (dark, accent) = theme::current(ui.ctx());
    let pal = theme::palette(dark);
    let font = egui::FontId::proportional(12.5);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, pal.text);
    let size = Vec2::new(galley.size().x + 18.0, 26.0);
    let (r, resp) = ui.allocate_exact_size(size, Sense::click());
    let bg = if selected {
        theme::accent_soft(accent, 70)
    } else if resp.hovered() {
        theme::subtle_fill(dark, 1)
    } else {
        theme::subtle_fill(dark, 1).gamma_multiply(0.5)
    };
    ui.painter().rect_filled(r, 6.0, bg);
    if selected {
        ui.painter().rect_stroke(
            r,
            6.0,
            egui::Stroke::new(1.0, theme::accent_soft(accent, 150)),
            egui::StrokeKind::Inside,
        );
    }
    let ink = if !ui.is_enabled() {
        pal.text_tertiary
    } else if selected {
        theme::accent_fg(accent, dark)
    } else {
        pal.text
    };
    ui.painter()
        .galley(r.center() - galley.size() / 2.0, galley, ink);
    if resp.hovered() && ui.is_enabled() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// Section caption (sentence case is not used: these are labels).
pub fn caption(ui: &mut egui::Ui, text: &str, pal: &Palette) {
    ui.label(RichText::new(text).size(12.0).color(pal.text_secondary));
}
