//! Beautify panel (F): padding, background (solid, gradient, blurred capture), rounded
//! corners, shadow and canvas aspect for sharing. A small live preview shows the result; the
//! frame is applied to the flattened image when copying, saving or pinning (never to Copy
//! Text), on the finishing worker. Settings are remembered in `[editor.beautify]`.

use egui::{Align2, Area, RichText};
use stinkshot_core::beautify::{self, Aspect, BackgroundKind, Beautify};
use stinkshot_core::config::parse_hex_color;

use super::panels;
use super::*;
use crate::theme::Palette;

/// Preview size limit, egui points (also px of the source thumbnail).
const PREVIEW_W: u32 = 296;
const PREVIEW_H: u32 = 170;
const PANEL_W: f32 = 320.0;

#[derive(Clone, Debug, PartialEq)]
pub enum BeautifyAction {
    Set(Beautify),
    Close,
}

/// Source thumbnail of the capture and the preview built from it.
pub struct Preview {
    /// Base the thumbnail came from, and the doc it showed (None = unannotated base).
    base: Arc<Rgba>,
    doc: Option<Doc>,
    thumb: Rgba,
    /// Thumbnail px per capture px.
    k: f32,
    shown: Option<(Beautify, egui::TextureHandle)>,
}

/// The settings scaled to a thumbnail (padding and corners shrink with it).
pub fn scaled(b: &Beautify, k: f32) -> Beautify {
    Beautify {
        padding: (b.padding as f32 * k).round() as u32,
        corner_radius: b.corner_radius * k,
        ..b.clone()
    }
}

impl Overlay {
    pub(super) fn toggle_beautify(&mut self, shared: &mut Shared) {
        if self.beautify_open {
            self.close_beautify(shared);
            return;
        }
        if !self.annotating() || self.sel.is_none() {
            return;
        }
        self.commit_text();
        self.popup = None;
        self.smart = None;
        self.scan = None;
        self.beautify_open = true;
    }

    pub(super) fn close_beautify(&mut self, shared: &mut Shared) {
        self.beautify_open = false;
        self.beautify_preview = None;
        self.persist_beautify(shared);
    }

    pub(super) fn persist_beautify(&self, shared: &mut Shared) {
        if shared.config.editor.beautify != self.beautify {
            shared.config.editor.beautify = self.beautify.clone();
            shared.config_dirty = true;
        }
    }

    pub(super) fn apply_beautify_action(&mut self, shared: &mut Shared, a: BeautifyAction) {
        match a {
            BeautifyAction::Set(b) => self.beautify = b,
            BeautifyAction::Close => self.close_beautify(shared),
        }
    }

    /// Keep the preview in step with the capture (annotations included once the render
    /// worker has them) and the settings. Cheap when nothing changed.
    pub(super) fn update_beautify_preview(&mut self, ctx: &Context) {
        if !self.beautify_open {
            return;
        }
        let Some(base) = self.base.clone() else {
            return;
        };
        let busy = !matches!(self.drag, Drag::None | Drag::Ignore);
        let stale_base = self
            .beautify_preview
            .as_ref()
            .is_none_or(|p| !Arc::ptr_eq(&p.base, &base));
        let stale_doc = self.beautify_preview.as_ref().is_some_and(|p| {
            p.doc.as_ref() != Some(&self.doc) && !(self.doc.shapes.is_empty() && p.doc.is_none())
        });
        if stale_base || (stale_doc && !busy) {
            let annotated = if self.doc.shapes.is_empty() {
                None
            } else {
                self.renderer.as_ref().and_then(|r| r.image_for(&self.doc))
            };
            // Keep the old thumbnail until the render of the new doc is ready.
            if stale_base || annotated.is_some() || self.doc.shapes.is_empty() {
                let doc = annotated.as_ref().map(|_| self.doc.clone());
                let src = annotated.as_ref().unwrap_or(&base);
                let thumb = beautify::preview_thumbnail(src, PREVIEW_W, PREVIEW_H);
                let k = thumb.width as f32 / src.width.max(1) as f32;
                self.beautify_preview = Some(Preview {
                    base: base.clone(),
                    doc,
                    thumb,
                    k,
                    shown: None,
                });
            }
        }
        let b = self.beautify.clone();
        if let Some(p) = self.beautify_preview.as_mut()
            && p.shown.as_ref().is_none_or(|(s, _)| *s != b)
        {
            let img = beautify::apply(&p.thumb, &scaled(&b, p.k));
            // Fit the framed thumbnail back into the preview box.
            let img = beautify::preview_thumbnail(&img, PREVIEW_W, PREVIEW_H);
            let ci = egui::ColorImage::from_rgba_unmultiplied(
                [img.width as usize, img.height as usize],
                &img.data,
            );
            let tex = ctx.load_texture("beautify-preview", ci, egui::TextureOptions::LINEAR);
            p.shown = Some((b, tex));
        }
    }

    pub(super) fn beautify_texture(&self) -> Option<egui::TextureHandle> {
        self.beautify_preview
            .as_ref()
            .and_then(|p| p.shown.as_ref().map(|(_, t)| t.clone()))
    }
}

pub fn panel_height() -> f32 {
    430.0
}

