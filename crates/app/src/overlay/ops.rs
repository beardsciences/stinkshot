//! Editing operations: tools, colors, undo/redo, shape creation and deletion, inline text
//! editing lifecycle, accepting a region, and finishing (flatten + command push).

use super::*;

impl Overlay {
    pub(super) fn set_tool(&mut self, t: Tool) {
        self.commit_text();
        self.tool = if t == self.tool && t != Tool::Select {
            Tool::Select
        } else {
            t
        };
        self.magnifier_source = None;
        self.magnifier_cursor = None;
        if self.tool != Tool::Select {
            self.selected = None;
        }
        self.popup = None;
    }

    pub(super) fn set_color(&mut self, c: [u8; 4]) {
        self.color = [c[0], c[1], c[2], 255];
        self.prefs_dirty = true;
        if let Some(id) = self.selected {
            let pre = self.doc.clone();
            if let Some(s) = self.doc.get_mut(id) {
                shapes::recolor(s, self.color);
            }
            self.undo.push(&pre);
        }
    }

    pub(super) fn set_width(&mut self, w: f32) {
        self.width = w.clamp(1.0, 64.0);
        self.prefs_dirty = true;
        if let Some(id) = self.selected {
            let pre = self.doc.clone();
            if let Some(s) = self.doc.get_mut(id) {
                match &mut s.kind {
                    ShapeKind::Step { radius, .. } => *radius = shapes::step_radius(self.width),
                    ShapeKind::Highlighter { .. } => s.style.width = (self.width * 4.0).max(14.0),
                    _ => s.style.width = self.width,
                }
            }
            self.undo.push(&pre);
        }
    }

    /// `[` / `]`: grow or shrink the selected shape, or the default stroke width.
    pub(super) fn bump_size(&mut self, delta: f32) {
        if let Some(id) = self.selected {
            let pre = self.doc.clone();
            if let Some(s) = self.doc.get_mut(id) {
                shapes::adjust_size(s, delta);
                self.width = s.style.width.clamp(1.0, 64.0);
            }
            self.undo.push(&pre);
        } else {
            self.width = (self.width + delta).clamp(1.0, 64.0);
        }
        self.prefs_dirty = true;
    }

    fn after_history_change(&mut self) {
        if let Some(sel) = self.sel
            && self.sel_editable()
        {
            self.doc.base_width = sel.w.max(1) as u32;
            self.doc.base_height = sel.h.max(1) as u32;
        }
        if self.selected.is_some_and(|id| self.doc.get(id).is_none()) {
            self.selected = None;
        }
        self.live = None;
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
        let Some(id) = self.selected.take() else {
            return;
        };
        let pre = self.doc.clone();
        if let Some(removed) = self.doc.remove(id) {
            if matches!(removed.kind, ShapeKind::Step { .. }) {
                self.doc.renumber_steps();
            }
            self.undo.push(&pre);
        }
    }

    /// Add a shape, record undo, select it.
    pub(super) fn create_shape(&mut self, kind: ShapeKind, style: Style) -> ShapeId {
        let pre = self.doc.clone();
        let id = self.doc.add(kind, style);
        self.undo.push(&pre);
        self.selected = Some(id);
        id
    }

    /// Add a textual shape and open the inline editor on it. The undo snapshot is taken
    /// when the edit commits (so an abandoned empty shape leaves no history entry).
    pub(super) fn create_textual(&mut self, kind: ShapeKind, style: Style) {
        let pre = self.doc.clone();
        let id = self.doc.add(kind, style);
        self.selected = Some(id);
        self.begin_edit(id, true, pre);
    }

    pub(super) fn begin_edit(&mut self, id: ShapeId, is_new: bool, pre: Doc) {
        let buffer = self
            .doc
            .get(id)
            .and_then(|s| shapes::text_of(&s.kind))
            .unwrap_or("")
            .to_string();
        self.live = Some(id);
        self.live_release_pending = false;
        self.edit = Some(EditState {
            id,
            is_new,
            pre,
            buffer,
            just_started: true,
        });
    }

