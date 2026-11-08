//! Editing operations: tools, colors (per-tool memory, recent colors, eyedropper),
//! undo/redo, shape creation and deletion, copy / paste / duplicate, z-order, the arrow and
//! text options, the inline text editing lifecycle, accepting a region, and finishing
//! (flatten + command push).

use stinkshot_core::annotate::ArrowHead;

use super::*;

/// Option buttons on the context bar of a selected arrow or text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ShapeOpt {
    TextSmaller,
    TextLarger,
    Bold(bool),
    Pill(bool),
    Double(bool),
    Head(ArrowHead),
    Straighten,
}

impl Overlay {
    pub(super) fn set_tool(&mut self, t: Tool) {
        self.commit_text();
        if t != Tool::Select {
            self.combine_regions();
        }
        self.tool = if t == self.tool && t != Tool::Select {
            Tool::Select
        } else {
            t
        };
        self.magnifier_source = None;
        self.magnifier_cursor = None;
        self.picking = false;
        if self.tool != Tool::Select {
            self.selected.clear();
        }
        if self.tool.has_style() {
            let (c, w) = self
                .tool_styles
                .get(&self.tool)
                .copied()
                .unwrap_or_else(|| default_tool_style(self.tool, self.color, self.width));
            self.color = c;
            self.width = w;
        }
        self.popup = None;
    }

    /// Remember `color` / `width` for the active drawing tool.
    fn remember_tool_style(&mut self) {
        if self.tool.has_style() {
            self.tool_styles.insert(self.tool, (self.color, self.width));
        }
    }

    /// Apply a color: the selected shapes take it, the active tool remembers it, and it goes
    /// to the front of the recent colors.
    pub(super) fn set_color(&mut self, c: [u8; 4]) {
        self.color = [c[0], c[1], c[2], 255];
        self.prefs_dirty = true;
        push_recent(&mut self.recent, self.color);
        self.remember_tool_style();
        let targets = self.option_targets();
        if !targets.is_empty() {
            let pre = self.doc.clone();
            for id in targets {
                if let Some(s) = self.doc.get_mut(id) {
                    shapes::recolor(s, self.color);
                }
            }
            if self.edit.is_none() && self.doc != pre {
                self.undo.push(&pre);
            }
        }
    }

    pub(super) fn set_width(&mut self, w: f32) {
        self.width = w.clamp(1.0, 64.0);
        self.prefs_dirty = true;
        self.remember_tool_style();
        if self.selected.is_empty() {
            return;
        }
        let pre = self.doc.clone();
        for id in self.selected.clone() {
            if let Some(s) = self.doc.get_mut(id) {
                match &mut s.kind {
                    ShapeKind::Step { radius, .. } => *radius = shapes::step_radius(self.width),
                    ShapeKind::Highlighter { .. } => s.style.width = (self.width * 4.0).max(14.0),
                    _ => s.style.width = self.width,
                }
            }
        }
        if self.doc != pre {
            self.undo.push(&pre);
        }
    }

    /// `[` / `]`: grow or shrink the selected shapes, or the default stroke width.
    pub(super) fn bump_size(&mut self, delta: f32) {
        if self.selected.is_empty() {
            self.width = (self.width + delta).clamp(1.0, 64.0);
            self.remember_tool_style();
        } else {
            let pre = self.doc.clone();
            for id in self.selected.clone() {
                if let Some(s) = self.doc.get_mut(id) {
                    shapes::adjust_size(s, delta);
                    self.width = s.style.width.clamp(1.0, 64.0);
                }
            }
            if self.doc != pre {
                self.undo.push(&pre);
            }
        }
        self.prefs_dirty = true;
    }

    fn after_history_change(&mut self) {
        self.sync_doc_size();
        let doc = &self.doc;
        self.selected.retain(|id| doc.get(*id).is_some());
        self.live.clear();
        self.live_release_pending = false;
    }

    pub(super) fn do_undo(&mut self) {
        if self.edit.is_some() {
            return;
        }
        if let Some(d) = self.undo.undo(&self.doc) {
            self.doc = d;
            self.after_history_change();
        }
    }

