//! Region overlay + inline annotation editor (docs/design/Main.dc.html).
//!
//! One borderless, topmost, tool-window viewport covering `frozen.bounds` exactly in
//! physical px (use `window_util::set_rect` in `on_created`; the egui scale factor of the
//! spanning window may differ from other monitors, so convert physical px <-> points with
//! `ctx.pixels_per_point()` and draw the frozen texture 1:1 in physical pixels).
//!
//! Behavior summary (full spec in docs/ARCHITECTURE.md, "Overlay"):
//! - Frozen screenshot fills the window; outside the selection is dimmed (palette.overlay_dim).
//! - Hover: highlight the UI element (via `uia::ElementSnapper`) or window under the cursor;
//!   wheel cycles element -> parent -> window; click selects it. Drag draws a free region.
//! - Selection has 8 handles; Space+drag moves; arrows nudge 1px (Shift = 10px, Ctrl =
//!   resize). Size label + aspect presets (Free, 16:9, 4:3, 1:1, exact WxH input) above it.
//! - Loupe near the cursor: 11x11 px grid zoomed, coords, scale, hex color; C copies hex.
//! - After a selection exists, the tool bar (design order) appears below it (or above /
//!   inside when no room); action bar on its right: OCR, Pin, Save, Cancel, Copy (Enter).
//! - Tools create `annotate::ShapeKind`s in base-image px; shapes are selectable, movable,
//!   resizable, deletable (Del), recolorable; Ctrl+Z / Ctrl+Y undo/redo; text editing inline.
//! - Committed shapes render through `stinkshot_core::annotate::render` into a texture
//!   (re-rendered on change, off the UI thread if > 16 ms); the shape being dragged is
//!   previewed with the egui painter.
//! - Finish pushes `Command::CaptureDone` / `RegionChosen` / `Cancelled` and returns Close.
//!
//! Module layout: `selection` (rect math), `shapes` (annotation math), `undo`, `hover`
//! (snap picking), `render` (worker + tiled textures), `paint` (painter drawing), `ui`
//! (egui widgets). This file owns the state machine.

// The shell does not construct `Overlay` until integration; drop this allow then.

mod hover;
mod paint;
mod render;
mod selection;
mod shapes;
mod ui;
mod undo;

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration;

use egui::{Color32, Context, CursorIcon, Event, Key, LayerId, Order, Pos2, Rect, Vec2};
use stinkshot_core::annotate::{Doc, Shape, ShapeId, ShapeKind, Style};
use stinkshot_core::config::{parse_hex_color, to_hex_color};
use stinkshot_core::fonts::Fonts;
use stinkshot_core::{PtF, PxPoint, PxRect, RectF, Rgba};
use stinkshot_platform::uia::ElementSnapper;
use stinkshot_platform::windows_enum::{self, WinInfo};
use stinkshot_platform::{Hwnd, clipboard, monitors, window_util};

use self::paint::ImgToPt;
use self::render::{Renderer, TiledTexture};
use self::selection::{Handle8, Mapping, Ratio};
use self::shapes::{Tool, ToolParams};
use self::ui::{ChipState, Popup, UiAction};
use self::undo::UndoStack;
use crate::state::{
    CaptureOutput, Command, FinishAction, OverlayMode, OverlayRequest, RegionPurpose, Shared,
    SourceInfo, Surface, SurfaceStatus,
};
use crate::theme;

/// Selection handle grab radius and shape handle grab radius, in egui points.
const SEL_HANDLE_PT: f32 = 9.0;
const SHAPE_HANDLE_PT: f32 = 9.0;
const SHAPE_TOL_PT: f32 = 4.0;
/// A press that moves less than this many points is a click, not a drag.
const CLICK_SLOP_PT: f32 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Capture,
    Select(RegionPurpose),
    Annotate,
}

struct EditState {
    id: ShapeId,
    is_new: bool,
    pre: Doc,
    buffer: String,
    just_started: bool,
}

