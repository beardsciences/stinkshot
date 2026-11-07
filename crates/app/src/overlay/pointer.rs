//! Pointer handling: hover snapping, region dragging, selection resize/move, shape
//! creation, selection, move and resize.

use egui::PointerButton;

use super::selection::{hit_handle, px_to_f, resize_px_rect};
use super::shapes::ShapeHandle;
use super::*;

/// Shown when a click outside the selection is ignored because annotations exist.
const NOTICE_KEEPS_ANNOTATIONS: &str =
    "Annotations Keep This Region: Press Esc to Discard and Start Over";

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

impl Overlay {
    /// Rect being dragged out (overlay-local px) for painting, if a region drag is active.
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
                self.effective_aspect(shift),
            );
            return Some(r);
        }
        None
    }

    pub(super) fn handle_pointer(&mut self, ctx: &Context, shared: &mut Shared, inp: &Input) {
        let ppp = ctx.pixels_per_point();
        if let Some(p) = inp.pos {
            self.last_pointer = Some(p);
        }
        if self.edit.is_none() && self.exact_open && inp.pressed && !inp.over_ui {
            self.exact_open = false;
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
        let Some(p) = inp.pos else { return };
        if inp.over_ui {
            return;
        }
        if self.edit.is_some() {
            return;
        }
        if self.sel.is_none() {
            self.update_snap(ctx, shared, p, inp);
            ctx.set_cursor_icon(CursorIcon::Crosshair);
            return;
        }
        if let (Some(src), true) = (self.magnifier_source, self.tool == Tool::Magnifier) {
            let _ = src;
            self.magnifier_cursor = Some(self.to_image(p));
        }
        ctx.set_cursor_icon(self.cursor_for(p, ppp));
    }

    fn update_snap(&mut self, ctx: &Context, shared: &Shared, p: (f32, f32), inp: &Input) {
        let local = (p.0.floor() as i32, p.1.floor() as i32);
        let abs = PxPoint {
            x: local.0 + self.bounds.x,
            y: local.1 + self.bounds.y,
        };
        let win = windows_enum::window_at(&self.windows, abs).cloned();
        let (mon_local, _) = self.monitor_at(local);
        let mon_abs = hover::local_to_virtual(mon_local, self.bounds);

        let mut rects: Vec<PxRect> = Vec::new();
        let mut labels: Vec<stinkshot_platform::uia::ElementLabel> = Vec::new();
        if let (Some(s), true) = (&self.snapper, shared.config.capture.snap_to_elements) {
            if self.snap_req != Some(abs) {
                if let Some(w) = &win {
                    s.request(w.hwnd, abs);
                }
                self.snap_req = Some(abs);
            }
            if let Some(res) = s.latest() {
                // Ignore results that belong to another window than the one under the cursor.
                let in_window = match (&win, res.rects.last()) {
                    (Some(w), Some(top)) => {
                        let grown =
                            PxRect::new(w.rect.x - 3, w.rect.y - 3, w.rect.w + 6, w.rect.h + 6);
                        grown
                            .intersect(top)
                            .is_some_and(|i| i.area() * 10 >= top.area() * 9)
                    }
                    _ => false,
                };
                if in_window {
                    rects = res.rects.clone();
                    labels = res.labels.clone();
                }
                if res.point != abs {
                    ctx.request_repaint_after(Duration::from_millis(16));
                }
            } else {
                ctx.request_repaint_after(Duration::from_millis(16));
            }
        }

        let sig = rects.first().copied();
        if sig != self.snap_sig {
            self.snap_sig = sig;
            self.snap_level = 0;
            self.snap_wheel_used = false;
        }
        if inp.wheel_y != 0.0 {
            self.snap_level =
                hover::cycle_level(self.snap_level, inp.wheel_y, rects.len().saturating_sub(1));
            self.snap_wheel_used = true;
        }
        let picked = hover::pick_snap_rect(
            &rects,
            self.snap_level,
            abs,
            win.map(|w| w.rect),
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
        let measure = self.measurer();
        let img = self.to_image(p);
        if self.annotating()
            && let Some(s) = self.selected.and_then(|id| self.doc.get(id))
            && let Some(h) =
                shapes::hit_handle(s, img, SHAPE_HANDLE_PT * ppp / self.scale, &measure)
        {
            return match h {
                ShapeHandle::Rect(h) | ShapeHandle::Src(h) => handle_cursor(h),
                _ => CursorIcon::Crosshair,
            };
        }
        if self.sel_editable()
            && self.tool == Tool::Select
            && let Some(h) = hit_handle(px_to_f(sel), p, SEL_HANDLE_PT * ppp)
        {
            return handle_cursor(h);
        }
        let inside = sel.contains(PxPoint {
            x: p.0.floor() as i32,
            y: p.1.floor() as i32,
        });
        match self.tool {
            Tool::Select => {
                if self.annotating()
                    && shapes::hit_test(&self.doc, img, SHAPE_TOL_PT * ppp / self.scale, &measure)
                        .is_some()
                {
                    CursorIcon::Move
                } else if inside && self.sel_editable() {
                    CursorIcon::Grab
                } else {
                    CursorIcon::Default
                }
            }
            Tool::Text if inside => CursorIcon::Text,
            _ if inside => CursorIcon::Crosshair,
            _ => CursorIcon::Default,
        }
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
        let measure = self.measurer();
        let img = self.to_image(p);
        let handle_r = SHAPE_HANDLE_PT * ppp / self.scale;
        let tol = SHAPE_TOL_PT * ppp / self.scale;
        let local_px = PxPoint {
            x: p.0.floor() as i32,
            y: p.1.floor() as i32,
        };
        let inside = sel.contains(local_px);

        // 1. Handles of the selected annotation.
        if self.annotating()
            && let Some(id) = self.selected
        {
            let hit = self
                .doc
                .get(id)
                .and_then(|s| shapes::hit_handle(s, img, handle_r, &measure));
            if let Some(h) = hit {
                self.live = Some(id);
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
            && let Some(h) = hit_handle(px_to_f(sel), p, SEL_HANDLE_PT * ppp)
        {
            self.drag = Drag::ResizeSel {
                handle: h,
                start_sel: sel,
            };
            return;
        }
        // 3. Space moves the selection.
        if inp.space && self.sel_editable() {
            self.drag = Drag::MoveSel {
                start_sel: sel,
                start: p,
            };
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

        let new_region_ok = self.sel_editable() && self.doc.shapes.is_empty();
        let start_region = |s: &mut Overlay| {
            s.hover = None;
            s.selected = None;
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
                    self.selected = Some(id);
                    self.drag = Drag::MoveShape {
                        id,
                        last: img,
                        pre: self.doc.clone(),
                        start: p,
                        moved: false,
                    };
                } else {
                    self.selected = None;
                    if inside && self.sel_editable() {
                        self.drag = Drag::MoveSel {
                            start_sel: sel,
                            start: p,
                        };
                    } else if !inside && new_region_ok {
                        start_region(self);
                    } else if !inside && self.sel_editable() {
                        self.flash_notice(NOTICE_KEEPS_ANNOTATIONS);
                    }
                }
            }
            _ if !inside => {
                if new_region_ok {
                    start_region(self);
                } else if self.sel_editable() {
                    self.flash_notice(NOTICE_KEEPS_ANNOTATIONS);
                }
            }
            Tool::Text => {
                let hit = shapes::hit_test(&self.doc, img, tol, &measure).filter(|id| {
                    self.doc
                        .get(*id)
                        .is_some_and(|s| shapes::is_textual(&s.kind))
                });
                if let Some(id) = hit {
                    self.selected = Some(id);
                    self.begin_edit(id, false, self.doc.clone());
                } else {
                    let style = shapes::style_for_tool(Tool::Text, &self.params().style);
                    let at = self.clamp_image(img);
                    self.create_textual(
                        ShapeKind::Text {
                            pos: at,
                            text: String::new(),
                            size: self.font_size,
                            background: None,
                        },
                        style,
                    );
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
                let a = self.clamp_image(img);
                self.selected = None;
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
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        match drag {
            Drag::None | Drag::Ignore => {}
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
                if dist2(start, cur) >= CLICK_SLOP_PT * ppp {
                    moved = true;
                }
                if inp.released {
                    if moved {
                        let r = selection::drag_rect(
                            round_pt(anchor),
                            round_pt(cur),
                            bounds,
                            self.effective_aspect(inp.shift),
                        );
                        if r.w >= 1 && r.h >= 1 {
                            self.accept_region(r, shared);
                        }
                    } else if let Some(h) = self.hover {
                        self.accept_region(h, shared);
                    }
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
                self.set_sel(selection::move_rect(start_sel, dx, dy, bounds));
                if !inp.released {
                    self.drag = Drag::MoveSel { start_sel, start };
                }
            }
            Drag::ResizeSel { handle, start_sel } => {
                let new = resize_px_rect(
                    start_sel,
                    handle,
                    round_pt(p),
                    bounds,
                    self.effective_aspect(inp.shift),
                );
                self.set_sel(new);
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
                let ip = self.clamp_image(self.to_image(p));
                let cur = ip;
                if matches!(tool, Tool::Pen | Tool::Highlighter)
                    && points.last().is_none_or(|l| shapes::dist(*l, ip) >= 0.5)
                {
                    points.push(ip);
                }
                if inp.released {
                    self.finish_draw(tool, a, cur, points, inp.shift);
                } else {
                    self.drag = Drag::Draw {
                        tool,
                        a,
                        cur,
                        points,
                    };
                }
            }
            Drag::MoveShape {
                id,
                mut last,
                pre,
                start,
                mut moved,
            } => {
                if !moved && dist2(start, p) >= CLICK_SLOP_PT * ppp {
                    moved = true;
                    // From now on the shape is painted live instead of baked into the render.
                    self.live = Some(id);
                    self.live_release_pending = false;
                }
                let ip = self.to_image(p);
                if moved {
                    let (dx, dy) = (ip.x - last.x, ip.y - last.y);
                    if let Some(s) = self.doc.get_mut(id) {
                        shapes::translate(&mut s.kind, dx, dy);
                    }
                    last = ip;
                }
                if inp.released {
                    if self.doc != pre {
                        self.undo.push(&pre);
                    }
                    self.release_live();
                    let double =
                        ctx.input(|i| i.pointer.button_double_clicked(PointerButton::Primary));
                    if double && !moved {
                        let textual = self
                            .doc
                            .get(id)
                            .is_some_and(|s| shapes::is_textual(&s.kind));
                        if textual {
                            self.begin_edit(id, false, self.doc.clone());
                        }
                    }
                } else {
                    self.drag = Drag::MoveShape {
                        id,
                        last,
                        pre,
                        start,
                        moved,
                    };
                }
            }
            Drag::ResizeShape { id, handle, pre } => {
                let ip = self.to_image(p);
                let measure = self.measurer();
                if let Some(s) = self.doc.get_mut(id) {
                    shapes::drag_handle(s, handle, PtF::new(ip.x, ip.y), inp.shift, &measure);
                }
                if inp.released {
                    if self.doc != pre {
                        self.undo.push(&pre);
                    }
                    self.release_live();
                } else {
                    self.drag = Drag::ResizeShape { id, handle, pre };
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
