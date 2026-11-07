//! Per-frame driver: input -> state, render cache upkeep, canvas painting and chrome, and
//! typing straight onto the image (live rendered text with a caret and selection).

use egui::{Id, Modifiers};
use stinkshot_core::annotate;

use super::paint::{
    around, color32, dashed_rect, paint_group_box, paint_guides, paint_loupe, paint_marquee,
    paint_selected_outline, paint_selection_chrome, paint_shape, paint_shape_handles,
    paint_spotlight_dim,
};
use super::*;

/// Draw a rendered shape patch (image px rect) through the image mapping.
fn paint_patch(painter: &egui::Painter, m: ImgToPt, rect: PxRect, tex: &egui::TextureHandle) {
    let r = m.r(RectF::new(
        rect.x as f32,
        rect.y as f32,
        rect.w as f32,
        rect.h as f32,
    ));
    painter.image(
        tex.id(),
        r,
        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
        Color32::WHITE,
    );
}

/// Caret blink: solid for this long after typing or moving, then on/off at this period.
const CARET_SOLID: Duration = Duration::from_millis(500);
const CARET_BLINK: Duration = Duration::from_millis(530);

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
            let inp = self.read_input(ctx, ppp);
            let keys = Self::read_keys(ctx);
            // Typing owns the keyboard; Esc that ends typing must not also deselect.
            let was_editing = self.edit.is_some();
            self.text_keys(ctx);
            if !was_editing {
                self.handle_keys(ctx, shared, &keys, &inp);
            }
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
            self.read_input(ctx, ppp)
        };
        self.update_text_patch(ctx);
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

    fn poll_render(&mut self, ctx: &Context) {
        let results = self.renderer.as_ref().map(|r| r.poll()).unwrap_or_default();
        for r in results {
            match r.out {
                Some(render::RenderOut::Full(tiles)) => {
                    self.rendered_tex = Some(TiledTexture::from_tiles(ctx, "ov-rendered", tiles));
                    self.rendered_doc = Some(r.doc);
                    self.render_ok = true;
                }
                Some(render::RenderOut::Patch(pieces)) => {
                    // Patches build on the texture the worker last sent; without it (the
                    // cache was dropped meanwhile) the next submit asks for a full image.
                    let ok = self.rendered_tex.as_mut().is_some_and(|t| t.patch(pieces));
                    match ok {
                        true => {
                            self.rendered_doc = Some(r.doc);
                            self.render_ok = true;
                        }
                        false => {
                            self.rendered_tex = None;
                            self.rendered_doc = None;
                            self.submitted = None;
                        }
                    }
                }
                None => {
                    self.rendered_tex = None;
                    self.rendered_doc = Some(r.doc);
                    self.render_ok = false;
                }
            }
        }
        if self.live_release_pending && self.rendered_doc.as_ref() == Some(&self.doc) {
            self.live.clear();
            self.live_release_pending = false;
        }
        if self.settled_text.as_ref().is_some_and(|p| {
            self.rendered_doc
                .as_ref()
                .is_some_and(|d| d.get(p.shape.id) == Some(&p.shape))
                || self.doc.get(p.shape.id) != Some(&p.shape)
        }) {
            self.settled_text = None;
        }
    }

    /// Submit a new render when the committed (non-live) document changed.
    fn update_render(&mut self) {
        if !self.annotating() || self.sel.is_none() {
            return;
        }
        if self.doc.shapes.is_empty() {
            self.clear_render_cache();
            self.live.clear();
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
            shapes::doc_without(&self.doc, &self.live)
        };
        if rd.shapes.is_empty() {
            self.clear_render_cache();
            return;
        }
        if self.submitted.as_ref() != Some(&rd) {
            let base = self.ensure_base();
            let fonts = self.fonts.clone();
            let need_full = self.rendered_tex.is_none();
            self.submitted = Some(rd.clone());
            if let Some(r) = self.renderer.as_mut() {
                r.submit(base, rd, fonts, need_full);
            }
        }
    }

    /// Shapes the cached render does not (yet) show correctly are painted live.
    fn needs_live_paint(&self, s: &Shape) -> bool {
        if self.live.contains(&s.id) || !self.render_ok {
            return true;
        }
        match &self.rendered_doc {
            Some(d) => d.get(s.id) != Some(s),
            None => true,
        }
    }

    // ---------------------------------------------------------------- typing on the image

    /// Re-render the text being typed when it changed (exact final pixels, small canvas).
    fn update_text_patch(&mut self, ctx: &Context) {
        let Some(e) = self.edit.as_ref() else { return };
        let Some(shape) = self.doc.get(e.id).cloned() else {
            return;
        };
        if e.patch.as_ref().is_some_and(|p| p.shape == shape) {
            return;
        }
        let patch = annotate::render_shape_patch(&shape, &self.fonts).map(|(rect, img)| {
            let ci = egui::ColorImage::from_rgba_unmultiplied(
                [img.width as usize, img.height as usize],
                &img.data,
            );
            let tex = ctx.load_texture(
                "ov-text-edit",
                ci,
                egui::TextureOptions {
                    magnification: egui::TextureFilter::Linear,
                    minification: egui::TextureFilter::Linear,
                    ..egui::TextureOptions::LINEAR
                },
            );
            (rect, tex)
        });
        if let Some(e) = self.edit.as_mut() {
            e.patch = Some(match patch {
                Some((rect, tex)) => TextPatch {
                    shape,
                    rect,
                    tex: Some(tex),
                },
                None => TextPatch {
                    shape,
                    rect: PxRect::default(),
                    tex: None,
                },
            });
        }
    }

    /// Keyboard while typing on the image: characters, caret movement and selection,
    /// clipboard, Enter for a new line, Esc or Ctrl+Enter to finish, Ctrl+B for bold.
    fn text_keys(&mut self, ctx: &Context) {
        if self.edit.is_none() {
            return;
        }
        let (done, events) = ctx.input_mut(|i| {
            let done = i.consume_key(Modifiers::NONE, Key::Escape)
                || i.consume_key(Modifiers::COMMAND, Key::Enter);
            (done, i.events.clone())
        });
        if done {
            self.commit_text();
            return;
        }
        let mut changed = false;
        let mut bold = false;
        let mut vertical: Option<i32> = None;
        let mut line_edge: Option<(bool, bool)> = None;
        if let Some(e) = self.edit.as_mut() {
            for ev in events {
                match ev {
                    Event::Text(t) => {
                        let t: String = t.chars().filter(|c| !c.is_control()).collect();
                        if !t.is_empty() {
                            e.caret.insert(&mut e.buffer, &t);
                            changed = true;
                        }
                    }
                    Event::Paste(t) => {
                        let t = t.replace("\r\n", "\n");
                        e.caret.insert(&mut e.buffer, &t);
                        changed = true;
                    }
                    Event::Copy | Event::Cut => {
                        let sel = e.caret.selected(&e.buffer).to_string();
                        if !sel.is_empty() {
                            if let Err(err) = clipboard::set_text(&sel) {
                                log::warn!("copy text: {err:#}");
                            }
                            if matches!(ev, Event::Cut) {
                                e.caret.insert(&mut e.buffer, "");
                                changed = true;
                            }
                        }
                    }
                    Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        let word = modifiers.command;
                        let ext = modifiers.shift;
                        match key {
                            Key::Enter if !modifiers.command => {
                                e.caret.insert(&mut e.buffer, "\n");
                                changed = true;
                            }
                            Key::Backspace => {
                                e.caret.backspace(&mut e.buffer, word);
                                changed = true;
                            }
                            Key::Delete => {
                                e.caret.delete(&mut e.buffer, word);
                                changed = true;
                            }
                            Key::ArrowLeft => e.caret.left(&e.buffer, word, ext),
                            Key::ArrowRight => e.caret.right(&e.buffer, word, ext),
                            Key::ArrowUp => vertical = Some(-1),
                            Key::ArrowDown => vertical = Some(1),
                            Key::Home => line_edge = Some((false, ext)),
                            Key::End => line_edge = Some((true, ext)),
                            Key::A if modifiers.command => e.caret.select_all(&e.buffer),
                            Key::B if modifiers.command => bold = true,
                            _ => {}
                        }
                        if modifiers.shift && !matches!(key, Key::Enter) {
                            vertical = vertical.map(|v| v * 2);
                        }
                    }
                    _ => {}
                }
            }
            e.caret.clamp(&e.buffer);
            e.blink_from = Instant::now();
            if changed {
                let (id, text) = (e.id, e.buffer.clone());
                if let Some(s) = self.doc.get_mut(id) {
                    shapes::set_text(&mut s.kind, &text);
                }
            }
        }
        if let Some(v) = vertical {
            self.caret_vertical(v.signum(), v.abs() > 1);
        }
        if let Some((end, ext)) = line_edge {
            self.caret_line_edge(end, ext);
        }
        if bold {
            self.toggle_bold();
        }
    }

    /// Caret (x, line top, line height) in image px for byte offset `i`.
    fn caret_pos(&self, i: usize) -> Option<(f32, f32, f32)> {
        let e = self.edit.as_ref()?;
        let shape = self.doc.get(e.id)?;
        let (size, bold, lines) = annotate::text_layout(&self.fonts, &shape.kind)?;
        let ranges: Vec<std::ops::Range<usize>> = lines.iter().map(|l| l.range.clone()).collect();
        let li = textedit::line_of(&ranges, i);
        let line = lines.get(li)?;
        let end = i.clamp(line.range.start, line.range.end);
        let x = line.origin.x
            + self
                .fonts
                .line_width(&e.buffer[line.range.start..end], size, bold);
        Some((x, line.origin.y, size * stinkshot_core::fonts::LINE_HEIGHT))
    }

    fn caret_vertical(&mut self, dir: i32, extend: bool) {
        let Some(cur) = self.edit.as_ref().map(|e| e.caret.cursor) else {
            return;
        };
        let Some((x, top, lh)) = self.caret_pos(cur) else {
            return;
        };
        let y = top + lh / 2.0 + dir as f32 * lh;
        if let Some(i) = self.caret_at(PtF::new(x, y))
            && let Some(e) = self.edit.as_mut()
        {
            e.caret.move_to(i, extend);
        }
    }

    fn caret_line_edge(&mut self, end: bool, extend: bool) {
        let Some(e) = self.edit.as_ref() else { return };
        let Some(shape) = self.doc.get(e.id) else {
            return;
        };
        let Some((_, _, lines)) = annotate::text_layout(&self.fonts, &shape.kind) else {
            return;
        };
        let ranges: Vec<std::ops::Range<usize>> = lines.iter().map(|l| l.range.clone()).collect();
        let li = textedit::line_of(&ranges, e.caret.cursor);
        if let Some(r) = ranges.get(li).cloned()
            && let Some(e) = self.edit.as_mut()
        {
            e.caret.move_to(if end { r.end } else { r.start }, extend);
        }
    }

    /// The text being typed: its rendered pixels, the selection and the caret.
    fn paint_text_edit(&self, painter: &egui::Painter, ctx: &Context, m: ImgToPt, accent: Color32) {
        let Some(e) = self.edit.as_ref() else { return };
        let Some(shape) = self.doc.get(e.id) else {
            return;
        };
        if let Some(TextPatch {
            rect,
            tex: Some(tex),
            ..
        }) = &e.patch
        {
            paint_patch(painter, m, *rect, tex);
        }
        let Some((size, bold, lines)) = annotate::text_layout(&self.fonts, &shape.kind) else {
            return;
        };
        let lh = size * stinkshot_core::fonts::LINE_HEIGHT;
        // Selection: one band per line.
        let sel = e.caret.range();
        if !sel.is_empty() {
            for l in &lines {
                let a = sel.start.max(l.range.start);
                let b = sel.end.min(l.range.end);
                // A selected line break shows as a short band at the line end.
                let crosses = sel.end > l.range.end && sel.start <= l.range.end;
                if a > b || (a == b && !crosses) {
                    continue;
                }
                let x0 = l.origin.x
                    + self
                        .fonts
                        .line_width(&e.buffer[l.range.start..a], size, bold);
                let mut x1 = l.origin.x
                    + self
                        .fonts
                        .line_width(&e.buffer[l.range.start..b], size, bold);
                if crosses {
                    x1 += size * 0.3;
                }
                painter.rect_filled(
                    m.r(RectF::new(x0, l.origin.y, (x1 - x0).max(1.0), lh)),
                    0.0,
                    accent.gamma_multiply(0.35),
                );
            }
        }
        // Caret: text-colored with a thin contrasting edge, blinking after a moment.
        let since = e.blink_from.elapsed();
        let on = since < CARET_SOLID
            || ((since - CARET_SOLID).as_millis() / CARET_BLINK.as_millis()) % 2 == 1;
        let wait = if since < CARET_SOLID {
            CARET_SOLID - since
        } else {
            let p = CARET_BLINK.as_millis() as u64;
            Duration::from_millis(p - ((since - CARET_SOLID).as_millis() as u64 % p))
        };
        ctx.request_repaint_after(wait);
        if on && let Some((x, top, lh)) = self.caret_pos(e.caret.cursor) {
            let ink = match &shape.kind {
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
            let a = m.p(PtF::new(x, top + lh * 0.08));
            let b = m.p(PtF::new(x, top + lh * 0.92));
            let w = (m.len(size) / 14.0).clamp(1.5, 3.0);
            let edge = if theme::luminance(ink) > 0.5 {
                Color32::from_black_alpha(140)
            } else {
                Color32::from_white_alpha(170)
            };
            painter.line_segment([a, b], egui::Stroke::new(w + 2.0, edge));
            painter.line_segment([a, b], egui::Stroke::new(w, ink));
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
        let zoomed = !self.view.is_identity();
        if zoomed {
            painter.rect_filled(screen, 0.0, Color32::from_rgb(0x10, 0x10, 0x12));
        }
        let origin_pt = Pos2::new(self.view.off.0 * k, self.view.off.1 * k);
        let kz = k * self.view.zoom;
        if let Some(t) = &self.frozen_tex {
            t.paint(&painter, origin_pt, kz, Color32::WHITE);
        }
        // The editor shows the whole picture (cropped-away parts dim like the screen).
        if matches!(self.kind, Kind::Annotate)
            && let Some(t) = &self.base_tex
        {
            let (x, y) = self.view.to_screen(self.img_origin);
            t.paint(
                &painter,
                Pos2::new(x * k, y * k),
                kz * self.scale,
                Color32::WHITE,
            );
        }

        let preview = self.preview_rect(inp.shift);
        let clear_px = preview.or(self.sel).or(self.hover);
        let clear = clear_px.map(|r| self.canvas_rect_pt(r, ppp).intersect(screen));
        match clear {
            Some(c) if c.is_positive() => {
                for r in around(screen, c) {
                    painter.rect_filled(r, 0.0, pal.overlay_dim);
                }
            }
            _ => {
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
            dashed_rect(
                &painter,
                self.canvas_rect_pt(lr, ppp),
                Color32::from_white_alpha(70),
            );
        }

        // Selection contents.
        if let (Some(sel), true, Some(m)) = (self.sel, self.annotating(), self.img_to_pt(ppp)) {
            let sel_pt = self.canvas_rect_pt(sel, ppp);
            let drawn_render = self.rendered_tex.is_some() && self.rendered_doc.is_some();
            if drawn_render && let Some(t) = &self.rendered_tex {
                t.paint(&painter, m.origin, m.k, Color32::WHITE);
            }

            // Live previews, clipped to the selection.
            let clip = painter.with_clip_rect(sel_pt.intersect(screen));
            let editing = self.edit.as_ref().map(|e| e.id);
            for s in &self.doc.shapes {
                if editing == Some(s.id) {
                    continue;
                }
                if self.needs_live_paint(s) {
                    if let Some(p) = self.settled_text.as_ref().filter(|p| p.shape == *s)
                        && let Some(tex) = &p.tex
                    {
                        paint_patch(&clip, m, p.rect, tex);
                        continue;
                    }
                    if let ShapeKind::Spotlight { rect, dim } = &s.kind {
                        paint_spotlight_dim(&clip, sel_pt, m.r(*rect), *dim);
                    }
                    paint_shape(&clip, m, s, false);
                }
            }
            self.paint_text_edit(&clip, ctx, m, accent);
            self.paint_drag_preview(&clip, m, inp);
            self.paint_smart_findings(&clip, m, accent);

            let measure = self.measurer();
            if self.edit.is_none() {
                if let Some(s) = self.sel_one().and_then(|id| self.doc.get(id)) {
                    let hot = match (&self.drag, self.hover_handle) {
                        (Drag::ResizeShape { handle, .. }, _) => Some(*handle),
                        (_, Some(HoverHandle::Shape(h))) => Some(h),
                        _ => None,
                    };
                    paint_shape_handles(&painter, m, s, accent, &measure, hot);
                } else if self.selected.len() > 1 {
                    for s in self
                        .doc
                        .shapes
                        .iter()
                        .filter(|s| self.selected.contains(&s.id))
                    {
                        paint_selected_outline(&painter, m, s, &measure);
                    }
                    if let Some(b) = shapes::group_bounds(&self.doc, &self.selected, &measure) {
                        paint_group_box(&painter, m.r(b), accent);
                    }
                }
            }
            if let Some(r) = self.marquee_rect() {
                paint_marquee(&painter, m.r(r), accent);
            }
            paint_guides(&painter, m, &self.guides, accent);
        }

        // Selection / hover / drag chrome.
        if let Some(r) = preview {
            paint_selection_chrome(&painter, self.canvas_rect_pt(r, ppp), accent, false, None);
        } else if let Some(sel) = self.sel {
            let handles = self.sel_editable() && self.tool == Tool::Select && self.edit.is_none();
            let hot = match (&self.drag, self.hover_handle) {
                (Drag::ResizeSel { handle, .. }, _) => Some(*handle),
                (_, Some(HoverHandle::Sel(h))) => Some(h),
                _ => None,
            };
            paint_selection_chrome(
                &painter,
                self.canvas_rect_pt(sel, ppp),
                accent,
                handles,
                hot,
            );
        } else if let Some(h) = self.hover {
            paint_selection_chrome(&painter, self.canvas_rect_pt(h, ppp), accent, false, None);
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
                (keys::BOLD.label, "Bold"),
            ];
        }
        // Esc steps back one level before it cancels; say which.
        let help = ("?", "Shortcuts");
        if self.help_open {
            return vec![("F1", "Close Shortcuts"), ("Esc", "Close Shortcuts")];
        }
        if self.picking {
            return vec![("Click", "Pick Color"), ("Esc", "Cancel"), help];
        }
        match self.smart_view() {
            Some(smart_redact::PanelView::Review(_)) => {
                return vec![("Enter", "Redact Selected"), ("Esc", "Close Panel"), help];
            }
            Some(smart_redact::PanelView::Scanning) => return vec![("Esc", "Close Panel"), help],
            _ => {}
        }
        let esc = if self.smart_active() {
            ("Esc", "Close Panel")
        } else if self.annotating()
            && self.sel.is_some()
            && (!self.selected.is_empty() || self.tool != Tool::Select)
        {
            ("Esc", "Deselect")
        } else if self.discard_prompt_active() {
            ("Esc", "Discard")
        } else {
            ("Esc", "Cancel")
        };
        if self.annotating() && self.sel.is_some() && !self.selected.is_empty() {
            // Ctrl+C copies the shapes now; say so, and that Enter still copies the image.
            return vec![
                ("Enter", "Copy Image"),
                (keys::COPY.label, "Copy Shapes"),
                (keys::DUPLICATE.label, "Duplicate"),
                ("Del", "Delete"),
                esc,
                help,
            ];
        }
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
                ("Ctrl+Wheel", "Zoom"),
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

    /// The context bar for the text being typed or the one selected arrow / text.
    fn context_bar_state(&self) -> Option<(ShapeId, ui::ContextBar)> {
        let id = self.edit.as_ref().map(|e| e.id).or(self.sel_one())?;
        let s = self.doc.get(id)?;
        let bar = match &s.kind {
            ShapeKind::Text {
                bold,
                background,
                size,
                ..
            } => ui::ContextBar::Text {
                bold: *bold,
                pill: background.is_some(),
                size: *size,
            },
            ShapeKind::Arrow {
                double, head, ctrl, ..
            } => ui::ContextBar::Arrow {
                double: *double,
                head: *head,
                curved: ctrl.is_some(),
            },
            _ => return None,
        };
        Some((id, bar))
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
        let ref_screen = inp
            .spos
            .or(self.last_screen)
            .unwrap_or((self.cursor_local.0 as f32, self.cursor_local.1 as f32));
        let (ref_mon, ref_scale) = self.monitor_at((ref_screen.0 as i32, ref_screen.1 as i32));

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
        let loupe_active = inp.pos.is_some()
            && !inp.over_ui
            && self.edit.is_none()
            && !self.help_open
            && (self.picking
                || (shared.config.capture.show_loupe
                    && (self.sel.is_none()
                        || matches!(
                            self.drag,
                            Drag::Selecting { .. } | Drag::ResizeSel { .. } | Drag::MoveSel { .. }
                        ))));
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
        } else if self.picking {
            Some("Pick a Color from the Image".into())
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
            let h = self.canvas_rect_screen(h);
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
            Drag::Draw { .. }
                | Drag::MoveShapes { moved: true, .. }
                | Drag::ResizeShape { .. }
                | Drag::Marquee { .. }
                | Drag::Pan { .. }
        );

        // Size label + presets above the selection (kept on screen when zoomed in).
        if let (Some(r), false) = (chip_rect, manipulating) {
            let rs = self.canvas_rect_screen(r);
            let mon = self.monitor_for(rs);
            let y = (rs.y as f32 * k - 34.0).max(mon.y as f32 * k + 52.0);
            let x = (rs.x as f32 * k).clamp(
                mon.x as f32 * k + 6.0,
                (mon.right() as f32 * k - 260.0).max(mon.x as f32 * k + 6.0),
            );
            let pos_pt = if (rs.y as f32) * k >= 40.0 {
                Pos2::new(x, y)
            } else {
                Pos2::new(rs.x as f32 * k + 6.0, rs.y as f32 * k + 6.0)
            };
            let size = if matches!(self.kind, Kind::Annotate) {
                (self.doc.base_width as i32, self.doc.base_height as i32)
            } else {
                (r.w, r.h)
            };
            let editable = matches!(self.kind, Kind::Capture) && preview.is_none();
            let zoom = (!self.view.is_identity() || matches!(self.kind, Kind::Annotate))
                .then(|| view::percent(self.view, self.scale));
            let mut text = std::mem::take(&mut self.exact_text);
            let mut st = ChipState {
                size,
                ratio: self.aspect,
                exact_open: self.exact_open,
                exact_just_opened: self.exact_just_opened,
                exact_text: &mut text,
                purpose: None,
                editable,
                zoom: zoom.filter(|_| self.sel.is_some() && preview.is_none()),
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
            let sel_s = self.canvas_rect_screen(sel);
            let gap = 8.0 * ppp;
            let tool_px = (self.bar_sizes.0.x * ppp, self.bar_sizes.0.y * ppp);
            let act_px = (self.bar_sizes.1.x * ppp, self.bar_sizes.1.y * ppp);
            // Docked: follows the selection. Undocked (grip dragged): stays where it was
            // left, clamped to the monitor it is on.
            let ((tx, ty), (ax, ay), mon) = match self.effective_bar_origin(ppp, sel_s) {
                Some(origin) => self.undocked_bars(origin, ppp, sel_s),
                None => {
                    let mon = self.monitor_for(sel_s);
                    // A zoomed selection can be larger than the monitor: dock to the part
                    // that is on screen.
                    let vis = sel_s.intersect(&mon).unwrap_or(sel_s);
                    let (t, a) = selection::layout_bars(vis, mon, tool_px, act_px, gap, gap);
                    (t, a, mon)
                }
            };
            self.bar_drawn_at = Some((tx, ty));
            let (color, width) = match self.sel_one().and_then(|id| self.doc.get(id)) {
                Some(s) => (shapes::display_color(s), shapes::display_width(s)),
                None => (self.color, self.width),
            };
            let info = ui::BarInfo {
                tool: self.tool,
                color,
                width,
                picking: self.picking,
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
            let screen = Rect::from_min_size(
                Pos2::new(mon.x as f32 * k, mon.y as f32 * k),
                Vec2::new(mon.w as f32 * k, mon.h as f32 * k),
            );

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
                    &self.recent,
                    color,
                    pal,
                    actions,
                ),
                Some(Popup::Width) => {
                    ui::width_popup(ctx, self.width_btn, screen, width, pal, accent, actions)
                }
                None => {}
            }

            // Options for the selected arrow or text (or the text being typed).
            let quiet = matches!(
                self.drag,
                Drag::MoveShapes { moved: true, .. }
                    | Drag::ResizeShape { .. }
                    | Drag::Marquee { .. }
                    | Drag::Pan { .. }
                    | Drag::Draw { .. }
            );
            if !quiet
                && self.popup.is_none()
                && let Some((id, bar)) = self.context_bar_state()
                && let (Some(s), Some(m)) = (self.doc.get(id), self.img_to_pt(ppp))
            {
                let measure = self.measurer();
                let anchor = m.r(shapes::bounds(s, &measure)).expand(6.0);
                let mon_s = self.monitor_for(self.canvas_rect_screen(sel));
                let mon_pt = Rect::from_min_size(
                    Pos2::new(mon_s.x as f32 * k, mon_s.y as f32 * k + 48.0),
                    Vec2::new(mon_s.w as f32 * k, mon_s.h as f32 * k - 48.0),
                );
                ui::context_bar(ctx, anchor, mon_pt, bar, pal, actions);
            }
        }

        // Loupe (and the eyedropper's).
        if loupe_active && let (Some(p), Some(sp)) = (inp.pos, inp.spos) {
            let ptr = Pos2::new(sp.0 * k, sp.1 * k);
            let size = paint::LOUPE_CELLS as f32 * paint::LOUPE_CELL;
            let total_h = size + 6.0 + 48.0;
            let mon = Rect::from_min_size(
                Pos2::new(ref_mon.x as f32 * k, ref_mon.y as f32 * k),
                Vec2::new(ref_mon.w as f32 * k, ref_mon.h as f32 * k),
            );
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
            // In the editor the eyedropper loupe magnifies the picture itself.
            let (img, cx, cy): (&Rgba, i32, i32) = match (self.kind, &self.orig) {
                (Kind::Annotate, Some(o)) if self.picking => (
                    o,
                    ((p.0 - self.img_origin.0) / self.scale).floor() as i32,
                    ((p.1 - self.img_origin.1) / self.scale).floor() as i32,
                ),
                _ => (&self.frozen_image, p.0.floor() as i32, p.1.floor() as i32),
            };
            let rgb = paint_loupe(&lp, Pos2::new(x, y), img, cx, cy, accent);
            let rgb = if self.picking { self.sample_at(p) } else { rgb };
            let hex = hover::hex_string(rgb);
            ui::loupe_plate(
                ctx,
                Pos2::new(x, y + size + 6.0),
                (
                    p.0.floor() as i32 + self.bounds.x,
                    p.1.floor() as i32 + self.bounds.y,
                ),
                (ref_scale * 100.0).round() as i32,
                &hex,
                rgb,
                hex_flash.as_ref().map(|(t, ok)| (t.as_str(), *ok)),
                pal,
                accent,
            );
        }
    }
}