enum Drag {
    None,
    /// Pressed on UI chrome: ignore the rest of the gesture.
    Ignore,
    /// Dragging a new region (or clicking to snap). Coordinates are overlay-local px.
    Selecting {
        anchor: (f32, f32),
        start: (f32, f32),
        cur: (f32, f32),
        moved: bool,
    },
    MoveSel {
        start_sel: PxRect,
        start: (f32, f32),
    },
    ResizeSel {
        handle: Handle8,
        start_sel: PxRect,
    },
    Draw {
        tool: Tool,
        a: PtF,
        cur: PtF,
        points: Vec<PtF>,
    },
    MoveShape {
        id: ShapeId,
        last: PtF,
        pre: Doc,
        start: (f32, f32),
        moved: bool,
    },
    ResizeShape {
        id: ShapeId,
        handle: shapes::ShapeHandle,
        pre: Doc,
    },
}

struct Finishing {
    rx: Receiver<Option<Rgba>>,
    action: FinishAction,
    base: Arc<Rgba>,
    doc: Doc,
    rect: PxRect,
}

/// Per-frame input snapshot. Positions are overlay-local physical px.
#[derive(Default)]
struct Input {
    pos: Option<(f32, f32)>,
    pressed: bool,
    down: bool,
    released: bool,
    shift: bool,
    space: bool,
    wheel_y: f32,
    over_ui: bool,
}

#[derive(Default)]
struct Keys {
    esc: bool,
    enter: bool,
    copy_event: bool,
    save: bool,
    pin: bool,
    text: bool,
    undo: bool,
    redo: bool,
    delete: bool,
    hex: bool,
    tool: Option<Tool>,
    palette: Option<usize>,
    bracket: i32,
    arrows: (i32, i32),
    shift: bool,
    ctrl: bool,
}

