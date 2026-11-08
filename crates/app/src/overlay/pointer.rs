//! Pointer handling: hover snapping, region dragging, selection resize/move, shape
//! creation, selection (click, Shift+click, marquee), moving and resizing with smart
//! guides, zoom and pan, the eyedropper, and caret placement while typing on the image.

use super::selection::{hit_handle, px_to_f, resize_px_rect};
use super::shapes::ShapeHandle;
use super::*;

fn dist2(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

fn round_pt(p: (f32, f32)) -> (i32, i32) {
    (p.0.round() as i32, p.1.round() as i32)
}

fn handle_cursor(h: Handle8) -> CursorIcon {
    match h {
        Handle8::TopLeft | Handle8::BottomRight => CursorIcon::ResizeNwSe,
        Handle8::TopRight | Handle8::BottomLeft => CursorIcon::ResizeNeSw,
        Handle8::Top | Handle8::Bottom => CursorIcon::ResizeVertical,
        Handle8::Left | Handle8::Right => CursorIcon::ResizeHorizontal,
    }
}

/// Cursor for a shape handle: resize arrows on box handles, a grab hand on points.
fn shape_handle_cursor(h: ShapeHandle, dragging: bool) -> CursorIcon {
    match h {
        ShapeHandle::Rect(h) | ShapeHandle::Src(h) => handle_cursor(h),
        ShapeHandle::P1 | ShapeHandle::P2 | ShapeHandle::Mid | ShapeHandle::Tail => {
            if dragging {
                CursorIcon::Grabbing
            } else {
                CursorIcon::Grab
            }
        }
    }
}

impl Overlay {
    /// Rect being dragged out (canvas px) for painting, if a region drag is active.
    pub(super) fn preview_rect(&self, shift: bool) -> Option<PxRect> {
        if let Drag::Selecting {
            anchor,
            cur,
            moved: true,
            ..
        } = &self.drag
        {
            let r = selection::drag_rect(
                round_pt(*anchor),
                round_pt(*cur),
                self.local_bounds(),
                self.effective_aspect(shift && !self.adding_region),
            );
            return Some(r);
        }
        None
    }

    /// Marquee rectangle (image px) while rubber-band selecting.
    pub(super) fn marquee_rect(&self) -> Option<RectF> {
        match &self.drag {
            Drag::Marquee { start, cur, .. } => Some(RectF::from_points(*start, *cur)),
            _ => None,
        }
    }

    /// Edges and centres the dragged shapes snap to: the other shapes and the image.
    fn snap_targets(&self, exclude: &[ShapeId]) -> Vec<RectF> {
        let measure = self.measurer();
        let (w, h) = self.image_size();
        let mut v: Vec<RectF> = self
            .doc
            .shapes
            .iter()
            .filter(|s| !exclude.contains(&s.id))
            .map(|s| shapes::bounds(s, &measure))
            .collect();
        v.push(RectF::new(0.0, 0.0, w, h));
        v
    }

    pub(super) fn handle_pointer(&mut self, ctx: &Context, shared: &mut Shared, inp: &Input) {
        let ppp = ctx.pixels_per_point();
        if let Some(p) = inp.pos {
            self.last_pointer = Some(p);
        }
        if let Some(s) = inp.spos {
            self.last_screen = Some(s);
        }
        if self.edit.is_none() && self.exact_open && inp.pressed && !inp.over_ui {
            self.exact_open = false;
        }

        // Ctrl+wheel zooms around the cursor; the plain wheel scrolls a zoomed view.
        if self.sel.is_some() && !inp.over_ui && !self.help_open {
            if (inp.zoom - 1.0).abs() > 1e-4
                && let Some(s) = inp.spos
            {
                self.set_view(view::zoom_by(self.view, s, inp.zoom, self.scale));
            } else if inp.wheel != Vec2::ZERO && !self.view.is_identity() {
                let d = if inp.shift && inp.wheel.x == 0.0 {
                    (inp.wheel.y * ppp, 0.0)
                } else {
                    (inp.wheel.x * ppp, inp.wheel.y * ppp)
                };
                self.set_view(self.view.pan(d));
            }
        }

        // Middle drag pans (any tool, any time a selection exists).
        if matches!(self.drag, Drag::None)
            && inp.middle_pressed
            && self.sel.is_some()
            && !inp.over_ui
            && let Some(s) = inp.spos
        {
            self.drag = Drag::Pan {
                start: s,
                view: self.view,
            };
        }
        if let Drag::Pan { start, view } = self.drag {
            if let Some(s) = inp.spos {
                self.set_view(view.pan((s.0 - start.0, s.1 - start.1)));
            }
            ctx.set_cursor_icon(CursorIcon::Grabbing);
            if !inp.middle_down && !inp.down {
                self.drag = Drag::None;
            }
            return;
        }

        if matches!(self.drag, Drag::Ignore) {
            if !inp.down || inp.released {
                self.drag = Drag::None;
                if std::mem::take(&mut self.tray_double_pending) {
                    self.cancel(shared);
                    shared.outbox.push(Command::TrayDoubleClick);
                }
            }
            return;
        }

        if matches!(self.drag, Drag::None) {
            if inp.pressed && self.help_open {
                // A click closes the cheat sheet and does nothing else.
                self.help_open = false;
                self.drag = Drag::Ignore;
                return;
            }
            if inp.pressed && inp.over_ui {
                self.drag = Drag::Ignore;
                return;
            }
            // The second click of a double click on the tray icon lands here when the
            // overlay opened under the cursor in time: that means History, not a capture.
            if inp.pressed
                && self.sel.is_none()
                && let Some(p) = inp.pos
                && is_tray_double_click(
                    self.from_tray_click,
                    self.opened_at.elapsed(),
                    self.cursor_local,
                    (p.0.floor() as i32, p.1.floor() as i32),
                )
            {
                // Swallow the press and act on the release (see `tray_double_pending`).
                self.tray_double_pending = true;
                self.drag = Drag::Ignore;
                return;
            }
            if inp.pressed {
                if let Some(p) = inp.pos {
                    self.popup = None;
                    if self.text_press(p, inp, ppp) {
                        return;
                    }
                    self.commit_text();
                    self.begin_press(ctx, shared, p, inp, ppp);
                }
            } else {
                self.hover_update(ctx, shared, inp, ppp);
                return;
            }
        }
        if !matches!(self.drag, Drag::None | Drag::Ignore) {
            self.continue_drag(ctx, shared, inp, ppp);
        }
    }

    // ---------------------------------------------------------------- hover

    fn hover_update(&mut self, ctx: &Context, shared: &mut Shared, inp: &Input, ppp: f32) {
        self.hover_handle = None;
        let Some(p) = inp.pos else { return };
        if inp.over_ui {
            return;
        }
        if self.sel.is_none() {
            self.update_snap(ctx, shared, p, inp);
            ctx.set_cursor_icon(CursorIcon::Crosshair);
            return;
        }
        // Adding a region: highlight the window or element under the cursor (click adds it).
        let outside = self.sel.is_some_and(|s| {
            !s.contains(PxPoint {
                x: p.0.floor() as i32,
                y: p.1.floor() as i32,
            })
        });
        if self.add_armed(inp.shift)
            && outside
            && self.region_at(p).is_none()
            && self.tool == Tool::Select
            && self.handle_under(p, ppp).is_none()
        {
            self.update_snap(ctx, shared, p, inp);
            ctx.set_cursor_icon(CursorIcon::Crosshair);
            return;
        }
        self.hover = None;
        if self.edit.is_some() {
            let over_text = self
                .edit
                .as_ref()
                .and_then(|e| self.doc.get(e.id))
                .is_some_and(|s| {
                    let m = self.measurer();
                    shapes::bounds(s, &m)
                        .expand(self.pt_to_img(4.0, ppp))
                        .contains(self.to_image(p))
                });
            if over_text {
                ctx.set_cursor_icon(CursorIcon::Text);
            }
            return;
        }
        if self.picking {
            ctx.set_cursor_icon(CursorIcon::Crosshair);
            return;
        }
        if inp.space && (self.pan_on_space()) {
            ctx.set_cursor_icon(CursorIcon::Grab);
            return;
        }
        if let (Some(_), true) = (self.magnifier_source, self.tool == Tool::Magnifier) {
            self.magnifier_cursor = Some(self.to_image(p));
        }
        self.hover_handle = self.handle_under(p, ppp);
        ctx.set_cursor_icon(self.cursor_for(p, ppp));
    }

    /// Space+drag pans when the view is zoomed or in the editor; otherwise it moves the
    /// region (the long-standing capture gesture).
    fn pan_on_space(&self) -> bool {
        !self.view.is_identity() || matches!(self.kind, Kind::Annotate)
    }

    fn handle_under(&self, p: (f32, f32), ppp: f32) -> Option<HoverHandle> {
        let measure = self.measurer();
        let img = self.to_image(p);
        if self.annotating()
            && let Some(s) = self.sel_one().and_then(|id| self.doc.get(id))
            && let Some(h) =
                shapes::hit_handle(s, img, self.pt_to_img(SHAPE_HANDLE_PT, ppp), &measure)
        {
            return Some(HoverHandle::Shape(h));
        }
        let sel = self.sel?;
        if self.sel_editable()
            && self.tool == Tool::Select
            && let Some(h) = hit_handle(px_to_f(sel), p, self.pt_to_canvas(SEL_HANDLE_PT, ppp))
        {
            return Some(HoverHandle::Sel(h));
        }
        None
    }

    fn update_snap(&mut self, ctx: &Context, shared: &Shared, p: (f32, f32), inp: &Input) {
        let local = (p.0.floor() as i32, p.1.floor() as i32);
        let abs = PxPoint {
            x: local.0 + self.bounds.x,
            y: local.1 + self.bounds.y,
        };
        // Only what is visible at the point: the topmost window over the full Z-ordered
        // stack (untitled popups and our own capturable windows included), with element
        // rects clipped to the part of it no other window covers.
        let target = hover::hover_target(&self.layers, abs);
        let (mon_local, _) = self.monitor_at(local);
        let mon_abs = hover::local_to_virtual(mon_local, self.bounds);

        let mut rects: Vec<PxRect> = Vec::new();
        let mut labels: Vec<stinkshot_platform::uia::ElementLabel> = Vec::new();
        let uia = target.as_ref().and_then(|t| t.uia);
        if let (Some(s), true, Some(t)) = (
            &self.snapper,
            shared.config.capture.snap_to_elements,
            target.as_ref().filter(|t| t.uia.is_some()),
        ) {
            if self.snap_req != Some(abs) {
                if let Some(h) = uia {
                    s.request(h, abs);
                }
                self.snap_req = Some(abs);
            }
            if let Some(res) = s.latest() {
                // Ignore results that belong to another window than the one under the cursor.
                if hover::result_matches_window(res.rects.last(), t.frame) {
                    for (i, r) in occlusion::clip_chain(&res.rects, abs, &t.occluders) {
                        rects.push(r);
                        labels.push(res.labels.get(i).cloned().unwrap_or_default());
                    }
                }
                if res.point != abs {
                    ctx.request_repaint_after(Duration::from_millis(16));
                }
            } else {
                ctx.request_repaint_after(Duration::from_millis(16));
            }
        }

        // No element chain (snapping off, our own window, UIA pending or failed): still
        // offer the window levels, so wheel-up reaches the whole window.
        if rects.is_empty()
            && let Some(t) = target.as_ref()
        {
            for r in &t.levels {
                rects.push(*r);
                labels.push(stinkshot_platform::uia::ElementLabel {
                    control: "Window",
                    name: String::new(),
                });
            }
        }

        let sig = rects.first().copied();
        if sig != self.snap_sig {
            self.snap_sig = sig;
            self.snap_level = 0;
            self.snap_wheel_used = false;
            // Scrolling capture: start at the page (a browser's document element), so one
            // click picks the web page without the tabs, address bar or window frame.
            if matches!(self.kind, Kind::Select(RegionPurpose::Scrolling))
                && let Some(i) = hover::scroll_target_level(&labels)
            {
                self.snap_level = i;
            }
        }
        if inp.wheel.y != 0.0 {
            self.snap_level =
                hover::cycle_level(self.snap_level, inp.wheel.y, rects.len().saturating_sub(1));
            self.snap_wheel_used = true;
        }
        let picked = hover::pick_snap_rect(
            &rects,
            self.snap_level,
            abs,
            target.as_ref().and_then(|t| t.levels.first().copied()),
            Some(mon_abs),
        );
        self.hover = picked.and_then(|(r, lvl)| {
            self.snap_level = lvl;
            hover::virtual_to_local(r, self.bounds)
        });
        // Name the highlighted element once the user starts wheel-cycling, so it is clear
        // which parent is selected.
        let valid = hover::valid_snap_indices(&rects, abs);
        self.snap_label = match (self.snap_wheel_used, valid.get(self.snap_level)) {
            (true, Some(&i)) if self.hover.is_some() => labels
                .get(i)
                .map(|l| snap_label_text(l.control, &l.name, self.snap_level, valid.len())),
            _ => None,
        };
    }

    fn cursor_for(&self, p: (f32, f32), ppp: f32) -> CursorIcon {
        let Some(sel) = self.sel else {
            return CursorIcon::Crosshair;
        };
        match self.hover_handle {
            Some(HoverHandle::Shape(h)) => return shape_handle_cursor(h, false),
            Some(HoverHandle::Sel(h)) => return handle_cursor(h),
            None => {}
        }
        let measure = self.measurer();
        let img = self.to_image(p);
        let inside = sel.contains(PxPoint {
            x: p.0.floor() as i32,
            y: p.1.floor() as i32,
        });
        match self.tool {
            Tool::Select => {
                if self.annotating()
                    && shapes::hit_test(&self.doc, img, self.pt_to_img(SHAPE_TOL_PT, ppp), &measure)
                        .is_some()
                {
                    CursorIcon::Move
                } else if inside && self.select_drag_moves_region() {
                    CursorIcon::Grab
                } else if inside || matches!(self.kind, Kind::Annotate) {
                    CursorIcon::Crosshair
                } else {
                    CursorIcon::Default
                }
            }
            Tool::Text if inside => CursorIcon::Text,
            _ if inside => CursorIcon::Crosshair,
            _ if self.can_start_region() => CursorIcon::Crosshair,
            _ => CursorIcon::Default,
        }
    }

    /// With the Select tool, a drag on empty space inside the region moves the region only
    /// while there is nothing to select (a fresh capture); once shapes exist it draws a
    /// selection marquee.
    fn select_drag_moves_region(&self) -> bool {
        matches!(self.kind, Kind::Capture) && self.doc.shapes.is_empty()
    }

    // ---------------------------------------------------------------- text caret

    /// A press while typing on the image: inside the text it moves the caret (and starts a
    /// selection drag, double click selects a word). Returns true when handled.
    fn text_press(&mut self, p: (f32, f32), inp: &Input, ppp: f32) -> bool {
        let Some(e) = self.edit.as_ref() else {
            return false;
        };
        let Some(shape) = self.doc.get(e.id) else {
            return false;
        };
        let m = self.measurer();
        let img = self.to_image(p);
        if !shapes::bounds(shape, &m)
            .expand(self.pt_to_img(4.0, ppp))
            .contains(img)
        {
            return false;
        }
        let Some(i) = self.caret_at(img) else {
            return false;
        };
        if let Some(e) = self.edit.as_mut() {
            if inp.double_click {
                let r = textedit::word_at(&e.buffer, i);
                e.caret = Caret {
                    anchor: r.start,
                    cursor: r.end,
                };
            } else {
                e.caret.move_to(i, inp.shift);
            }
            e.blink_from = Instant::now();
            e.selecting = !inp.double_click;
        }
        self.drag = Drag::TextSelect;
        true
    }

    // ---------------------------------------------------------------- press

    fn begin_press(
        &mut self,
        _ctx: &Context,
        _shared: &mut Shared,
        p: (f32, f32),
        inp: &Input,
        ppp: f32,
    ) {
        let Some(sel) = self.sel else {
            self.drag = Drag::Selecting {
                anchor: p,
                start: p,
                cur: p,
                moved: false,
            };
            return;
        };
        // Space+drag pans a zoomed view (and always in the editor).
        if inp.space
            && self.pan_on_space()
            && let Some(s) = inp.spos
        {
            self.drag = Drag::Pan {
                start: s,
                view: self.view,
            };
            return;
        }
        // Eyedropper: a click picks the color under the cursor.
        if self.picking {
            let c = self.sample_at(p);
            self.picking = false;
            self.set_color([c[0], c[1], c[2], 255]);
            self.drag = Drag::Ignore;
            return;
        }
        let measure = self.measurer();
        let img = self.to_image(p);
        let handle_r = self.pt_to_img(SHAPE_HANDLE_PT, ppp);
        let tol = self.pt_to_img(SHAPE_TOL_PT, ppp);
        let local_px = PxPoint {
            x: p.0.floor() as i32,
            y: p.1.floor() as i32,
        };
        let inside = sel.contains(local_px);
        let screen = inp.spos.unwrap_or(p);

        // 1. Handles of the selected annotation.
        if self.annotating()
            && let Some(id) = self.sel_one()
        {
            let hit = self
                .doc
                .get(id)
                .and_then(|s| shapes::hit_handle(s, img, handle_r, &measure));
            if let Some(h) = hit {
                self.live = vec![id];
                self.live_release_pending = false;
                self.drag = Drag::ResizeShape {
                    id,
                    handle: h,
                    pre: self.doc.clone(),
                };
                return;
            }
        }
        // 2. Selection handles.
        if self.sel_editable()
            && self.tool == Tool::Select
            && let Some(h) = hit_handle(px_to_f(sel), p, self.pt_to_canvas(SEL_HANDLE_PT, ppp))
        {
            self.drag = Drag::ResizeSel {
                handle: h,
                start_sel: sel,
            };
            return;
        }
        // 3. Space moves the selection (unzoomed capture).
        if inp.space && self.can_start_region() {
            self.drag = Drag::MoveSel {
                start_sel: sel,
                start: p,
            };
            return;
        }
        // 4. Multi-region: an earlier region becomes active, Shift (or the toggle) adds one.
        if !inside && self.press_regions(p, inp.shift) {
            return;
        }
        if !self.annotating() {
            self.hover = None;
            self.drag = Drag::Selecting {
                anchor: p,
                start: p,
                cur: p,
                moved: false,
            };
            return;
        }

        let start_region = |s: &mut Overlay| {
            s.hover = None;
            s.selected.clear();
            s.drag = Drag::Selecting {
                anchor: p,
                start: p,
                cur: p,
                moved: false,
            };
        };

        match self.tool {
            Tool::Select => {
                if let Some(id) = shapes::hit_test(&self.doc, img, tol, &measure) {
                    let mut toggled = false;
                    if inp.shift {
                        if let Some(i) = self.selected.iter().position(|s| *s == id) {
                            self.selected.remove(i);
                            self.drag = Drag::Ignore;
                            return;
                        }
                        self.selected.push(id);
                        toggled = true;
                    } else if !self.selected.contains(&id) {
                        self.select_only(id);
                    }
                    let ids = self.selected.clone();
                    self.drag = Drag::MoveShapes {
                        bbox: shapes::group_bounds(&self.doc, &ids, &measure),
                        ids,
                        start: img,
                        start_screen: screen,
                        applied: (0.0, 0.0),
                        pre: self.doc.clone(),
                        moved: false,
                        toggled,
                    };
                } else if inside && self.select_drag_moves_region() && !inp.shift {
                    self.selected.clear();
                    self.drag = Drag::MoveSel {
                        start_sel: sel,
                        start: p,
                    };
                } else if inside || matches!(self.kind, Kind::Annotate) {
                    let before = self.selected.clone();
                    if !inp.shift {
                        self.selected.clear();
                    }
                    self.drag = Drag::Marquee {
                        start: img,
                        cur: img,
                        additive: inp.shift,
                        before,
                        start_screen: screen,
                    };
                } else if self.can_start_region() {
                    start_region(self);
                } else {
                    self.selected.clear();
                    self.drag = Drag::Ignore;
                }
            }
            _ if !inside => {
                if self.can_start_region() {
                    start_region(self);
                } else {
                    self.drag = Drag::Ignore;
                }
            }
            Tool::Text => {
                let hit = shapes::hit_test(&self.doc, img, tol, &measure).filter(|id| {
                    self.doc
                        .get(*id)
                        .is_some_and(|s| shapes::is_textual(&s.kind))
                });
                if let Some(id) = hit {
                    self.select_only(id);
                    self.begin_edit(id, false, self.doc.clone());
                    if let Some(i) = self.caret_at(img)
                        && let Some(e) = self.edit.as_mut()
                    {
                        e.caret = Caret::at(i);
                    }
                } else {
                    let (kind, style) = shapes::new_text(self.clamp_image(img), &self.params());
                    self.create_textual(kind, style);
                }
                self.drag = Drag::Ignore;
            }
            Tool::Step => {
                let style = shapes::style_for_tool(Tool::Step, &self.params().style);
                let n = self.doc.next_step_number();
                let radius = shapes::step_radius(self.width);
                self.create_shape(
                    ShapeKind::Step {
                        center: self.clamp_image(img),
                        n,
                        radius,
                    },
                    style,
                );
                self.drag = Drag::Ignore;
            }
            Tool::Magnifier if self.magnifier_source.is_some() => {
                if let Some(src) = self.magnifier_source.take() {
                    let (w, h) = (
                        (src.w * 2.0).clamp(48.0, 400.0),
                        (src.h * 2.0).clamp(48.0, 400.0),
                    );
                    let c = self.clamp_image(img);
                    let dest = RectF::new(c.x - w / 2.0, c.y - h / 2.0, w, h);
                    let style = shapes::style_for_tool(Tool::Magnifier, &self.params().style);
                    self.create_shape(
                        ShapeKind::Magnifier {
                            source: src,
                            dest,
                            circular: true,
                        },
                        style,
                    );
                    self.magnifier_cursor = None;
                }
                self.drag = Drag::Ignore;
            }
            t => {
                let mut a = self.clamp_image(img);
                if !inp.alt && !matches!(t, Tool::Pen | Tool::Highlighter) {
                    let tol = self.pt_to_img(snap::SNAP_PT, ppp);
                    let (q, _) = snap::snap_point(a, &self.snap_targets(&[]), tol, true, true);
                    a = q;
                }
                self.selected.clear();
                self.drag = Drag::Draw {
                    tool: t,
                    a,
                    cur: a,
                    points: vec![a],
                };
            }
        }
    }

    // ---------------------------------------------------------------- drag

    fn continue_drag(&mut self, ctx: &Context, shared: &mut Shared, inp: &Input, ppp: f32) {
        let Some(p) = inp.pos.or(self.last_pointer) else {
            return;
        };
        let bounds = self.local_bounds();
        let snap_tol = self.pt_to_img(snap::SNAP_PT, ppp);
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        match drag {
            Drag::None | Drag::Ignore | Drag::Pan { .. } => {}
            Drag::TextSelect => {
                let img = self.to_image(p);
                if let Some(i) = self.caret_at(img)
                    && let Some(e) = self.edit.as_mut()
                    && e.selecting
                {
                    e.caret.move_to(i, true);
                    e.blink_from = Instant::now();
                }
                ctx.set_cursor_icon(CursorIcon::Text);
                if !inp.released && inp.down {
                    self.drag = Drag::TextSelect;
                } else if let Some(e) = self.edit.as_mut() {
                    e.selecting = false;
                    // egui reports a double click on the second release: select the word.
                    if inp.double_click {
                        let r = textedit::word_at(&e.buffer, e.caret.cursor);
                        e.caret = Caret {
                            anchor: r.start,
                            cursor: r.end,
                        };
                    }
                }
            }
            Drag::Selecting {
                mut anchor,
                start,
                mut cur,
                mut moved,
            } => {
                if inp.space {
                    anchor = (anchor.0 + p.0 - cur.0, anchor.1 + p.1 - cur.1);
                }
                cur = p;
                if dist2(start, cur) >= self.pt_to_canvas(CLICK_SLOP_PT, ppp) {
                    moved = true;
                }
                if inp.released {
                    // Shift held to add a region is not the 1:1 constraint.
                    let shift = inp.shift && !self.adding_region;
                    if moved {
                        let r = selection::drag_rect(
                            round_pt(anchor),
                            round_pt(cur),
                            bounds,
                            self.effective_aspect(shift),
                        );
                        if r.w >= 1 && r.h >= 1 {
                            self.accept_region(r, shared);
                        }
                    } else if let Some(h) = self.hover {
                        self.accept_region(h, shared);
                    }
                    self.cancel_add();
                } else {
                    self.drag = Drag::Selecting {
                        anchor,
                        start,
                        cur,
                        moved,
                    };
                }
            }
            Drag::MoveSel { start_sel, start } => {
                let (dx, dy) = round_pt((p.0 - start.0, p.1 - start.1));
                self.set_sel(selection::move_rect(start_sel, dx, dy, self.sel_bounds()));
                ctx.set_cursor_icon(CursorIcon::Grabbing);
                if !inp.released {
                    self.drag = Drag::MoveSel { start_sel, start };
                }
            }
            Drag::ResizeSel { handle, start_sel } => {
                let new = resize_px_rect(
                    start_sel,
                    handle,
                    round_pt(p),
                    self.sel_bounds(),
                    self.effective_aspect(inp.shift),
                );
                self.set_sel(new);
                ctx.set_cursor_icon(handle_cursor(handle));
                if !inp.released {
                    self.drag = Drag::ResizeSel { handle, start_sel };
                }
            }
            Drag::Draw {
                tool,
                a,
                cur: _,
                mut points,
            } => {
                let mut ip = self.clamp_image(self.to_image(p));
                self.guides.clear();
                let freehand = matches!(tool, Tool::Pen | Tool::Highlighter);
                if freehand {
                    if points.last().is_none_or(|l| shapes::dist(*l, ip) >= 0.5) {
                        points.push(ip);
                    }
                } else if !(inp.alt || inp.shift && matches!(tool, Tool::Arrow | Tool::Line)) {
                    let (q, g) =
                        snap::snap_point(ip, &self.snap_targets(&[]), snap_tol, true, true);
                    ip = q;
                    self.guides = g;
                }
                ctx.set_cursor_icon(CursorIcon::Crosshair);
                if inp.released {
                    self.guides.clear();
                    self.finish_draw(tool, a, ip, points, inp.shift);
                } else {
                    self.drag = Drag::Draw {
                        tool,
                        a,
                        cur: ip,
                        points,
                    };
                }
            }
            Drag::MoveShapes {
                ids,
                start,
                start_screen,
                mut applied,
                bbox,
                pre,
                mut moved,
                toggled,
            } => {
                let sp = inp.spos.unwrap_or(start_screen);
                if !moved && dist2(start_screen, sp) >= CLICK_SLOP_PT * ppp {
                    moved = true;
                    // From now on the shapes are painted live instead of baked into the render.
                    self.live = ids.clone();
                    self.live_release_pending = false;
                }
                if moved {
                    let ip = self.to_image(p);
                    let mut d = (ip.x - start.x, ip.y - start.y);
                    if inp.shift {
                        d = shapes::constrain_axis(d);
                    }
                    self.guides.clear();
                    if let (Some(b), false) = (bbox, inp.alt) {
                        let moved_box = RectF::new(b.x + d.0, b.y + d.1, b.w, b.h);
                        let ((sx, sy), g) =
                            snap::snap_rect(moved_box, &self.snap_targets(&ids), snap_tol);
                        // Shift keeps the drag on one axis: only snap along it.
                        let (sx, sy) = if inp.shift {
                            if d.1 == 0.0 { (sx, 0.0) } else { (0.0, sy) }
                        } else {
                            (sx, sy)
                        };
                        d = (d.0 + sx, d.1 + sy);
                        let snapped = RectF::new(b.x + d.0, b.y + d.1, b.w, b.h);
                        self.guides = g
                            .into_iter()
                            .filter(|g| {
                                if g.vertical {
                                    sx != 0.0 || is_on(snapped, g.at, true)
                                } else {
                                    sy != 0.0 || is_on(snapped, g.at, false)
                                }
                            })
                            .collect();
                    }
                    let step = (d.0 - applied.0, d.1 - applied.1);
                    for id in &ids {
                        if let Some(s) = self.doc.get_mut(*id) {
                            shapes::translate(&mut s.kind, step.0, step.1);
                        }
                    }
                    applied = d;
                    ctx.set_cursor_icon(CursorIcon::Grabbing);
                }
                if inp.released {
                    self.guides.clear();
                    if self.doc != pre {
                        self.undo.push(&pre);
                    }
                    self.release_live();
                    if !moved && !toggled && inp.double_click {
                        let one = (ids.len() == 1).then(|| ids[0]);
                        let textual = one
                            .and_then(|id| self.doc.get(id))
                            .is_some_and(|s| shapes::is_textual(&s.kind));
                        if let (Some(id), true) = (one, textual) {
                            self.begin_edit(id, false, self.doc.clone());
                            let img = self.to_image(p);
                            if let Some(i) = self.caret_at(img)
                                && let Some(e) = self.edit.as_mut()
                            {
                                let r = textedit::word_at(&e.buffer, i);
                                e.caret = Caret {
                                    anchor: r.start,
                                    cursor: r.end,
                                };
                            }
                        }
                    }
                } else {
                    self.drag = Drag::MoveShapes {
                        ids,
                        start,
                        start_screen,
                        applied,
                        bbox,
                        pre,
                        moved,
                        toggled,
                    };
                }
            }
            Drag::ResizeShape { id, handle, pre } => {
                let mut ip = self.to_image(p);
                let measure = self.measurer();
                self.guides.clear();
                // Snap the dragged point (box handles on the axes they move, line ends and
                // the callout tail on both). Shift's own constraint wins over snapping.
                if !(inp.alt || inp.shift && matches!(handle, ShapeHandle::P1 | ShapeHandle::P2)) {
                    let (sx, sy) = match handle {
                        ShapeHandle::Rect(h) | ShapeHandle::Src(h) => {
                            (h.h_dir() != 0, h.v_dir() != 0)
                        }
                        ShapeHandle::Mid => (false, false),
                        _ => (true, true),
                    };
                    let text = self
                        .doc
                        .get(id)
                        .is_some_and(|s| matches!(s.kind, ShapeKind::Text { .. }));
                    if (sx || sy) && !text {
                        let (q, g) =
                            snap::snap_point(ip, &self.snap_targets(&[id]), snap_tol, sx, sy);
                        ip = q;
                        self.guides = g;
                    }
                }
                if let Some(s) = self.doc.get_mut(id) {
                    shapes::drag_handle(s, handle, ip, inp.shift, &measure);
                }
                ctx.set_cursor_icon(shape_handle_cursor(handle, true));
                if inp.released {
                    self.guides.clear();
                    // Double click on the curve handle straightens the arrow.
                    if handle == ShapeHandle::Mid
                        && inp.double_click
                        && let Some(s) = self.doc.get_mut(id)
                    {
                        shapes::straighten(&mut s.kind);
                    }
                    if self.doc != pre {
                        self.undo.push(&pre);
                    }
                    self.release_live();
                } else {
                    self.drag = Drag::ResizeShape { id, handle, pre };
                }
            }
            Drag::Marquee {
                start,
                additive,
                before,
                start_screen,
                ..
            } => {
                let cur = self.to_image(p);
                let measure = self.measurer();
                let sp = inp.spos.unwrap_or(start_screen);
                let dragged = dist2(start_screen, sp) >= CLICK_SLOP_PT * ppp;
                let hits = if dragged {
                    shapes::shapes_in_rect(&self.doc, RectF::from_points(start, cur), &measure)
                } else {
                    Vec::new()
                };
                let mut sel = if additive { before.clone() } else { Vec::new() };
                for id in hits {
                    if !sel.contains(&id) {
                        sel.push(id);
                    }
                }
                self.selected = sel;
                ctx.set_cursor_icon(CursorIcon::Crosshair);
                if !inp.released {
                    self.drag = Drag::Marquee {
                        start,
                        cur,
                        additive,
                        before,
                        start_screen,
                    };
                }
            }
        }
    }

    fn finish_draw(&mut self, tool: Tool, a: PtF, cur: PtF, points: Vec<PtF>, shift: bool) {
        match tool {
            Tool::Pen | Tool::Highlighter => {
                let pts = shapes::smooth_points(&points);
                if pts.is_empty() {
                    return;
                }
                let style = shapes::style_for_tool(tool, &self.params().style);
                let kind = if tool == Tool::Pen {
                    ShapeKind::Pen { points: pts }
                } else {
                    ShapeKind::Highlighter { points: pts }
                };
                self.create_shape(kind, style);
            }
            Tool::Magnifier => {
                let r = RectF::from_points(a, cur);
                let r = if shift {
                    let s = r.w.max(r.h);
                    RectF::new(r.x, r.y, s, s)
                } else {
                    r
                };
                if r.w >= 6.0 && r.h >= 6.0 {
                    self.magnifier_source = Some(r);
                }
            }
            t => {
                if let Some((kind, style)) =
                    shapes::shape_from_drag(t, a, cur, shift, &self.params())
                {
                    if shapes::is_textual(&kind) {
                        self.create_textual(kind, style);
                    } else {
                        self.create_shape(kind, style);
                    }
                }
            }
        }
    }
}

/// Is an edge or centre of `r` on the line at `at`?
fn is_on(r: RectF, at: f32, vertical: bool) -> bool {
    let vals = if vertical {
        [r.x, r.x + r.w / 2.0, r.x + r.w]
    } else {
        [r.y, r.y + r.h / 2.0, r.y + r.h]
    };
    vals.iter().any(|v| (v - at).abs() < 0.01)
}
