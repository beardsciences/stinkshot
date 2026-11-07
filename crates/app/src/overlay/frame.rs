//! Per-frame driver: input -> state, render cache upkeep, canvas painting and chrome.

use egui::{Align2, FontId, Frame, Id, Margin, Modifiers, TextEdit};

use super::paint::{
    around, color32, dashed_rect, paint_loupe, paint_selection_chrome, paint_shape,
    paint_shape_handles, paint_spotlight_dim,
};
use super::*;

fn rect_pt(r: PxRect, k: f32) -> Rect {
    Rect::from_min_size(
        Pos2::new(r.x as f32 * k, r.y as f32 * k),
        Vec2::new(r.w as f32 * k, r.h as f32 * k),
    )
}

impl Overlay {
    pub(super) fn frame(&mut self, ctx: &Context, shared: &mut Shared) -> SurfaceStatus {
        self.init_once(ctx);
        let ppp = ctx.pixels_per_point();

        if !self.closed && ctx.input(|i| i.viewport().close_requested()) {
            self.cancel(shared);
        }
        if self.closed {
            return SurfaceStatus::Close;
        }

        self.poll_render(ctx);
        self.poll_smart_redact(shared);
        if self.finishing.is_some() {
            self.poll_finish(ctx, shared);
        } else {
            let inp = Self::read_input(ctx, ppp);
            let keys = Self::read_keys(ctx);
            self.handle_keys(ctx, shared, &keys, &inp);
            if !self.closed {
                self.handle_pointer(ctx, shared, &inp);
            }
            self.update_render();
        }
        if self.closed {
            return SurfaceStatus::Close;
        }

        let inp = if self.finishing.is_some() {
            Input::default()
        } else {
            Self::read_input(ctx, ppp)
        };
        self.paint_canvas(ctx, shared, &inp, ppp);
        let mut actions = Vec::new();
        self.paint_ui(ctx, shared, &inp, ppp, &mut actions);
        self.apply_actions(ctx, shared, actions);

        if self.closed {
            SurfaceStatus::Close
        } else {
            SurfaceStatus::Open
        }
    }

    // ---------------------------------------------------------------- render cache

    fn clear_render_cache(&mut self) {
        self.rendered_tex = None;
        self.rendered_doc = None;
        self.rendered_img = None;
        self.submitted = None;
    }

    fn poll_render(&mut self, ctx: &Context) {
        let result = self.renderer.as_ref().and_then(|r| r.poll());
        if let Some(r) = result
            && self.submitted.as_ref() == Some(&r.doc)
        {
            match r.image {
                Some(img) => {
                    self.rendered_tex = Some(TiledTexture::upload(ctx, "ov-rendered", &img, false));
                    self.rendered_img = Some(Arc::new(img));
                    self.rendered_doc = Some(r.doc);
                    self.render_ok = true;
                }
                None => {
                    self.rendered_tex = None;
                    self.rendered_img = None;
                    self.rendered_doc = Some(r.doc);
                    self.render_ok = false;
                }
            }
        }
        if self.live_release_pending && self.rendered_doc.as_ref() == Some(&self.doc) {
            self.live = None;
            self.live_release_pending = false;
        }
    }

    /// Submit a new render when the committed (non-live) document changed.
    fn update_render(&mut self) {
        if !self.annotating() || self.sel.is_none() {
            return;
        }
        if self.doc.shapes.is_empty() {
            self.clear_render_cache();
            self.live = None;
            self.live_release_pending = false;
            return;
        }
        if matches!(self.drag, Drag::MoveSel { .. } | Drag::ResizeSel { .. }) {
            self.clear_render_cache();
            return;
        }
        let rd = if self.live_release_pending {
            self.doc.clone()
        } else {
            shapes::doc_without(&self.doc, self.live)
        };
        if rd.shapes.is_empty() {
            self.clear_render_cache();
            return;
        }
        if self.submitted.as_ref() != Some(&rd) {
            let base = self.ensure_base();
            let fonts = self.fonts.clone();
            self.submitted = Some(rd.clone());
            if let Some(r) = self.renderer.as_mut() {
                r.submit(base, rd, fonts);
            }
        }
    }