    pub(super) fn do_redo(&mut self) {
        if self.edit.is_some() {
            return;
        }
        if let Some(d) = self.undo.redo(&self.doc) {
            self.doc = d;
            self.after_history_change();
        }
    }

    pub(super) fn delete_selected(&mut self) {
        let ids = std::mem::take(&mut self.selected);
        if ids.is_empty() {
            return;
        }
        let pre = self.doc.clone();
        let mut steps = false;
        for id in ids {
            if let Some(removed) = self.doc.remove(id) {
                steps |= matches!(removed.kind, ShapeKind::Step { .. });
            }
        }
        if steps {
            self.doc.renumber_steps();
        }
        if self.doc != pre {
            self.undo.push(&pre);
        }
    }

    // ---------------------------------------------------------------- copies and order

    /// Ctrl+C with shapes selected: copy them inside the editor (the image is not copied).
    pub(super) fn copy_shapes(&mut self) {
        self.clipboard = shapes::collect(&self.doc, &self.selected);
        self.pastes = 0;
        let n = self.clipboard.len();
        self.flash_notice(&if n == 1 {
            "Copied 1 Shape: Ctrl+V to Paste".to_string()
        } else {
            format!("Copied {n} Shapes: Ctrl+V to Paste")
        });
    }

    /// Add `src` again, `steps` copy offsets down and to the right, and select the copies.
    fn add_offset_copies(&mut self, src: Vec<Shape>, steps: u32, ppp: f32) {
        if src.is_empty() {
            return;
        }
        let measure = self.measurer();
        let k = self.pt_to_img(COPY_OFFSET_PT, ppp) * steps as f32;
        let group = src
            .iter()
            .map(|s| shapes::bounds(s, &measure))
            .reduce(shapes::union)
            .unwrap_or_default();
        let (w, h) = self.image_size();
        let off = shapes::paste_offset(group, (k, k), w, h);
        let pre = self.doc.clone();
        let ids = shapes::add_copies(&mut self.doc, &src, off);
        self.undo.push(&pre);
        self.selected = ids;
        if self.tool != Tool::Select {
            self.tool = Tool::Select;
        }
    }

    /// Ctrl+V: paste shapes copied with Ctrl+C; each paste lands one step further.
    pub(super) fn paste_shapes(&mut self, ppp: f32) {
        if self.clipboard.is_empty() {
            return;
        }
        self.pastes += 1;
        self.add_offset_copies(self.clipboard.clone(), self.pastes, ppp);
    }

    /// Ctrl+D: duplicate the selected shapes next to the originals.
    pub(super) fn duplicate_selected(&mut self, ppp: f32) {
        let src = shapes::collect(&self.doc, &self.selected);
        self.add_offset_copies(src, 1, ppp);
    }

    /// Ctrl+] / Ctrl+[ (one step), with Shift to the front / back.
    pub(super) fn reorder(&mut self, how: i32) {
        if self.selected.is_empty() {
            return;
        }
        let pre = self.doc.clone();
        let ids = self.selected.clone();
        let changed = match how {
            1 => shapes::bring_forward(&mut self.doc, &ids),
            -1 => shapes::send_backward(&mut self.doc, &ids),
            2 => shapes::bring_to_front(&mut self.doc, &ids),
            -2 => shapes::send_to_back(&mut self.doc, &ids),
            _ => false,
        };
        if changed {
            self.undo.push(&pre);
        }
    }

    // ---------------------------------------------------------------- options

    /// Shapes the context bar and color changes act on: the text being typed, else the
    /// selection.
    fn option_targets(&self) -> Vec<ShapeId> {
        match &self.edit {
            Some(e) => vec![e.id],
            None => self.selected.clone(),
        }
    }