    /// Finish inline text editing, keeping the text (empty text deletes the shape).
    pub(super) fn commit_text(&mut self) {
        let Some(e) = self.edit.take() else { return };
        let empty = e.buffer.trim().is_empty();
        if self.doc.get(e.id).is_some() {
            if empty {
                self.doc.remove(e.id);
                if self.selected == Some(e.id) {
                    self.selected = None;
                }
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
        if self.live.is_some() {
            self.live_release_pending = true;
        }
    }

    // ---------------------------------------------------------------- accepting / finishing

    pub(super) fn accept_region(&mut self, r: PxRect, shared: &mut Shared) {
        let r = selection::clamp_rect(r, self.local_bounds());
        if let Kind::Select(purpose) = self.kind {
            self.finish_region(shared, purpose, r);
            return;
        }
        self.set_sel(r);
        self.tool = Tool::Select;
        self.drag = Drag::None;
    }

    fn persist_prefs(&mut self, shared: &mut Shared) {
        if !self.prefs_dirty {
            return;
        }
        let hex = to_hex_color(self.color);
        if shared.config.editor.color != hex {
            shared.config.editor.color = hex;
            shared.config_dirty = true;
        }
        if (shared.config.editor.stroke_width - self.width).abs() > 0.01 {
            shared.config.editor.stroke_width = self.width;
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
        let Some(sel) = self.sel else { return };
        let base = self.ensure_base();
        let mut doc = self.doc.clone();
        doc.base_width = base.width;
        doc.base_height = base.height;
        let rect = hover::local_to_virtual(sel, self.bounds);

        if doc.shapes.is_empty() {
            let flat = (*base).clone();
            self.complete(shared, action, base, doc, flat, rect);
            return;
        }
        // The cached render is exactly the flattened output when it matches the document.
        if self.render_ok
            && self.rendered_doc.as_ref() == Some(&doc)
            && let Some(img) = self.rendered_img.clone()
        {
            let flat = (*img).clone();
            self.complete(shared, action, base, doc, flat, rect);
            return;
        }
        let (tx, rx) = channel();
        let (b, d, fonts, ctx2) = (base.clone(), doc.clone(), self.fonts.clone(), ctx.clone());
        let spawned = std::thread::Builder::new()
            .name("overlay-finish".into())
            .spawn(move || {
                let r = render::flatten(&b, &d, &fonts);
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
            Ok(Some(img)) => {
                if let Some(f) = self.finishing.take() {
                    self.complete(shared, f.action, f.base, f.doc, img, f.rect);
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
        flat: Rgba,
        rect: PxRect,
    ) {
        self.persist_prefs(shared);
        let sel = self.sel.unwrap_or_default();
        let (mode, editing, source) = match &self.annotate_meta {
            Some((m, e, s)) => (*m, *e, s.clone()),
            None => ("region", None, self.source_info(sel)),
        };
        if self.annotate_meta.is_none() {
            self.remember_region(shared, rect);
        }
        shared.outbox.push(Command::CaptureDone(CaptureOutput {
            base: (*base).clone(),
            doc,
            flattened: flat,
            rect,
            action,
            source,
            mode,
            editing,
        }));
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
                let start_origin = self.bar_origin.or(self.bar_drawn_at).unwrap_or_default();
                self.grip_drag = Some(toolbar::GripDrag {
                    start_pointer: px(p),
                    start_origin,
                });
                self.bar_origin = Some(start_origin);
                self.popup = None;
            }
            GripEvent::Move(p) => {
                if let Some(d) = self.grip_drag {
                    self.bar_origin = Some(d.target(px(p)));
                }
            }
            GripEvent::End => {
                self.grip_drag = None;
                // Store the clamped spot, not the raw pointer target, so the next drag
                // starts from where the bar really is.
                if let (Some(origin), Some(sel)) = (self.bar_origin, self.sel) {
                    let (at, _, _) = self.undocked_bars(origin, ppp, sel);
                    self.bar_origin = Some(at);
                    toolbar::remember(Some(toolbar::to_virtual(at, self.bounds)));
                }
            }
            GripEvent::Redock => {
                self.grip_drag = None;
                self.bar_origin = None;
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
                UiAction::Ratio(r) => {
                    self.aspect = r;
                    if let (Some(rr), Some(sel)) = (r, self.sel)
                        && self.sel_editable()
                    {
                        let new = selection::apply_ratio(sel, rr, self.local_bounds());
                        self.set_sel(new);
                    }
                }
                UiAction::OpenExact => {
                    if let Some(sel) = self.sel {
                        self.exact_text = format!("{} x {}", sel.w, sel.h);
                    }
                    self.exact_open = true;
                    self.exact_just_opened = true;
                }
                UiAction::ExactSize(w, h) => {
                    if let Some(sel) = self.sel {
                        let new = selection::apply_exact_size(sel, w, h, self.local_bounds());
                        self.set_sel(new);
                        self.aspect = None;
                    }
                    self.exact_open = false;
                }
                UiAction::CloseExact => self.exact_open = false,
                UiAction::TogglePopup(p) => {
                    self.popup = if self.popup == Some(p) { None } else { Some(p) };
                }
                UiAction::Grip(ev) => self.grip(ctx, ev),
            }
        }
    }
}