    /// Shapes the cached render does not (yet) show correctly are painted live.
    fn needs_live_paint(&self, s: &Shape) -> bool {
        if Some(s.id) == self.live || !self.render_ok {
            return true;
        }
        match &self.rendered_doc {
            Some(d) => d.get(s.id) != Some(s),
            None => true,
        }
    }

    // ---------------------------------------------------------------- canvas

    fn paint_canvas(&mut self, ctx: &Context, shared: &Shared, inp: &Input, ppp: f32) {
        let k = 1.0 / ppp;
        let pal = theme::palette(shared.dark);
        let accent = color32(shared.accent);
        let screen = Rect::from_min_size(
            Pos2::ZERO,
            Vec2::new(self.bounds.w as f32 * k, self.bounds.h as f32 * k),
        );
        let painter = ctx.layer_painter(LayerId::background());
        if let Some(t) = &self.frozen_tex {
            t.paint(&painter, Pos2::ZERO, k, Color32::WHITE);
        }

        let preview = self.preview_rect(inp.shift);
        let clear_px = preview.or(self.sel).or(self.hover);
        let clear = clear_px.map(|r| rect_pt(r, k).intersect(screen));
        match clear {
            Some(c) => {
                for r in around(screen, c) {
                    painter.rect_filled(r, 0.0, pal.overlay_dim);
                }
            }
            None => {
                painter.rect_filled(screen, 0.0, pal.overlay_dim);
            }
        }

        // Faint reminder of the previous region while nothing is selected yet.
        if self.sel.is_none()
            && self.hover.is_none()
            && matches!(self.kind, Kind::Capture | Kind::Select(_))
            && let Some(lr) = self
                .last_region
                .and_then(|r| hover::virtual_to_local(r, self.bounds))
        {
            dashed_rect(&painter, rect_pt(lr, k), Color32::from_white_alpha(70));
        }

        // Selection contents.
        if let (Some(sel), true) = (self.sel, self.annotating()) {
            let sel_pt = rect_pt(sel, k);
            let kk = k * self.scale;
            let drawn_render = self.rendered_tex.is_some() && self.rendered_doc.is_some();
            match self.kind {
                Kind::Annotate => {
                    if drawn_render {
                        if let Some(t) = &self.rendered_tex {
                            t.paint(&painter, sel_pt.min, kk, Color32::WHITE);
                        }
                    } else if let Some(t) = &self.base_tex {
                        t.paint(&painter, sel_pt.min, kk, Color32::WHITE);
                    }
                }
                _ => {
                    if drawn_render && let Some(t) = &self.rendered_tex {
                        t.paint(&painter, sel_pt.min, kk, Color32::WHITE);
                    }
                }
            }

            // Live previews, clipped to the selection.
            let clip = painter.with_clip_rect(sel_pt);
            let m = ImgToPt {
                origin: sel_pt.min,
                k: kk,
            };
            let editing = self.edit.as_ref().map(|e| e.id);
            for s in &self.doc.shapes {
                if self.needs_live_paint(s) {
                    if let ShapeKind::Spotlight { rect, dim } = &s.kind {
                        paint_spotlight_dim(&clip, sel_pt, m.r(*rect), *dim);
                    }
                    paint_shape(&clip, m, s, editing == Some(s.id));
                }
            }
            self.paint_drag_preview(&clip, m, inp);
            self.paint_smart_findings(&clip, m, accent);

            if self.edit.is_none()
                && let Some(s) = self.selected.and_then(|id| self.doc.get(id))
            {
                let measure = self.measurer();
                paint_shape_handles(&painter, m, s, accent, &measure);
            }
        }

        // Selection / hover / drag chrome.
        if let Some(r) = preview {
            paint_selection_chrome(&painter, rect_pt(r, k), accent, false);
        } else if let Some(sel) = self.sel {
            let handles = self.sel_editable() && self.tool == Tool::Select && self.edit.is_none();
            paint_selection_chrome(&painter, rect_pt(sel, k), accent, handles);
        } else if let Some(h) = self.hover {
            paint_selection_chrome(&painter, rect_pt(h, k), accent, false);
        }
    }