    pub(super) fn apply_shape_opt(&mut self, opt: ShapeOpt) {
        let targets = self.option_targets();
        let pre = self.doc.clone();
        for id in &targets {
            let Some(s) = self.doc.get_mut(*id) else {
                continue;
            };
            match opt {
                ShapeOpt::TextSmaller | ShapeOpt::TextLarger => {
                    shapes::bump_text_size(&mut s.kind, opt == ShapeOpt::TextLarger);
                    if let ShapeKind::Text { size, .. } = s.kind {
                        self.font_size = size.clamp(8.0, 200.0);
                        self.prefs_dirty = true;
                    }
                }
                ShapeOpt::Bold(on) => {
                    shapes::set_bold(&mut s.kind, on);
                    self.options.text_bold = on;
                }
                ShapeOpt::Pill(on) => {
                    shapes::set_pill(s, on);
                    self.options.text_pill = on;
                }
                ShapeOpt::Double(on) => {
                    shapes::set_arrow_double(&mut s.kind, on);
                    self.options.arrow_double = on;
                }
                ShapeOpt::Head(h) => {
                    shapes::set_arrow_head(&mut s.kind, h);
                    self.options.arrow_head = h;
                }
                ShapeOpt::Straighten => shapes::straighten(&mut s.kind),
            }
        }
        if let Some(e) = self.edit.as_mut() {
            e.blink_from = Instant::now();
        } else if self.doc != pre {
            self.undo.push(&pre);
        }
    }

    /// Ctrl+B: toggle bold on the text being typed or the selected text.
    pub(super) fn toggle_bold(&mut self) {
        let on = self
            .option_targets()
            .first()
            .and_then(|id| self.doc.get(*id))
            .map(|s| matches!(s.kind, ShapeKind::Text { bold: true, .. }));
        match on {
            Some(b) => self.apply_shape_opt(ShapeOpt::Bold(!b)),
            None => self.options.text_bold = !self.options.text_bold,
        }
    }

    /// I or the toolbar button: pick a color from the captured image.
    pub(super) fn toggle_eyedropper(&mut self) {
        self.commit_text();
        self.picking = !self.picking;
        self.popup = None;
    }

    // ---------------------------------------------------------------- creation

    /// Add a shape, record undo, select it.
    pub(super) fn create_shape(&mut self, kind: ShapeKind, style: Style) -> ShapeId {
        let pre = self.doc.clone();
        let id = self.doc.add(kind, style);
        self.undo.push(&pre);
        self.select_only(id);
        id
    }

    /// Add a textual shape and start typing on it. The undo snapshot is taken when the edit
    /// commits (so an abandoned empty shape leaves no history entry).
    pub(super) fn create_textual(&mut self, kind: ShapeKind, style: Style) {
        let pre = self.doc.clone();
        let id = self.doc.add(kind, style);
        self.select_only(id);
        self.begin_edit(id, true, pre);
    }

    pub(super) fn begin_edit(&mut self, id: ShapeId, is_new: bool, pre: Doc) {
        let buffer = self
            .doc
            .get(id)
            .and_then(|s| shapes::text_of(&s.kind))
            .unwrap_or("")
            .to_string();
        self.live = vec![id];
        self.live_release_pending = false;
        self.picking = false;
        self.edit = Some(EditState {
            id,
            is_new,
            pre,
            caret: Caret::at(buffer.len()),
            buffer,
            blink_from: Instant::now(),
            patch: None,
            selecting: false,
            history: Default::default(),
            preedit: String::new(),
        });
    }

    /// Finish inline text editing, keeping the text (empty text deletes the shape).
    pub(super) fn commit_text(&mut self) {
        let Some(mut e) = self.edit.take() else {
            return;
        };
        // An unfinished IME composition is dropped (the shape showed it, the buffer did not).
        self.settled_text = if e.preedit.is_empty() {
            e.patch.take()
        } else {
            None
        };
        let empty = e.buffer.trim().is_empty();
        if self.doc.get(e.id).is_some() {
            if empty {
                self.doc.remove(e.id);
                self.selected.retain(|id| *id != e.id);
                if !e.is_new {
                    self.undo.push(&e.pre);
                }
                if e.is_new {
                    // Nothing was ever committed for this shape.
                    self.doc = e.pre;
                }
            } else {
                if let Some(s) = self.doc.get_mut(e.id) {
                    shapes::set_text(&mut s.kind, &e.buffer);
                }
                if self.doc != e.pre {
                    self.undo.push(&e.pre);
                }
            }
        }
        self.release_live();
    }