pub struct Overlay {
    kind: Kind,
    bounds: PxRect,
    frozen_image: Rgba,
    cursor_local: (i32, i32),
    windows: Vec<WinInfo>,
    last_region: Option<PxRect>,
    fonts: Arc<Fonts>,
    /// Monitor rects in overlay-local px with their DPI scale.
    monitors: Vec<(PxRect, f32)>,
    annotate_meta: Option<(&'static str, Option<stinkshot_history::ItemId>, SourceInfo)>,

    // Selection (overlay-local px). In Annotate mode this is the displayed image rect.
    sel: Option<PxRect>,
    /// Local px per image px (1.0 outside Annotate mode).
    scale: f32,
    aspect: Option<Ratio>,
    exact_open: bool,
    exact_just_opened: bool,
    exact_text: String,
    hover: Option<PxRect>,
    snap_level: usize,
    snap_sig: Option<PxRect>,
    snap_req: Option<PxPoint>,
    snapper: Option<ElementSnapper>,

    // Annotation state.
    base: Option<Arc<Rgba>>,
    doc: Doc,
    undo: UndoStack,
    tool: Tool,
    color: [u8; 4],
    width: f32,
    font_size: f32,
    blur_radius: f32,
    pixelate_block: u32,
    palette: Vec<[u8; 4]>,
    selected: Option<ShapeId>,
    magnifier_source: Option<RectF>,
    magnifier_cursor: Option<PtF>,
    edit: Option<EditState>,
    drag: Drag,
    popup: Option<Popup>,
    bar_sizes: (Vec2, Vec2),
    color_btn: Rect,
    width_btn: Rect,
    prefs_dirty: bool,
    last_pointer: Option<(f32, f32)>,

    // Rendering.
    renderer: Option<Renderer>,
    frozen_tex: Option<TiledTexture>,
    base_tex: Option<TiledTexture>,
    rendered_tex: Option<TiledTexture>,
    rendered_doc: Option<Doc>,
    rendered_img: Option<Arc<Rgba>>,
    submitted: Option<Doc>,
    render_ok: bool,
    /// Shape excluded from cached renders while it is manipulated and painted live.
    live: Option<ShapeId>,
    live_release_pending: bool,
    initialized: bool,

    finishing: Option<Finishing>,
    closed: bool,
}

impl Overlay {
    pub fn new(req: OverlayRequest, shared: &Shared) -> Self {
        let OverlayRequest {
            mode,
            frozen,
            windows,
            last_region,
        } = req;
        let bounds = frozen.bounds;
        let cfg = &shared.config.editor;
        let color = parse_hex_color(&cfg.color).unwrap_or([0xff, 0x8a, 0x3d, 0xff]);
        let palette: Vec<[u8; 4]> = cfg
            .palette
            .iter()
            .filter_map(|h| parse_hex_color(h))
            .collect();
        let palette = if palette.is_empty() {
            vec![[0xff, 0x8a, 0x3d, 0xff]]
        } else {
            palette
        };

        let mon_list = monitors::list();
        let monitors_local: Vec<(PxRect, f32)> = mon_list
            .iter()
            .map(|m| (m.rect.translate(-bounds.x, -bounds.y), m.scale))
            .collect();
        let cursor_local = (frozen.cursor.x - bounds.x, frozen.cursor.y - bounds.y);

        let mut o = Overlay {
            kind: Kind::Capture,
            bounds,
            frozen_image: frozen.image,
            cursor_local,
            windows,
            last_region,
            fonts: shared.fonts.clone(),
            monitors: monitors_local,
            annotate_meta: None,
            sel: None,
            scale: 1.0,
            aspect: None,
            exact_open: false,
            exact_just_opened: false,
            exact_text: String::new(),
            hover: None,
            snap_level: 0,
            snap_sig: None,
            snap_req: None,
            snapper: None,
            base: None,
            doc: Doc::new(1, 1),
            undo: UndoStack::new(),
            tool: Tool::Select,
            color: [color[0], color[1], color[2], 255],
            width: cfg.stroke_width.clamp(1.0, 64.0),
            font_size: cfg.font_size.clamp(8.0, 200.0),
            blur_radius: cfg.blur_radius,
            pixelate_block: cfg.pixelate_block,
            palette,
            selected: None,
            magnifier_source: None,
            magnifier_cursor: None,
            edit: None,
            drag: Drag::None,
            popup: None,
            bar_sizes: (Vec2::new(690.0, 46.0), Vec2::new(280.0, 46.0)),
            color_btn: Rect::NOTHING,
            width_btn: Rect::NOTHING,
            prefs_dirty: false,
            last_pointer: None,
            renderer: None,
            frozen_tex: None,
            base_tex: None,
            rendered_tex: None,
            rendered_doc: None,
            rendered_img: None,
            submitted: None,
            render_ok: true,
            live: None,
            live_release_pending: false,
            initialized: false,
            finishing: None,
            closed: false,
        };

        if shared.config.capture.snap_to_elements {
            match ElementSnapper::new() {
                Ok(s) => o.snapper = Some(s),
                Err(e) => log::warn!("overlay: element snapping unavailable: {e:#}"),
            }
        }

        match mode {
            OverlayMode::Capture => o.kind = Kind::Capture,
            OverlayMode::SelectRegion(p) => o.kind = Kind::Select(p),
            OverlayMode::Annotate {
                base,
                mut doc,
                mode,
                editing,
                source,
            } => {
                o.kind = Kind::Annotate;
                doc.base_width = base.width;
                doc.base_height = base.height;
                o.doc = doc;
                o.annotate_meta = Some((mode, editing, source));
                o.layout_annotate(&base);
                o.base = Some(Arc::new(base));
            }
        }
        o
    }