    fn paint_drag_preview(&self, painter: &egui::Painter, m: ImgToPt, inp: &Input) {
        if let Drag::Draw {
            tool,
            a,
            cur,
            points,
        } = &self.drag
        {
            match tool {
                Tool::Pen | Tool::Highlighter => {
                    let style = shapes::style_for_tool(*tool, &self.params().style);
                    let kind = if *tool == Tool::Pen {
                        ShapeKind::Pen {
                            points: points.clone(),
                        }
                    } else {
                        ShapeKind::Highlighter {
                            points: points.clone(),
                        }
                    };
                    paint_shape(painter, m, &Shape { id: 0, kind, style }, false);
                }
                Tool::Magnifier => {
                    dashed_rect(painter, m.r(RectF::from_points(*a, *cur)), Color32::WHITE);
                }
                t => {
                    if let Some((kind, style)) =
                        shapes::shape_from_drag(*t, *a, *cur, inp.shift, &self.params())
                    {
                        if let ShapeKind::Spotlight { rect, dim } = &kind {
                            paint_spotlight_dim(painter, painter.clip_rect(), m.r(*rect), *dim);
                        }
                        paint_shape(painter, m, &Shape { id: 0, kind, style }, false);
                    }
                }
            }
        }
        if let (Some(src), Some(c)) = (self.magnifier_source, self.magnifier_cursor) {
            if self.tool == Tool::Magnifier {
                dashed_rect(painter, m.r(src), Color32::WHITE);
                let (w, h) = (
                    (src.w * 2.0).clamp(48.0, 400.0),
                    (src.h * 2.0).clamp(48.0, 400.0),
                );
                let d = m.r(RectF::new(c.x - w / 2.0, c.y - h / 2.0, w, h));
                painter.add(egui::Shape::ellipse_filled(
                    d.center(),
                    d.size() / 2.0,
                    Color32::from_black_alpha(80),
                ));
                painter.add(egui::Shape::ellipse_stroke(
                    d.center(),
                    d.size() / 2.0,
                    egui::Stroke::new(2.5, Color32::WHITE),
                ));
            }
        } else if let Some(src) = self.magnifier_source
            && self.tool == Tool::Magnifier
        {
            dashed_rect(painter, m.r(src), Color32::WHITE);
        }
    }

    // ---------------------------------------------------------------- chrome

    fn hint_items(&self) -> Vec<(&'static str, &'static str)> {
        if self.edit.is_some() {
            return vec![
                ("Esc", "Done"),
                ("Ctrl+Enter", "Done"),
                ("Enter", "New Line"),
            ];
        }
        // Esc steps back one level before it cancels; say which.
        let help = ("?", "Shortcuts");
        if self.help_open {
            return vec![("F1", "Close Shortcuts"), ("Esc", "Close Shortcuts")];
        }
        let esc = if self.smart_active() {
            ("Esc", "Close Panel")
        } else if self.annotating()
            && self.sel.is_some()
            && (self.selected.is_some() || self.tool != Tool::Select)
        {
            ("Esc", "Deselect")
        } else if self.discard_prompt_active() {
            ("Esc", "Discard")
        } else {
            ("Esc", "Cancel")
        };
        match (self.sel.is_some(), self.kind) {
            (false, _) => vec![
                ("Drag", "Region"),
                ("Click", "Window or Element"),
                ("Wheel", "Parent or Child"),
                ("Enter", "Select Highlight"),
                esc,
                help,
            ],
            (true, Kind::Annotate) => vec![
                ("Enter", "Copy"),
                (keys::SAVE.label, "Save"),
                (keys::UNDO.label, "Undo"),
                ("Del", "Delete"),
                esc,
                help,
            ],
            (true, _) => vec![
                ("Enter", "Copy"),
                (keys::SAVE.label, "Save"),
                (keys::UNDO.label, "Undo"),
                ("Arrows", "Nudge"),
                esc,
                help,
            ],
        }
    }