    pub(super) fn release_live(&mut self) {
        if !self.live.is_empty() {
            self.live_release_pending = true;
        }
    }

    /// Byte offset in the text being typed nearest to image point `p`.
    pub(super) fn caret_at(&self, p: PtF) -> Option<usize> {
        let e = self.edit.as_ref()?;
        if !e.preedit.is_empty() {
            // The shape shows the IME composition, which is not in the buffer.
            return None;
        }
        let shape = self.doc.get(e.id)?;
        let (size, bold, lines) = stinkshot_core::annotate::text_layout(&self.fonts, &shape.kind)?;
        let first = lines.first()?;
        let lh = size * stinkshot_core::fonts::LINE_HEIGHT;
        let row = (((p.y - first.origin.y) / lh).floor().max(0.0) as usize).min(lines.len() - 1);
        let line = &lines[row];
        let text = &e.buffer;
        let mut best = (line.range.start, f32::INFINITY);
        let mut i = line.range.start;
        loop {
            let x = line.origin.x
                + self
                    .fonts
                    .line_width(&text[line.range.start..i], size, bold);
            let d = (x - p.x).abs();
            if d < best.1 {
                best = (i, d);
            }
            if i >= line.range.end {
                break;
            }
            i += text[i..].chars().next().map_or(1, |c| c.len_utf8());
        }
        Some(best.0)
    }

    // ---------------------------------------------------------------- accepting / finishing

    pub(super) fn accept_region(&mut self, r: PxRect, shared: &mut Shared) {
        let r = selection::clamp_rect(r, self.local_bounds());
        if let Kind::Select(purpose) = self.kind {
            self.finish_region(shared, purpose, r);
            return;
        }
        self.set_sel(r);
        self.adding_region = false;
        self.set_tool(Tool::Select);
        self.drag = Drag::None;
    }

    fn persist_prefs(&mut self, shared: &mut Shared) {
        let ed = &mut shared.config.editor;
        let mut dirty = false;
        if self.prefs_dirty {
            let hex = to_hex_color(self.color);
            if ed.color != hex {
                ed.color = hex;
                dirty = true;
            }
            if (ed.stroke_width - self.width).abs() > 0.01 {
                ed.stroke_width = self.width;
                dirty = true;
            }
            if (ed.font_size - self.font_size).abs() > 0.01 {
                ed.font_size = self.font_size;
                dirty = true;
            }
        }
        let recent: Vec<String> = self.recent.iter().map(|c| to_hex_color(*c)).collect();
        if ed.recent_colors != recent {
            ed.recent_colors = recent;
            dirty = true;
        }
        let styles = self.tool_style_config();
        if ed.tool_styles != styles {
            ed.tool_styles = styles;
            dirty = true;
        }
        if dirty {
            shared.config_dirty = true;
        }
    }

    fn remember_region(&self, shared: &mut Shared, virtual_rect: PxRect) {
        shared.config.capture.last_region = Some(virtual_rect);
        shared.config_dirty = true;
    }

    fn finish_region(&mut self, shared: &mut Shared, purpose: RegionPurpose, local: PxRect) {
        let rect = hover::local_to_virtual(local, self.bounds);
        let source = self.source_info(local);
        self.remember_region(shared, rect);
        shared.outbox.push(Command::RegionChosen {
            purpose,
            rect,
            source,
        });
        self.closed = true;
    }