    /// Center the pre-captured image on the monitor under the cursor (scaled down to fit).
    fn layout_annotate(&mut self, base: &Rgba) {
        let area = self
            .monitors
            .iter()
            .map(|(r, _)| *r)
            .find(|r| {
                r.contains(PxPoint {
                    x: self.cursor_local.0,
                    y: self.cursor_local.1,
                })
            })
            .unwrap_or(PxRect::new(0, 0, self.bounds.w, self.bounds.h));
        // Leave room for the toolbars below the image.
        let usable = PxRect::new(area.x, area.y, area.w, (area.h - 80).max(area.h / 2));
        let s = selection::fit_scale(
            base.width as f32,
            base.height as f32,
            (usable.w as f32, usable.h as f32),
            0.9,
        );
        let (w, h) = (
            (base.width as f32 * s).round().max(1.0),
            (base.height as f32 * s).round().max(1.0),
        );
        let (x, y) = selection::center_in(w, h, usable);
        self.scale = s;
        self.sel = Some(PxRect::new(
            x.round() as i32,
            y.round() as i32,
            w as i32,
            h as i32,
        ));
    }

    // ---------------------------------------------------------------- geometry helpers

    fn local_bounds(&self) -> PxRect {
        PxRect::new(0, 0, self.bounds.w, self.bounds.h)
    }

    fn mapping(&self) -> Option<Mapping> {
        self.sel.map(|s| Mapping {
            origin: (s.x as f32, s.y as f32),
            scale: self.scale,
        })
    }

    fn to_image(&self, p: (f32, f32)) -> PtF {
        let m = self.mapping().unwrap_or(Mapping {
            origin: (0.0, 0.0),
            scale: 1.0,
        });
        let (x, y) = m.to_image(p);
        PtF::new(x, y)
    }

    fn image_size(&self) -> (f32, f32) {
        (self.doc.base_width as f32, self.doc.base_height as f32)
    }

    fn clamp_image(&self, p: PtF) -> PtF {
        let (w, h) = self.image_size();
        PtF::new(p.x.clamp(0.0, w), p.y.clamp(0.0, h))
    }

    fn sel_editable(&self) -> bool {
        !matches!(self.kind, Kind::Annotate)
    }

    fn annotating(&self) -> bool {
        matches!(self.kind, Kind::Capture | Kind::Annotate)
    }

    fn monitor_for(&self, r: PxRect) -> PxRect {
        let c = PxPoint {
            x: r.x + r.w / 2,
            y: r.y + r.h / 2,
        };
        self.monitors
            .iter()
            .map(|(m, _)| *m)
            .find(|m| m.contains(c))
            .unwrap_or(self.local_bounds())
    }

    fn monitor_at(&self, p: (i32, i32)) -> (PxRect, f32) {
        let pt = PxPoint { x: p.0, y: p.1 };
        self.monitors
            .iter()
            .find(|(m, _)| m.contains(pt))
            .copied()
            .unwrap_or((self.local_bounds(), 1.0))
    }

    fn effective_aspect(&self, shift: bool) -> Option<Ratio> {
        if shift { Some((1, 1)) } else { self.aspect }
    }

    fn measurer(&self) -> impl Fn(&str, f32) -> (f32, f32) + 'static {
        let fonts = self.fonts.clone();
        move |t: &str, s: f32| fonts.measure(t, s, false)
    }

    fn params(&self) -> ToolParams {
        ToolParams {
            style: Style {
                color: self.color,
                width: self.width,
                fill: None,
                shadow: true,
            },
            font_size: self.font_size,
            blur_radius: self.blur_radius,
            pixelate_block: self.pixelate_block,
        }
    }

    /// Change the selection rect. Annotations keep their on-screen position when the
    /// origin or size changes (shapes are re-based onto the new top-left).
    fn set_sel(&mut self, new: PxRect) {
        let old = self.sel;
        if self.sel_editable() {
            if let Some(o) = old
                && (o.x, o.y) != (new.x, new.y)
                && !self.doc.shapes.is_empty()
            {
                let (dx, dy) = ((o.x - new.x) as f32, (o.y - new.y) as f32);
                shapes::translate_all(&mut self.doc, dx, dy);
                self.undo.map_docs(|d| shapes::translate_all(d, dx, dy));
            }
            self.doc.base_width = new.w.max(1) as u32;
            self.doc.base_height = new.h.max(1) as u32;
            if old != Some(new) {
                self.base = None;
                self.rendered_tex = None;
                self.rendered_doc = None;
                self.rendered_img = None;
                self.submitted = None;
            }
        }
        self.sel = Some(new);
    }