    fn paint_ui(
        &mut self,
        ctx: &Context,
        shared: &mut Shared,
        inp: &Input,
        ppp: f32,
        actions: &mut Vec<UiAction>,
    ) {
        let k = 1.0 / ppp;
        let pal = theme::palette(shared.dark);
        let accent = color32(shared.accent);
        let ref_local = inp
            .pos
            .or(self.last_pointer)
            .unwrap_or((self.cursor_local.0 as f32, self.cursor_local.1 as f32));
        let (ref_mon, ref_scale) = self.monitor_at((ref_local.0 as i32, ref_local.1 as i32));

        let busy = self.finishing.is_some();
        if busy {
            ui::busy_pill(
                ctx,
                Pos2::new(
                    ref_mon.x as f32 * k + ref_mon.w as f32 * k / 2.0,
                    ref_mon.y as f32 * k + ref_mon.h as f32 * k / 2.0,
                ),
                pal,
            );
            return;
        }

        // Loupe visibility decides where copied-color feedback goes.
        let loupe_active = shared.config.capture.show_loupe
            && inp.pos.is_some()
            && !inp.over_ui
            && self.edit.is_none()
            && !self.help_open
            && (self.sel.is_none()
                || matches!(
                    self.drag,
                    Drag::Selecting { .. } | Drag::ResizeSel { .. } | Drag::MoveSel { .. }
                ));
        let hex_flash = self.hex_flash_text();
        if hex_flash.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if self.notice.as_ref().is_some_and(|n| n.until.is_some()) {
            ctx.request_repaint_after(Duration::from_millis(250));
        }

        // Hint bar.
        let leading: Option<String> = if self.discard_prompt_active() {
            if let Some(t) = self.discard_armed {
                ctx.request_repaint_after(DISCARD_WINDOW.saturating_sub(t.elapsed()));
            }
            Some("Press Esc Again to Discard Your Annotations".into())
        } else if let (Some((text, _)), false) = (&hex_flash, loupe_active) {
            Some(text.clone())
        } else if let Some(n) = self.notice_text() {
            Some(n.to_string())
        } else if let Kind::Select(p) = self.kind {
            Some(hover::purpose_label(p).into())
        } else {
            None
        };
        let items = self.hint_items();
        ui::hint_bar(
            ctx,
            Pos2::new(
                (ref_mon.x as f32 + ref_mon.w as f32 / 2.0) * k,
                ref_mon.y as f32 * k + 18.0,
            ),
            leading.as_deref(),
            &items,
            pal,
            accent,
        );

        if self.help_open {
            ui::shortcut_sheet(
                ctx,
                Pos2::new(
                    (ref_mon.x as f32 + ref_mon.w as f32 / 2.0) * k,
                    (ref_mon.y as f32 + ref_mon.h as f32 / 2.0) * k,
                ),
                ref_mon.w as f32 * k,
                &keys::sheet(matches!(self.kind, Kind::Select(_))),
                pal,
                accent,
            );
        }

        // Which element the wheel picked (type, name, level), next to the highlight.
        if self.sel.is_none()
            && matches!(self.drag, Drag::None)
            && !self.help_open
            && let (Some(h), Some(label)) = (self.hover, &self.snap_label)
        {
            let pos_pt = if (h.y as f32) * k >= 40.0 {
                Pos2::new(h.x as f32 * k, h.y as f32 * k - 34.0)
            } else {
                Pos2::new(h.x as f32 * k + 6.0, h.y as f32 * k + 6.0)
            };
            ui::element_chip(ctx, pos_pt, label, pal);
        }

        let selecting = matches!(self.drag, Drag::Selecting { moved: true, .. });
        let preview = self.preview_rect(inp.shift);
        let chip_rect = preview.or(self.sel);
        let manipulating = matches!(
            self.drag,
            Drag::Draw { .. } | Drag::MoveShape { .. } | Drag::ResizeShape { .. }
        );

        // Size label + presets above the selection.
        if let (Some(r), false) = (chip_rect, manipulating) {
            let pos_pt = if (r.y as f32) * k >= 40.0 {
                Pos2::new(r.x as f32 * k, r.y as f32 * k - 34.0)
            } else {
                Pos2::new(r.x as f32 * k + 6.0, r.y as f32 * k + 6.0)
            };
            let size = if matches!(self.kind, Kind::Annotate) {
                (self.doc.base_width as i32, self.doc.base_height as i32)
            } else {
                (r.w, r.h)
            };
            let editable =
                self.sel_editable() && !matches!(self.kind, Kind::Select(_)) && preview.is_none();
            let mut text = std::mem::take(&mut self.exact_text);
            let mut st = ChipState {
                size,
                ratio: self.aspect,
                exact_open: self.exact_open,
                exact_just_opened: self.exact_just_opened,
                exact_text: &mut text,
                purpose: None,
                editable,
            };
            ui::size_chips(ctx, pos_pt, &mut st, pal, accent, actions);
            self.exact_text = text;
            self.exact_just_opened = false;
        }

        // Tool bar + action bar.
        if self.annotating()
            && self.sel.is_some()
            && !selecting
            && !matches!(self.drag, Drag::Selecting { .. })
            && let Some(sel) = self.sel
        {
            let gap = 8.0 * ppp;
            let tool_px = (self.bar_sizes.0.x * ppp, self.bar_sizes.0.y * ppp);
            let act_px = (self.bar_sizes.1.x * ppp, self.bar_sizes.1.y * ppp);
            // Docked: follows the selection. Undocked (grip dragged): stays where it was
            // left, clamped to the monitor it is on.
            let ((tx, ty), (ax, ay), mon) = match self.effective_bar_origin(ppp, sel) {
                Some(origin) => self.undocked_bars(origin, ppp, sel),
                None => {
                    let mon = self.monitor_for(sel);
                    let (t, a) = selection::layout_bars(sel, mon, tool_px, act_px, gap, gap);
                    (t, a, mon)
                }
            };
            self.bar_drawn_at = Some((tx, ty));
            let info = ui::BarInfo {
                tool: self.tool,
                color: self.color,
                width: self.width,
            };
            let lifted = self.grip_drag.is_some();
            let lay = ui::tool_bar(ctx, Pos2::new(tx * k, ty * k), &info, pal, lifted, actions);
            let act_rect = ui::action_bar(
                ctx,
                Pos2::new(ax * k, ay * k),
                pal,
                shared.ocr.is_some(),
                self.smart_active(),
                lifted,
                actions,
            );
            self.bar_sizes = (lay.rect.size(), act_rect.size());
            self.color_btn = lay.color_btn;
            self.width_btn = lay.width_btn;

            // Popups open on the bar's monitor, wherever the bar is.
            let screen = rect_pt(mon, k);

            // Find Sensitive Info panel: under the action bar, right edges aligned (above
            // it when there is no room below), kept on the bar's monitor.
            if let Some(view) = self.smart_view() {
                if matches!(view, smart_redact::PanelView::Empty) {
                    ctx.request_repaint_after(Duration::from_millis(250));
                }
                let h = smart_redact::panel_height(&view);
                let w = 330.0;
                let x = act_rect
                    .right()
                    .clamp(screen.left() + w + 8.0, screen.right() - 8.0);
                let (y, pivot) = if act_rect.bottom() + 8.0 + h <= screen.bottom() {
                    (act_rect.bottom() + 8.0, egui::Align2::RIGHT_TOP)
                } else {
                    (act_rect.top() - 8.0, egui::Align2::RIGHT_BOTTOM)
                };
                let mut smart_actions = Vec::new();
                smart_redact::panel(
                    ctx,
                    Pos2::new(x, y),
                    pivot,
                    &view,
                    pal,
                    accent,
                    &mut smart_actions,
                );
                actions.extend(smart_actions.into_iter().map(UiAction::Smart));
            }
            match self.popup {
                Some(Popup::Color) => ui::color_popup(
                    ctx,
                    self.color_btn,
                    screen,
                    &self.palette,
                    self.color,
                    pal,
                    actions,
                ),
                Some(Popup::Width) => ui::width_popup(
                    ctx,
                    self.width_btn,
                    screen,
                    self.width,
                    pal,
                    accent,
                    actions,
                ),
                None => {}
            }
        }

        self.text_edit_ui(ctx, ppp, accent);

        // Loupe.
        if loupe_active && let Some(p) = inp.pos {
            let cell = (p.0.floor() as i32, p.1.floor() as i32);
            let ptr = Pos2::new(p.0 * k, p.1 * k);
            let size = paint::LOUPE_CELLS as f32 * paint::LOUPE_CELL;
            let total_h = size + 6.0 + 48.0;
            let mon = rect_pt(ref_mon, k);
            let mut x = ptr.x + 24.0;
            let mut y = ptr.y + 24.0;
            if x + size + 8.0 > mon.right() {
                x = ptr.x - 24.0 - size;
            }
            if y + total_h + 8.0 > mon.bottom() {
                y = ptr.y - 24.0 - total_h;
            }
            let lp = ctx.layer_painter(LayerId::new(
                Order::Tooltip,
                Id::unique("stinkshot-ov-loupe"),
            ));
            let rgb = paint_loupe(
                &lp,
                Pos2::new(x, y),
                &self.frozen_image,
                cell.0,
                cell.1,
                accent,
            );
            let hex = hover::hex_string(rgb);
            ui::loupe_plate(
                ctx,
                Pos2::new(x, y + size + 6.0),
                (cell.0 + self.bounds.x, cell.1 + self.bounds.y),
                (ref_scale * 100.0).round() as i32,
                &hex,
                rgb,
                hex_flash.as_ref().map(|(t, ok)| (t.as_str(), *ok)),
                pal,
                accent,
            );
        }
    }