    pub(super) fn finish(&mut self, ctx: &Context, shared: &mut Shared, action: FinishAction) {
        if self.finishing.is_some() || self.closed {
            return;
        }
        self.commit_text();
        self.combine_regions();
        let Some(sel) = self.sel else { return };
        let base = self.ensure_base();
        let mut doc = self.doc.clone();
        doc.base_width = base.width;
        doc.base_height = base.height;
        let rect = hover::local_to_virtual(sel, self.bounds);

        // Beautify frames image exports (never Copy Text), on the worker below.
        let frame = (self.beautify.enabled && action != FinishAction::CopyText)
            .then(|| self.beautify.clone());
        let ready: Option<Rgba> = if doc.shapes.is_empty() {
            Some((*base).clone())
        } else if self.render_ok
            && let Some(flat) = self.renderer.as_ref().and_then(|r| r.image_for(&doc))
            && flat.width == base.width
            && flat.height == base.height
        {
            // The worker's image is exactly the flattened output when it shows this document.
            Some(flat)
        } else {
            None
        };
        let ready = match (ready, &frame) {
            (Some(flat), None) => {
                self.complete(shared, action, base, doc, (flat, None), rect);
                return;
            }
            (r, _) => r,
        };
        let (tx, rx) = channel();
        let (b, d, fonts, ctx2) = (base.clone(), doc.clone(), self.fonts.clone(), ctx.clone());
        let spawned = std::thread::Builder::new()
            .name("overlay-finish".into())
            .spawn(move || {
                let flat = match ready {
                    Some(f) => Some(f),
                    None => render::flatten(&b, &d, &fonts),
                };
                let r = match (flat, frame) {
                    (Some(f), Some(fr)) => {
                        Some((stinkshot_core::beautify::apply(&f, &fr), Some(f)))
                    }
                    (f, _) => f.map(|f| (f, None)),
                };
                let _ = tx.send(r);
                ctx2.request_repaint();
            });
        match spawned {
            Ok(_) => {
                self.finishing = Some(Finishing {
                    rx,
                    action,
                    base,
                    doc,
                    rect,
                })
            }
            Err(e) => {
                log::error!("overlay finish thread: {e}");
                shared.outbox.push(Command::Notify {
                    title: "Could Not Finish Capture".into(),
                    body: "Stinkshot could not start the final render. Press Enter to try again."
                        .into(),
                    error: true,
                })
            }
        }
    }