    /// Unannotated pixels of the selection.
    fn ensure_base(&mut self) -> Arc<Rgba> {
        if let Some(b) = &self.base {
            return b.clone();
        }
        let sel = self.sel.unwrap_or(self.local_bounds());
        let b = Arc::new(self.frozen_image.crop(sel));
        self.base = Some(b.clone());
        b
    }

    fn source_info(&self, local: PxRect) -> SourceInfo {
        let v = hover::local_to_virtual(local, self.bounds);
        let c = PxPoint {
            x: v.x + v.w / 2,
            y: v.y + v.h / 2,
        };
        windows_enum::window_at(&self.windows, c)
            .map(|w| SourceInfo {
                app: w.process.clone(),
                window_title: w.title.clone(),
            })
            .unwrap_or_default()
    }

    // ---------------------------------------------------------------- init

    fn init_once(&mut self, ctx: &Context) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        self.renderer = Some(Renderer::new(ctx));
        self.frozen_tex = Some(TiledTexture::upload(
            ctx,
            "ov-frozen",
            &self.frozen_image,
            true,
        ));
        if matches!(self.kind, Kind::Annotate)
            && let Some(b) = &self.base
        {
            self.base_tex = Some(TiledTexture::upload(ctx, "ov-base", b, false));
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    // ---------------------------------------------------------------- input reading

    fn read_input(ctx: &Context, ppp: f32) -> Input {
        let over_ui = ctx.is_pointer_over_egui();
        ctx.input(|i| Input {
            pos: i.pointer.latest_pos().map(|p| (p.x * ppp, p.y * ppp)),
            pressed: i.pointer.primary_pressed(),
            down: i.pointer.primary_down(),
            released: i.pointer.primary_released(),
            shift: i.modifiers.shift,
            space: i.key_down(Key::Space),
            wheel_y: i
                .events
                .iter()
                .filter_map(|e| {
                    if let Event::MouseWheel { delta, .. } = e {
                        Some(delta.y)
                    } else {
                        None
                    }
                })
                .sum(),
            over_ui,
        })
    }

    fn read_keys(ctx: &Context) -> Keys {
        ctx.input(|i| {
            let cmd = i.modifiers.command;
            let plain = !cmd && !i.modifiers.alt;
            let mut k = Keys {
                shift: i.modifiers.shift,
                ctrl: cmd,
                ..Keys::default()
            };
            k.esc = i.key_pressed(Key::Escape);
            k.enter = i.key_pressed(Key::Enter) && !cmd;
            k.copy_event = i.events.iter().any(|e| matches!(e, Event::Copy));
            k.save = cmd && i.key_pressed(Key::S);
            k.pin = cmd && i.key_pressed(Key::P);
            k.text = cmd && i.key_pressed(Key::T);
            k.undo = cmd && !i.modifiers.shift && i.key_pressed(Key::Z);
            k.redo = cmd && (i.key_pressed(Key::Y) || (i.modifiers.shift && i.key_pressed(Key::Z)));
            k.delete = i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace);
            if plain {
                k.hex = i.key_pressed(Key::C);
                let map: [(Key, Tool); 15] = [
                    (Key::V, Tool::Select),
                    (Key::A, Tool::Arrow),
                    (Key::L, Tool::Line),
                    (Key::R, Tool::Rect),
                    (Key::E, Tool::Ellipse),
                    (Key::P, Tool::Pen),
                    (Key::H, Tool::Highlighter),
                    (Key::T, Tool::Text),
                    (Key::N, Tool::Step),
                    (Key::B, Tool::Blur),
                    (Key::X, Tool::Pixelate),
                    (Key::K, Tool::Redact),
                    (Key::M, Tool::Magnifier),
                    (Key::O, Tool::Callout),
                    (Key::S, Tool::Spotlight),
                ];
                for (key, tool) in map {
                    if i.key_pressed(key) {
                        k.tool = Some(tool);
                    }
                }
                let nums = [
                    Key::Num1,
                    Key::Num2,
                    Key::Num3,
                    Key::Num4,
                    Key::Num5,
                    Key::Num6,
                    Key::Num7,
                    Key::Num8,
                ];
                for (idx, key) in nums.iter().enumerate() {
                    if i.key_pressed(*key) {
                        k.palette = Some(idx);
                    }
                }
                if i.key_pressed(Key::OpenBracket) {
                    k.bracket -= 1;
                }
                if i.key_pressed(Key::CloseBracket) {
                    k.bracket += 1;
                }
            }
            if i.key_pressed(Key::ArrowLeft) {
                k.arrows.0 -= 1;
            }
            if i.key_pressed(Key::ArrowRight) {
                k.arrows.0 += 1;
            }
            if i.key_pressed(Key::ArrowUp) {
                k.arrows.1 -= 1;
            }
            if i.key_pressed(Key::ArrowDown) {
                k.arrows.1 += 1;
            }
            k
        })
    }