fn swatch(ui: &mut egui::Ui, a: [u8; 4], b: [u8; 4], selected: bool, tip: &str) -> bool {
    let (dark, accent) = theme::current(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(26.0), egui::Sense::click());
    let inner = r.shrink(3.0);
    if a == b {
        ui.painter().rect_filled(inner, 5.0, paint::color32(a));
    } else {
        // Diagonal gradient: a mesh with the two colors on opposite corners.
        let mut mesh = egui::Mesh::default();
        let (ca, cb) = (paint::color32(a), paint::color32(b));
        let mid = ca.lerp_to_gamma(cb, 0.5);
        mesh.colored_vertex(inner.left_top(), ca);
        mesh.colored_vertex(inner.right_top(), mid);
        mesh.colored_vertex(inner.right_bottom(), cb);
        mesh.colored_vertex(inner.left_bottom(), mid);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        ui.painter().add(egui::Shape::mesh(mesh));
    }
    let stroke = if selected {
        egui::Stroke::new(2.0, accent)
    } else {
        egui::Stroke::new(1.0, theme::control_stroke(dark))
    };
    ui.painter()
        .rect_stroke(inner, 5.0, stroke, egui::StrokeKind::Outside);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.on_hover_text(tip).clicked()
}

fn hex4(s: &str) -> [u8; 4] {
    parse_hex_color(s)
        .map(|c| [c[0], c[1], c[2], 255])
        .unwrap_or([255, 255, 255, 255])
}

pub fn panel(
    ctx: &Context,
    pos: Pos2,
    pivot: Align2,
    b: &Beautify,
    preview: Option<&egui::TextureHandle>,
    pal: &Palette,
    out: &mut Vec<BeautifyAction>,
) {
    let mut v = b.clone();
    Area::new(egui::Id::unique("stinkshot-ov-beautify"))
        .order(Order::Foreground)
        .fixed_pos(pos)
        .pivot(pivot)
        .show(ctx, |ui| {
            panels::panel_frame(pal).show(ui, |ui| {
                ui.set_width(PANEL_W);
                ui.spacing_mut().item_spacing = Vec2::new(6.0, 7.0);
                if panels::header(ui, "Beautify", pal) {
                    out.push(BeautifyAction::Close);
                }
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Apply on Export").size(13.0).color(pal.text));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        theme::widgets::toggle(ui, &mut v.enabled);
                    });
                });
                ui.add_enabled_ui(v.enabled, |ui| {
                    // Preview.
                    let box_size = Vec2::new(PANEL_W, PREVIEW_H as f32);
                    let (r, _) = ui.allocate_exact_size(box_size, egui::Sense::hover());
                    ui.painter()
                        .rect_filled(r, 6.0, theme::subtle_fill(theme::current(ctx).0, 1));
                    if let Some(t) = preview {
                        let s = t.size_vec2();
                        let fit = (box_size.x / s.x).min(box_size.y / s.y).min(1.0);
                        let img = egui::Rect::from_center_size(r.center(), s * fit);
                        let tint = if v.enabled {
                            Color32::WHITE
                        } else {
                            Color32::from_white_alpha(110)
                        };
                        ui.painter().image(
                            t.id(),
                            img,
                            egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                            tint,
                        );
                    }

                    panels::caption(ui, "Background", pal);
                    ui.horizontal(|ui| {
                        for (kind, label) in [
                            (BackgroundKind::Gradient, "Gradient"),
                            (BackgroundKind::Solid, "Solid"),
                            (BackgroundKind::Blurred, "Blurred"),
                        ] {
                            if panels::chip(ui, label, v.background == kind).clicked() {
                                v.background = kind;
                            }
                        }
                    });
                    match v.background {
                        BackgroundKind::Gradient => {
                            ui.horizontal(|ui| {
                                for (a, bb) in beautify::GRADIENTS {
                                    let sel = v.gradient_from.eq_ignore_ascii_case(a)
                                        && v.gradient_to.eq_ignore_ascii_case(bb);
                                    if swatch(ui, hex4(a), hex4(bb), sel, "Gradient") {
                                        v.gradient_from = a.into();
                                        v.gradient_to = bb.into();
                                    }
                                }
                            });
                        }
                        BackgroundKind::Solid => {
                            ui.horizontal(|ui| {
                                for c in beautify::SOLIDS {
                                    let sel = v.color.eq_ignore_ascii_case(c);
                                    if swatch(ui, hex4(c), hex4(c), sel, c) {
                                        v.color = c.into();
                                    }
                                }
                            });
                        }
                        BackgroundKind::Blurred => {
                            panels::caption(ui, "Made from the capture itself.", pal);
                        }
                    }

                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Padding").size(13.0).color(pal.text));
                        ui.add(egui::Slider::new(&mut v.padding, 0..=256).suffix(" px"));
                    });
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Corners").size(13.0).color(pal.text));
                        ui.add(egui::Slider::new(&mut v.corner_radius, 0.0..=48.0).suffix(" px"));
                    });
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Shadow").size(13.0).color(pal.text));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            theme::widgets::toggle(ui, &mut v.shadow);
                        });
                    });
                    panels::caption(ui, "Aspect", pal);
                    ui.horizontal(|ui| {
                        for a in Aspect::ALL {
                            if panels::chip(ui, a.label(), v.aspect == a).clicked() {
                                v.aspect = a;
                            }
                        }
                    });
                });
            });
        });
    if v != *b {
        out.push(BeautifyAction::Set(v));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnail_settings_scale() {
        let b = Beautify {
            padding: 100,
            corner_radius: 20.0,
            ..Default::default()
        };
        let s = scaled(&b, 0.25);
        assert_eq!(s.padding, 25);
        assert_eq!(s.corner_radius, 5.0);
        assert_eq!(s.aspect, b.aspect);
        assert_eq!(s.background, b.background);
    }
}
