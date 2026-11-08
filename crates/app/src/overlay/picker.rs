//! Color picker. Standalone (Pick Color from the tray or its hotkey): the screen freezes, the
//! loupe follows the cursor, a click copies the pixel's color in the chosen format (Hex, RGB,
//! HSL; Tab cycles) and closes. A small panel under the hint bar holds the format choice and
//! the recent picks (click one to copy it again). In any overlay, C copies the color under
//! the cursor in the same format and adds it to the recent picks.

use egui::{Align2, Area, RichText};
use stinkshot_core::color::ColorFormat;

use super::panels;
use super::*;
use crate::theme::Palette;

/// Recent picks kept in `[editor] picker_recent`.
pub const RECENT_PICKS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerAction {
    Format(ColorFormat),
    /// Copy a recent pick (index) and close.
    Recent(usize),
}

/// Push `hex` to the front of the recent picks (no duplicates, at most `RECENT_PICKS`).
pub fn push_pick(recent: &mut Vec<String>, rgb: [u8; 3]) {
    let hex = ColorFormat::Hex.format(rgb);
    recent.retain(|h| !h.eq_ignore_ascii_case(&hex));
    recent.insert(0, hex);
    recent.truncate(RECENT_PICKS);
}

pub fn next_format(f: ColorFormat) -> ColorFormat {
    let all = ColorFormat::ALL;
    let i = all.iter().position(|x| *x == f).unwrap_or(0);
    all[(i + 1) % all.len()]
}

impl Overlay {
    pub(super) fn picker_mode(&self) -> bool {
        self.kind == Kind::Select(RegionPurpose::PickColor)
    }

    pub(super) fn picker_format(shared: &Shared) -> ColorFormat {
        ColorFormat::from_config(&shared.config.editor.picker_format)
    }

    /// Copy `rgb` as text in the chosen format and remember it. Returns the copied text.
    pub(super) fn copy_color(&mut self, shared: &mut Shared, rgb: [u8; 3]) -> Option<String> {
        let text = Self::picker_format(shared).format(rgb);
        let ok = match clipboard::set_text(&text) {
            Ok(()) => true,
            Err(e) => {
                log::warn!("copy color: {e:#}");
                false
            }
        };
        if ok {
            push_pick(&mut shared.config.editor.picker_recent, rgb);
            shared.config_dirty = true;
        }
        self.hex_flash = Some(HexFlash {
            hex: text.clone(),
            ok,
            at: Instant::now(),
        });
        ok.then_some(text)
    }

    /// Standalone picker: copy and close (a toast confirms once the screen is back).
    pub(super) fn pick_and_close(&mut self, shared: &mut Shared, rgb: [u8; 3]) {
        if let Some(text) = self.copy_color(shared, rgb) {
            shared.outbox.push(Command::Notify {
                title: "Color Copied".into(),
                body: text,
                error: false,
            });
            self.cancel(shared);
        }
    }

    pub(super) fn apply_picker_action(&mut self, shared: &mut Shared, a: PickerAction) {
        match a {
            PickerAction::Format(f) => {
                if shared.config.editor.picker_format != f.config_name() {
                    shared.config.editor.picker_format = f.config_name().into();
                    shared.config_dirty = true;
                }
            }
            PickerAction::Recent(i) => {
                let Some(c) = shared
                    .config
                    .editor
                    .picker_recent
                    .get(i)
                    .and_then(|h| parse_hex_color(h))
                else {
                    return;
                };
                self.pick_and_close(shared, [c[0], c[1], c[2]]);
            }
        }
    }

    pub(super) fn cycle_picker_format(&mut self, shared: &mut Shared) {
        let f = next_format(Self::picker_format(shared));
        self.apply_picker_action(shared, PickerAction::Format(f));
    }
}

/// Format chips and recent swatches, centred under the hint bar.
pub fn panel(
    ctx: &Context,
    pos: Pos2,
    format: ColorFormat,
    recent: &[String],
    pal: &Palette,
    out: &mut Vec<PickerAction>,
) {
    Area::new(egui::Id::unique("stinkshot-ov-picker"))
        .order(Order::Foreground)
        .fixed_pos(pos)
        .pivot(Align2::CENTER_TOP)
        .show(ctx, |ui| {
            panels::panel_frame(pal).show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Copy As")
                            .size(12.5)
                            .color(pal.text_secondary),
                    );
                    for f in ColorFormat::ALL {
                        if panels::chip(ui, f.label(), f == format)
                            .on_hover_text("Tab switches format")
                            .clicked()
                        {
                            out.push(PickerAction::Format(f));
                        }
                    }
                    if !recent.is_empty() {
                        ui.add_space(8.0);
                        ui.label(RichText::new("Recent").size(12.5).color(pal.text_secondary));
                        for (i, h) in recent.iter().enumerate() {
                            let Some(c) = parse_hex_color(h) else {
                                continue;
                            };
                            let (r, resp) =
                                ui.allocate_exact_size(Vec2::splat(22.0), egui::Sense::click());
                            ui.painter().circle_filled(
                                r.center(),
                                8.0,
                                paint::color32([c[0], c[1], c[2], 255]),
                            );
                            let ring = if resp.hovered() { pal.text } else { pal.stroke };
                            ui.painter().circle_stroke(
                                r.center(),
                                9.0,
                                egui::Stroke::new(1.2, ring),
                            );
                            if resp.hovered() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            let tip = format!("Copy {}", format.format([c[0], c[1], c[2]]));
                            if resp.on_hover_text(tip).clicked() {
                                out.push(PickerAction::Recent(i));
                            }
                        }
                    }
                });
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_picks_dedupe_and_cap() {
        let mut r = Vec::new();
        for i in 0..10u8 {
            push_pick(&mut r, [i, 0, 0]);
        }
        assert_eq!(r.len(), RECENT_PICKS);
        assert_eq!(r[0], "#090000");
        push_pick(&mut r, [5, 0, 0]);
        assert_eq!(r[0], "#050000");
        assert_eq!(r.iter().filter(|h| *h == "#050000").count(), 1);
        assert_eq!(r.len(), RECENT_PICKS);
    }

    #[test]
    fn tab_cycles_formats() {
        assert_eq!(next_format(ColorFormat::Hex), ColorFormat::Rgb);
        assert_eq!(next_format(ColorFormat::Rgb), ColorFormat::Hsl);
        assert_eq!(next_format(ColorFormat::Hsl), ColorFormat::Hex);
    }
}