    // ---------------------------------------------------------------- keyboard

    fn handle_keys(&mut self, ctx: &Context, shared: &mut Shared, k: &Keys, inp: &Input) {
        // Text editing owns the keyboard (Esc / Ctrl+Enter are handled by the editor itself).
        if self.edit.is_some() {
            return;
        }
        if self.exact_open {
            return;
        }
        if k.hex {
            self.copy_hex(shared);
        }

        if k.esc {
            self.on_escape(shared);
            return;
        }
        if k.enter || k.copy_event {
            match self.kind {
                Kind::Select(_) => {
                    let r = self.sel.or(self.hover);
                    if let Some(r) = r {
                        self.accept_region(r, shared);
                    }
                }
                _ => {
                    if self.sel.is_some() {
                        self.finish(ctx, shared, FinishAction::Copy);
                    } else if let Some(h) = self.hover {
                        self.accept_region(h, shared);
                    }
                }
            }
            return;
        }
        if self.annotating() && self.sel.is_some() {
            if k.save {
                self.finish(ctx, shared, FinishAction::SaveAs);
                return;
            }
            if k.pin {
                self.finish(ctx, shared, FinishAction::Pin);
                return;
            }
            if k.text {
                self.finish(ctx, shared, FinishAction::CopyText);
                return;
            }
            if k.undo {
                self.do_undo();
            }
            if k.redo {
                self.do_redo();
            }
            if k.delete {
                self.delete_selected();
            }
            if let Some(t) = k.tool {
                self.set_tool(t);
            }
            if let Some(idx) = k.palette
                && let Some(c) = self.palette.get(idx).copied()
            {
                self.set_color(c);
            }
            if k.bracket != 0 {
                self.bump_size(k.bracket as f32);
            }
        }

        // Arrows: nudge a selected shape, otherwise the selection rectangle.
        if k.arrows != (0, 0) && self.sel.is_some() && matches!(self.drag, Drag::None) {
            let (dx, dy) = k.arrows;
            let step = if k.shift { 10 } else { 1 };
            if let Some(id) = self.selected.filter(|_| self.annotating()) {
                let pre = self.doc.clone();
                let scale = self.scale.max(0.01);
                let (ix, iy) = (
                    dx as f32 * step as f32 / scale,
                    dy as f32 * step as f32 / scale,
                );
                if let Some(s) = self.doc.get_mut(id) {
                    shapes::translate(&mut s.kind, ix, iy);
                }
                self.undo.push(&pre);
            } else if self.sel_editable() {
                let sel = self.sel.unwrap_or_default();
                let bounds = self.local_bounds();
                let new = if k.ctrl {
                    selection::grow_rect(sel, dx * step, dy * step, bounds)
                } else {
                    selection::move_rect(sel, dx * step, dy * step, bounds)
                };
                self.set_sel(new);
            }
        } else if k.arrows != (0, 0) && self.sel.is_none() {
            // Nudge the hover highlight's cursor-equivalent: move the hover rect selection
            // by confirming the hover first, so arrows work for keyboard-only users.
            if let Some(h) = self.hover
                && self.sel_editable()
                && !matches!(self.kind, Kind::Select(_))
            {
                self.accept_region(h, shared);
            }
        }
        let _ = inp;
    }