    // ---------------------------------------------------------------- inline text editor

    fn text_edit_ui(&mut self, ctx: &Context, ppp: f32, accent: Color32) {
        if self.edit.is_none() {
            return;
        }
        let (esc, ctrl_enter) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::Escape),
                i.consume_key(Modifiers::COMMAND, Key::Enter),
            )
        });
        if esc || ctrl_enter {
            self.commit_text();
            return;
        }
        let (Some(sel), Some(edit_id)) = (self.sel, self.edit.as_ref().map(|e| e.id)) else {
            return;
        };
        let Some(shape) = self.doc.get(edit_id) else {
            self.edit = None;
            return;
        };
        let Some((pos, size, inner_w)) = shapes::text_anchor(&shape.kind) else {
            self.edit = None;
            return;
        };
        let k = 1.0 / ppp;
        let kk = k * self.scale;
        let origin = Pos2::new(sel.x as f32 * k, sel.y as f32 * k);
        let at = Pos2::new(origin.x + pos.x * kk, origin.y + pos.y * kk);
        let font_pt = (size * kk).max(6.0);
        let col = match &shape.kind {
            ShapeKind::Callout { .. } => {
                let t = hover::contrast_text(shape.style.fill.unwrap_or(shape.style.color));
                Color32::from_rgb(t[0], t[1], t[2])
            }
            _ => color32([
                shape.style.color[0],
                shape.style.color[1],
                shape.style.color[2],
                255,
            ]),
        };
        let measure = self.measurer();
        let buffer_now = self
            .edit
            .as_ref()
            .map(|e| e.buffer.clone())
            .unwrap_or_default();
        let w_pt = match inner_w {
            Some(w) => w * kk,
            None => (measure(&buffer_now, size).0 * kk + font_pt * 2.0).max(90.0),
        };

        let mut changed_text: Option<String> = None;
        if let Some(edit) = self.edit.as_mut() {
            let resp = egui::Area::new(Id::unique("stinkshot-ov-text-edit"))
                .order(Order::Foreground)
                .fixed_pos(at)
                .show(ctx, |ui| {
                    let out = TextEdit::multiline(&mut edit.buffer)
                        .font(FontId::proportional(font_pt))
                        .text_color(col)
                        .frame(Frame::NONE)
                        .margin(Margin::same(0))
                        .desired_width(w_pt)
                        .desired_rows(1)
                        .lock_focus(true)
                        .show(ui);
                    out.response.request_focus();
                    ui.painter().rect_stroke(
                        out.response.rect.expand(3.0),
                        2.0,
                        egui::Stroke::new(1.0, accent),
                        egui::epaint::StrokeKind::Outside,
                    );
                    out.response.changed()
                });
            if resp.inner {
                changed_text = Some(edit.buffer.clone());
            }
            edit.just_started = false;
        }
        if let Some(t) = changed_text
            && let Some(s) = self.doc.get_mut(edit_id)
        {
            shapes::set_text(&mut s.kind, &t);
        }
        let _ = Align2::LEFT_TOP;
    }
}