    pub(super) fn poll_finish(&mut self, ctx: &Context, shared: &mut Shared) {
        let Some(f) = &self.finishing else { return };
        match f.rx.try_recv() {
            Ok(Some(out)) => {
                if let Some(f) = self.finishing.take() {
                    self.complete(shared, f.action, f.base, f.doc, out, f.rect);
                }
            }
            Ok(None) | Err(TryRecvError::Disconnected) => {
                self.finishing = None;
                shared.outbox.push(Command::Notify {
                    title: "Could Not Render Annotations".into(),
                    body: "The capture was not saved. Try again, or remove the last annotation."
                        .into(),
                    error: true,
                });
            }
            Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(16)),
        }
    }

    fn complete(
        &mut self,
        shared: &mut Shared,
        action: FinishAction,
        base: Arc<Rgba>,
        doc: Doc,
        (flat, unframed): (Rgba, Option<Rgba>),
        rect: PxRect,
    ) {
        self.persist_prefs(shared);
        self.persist_beautify(shared);
        let sel = self.sel.unwrap_or_default();
        let (mode, editing, source) = match &self.annotate_meta {
            Some((m, e, s)) => (*m, *e, s.clone()),
            None => ("region", None, self.source_info(sel)),
        };
        if self.annotate_meta.is_none() {
            self.remember_region(shared, rect);
        }
        shared
            .outbox
            .push(Command::CaptureDone(Box::new(CaptureOutput {
                base: (*base).clone(),
                doc,
                flattened: flat,
                unframed,
                rect,
                action,
                source,
                mode,
                editing,
            })));
        self.closed = true;
    }

    // ---------------------------------------------------------------- toolbar actions

    /// Toolbar grip: drag undocks and moves the whole toolbar, double click docks it again.
    /// The undocked spot is remembered for the next overlays of this session.
    fn grip(&mut self, ctx: &Context, ev: GripEvent) {
        let ppp = ctx.pixels_per_point();
        let px = |p: Pos2| (p.x * ppp, p.y * ppp);
        match ev {
            GripEvent::Start(p) => {
                // Start from where the bar is drawn (docked, undocked, or a remembered spot
                // that is ignored on this monitor). Nothing undocks until the pointer moves:
                // egui also starts a drag on a long press that never moves.
                let start_origin = self.bar_drawn_at.or(self.bar_origin).unwrap_or_default();
                self.grip_drag = Some(toolbar::GripDrag::new(px(p), start_origin));
                self.popup = None;
            }
            GripEvent::Move(p) => {
                if let Some(d) = self.grip_drag.as_mut()
                    && d.track(px(p), toolbar::UNDOCK_THRESHOLD_PX)
                {
                    self.bar_origin = Some(d.target(px(p)));
                    self.bar_from_memory = false;
                }
            }
            GripEvent::End => {
                let moved = self.grip_drag.take().is_some_and(|d| d.moved);
                // Store the clamped spot, not the raw pointer target, so the next drag
                // starts from where the bar really is.
                if moved && let (Some(origin), Some(sel)) = (self.bar_origin, self.sel) {
                    let (at, _, _) = self.undocked_bars(origin, ppp, self.canvas_rect_screen(sel));
                    self.bar_origin = Some(at);
                    toolbar::remember(Some(toolbar::to_virtual(at, self.bounds)));
                }
            }
            GripEvent::Redock => {
                self.grip_drag = None;
                self.bar_origin = None;
                self.bar_from_memory = false;
                toolbar::remember(None);
            }
        }
        ctx.request_repaint();
    }

    pub(super) fn apply_actions(
        &mut self,
        ctx: &Context,
        shared: &mut Shared,
        actions: Vec<UiAction>,
    ) {
        for a in actions {
            match a {
                UiAction::Tool(t) => self.set_tool(t),
                UiAction::Finish(f) => self.finish(ctx, shared, f),
                UiAction::Cancel => self.cancel(shared),
                UiAction::Color(c) => {
                    self.set_color(c);
                    self.popup = None;
                }
                UiAction::Width(w) => {
                    self.set_width(w);
                    self.popup = None;
                }
                UiAction::Eyedropper => self.toggle_eyedropper(),
                UiAction::Shape(o) => self.apply_shape_opt(o),
                UiAction::ZoomFit => self.set_view(View::IDENTITY),
                UiAction::ToggleAddRegions => {
                    self.add_regions = !self.add_regions;
                    if self.add_regions {
                        self.flash_notice("Drag to add regions. Enter copies them all.");
                    }
                }
                UiAction::Ratio(r) => {
                    // The size chip ends typing (a resize may move the region origin).
                    self.commit_text();
                    self.aspect = r;
                    if let (Some(rr), Some(sel)) = (r, self.sel)
                        && self.sel_editable()
                    {
                        let new = selection::apply_ratio(sel, rr, self.sel_bounds());
                        self.set_sel(new);
                    }
                }
                UiAction::OpenExact => {
                    // The Exact field takes the keyboard: typing on the image ends.
                    self.commit_text();
                    if let Some(sel) = self.sel {
                        self.exact_text = format!("{} x {}", sel.w, sel.h);
                    }
                    self.exact_open = true;
                    self.exact_just_opened = true;
                }
                UiAction::ExactSize(w, h) => {
                    self.commit_text();
                    if let Some(sel) = self.sel {
                        let new = selection::apply_exact_size(sel, w, h, self.sel_bounds());
                        self.set_sel(new);
                        self.aspect = None;
                    }
                    self.exact_open = false;
                }
                UiAction::CloseExact => self.exact_open = false,
                UiAction::TogglePopup(p) => {
                    self.picking = false;
                    self.popup = if self.popup == Some(p) { None } else { Some(p) };
                }
                UiAction::Grip(ev) => self.grip(ctx, ev),
                UiAction::FindSensitive => {
                    self.scan = None;
                    if self.beautify_open {
                        self.close_beautify(shared);
                    }
                    self.toggle_smart_redact(ctx, shared)
                }
                UiAction::Smart(a) => self.apply_smart_action(shared, a),
                UiAction::ToggleScan => self.toggle_scan(ctx, shared),
                UiAction::Scan(a) => self.apply_scan_action(a),
                UiAction::ToggleBeautify => self.toggle_beautify(shared),
                UiAction::Beautify(a) => self.apply_beautify_action(shared, a),
                UiAction::Picker(a) => self.apply_picker_action(shared, a),
            }
        }
    }
}