    fn on_escape(&mut self, shared: &mut Shared) {
        if !matches!(self.drag, Drag::None | Drag::Ignore) {
            // Abort the gesture in progress; undo any live edit.
            match std::mem::replace(&mut self.drag, Drag::None) {
                Drag::MoveShape { pre, .. } | Drag::ResizeShape { pre, .. } => {
                    self.doc = pre;
                    self.release_live();
                }
                Drag::ResizeSel { start_sel, .. } | Drag::MoveSel { start_sel, .. } => {
                    self.set_sel(start_sel)
                }
                _ => {}
            }
            return;
        }
        if self.popup.is_some() {
            self.popup = None;
            return;
        }
        if self.magnifier_source.take().is_some() {
            return;
        }
        if self.annotating() && (self.selected.is_some() || self.tool != Tool::Select) {
            self.selected = None;
            self.tool = Tool::Select;
            return;
        }
        self.cancel(shared);
    }

    fn cancel(&mut self, shared: &mut Shared) {
        shared.outbox.push(Command::Cancelled);
        self.closed = true;
    }

    fn copy_hex(&mut self, shared: &mut Shared) {
        let Some(p) = self.last_pos_px() else { return };
        let c = paint::sample(&self.frozen_image, p.0, p.1);
        let hex = hover::hex_string(c);
        match clipboard::set_text(&hex) {
            Ok(()) => shared.outbox.push(Command::Notify {
                title: "Color Copied".into(),
                body: hex,
                error: false,
            }),
            Err(e) => shared.outbox.push(Command::Notify {
                title: "Could Not Copy Color".into(),
                body: format!("{e:#}"),
                error: true,
            }),
        }
    }

    fn last_pos_px(&self) -> Option<(i32, i32)> {
        self.last_pointer
            .map(|p| (p.0.floor() as i32, p.1.floor() as i32))
    }
}

// ------------------------------------------------------------------------------------
// The remaining impl blocks live next to the code they belong to.
// ------------------------------------------------------------------------------------

mod frame;
mod ops;
mod pointer;

impl Surface for Overlay {
    fn title(&self) -> String {
        "StinkShot.Overlay".into()
    }

    fn viewport_builder(&self, shared: &Shared) -> egui::ViewportBuilder {
        let _ = shared;
        // Initial guess in points at 100%; `on_created` sets the exact physical rect.
        egui::ViewportBuilder::default()
            .with_title(self.title())
            .with_decorations(false)
            .with_resizable(false)
            .with_transparent(false)
            .with_taskbar(false)
            .with_has_shadow(false)
            .with_always_on_top()
            .with_active(true)
            .with_position(Pos2::new(self.bounds.x as f32, self.bounds.y as f32))
            .with_inner_size(Vec2::new(self.bounds.w as f32, self.bounds.h as f32))
    }

    fn on_created(&mut self, hwnd: Hwnd, shared: &Shared) {
        let _ = shared;
        window_util::set_tool_window(hwnd, true);
        window_util::set_topmost(hwnd, true);
        window_util::set_rect(hwnd, self.bounds);
        window_util::exclude_from_capture(hwnd, true);
        window_util::force_foreground(hwnd);
    }

    fn show(&mut self, ctx: &Context, shared: &mut Shared) -> SurfaceStatus {
        self.frame(ctx, shared)
    }
}
