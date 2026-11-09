//! Region overlay + inline annotation editor (docs/design/Main.dc.html).
//!
//! One borderless, topmost, tool-window viewport covering `frozen.bounds` exactly in
//! physical px (use `window_util::set_rect` in `on_created`; the egui scale factor of the
//! spanning window may differ from other monitors, so convert physical px <-> points with
//! `ctx.pixels_per_point()` and draw the frozen texture 1:1 in physical pixels).
//!
//! Behavior summary (full spec in docs/ARCHITECTURE.md, "Overlay", and
//! docs/notes/app-overlay.md):
//! - Frozen screenshot fills the window; outside the selection is dimmed (palette.overlay_dim).
//! - Hover: highlight the UI element (via `uia::ElementSnapper`) or window under the cursor;
//!   wheel cycles element -> parent -> window; click selects it. Drag draws a free region.
//! - Selection has 8 handles; Space+drag moves; arrows nudge 1px (Shift = 10px, Ctrl =
//!   resize). Size label + aspect presets (Free, 16:9, 4:3, 1:1, exact WxH input) above it.
//!   The region stays adjustable after annotating (shapes keep their place on the image).
//! - Loupe near the cursor: 11x11 px grid zoomed, coords, scale, hex color; C copies hex.
//! - After a selection exists, the tool bar (design order) appears below it (or above /
//!   inside when no room); action bar on its right: OCR, Pin, Save, Cancel, Copy (Enter).
//! - Tools create `annotate::ShapeKind`s in base-image px; shapes are selectable (Shift+click,
//!   marquee), movable with smart guides, resizable, deletable, recolorable, reorderable,
//!   duplicable and copyable; Ctrl+Z / Ctrl+Y undo/redo; text is typed straight onto the
//!   image. Ctrl+wheel zooms, Space or middle drag pans.
//! - Committed shapes render through `stinkshot_core::annotate` into a texture on a worker
//!   (only the area an edit touched is re-rendered); the shapes being dragged are previewed
//!   with the egui painter.
//! - Finish pushes `Command::CaptureDone` / `RegionChosen` / `Cancelled` and returns Close.
//!
//! Coordinates: "canvas" px are overlay-local physical px of the frozen screen (the selection,
//! hover rects and gestures live there); "screen" px are what the window shows, the same as
//! canvas px until the editor is zoomed or panned (`view.rs`). Image px are base-image px
//! (annotations), mapped onto the canvas by `Mapping`.
//!
//! Module layout: `selection` (rect math), `shapes` (annotation math), `snap` (guides),
//! `view` (zoom/pan), `textedit` (caret logic), `undo`, `hover` (snap picking), `render`
//! (worker + tiled textures), `paint` (painter drawing), `ui` (egui widgets), `panels` (shared
//! panel look), `scan` (Codes and Text), `beautify_panel`, `picker` (color picker), `measure`
//! (ruler), `multi` (multi-region capture). This file owns the state machine.

mod beautify_panel;
mod hover;
mod keys;
mod measure;
mod multi;
mod paint;
mod panels;
mod picker;
mod render;
mod scan;
mod selection;
mod shapes;
mod smart_redact;
mod snap;
mod textedit;
mod toolbar;
mod ui;
mod undo;
mod view;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use egui::{Color32, Context, CursorIcon, Event, Key, LayerId, Order, Pos2, Rect, Vec2};
use stinkshot_core::annotate::{Doc, Shape, ShapeId, ShapeKind, Style};
use stinkshot_core::config::{ToolStyle, parse_hex_color, to_hex_color};
use stinkshot_core::fonts::Fonts;
use stinkshot_core::{PtF, PxPoint, PxRect, RectF, Rgba};
use stinkshot_platform::occlusion::{self, Layer};
use stinkshot_platform::uia::ElementSnapper;
use stinkshot_platform::windows_enum::{self, WinInfo};
use stinkshot_platform::{Hwnd, clipboard, monitors, window_util};

use self::paint::ImgToPt;
use self::render::{Renderer, TiledTexture};
use self::selection::{Handle8, Mapping, Ratio};
use self::shapes::{ShapeHandle, ShapeOptions, Tool, ToolParams};
use self::textedit::Caret;
use self::ui::{ChipState, GripEvent, Popup, UiAction};
use self::undo::UndoStack;
use self::view::View;
use crate::l10n::tr;
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
/// Ctrl+D / Ctrl+V offset, in screen points.
const COPY_OFFSET_PT: f32 = 16.0;
/// Frames presented on the cloaked overlay window before it is uncloaked (`prepare_window`).
const REVEAL_AFTER_PRESENTS: u8 = 2;
/// Most recent colors kept.
const RECENT_COLORS: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Capture,
    Select(RegionPurpose),
    Annotate,
}

/// Rendered pixels of the text shape being typed (exactly what the final render draws).
struct TextPatch {
    shape: Shape,
    rect: PxRect,
    tex: Option<egui::TextureHandle>,
}

struct EditState {
    id: ShapeId,
    is_new: bool,
    pre: Doc,
    buffer: String,
    caret: Caret,
    /// When the caret last moved or text changed (the caret stays solid for a moment).
    blink_from: Instant,
    patch: Option<TextPatch>,
    /// Pointer is dragging a text selection.
    selecting: bool,
    /// Ctrl+Z / Ctrl+Y while typing.
    history: textedit::TextHistory,
    /// IME composition in progress (shown in place of the selection, not yet in `buffer`).
    preedit: String,
}

/// What the pointer is over, for handle hover highlights and cursors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HoverHandle {
    Shape(ShapeHandle),
    Sel(Handle8),
}

enum Drag {
    None,
    /// Pressed on UI chrome: ignore the rest of the gesture.
    Ignore,
    /// Dragging a new region (or clicking to snap). Coordinates are canvas px.
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
    /// Moving the selected shapes; `start` is the press in image px, `applied` the offset
    /// already applied to the doc.
    MoveShapes {
        ids: Vec<ShapeId>,
        start: PtF,
        start_screen: (f32, f32),
        applied: (f32, f32),
        bbox: Option<RectF>,
        pre: Doc,
        moved: bool,
        /// Shift+click toggled this shape on press: a click without a drag keeps the toggle.
        toggled: bool,
    },
    ResizeShape {
        id: ShapeId,
        handle: ShapeHandle,
        pre: Doc,
    },
    /// Rubber-band selection with the Select tool (image px).
    Marquee {
        start: PtF,
        cur: PtF,
        additive: bool,
        before: Vec<ShapeId>,
        start_screen: (f32, f32),
    },
    /// Panning the zoomed view (screen px).
    Pan {
        start: (f32, f32),
        view: View,
    },
    /// Selecting text with the pointer while typing on the image.
    TextSelect,
    /// Dragging the Measure tool's ruler from `a` (image px).
    Measure {
        a: PtF,
    },
}

struct Finishing {
    /// The output image, and the image without its Beautify frame when one was applied.
    rx: Receiver<Option<(Rgba, Option<Rgba>)>>,
    action: FinishAction,
    base: Arc<Rgba>,
    doc: Doc,
    rect: PxRect,
}

/// Per-frame input snapshot. `pos` is canvas px, `spos` screen px.
#[derive(Default)]
struct Input {
    pos: Option<(f32, f32)>,
    spos: Option<(f32, f32)>,
    pressed: bool,
    down: bool,
    released: bool,
    middle_pressed: bool,
    middle_down: bool,
    shift: bool,
    alt: bool,
    space: bool,
    /// Plain wheel (no Ctrl), points.
    wheel: Vec2,
    /// Ctrl+wheel / pinch zoom factor this frame (1.0 = none).
    zoom: f32,
    over_ui: bool,
    double_click: bool,
}

#[derive(Default)]
struct Keys {
    esc: bool,
    enter: bool,
    copy_event: bool,
    paste: bool,
    duplicate: bool,
    save: bool,
    pin: bool,
    text: bool,
    smart: bool,
    undo: bool,
    redo: bool,
    delete: bool,
    hex: bool,
    help: bool,
    eyedropper: bool,
    /// Q: Codes and Text panel.
    scan: bool,
    /// F: Beautify panel.
    beautify: bool,
    /// Tab: next color format (picker).
    tab: bool,
    bold: bool,
    /// Z-order: +1 forward, -1 backward, +2 to front, -2 to back.
    order: i32,
    /// Zoom: +1 in, -1 out.
    zoom_step: i32,
    zoom_fit: bool,
    zoom_actual: bool,
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
    /// Z-ordered occlusion stack (topmost first), see `OverlayRequest::layers`.
    layers: Vec<Layer>,
    last_region: Option<PxRect>,
    fonts: Arc<Fonts>,
    /// Monitor rects in overlay-local (screen) px with their DPI scale.
    monitors: Vec<(PxRect, f32)>,
    /// DPI scale of the primary monitor, where winit creates new windows before moving them:
    /// the viewport builder divides the physical rect by it so the window is created at its
    /// final size (no oversized first frame, see `viewport_builder`).
    create_scale: f32,
    /// The overlay window once `prepare_window` has styled and shown it.
    window: Option<Hwnd>,
    /// Set while the window is shown but still cloaked (`prepare_window`), with the number of
    /// frames presented on it since; `after_paint` uncloaks it after `REVEAL_AFTER_PRESENTS`.
    reveal: Option<(Hwnd, u8)>,
    annotate_meta: Option<(&'static str, Option<stinkshot_history::ItemId>, SourceInfo)>,

    // Selection (canvas px). In Annotate mode this is the crop of the displayed image.
    sel: Option<PxRect>,
    /// Multi-region capture: regions added before the active `sel` (canvas px), see
    /// `multi.rs`. Combined into one picture before annotating or finishing.
    extra_regions: Vec<PxRect>,
    /// Add Regions toggle: a drag outside the selection adds a region without Shift.
    add_regions: bool,
    /// The region drag in progress adds a region (the previous one is in `extra_regions`).
    adding_region: bool,
    /// The editor shows a combined multi-region picture (transparent gaps).
    combined: bool,
    /// Canvas px per image px (1.0 outside Annotate mode).
    scale: f32,
    /// Annotate mode: the whole pre-captured image, where its top-left sits on the canvas,
    /// and the crop (image px) the selection shows.
    orig: Option<Arc<Rgba>>,
    img_origin: (f32, f32),
    crop: PxRect,
    /// Zoom and pan (identity = unzoomed).
    view: View,
    aspect: Option<Ratio>,
    exact_open: bool,
    exact_just_opened: bool,
    exact_text: String,
    hover: Option<PxRect>,
    /// Settings > Capture > Snap to Elements, read each frame: the measure tool snaps its
    /// ends to edges when on (Alt flips it either way).
    snap_edges: bool,
    snap_level: usize,
    snap_sig: Option<PxRect>,
    snap_req: Option<PxPoint>,
    snapper: Option<ElementSnapper>,
    /// Label of the highlighted element ("Pane \u{201c}Files\u{201d} \u{b7} Level 2 of 4"),
    /// shown once the wheel was used on the current element chain.
    snap_label: Option<String>,
    snap_wheel_used: bool,

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
    /// Newest first.
    recent: Vec<[u8; 4]>,
    /// Per-tool color and width (seeded from config, saved back on finish).
    tool_styles: HashMap<Tool, ([u8; 4], f32)>,
    /// Sticky arrow and text options for new shapes.
    options: ShapeOptions,
    /// Selected shapes, in the order they were picked.
    selected: Vec<ShapeId>,
    /// Shapes copied with Ctrl+C (editor only) and how often they were pasted.
    clipboard: Vec<Shape>,
    pastes: u32,
    magnifier_source: Option<RectF>,
    magnifier_cursor: Option<PtF>,
    edit: Option<EditState>,
    /// The last typed text's rendered pixels, shown until the cached render includes it (no
    /// flash of the painter's approximation after Esc).
    settled_text: Option<TextPatch>,
    drag: Drag,
    /// Smart guides of the gesture in progress (image px).
    guides: Vec<snap::Guide>,
    hover_handle: Option<HoverHandle>,
    /// Eyedropper is picking (I or the toolbar button).
    picking: bool,
    popup: Option<Popup>,
    bar_sizes: (Vec2, Vec2),
    /// Undocked toolbar origin (top-left of the tool bar, screen px, unclamped); None =
    /// docked under the selection. See `toolbar.rs`.
    bar_origin: Option<(f32, f32)>,
    /// `bar_origin` came from an earlier overlay (not dragged in this one): it applies only
    /// while the selection is on the same monitor, otherwise the bar docks.
    bar_from_memory: bool,
    /// Grip drag in progress.
    grip_drag: Option<toolbar::GripDrag>,
    /// Where the tool bar was drawn last frame (screen px), the start of a first undock.
    bar_drawn_at: Option<(f32, f32)>,
    color_btn: Rect,
    width_btn: Rect,
    prefs_dirty: bool,
    /// Last pointer position, canvas px and screen px.
    last_pointer: Option<(f32, f32)>,
    last_screen: Option<(f32, f32)>,

    // Rendering.
    renderer: Option<Renderer>,
    frozen_tex: Option<TiledTexture>,
    base_tex: Option<TiledTexture>,
    rendered_tex: Option<TiledTexture>,
    rendered_doc: Option<Doc>,
    submitted: Option<Doc>,
    render_ok: bool,
    /// Shapes excluded from cached renders while they are manipulated and painted live.
    live: Vec<ShapeId>,
    live_release_pending: bool,
    initialized: bool,

    finishing: Option<Finishing>,
    closed: bool,
    /// First Esc with unsaved annotations: a second Esc within `DISCARD_WINDOW` cancels.
    discard_armed: Option<Instant>,

    /// Inline hint-bar notice (replaces toasts while the screen is frozen).
    notice: Option<Notice>,
    /// The F1 / ? shortcut cheat sheet is showing.
    help_open: bool,
    /// Find Sensitive Info scan or review panel (see `smart_redact.rs`).
    smart: Option<smart_redact::SmartRedact>,
    /// Codes and Text panel (see `scan.rs`).
    scan: Option<scan::Scan>,
    /// Beautify panel (see `beautify_panel.rs`) and its settings (saved on close / finish).
    beautify_open: bool,
    beautify: stinkshot_core::beautify::Beautify,
    beautify_preview: Option<beautify_panel::Preview>,
    /// The Measure tool's ruler ends (image px), shown while the tool is active.
    ruler: Option<(PtF, PtF)>,
    /// Last C (copy color) result, shown in the loupe plate for a moment.
    hex_flash: Option<HexFlash>,
    /// Opened by a tray icon click (see `OverlayRequest::from_tray_click`).
    from_tray_click: bool,
    /// The press of a tray double click landed here; History opens on its release, so the
    /// button-up cannot fall through to the tray icon once the overlay is gone.
    tray_double_pending: bool,
    opened_at: Instant,
}

/// A short message in the hint bar's leading slot.
struct Notice {
    text: String,
    /// None: until a selection exists.
    until: Option<Instant>,
}

struct HexFlash {
    hex: String,
    ok: bool,
    at: Instant,
}

/// How long a transient notice or the copied-color flash stays.
const NOTICE_SECS: Duration = Duration::from_millis(2500);
const HEX_FLASH: Duration = Duration::from_millis(1400);
/// A press on the tray icon spot this soon after a tray-click overlay opened is the second
/// half of a double click (Windows' default double-click time is 500 ms).
const TRAY_DOUBLE_CLICK: Duration = Duration::from_millis(500);
const TRAY_DOUBLE_CLICK_SLOP_PX: i32 = 8;

/// Whether a press at `p` (overlay-local px) `since_open` after a tray-click overlay opened
/// with the cursor at `cursor` is really the second click of a double click on the icon.
fn is_tray_double_click(
    from_tray: bool,
    since_open: Duration,
    cursor: (i32, i32),
    p: (i32, i32),
) -> bool {
    from_tray
        && since_open < TRAY_DOUBLE_CLICK
        && (p.0 - cursor.0).abs() <= TRAY_DOUBLE_CLICK_SLOP_PX
        && (p.1 - cursor.1).abs() <= TRAY_DOUBLE_CLICK_SLOP_PX
}

/// Text for the wheel-snapping label. `level` is 0 for the innermost element.
fn snap_label_text(control: &str, name: &str, level: usize, count: usize) -> String {
    let name: String = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let name = if name.chars().count() > 40 {
        let cut: String = name.chars().take(39).collect();
        format!("{}\u{2026}", cut.trim_end())
    } else {
        name
    };
    let what = if name.is_empty() {
        control.to_string()
    } else {
        tr!("overlay.element.named", control = control, name = name)
    };
    tr!(
        "overlay.element.level",
        what = what,
        level = level + 1,
        count = count.max(level + 1)
    )
}

/// How long the "press Esc again to discard" prompt waits for the second Esc.
const DISCARD_WINDOW: Duration = Duration::from_secs(3);

/// Whether this Esc should only arm the discard prompt instead of closing the editor.
fn esc_needs_confirm(edited: bool, armed_at: Option<Instant>, now: Instant) -> bool {
    edited && armed_at.is_none_or(|t| now.saturating_duration_since(t) >= DISCARD_WINDOW)
}

/// Push `c` to the front of the recent colors (no duplicates, at most `RECENT_COLORS`).
fn push_recent(recent: &mut Vec<[u8; 4]>, c: [u8; 4]) {
    let c = [c[0], c[1], c[2], 255];
    recent.retain(|r| r[..3] != c[..3]);
    recent.insert(0, c);
    recent.truncate(RECENT_COLORS);
}

/// Default color and width for a tool without a remembered style: redaction boxes start
/// black, everything else uses the editor color.
fn default_tool_style(tool: Tool, color: [u8; 4], width: f32) -> ([u8; 4], f32) {
    match tool {
        Tool::Redact => ([0, 0, 0, 255], width),
        _ => (color, width),
    }
}

fn load_tool_styles(cfg: &stinkshot_core::config::Editor) -> HashMap<Tool, ([u8; 4], f32)> {
    cfg.tool_styles
        .iter()
        .filter_map(|(k, v)| {
            let t = Tool::from_config_key(k)?;
            let c = parse_hex_color(&v.color)?;
            Some((t, ([c[0], c[1], c[2], 255], v.width.clamp(1.0, 64.0))))
        })
        .collect()
}

impl Overlay {
    pub fn new(req: OverlayRequest, shared: &Shared) -> Self {
        let OverlayRequest {
            mode,
            frozen,
            windows,
            layers,
            last_region,
            notice,
            from_tray_click,
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
        let recent: Vec<[u8; 4]> = cfg
            .recent_colors
            .iter()
            .filter_map(|h| parse_hex_color(h))
            .map(|c| [c[0], c[1], c[2], 255])
            .take(RECENT_COLORS)
            .collect();

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
            layers: if layers.is_empty() {
                hover::layers_from_windows(&windows)
            } else {
                layers
            },
            windows,
            last_region,
            fonts: shared.fonts.clone(),
            monitors: monitors_local,
            create_scale: mon_list
                .iter()
                .find(|m| m.primary)
                .map_or(1.0, |m| m.scale)
                .max(0.1),
            window: None,
            reveal: None,
            annotate_meta: None,
            sel: None,
            extra_regions: Vec::new(),
            add_regions: false,
            adding_region: false,
            combined: false,
            scale: 1.0,
            orig: None,
            img_origin: (0.0, 0.0),
            crop: PxRect::default(),
            view: View::IDENTITY,
            aspect: None,
            exact_open: false,
            exact_just_opened: false,
            exact_text: String::new(),
            hover: None,
            snap_edges: shared.config.capture.snap_to_elements,
            snap_level: 0,
            snap_sig: None,
            snap_req: None,
            snapper: None,
            snap_label: None,
            snap_wheel_used: false,
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
            recent,
            tool_styles: load_tool_styles(cfg),
            options: ShapeOptions::default(),
            selected: Vec::new(),
            clipboard: Vec::new(),
            pastes: 0,
            magnifier_source: None,
            magnifier_cursor: None,
            edit: None,
            settled_text: None,
            drag: Drag::None,
            guides: Vec::new(),
            hover_handle: None,
            picking: false,
            popup: None,
            bar_sizes: (Vec2::new(760.0, 46.0), Vec2::new(280.0, 46.0)),
            bar_origin: toolbar::remembered().map(|v| toolbar::to_local(v, bounds)),
            bar_from_memory: true,
            grip_drag: None,
            bar_drawn_at: None,
            color_btn: Rect::NOTHING,
            width_btn: Rect::NOTHING,
            prefs_dirty: false,
            last_pointer: None,
            last_screen: None,
            renderer: None,
            frozen_tex: None,
            base_tex: None,
            rendered_tex: None,
            rendered_doc: None,
            submitted: None,
            render_ok: true,
            live: Vec::new(),
            live_release_pending: false,
            initialized: false,
            finishing: None,
            closed: false,
            discard_armed: None,
            notice: notice.map(|t| Notice {
                text: t.to_string(),
                until: None,
            }),
            help_open: false,
            smart: None,
            scan: None,
            beautify_open: false,
            beautify: cfg.beautify.clone(),
            beautify_preview: None,
            ruler: None,
            hex_flash: None,
            from_tray_click,
            tray_double_pending: false,
            opened_at: Instant::now(),
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
                // The editor centres the image on the cursor's monitor and leaves room for
                // the docked bars under it; a remembered spot would land on the image or on
                // another monitor, so the editor always opens docked.
                o.bar_origin = None;
                doc.base_width = base.width;
                doc.base_height = base.height;
                o.doc = doc;
                o.annotate_meta = Some((mode, editing, source));
                o.layout_annotate(&base);
                let base = Arc::new(base);
                o.orig = Some(base.clone());
                o.base = Some(base);
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
        self.img_origin = (x.round(), y.round());
        self.crop = PxRect::new(0, 0, base.width as i32, base.height as i32);
        self.sel = Some(self.sel_from_crop());
    }

    // ---------------------------------------------------------------- geometry helpers

    fn local_bounds(&self) -> PxRect {
        PxRect::new(0, 0, self.bounds.w, self.bounds.h)
    }

    /// Annotate mode: the canvas rect the crop occupies.
    fn sel_from_crop(&self) -> PxRect {
        let x0 = (self.img_origin.0 + self.crop.x as f32 * self.scale).round() as i32;
        let y0 = (self.img_origin.1 + self.crop.y as f32 * self.scale).round() as i32;
        let x1 = (self.img_origin.0 + self.crop.right() as f32 * self.scale).round() as i32;
        let y1 = (self.img_origin.1 + self.crop.bottom() as f32 * self.scale).round() as i32;
        PxRect::new(x0, y0, (x1 - x0).max(1), (y1 - y0).max(1))
    }

    /// Where the selection may go: the screen, or in Annotate mode the displayed image.
    fn sel_bounds(&self) -> PxRect {
        match (&self.orig, self.kind) {
            (Some(o), Kind::Annotate) => {
                let x0 = self.img_origin.0.round() as i32;
                let y0 = self.img_origin.1.round() as i32;
                PxRect::new(
                    x0,
                    y0,
                    ((o.width as f32 * self.scale).round() as i32).max(1),
                    ((o.height as f32 * self.scale).round() as i32).max(1),
                )
            }
            _ => self.local_bounds(),
        }
    }

    fn mapping(&self) -> Option<Mapping> {
        if matches!(self.kind, Kind::Annotate) {
            return Some(Mapping {
                origin: (
                    self.img_origin.0 + self.crop.x as f32 * self.scale,
                    self.img_origin.1 + self.crop.y as f32 * self.scale,
                ),
                scale: self.scale,
            });
        }
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

    /// Screen points of `pt` per image px.
    fn img_pt(&self, ppp: f32) -> f32 {
        self.scale * self.view.zoom / ppp
    }

    /// `pt` screen points in image px (handle radii, tolerances).
    fn pt_to_img(&self, pt: f32, ppp: f32) -> f32 {
        pt / self.img_pt(ppp)
    }

    /// `pt` screen points in canvas px.
    fn pt_to_canvas(&self, pt: f32, ppp: f32) -> f32 {
        pt * ppp / self.view.zoom
    }

    /// Canvas rect -> screen rect in points.
    fn canvas_rect_pt(&self, r: PxRect, ppp: f32) -> Rect {
        let (x, y, w, h) = self
            .view
            .rect_to_screen((r.x as f32, r.y as f32, r.w as f32, r.h as f32));
        Rect::from_min_size(Pos2::new(x / ppp, y / ppp), Vec2::new(w / ppp, h / ppp))
    }

    /// Canvas rect -> screen px rect (for toolbar and chip placement).
    fn canvas_rect_screen(&self, r: PxRect) -> PxRect {
        let (x, y, w, h) = self
            .view
            .rect_to_screen((r.x as f32, r.y as f32, r.w as f32, r.h as f32));
        PxRect::new(
            x.round() as i32,
            y.round() as i32,
            w.round() as i32,
            h.round() as i32,
        )
    }

    /// Image px -> screen points mapping for painting.
    fn img_to_pt(&self, ppp: f32) -> Option<ImgToPt> {
        let m = self.mapping()?;
        let (x, y) = self.view.to_screen(m.origin);
        Some(ImgToPt {
            origin: Pos2::new(x / ppp, y / ppp),
            k: self.img_pt(ppp),
        })
    }

    fn sel_editable(&self) -> bool {
        true
    }

    /// A press outside the selection may start a new region (not in the editor, whose
    /// region is the image).
    fn can_start_region(&self) -> bool {
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

    /// The undocked origin to lay the bars out from this frame, or None to dock. A spot
    /// remembered from an earlier overlay is ignored while the selection is on another
    /// monitor. `sel` is in screen px.
    fn effective_bar_origin(&self, ppp: f32, sel: PxRect) -> Option<(f32, f32)> {
        let origin = self.bar_origin?;
        if !self.bar_from_memory {
            return Some(origin);
        }
        let mons: Vec<PxRect> = self.monitors.iter().map(|(m, _)| *m).collect();
        toolbar::remembered_applies(
            origin,
            (self.bar_sizes.0.x * ppp, self.bar_sizes.0.y * ppp),
            (self.bar_sizes.1.x * ppp, self.bar_sizes.1.y * ppp),
            8.0 * ppp,
            &mons,
            self.monitor_for(sel),
        )
        .then_some(origin)
    }

    /// Undocked toolbar placement for `origin`: (tool bar pos, action bar pos, monitor),
    /// screen px. Sizes are last frame's, in physical px.
    fn undocked_bars(
        &self,
        origin: (f32, f32),
        ppp: f32,
        sel: PxRect,
    ) -> ((f32, f32), (f32, f32), PxRect) {
        let mons: Vec<PxRect> = self.monitors.iter().map(|(m, _)| *m).collect();
        toolbar::undocked_layout(
            origin,
            (self.bar_sizes.0.x * ppp, self.bar_sizes.0.y * ppp),
            (self.bar_sizes.1.x * ppp, self.bar_sizes.1.y * ppp),
            8.0 * ppp,
            &mons,
            self.monitor_for(sel),
        )
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

    fn measurer(&self) -> impl Fn(&str, f32, bool) -> (f32, f32) + 'static {
        let fonts = self.fonts.clone();
        move |t: &str, s: f32, b: bool| fonts.measure(t, s, b)
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
            options: self.options,
        }
    }

    /// The single selected shape, if exactly one is selected.
    fn sel_one(&self) -> Option<ShapeId> {
        match self.selected.as_slice() {
            [id] => Some(*id),
            _ => None,
        }
    }

    fn select_only(&mut self, id: ShapeId) {
        self.selected = vec![id];
    }

    /// Keep the doc's base size in step with the selection (or the crop).
    fn sync_doc_size(&mut self) {
        match self.kind {
            Kind::Annotate => {
                self.doc.base_width = self.crop.w.max(1) as u32;
                self.doc.base_height = self.crop.h.max(1) as u32;
            }
            _ => {
                if let Some(sel) = self.sel {
                    self.doc.base_width = sel.w.max(1) as u32;
                    self.doc.base_height = sel.h.max(1) as u32;
                }
            }
        }
    }

    /// Change the selection rect (canvas px). Annotations keep their place on the picture:
    /// when the region's origin moves they are re-based onto the new top-left (history too).
    /// In Annotate mode the rect becomes a crop of the image (inside it).
    fn set_sel(&mut self, new: PxRect) {
        let old = self.sel;
        if matches!(self.kind, Kind::Annotate) {
            let Some(orig) = self.orig.clone() else {
                self.sel = Some(new);
                return;
            };
            let to_img = |v: i32, o: f32| ((v as f32 - o) / self.scale).round() as i32;
            let (w, h) = (orig.width as i32, orig.height as i32);
            let x0 = to_img(new.x, self.img_origin.0).clamp(0, w - 1);
            let y0 = to_img(new.y, self.img_origin.1).clamp(0, h - 1);
            let x1 = to_img(new.right(), self.img_origin.0).clamp(x0 + 1, w);
            let y1 = to_img(new.bottom(), self.img_origin.1).clamp(y0 + 1, h);
            self.set_crop(PxRect::new(x0, y0, x1 - x0, y1 - y0));
            return;
        }
        if let Some(o) = old
            && (o.x, o.y) != (new.x, new.y)
        {
            self.rebase((o.x - new.x) as f32, (o.y - new.y) as f32);
        }
        self.doc.base_width = new.w.max(1) as u32;
        self.doc.base_height = new.h.max(1) as u32;
        if old != Some(new) {
            self.base = None;
            self.clear_render_cache();
        }
        self.sel = Some(new);
    }

    /// Annotate mode: set the crop (image px, inside the image) and place the selection on
    /// it. Annotations keep their place on the picture.
    fn set_crop(&mut self, crop: PxRect) {
        if let Some(o) = self.orig.as_ref() {
            debug_assert!(crop.x >= 0 && crop.y >= 0);
            debug_assert!(crop.right() <= o.width as i32 && crop.bottom() <= o.height as i32);
        }
        if crop != self.crop {
            let (dx, dy) = ((self.crop.x - crop.x) as f32, (self.crop.y - crop.y) as f32);
            if dx != 0.0 || dy != 0.0 {
                self.rebase(dx, dy);
            }
            self.crop = crop;
            self.base = None;
            self.clear_render_cache();
        }
        self.sel = Some(self.sel_from_crop());
        self.sync_doc_size();
    }

    /// The region or crop origin moved by (-dx, -dy): move every shape by (dx, dy) so it
    /// keeps its place on the picture, in the document, its history, the text being typed
    /// (its pre-edit document) and the shape clipboard.
    fn rebase(&mut self, dx: f32, dy: f32) {
        shapes::translate_all(&mut self.doc, dx, dy);
        self.undo.map_docs(|d| shapes::translate_all(d, dx, dy));
        if let Some(e) = self.edit.as_mut() {
            shapes::translate_all(&mut e.pre, dx, dy);
            e.patch = None;
        }
        shapes::translate_shapes(&mut self.clipboard, dx, dy);
        self.ruler = None;
    }

    /// Annotate mode: the whole image as a crop bounds rect (image px).
    fn image_bounds(&self) -> Option<PxRect> {
        self.orig
            .as_ref()
            .map(|o| PxRect::new(0, 0, o.width as i32, o.height as i32))
    }

    fn clear_render_cache(&mut self) {
        self.rendered_tex = None;
        self.rendered_doc = None;
        self.submitted = None;
    }

    /// Unannotated pixels of the selection.
    fn ensure_base(&mut self) -> Arc<Rgba> {
        self.combine_regions();
        if let Some(b) = &self.base {
            return b.clone();
        }
        let b = match (&self.orig, self.kind) {
            (Some(o), Kind::Annotate) => {
                if self.crop == PxRect::new(0, 0, o.width as i32, o.height as i32) {
                    o.clone()
                } else {
                    Arc::new(o.crop(self.crop))
                }
            }
            _ => {
                let sel = self.sel.unwrap_or(self.local_bounds());
                Arc::new(self.frozen_image.crop(sel))
            }
        };
        self.base = Some(b.clone());
        b
    }

    /// Color under canvas point `p` for the eyedropper and the loupe: the captured image
    /// (in the editor the picture itself, elsewhere the frozen screen).
    fn sample_at(&self, p: (f32, f32)) -> [u8; 3] {
        if let (Kind::Annotate, Some(orig)) = (self.kind, &self.orig) {
            let ix = ((p.0 - self.img_origin.0) / self.scale).floor() as i32;
            let iy = ((p.1 - self.img_origin.1) / self.scale).floor() as i32;
            if ix >= 0 && iy >= 0 && ix < orig.width as i32 && iy < orig.height as i32 {
                return paint::sample(orig, ix, iy);
            }
        }
        paint::sample(&self.frozen_image, p.0.floor() as i32, p.1.floor() as i32)
    }

    fn source_info(&self, local: PxRect) -> SourceInfo {
        let v = hover::local_to_virtual(local, self.bounds);
        let c = PxPoint {
            x: v.x + v.w / 2,
            y: v.y + v.h / 2,
        };
        // What is visible at the centre: our own History/Settings window is not a source,
        // an untitled popup reports the window it belongs over.
        let top = occlusion::top_layer_at(&self.layers, c).map(|i| self.layers[i]);
        if top.is_some_and(|l| l.own) {
            return SourceInfo::default();
        }
        top.and_then(|l| self.windows.iter().find(|w| w.hwnd == l.hwnd))
            .or_else(|| windows_enum::window_at(&self.windows, c))
            .map(|w| SourceInfo {
                app: w.process.clone(),
                window_title: w.title.clone(),
                scale: None,
            })
            .unwrap_or_default()
    }

    // ---------------------------------------------------------------- init

    fn init_once(&mut self, ctx: &Context) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        crate::timing::point(format_args!(
            "overlay: first frame, {:.1} ms after Overlay::new",
            self.opened_at.elapsed().as_secs_f64() * 1000.0
        ));
        self.renderer = Some(Renderer::new(ctx));
        self.frozen_tex = Some(TiledTexture::upload(
            ctx,
            "ov-frozen",
            &self.frozen_image,
            true,
        ));
        if matches!(self.kind, Kind::Annotate)
            && let Some(b) = &self.orig
        {
            self.base_tex = Some(TiledTexture::upload(ctx, "ov-base", b, false));
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        crate::timing::point(format_args!("overlay: textures uploaded"));
    }

    // ---------------------------------------------------------------- input reading

    fn read_input(&self, ctx: &Context, ppp: f32) -> Input {
        let over_ui = ctx.is_pointer_over_egui();
        let view = self.view;
        ctx.input(|i| {
            let spos = i.pointer.latest_pos().map(|p| (p.x * ppp, p.y * ppp));
            let ctrl = i.modifiers.command;
            let mut wheel = Vec2::ZERO;
            for e in &i.events {
                if let Event::MouseWheel {
                    unit,
                    delta,
                    modifiers,
                    ..
                } = e
                    && !modifiers.command
                {
                    let k = match unit {
                        egui::MouseWheelUnit::Point => 1.0,
                        egui::MouseWheelUnit::Line => 40.0,
                        egui::MouseWheelUnit::Page => 400.0,
                    };
                    wheel += *delta * k;
                }
            }
            Input {
                pos: spos.map(|s| view.to_canvas(s)),
                spos,
                pressed: i.pointer.primary_pressed(),
                down: i.pointer.primary_down(),
                released: i.pointer.primary_released(),
                middle_pressed: i.pointer.button_pressed(egui::PointerButton::Middle),
                middle_down: i.pointer.button_down(egui::PointerButton::Middle),
                shift: i.modifiers.shift,
                alt: i.modifiers.alt,
                space: i.key_down(Key::Space),
                wheel,
                zoom: if ctrl { i.zoom_delta() } else { 1.0 },
                over_ui,
                double_click: i
                    .pointer
                    .button_double_clicked(egui::PointerButton::Primary),
            }
        })
    }

    fn read_keys(ctx: &Context) -> Keys {
        // egui zooms the whole UI on Ctrl+= / Ctrl+- / Ctrl+0 at the end of the frame; the
        // editor uses them for its own zoom, so take them first.
        let (zoom_in, zoom_out, zoom_fit) = ctx.input_mut(|i| {
            use egui::{KeyboardShortcut, Modifiers};
            let mut take = |k: Key| {
                i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, k))
                    | i.consume_shortcut(&KeyboardShortcut::new(
                        Modifiers::COMMAND | Modifiers::SHIFT,
                        k,
                    ))
            };
            let zin = take(Key::Equals) | take(Key::Plus);
            let zout = take(Key::Minus);
            let fit = take(keys::ZOOM_FIT.key);
            (zin, zout, fit)
        });
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
            // egui-winit turns Ctrl+V into a paste of whatever the clipboard holds (text or
            // an image) instead of a key press.
            k.paste = i.events.iter().any(|e| {
                matches!(e, Event::Paste(_) | Event::PasteImage(_))
                    || matches!(e, Event::Key { key: Key::V, pressed: true, modifiers, .. } if modifiers.command)
            });
            k.duplicate = keys::DUPLICATE.pressed(i);
            k.save = keys::SAVE.pressed(i);
            k.pin = keys::PIN.pressed(i);
            k.text = keys::COPY_TEXT.pressed(i);
            k.smart = keys::FIND_SENSITIVE.pressed(i);
            k.undo = keys::UNDO.pressed(i) && !i.modifiers.shift;
            k.redo = keys::REDO.pressed(i)
                || (cmd && i.modifiers.shift && i.key_pressed(keys::UNDO.key));
            k.delete = i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace);
            k.help = keys::HELP.pressed(i)
                || i.events
                    .iter()
                    .any(|e| matches!(e, Event::Text(t) if t == keys::HELP_CHAR));
            k.bold = keys::BOLD.pressed(i);
            k.zoom_step = i32::from(zoom_in) - i32::from(zoom_out);
            k.zoom_fit = zoom_fit;
            k.zoom_actual = keys::ZOOM_ACTUAL.pressed(i);
            if cmd && !i.modifiers.alt {
                let fwd = i.key_pressed(Key::CloseBracket) || i.key_pressed(Key::CloseCurlyBracket);
                let back = i.key_pressed(Key::OpenBracket) || i.key_pressed(Key::OpenCurlyBracket);
                let far = if i.modifiers.shift { 2 } else { 1 };
                k.order = if fwd {
                    far
                } else if back {
                    -far
                } else {
                    0
                };
            }
            if plain {
                k.hex = keys::COPY_COLOR.pressed(i);
                k.eyedropper = keys::EYEDROPPER.pressed(i);
                k.scan = keys::SCAN.pressed(i);
                k.beautify = keys::BEAUTIFY.pressed(i);
                k.tab = i.key_pressed(Key::Tab) && !i.modifiers.shift;
                for (tool, key) in keys::TOOL_KEYS {
                    if i.key_pressed(key) {
                        k.tool = Some(tool);
                    }
                }
                for (idx, key) in keys::PALETTE_KEYS.iter().enumerate() {
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
        // Text editing owns the keyboard (handled in `text_keys`).
        if self.edit.is_some() {
            return;
        }
        if self.exact_open {
            return;
        }
        if k.help {
            self.help_open = !self.help_open;
            return;
        }
        if self.help_open {
            // The cheat sheet is modal for the keyboard: Esc (or F1 / ?) closes it.
            if k.esc {
                self.help_open = false;
            }
            return;
        }
        if k.hex {
            self.copy_hex(shared);
        }

        if k.esc {
            self.on_escape(shared);
            return;
        }
        if self.picker_mode() {
            if k.tab {
                self.cycle_picker_format(shared);
            }
            if (k.enter || k.copy_event)
                && let Some(p) = self.last_pointer
            {
                let c = self.sample_at(p);
                self.pick_and_close(shared, c);
            }
            return;
        }
        // Find Sensitive Info decides Enter and Ctrl+C first (they must not copy the
        // findings in the clear).
        if (k.enter || k.copy_event) && self.smart_key(shared, k.enter) {
            return;
        }
        // Ctrl+C with shapes selected copies the shapes (inside the editor only); Enter
        // always copies the image.
        if k.copy_event && !k.enter && self.annotating() && !self.selected.is_empty() {
            self.copy_shapes();
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
        if self.sel.is_some() {
            if k.zoom_step != 0 {
                let f = if k.zoom_step > 0 {
                    view::STEP
                } else {
                    1.0 / view::STEP
                };
                let anchor = self.zoom_anchor();
                self.set_view(view::zoom_by(self.view, anchor, f, self.scale));
            }
            if k.zoom_fit {
                self.set_view(View::IDENTITY);
            }
            if k.zoom_actual {
                let anchor = self.zoom_anchor();
                self.set_view(view::actual_size(self.view, anchor, self.scale));
            }
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
            if k.smart {
                self.scan = None;
                if self.beautify_open {
                    self.close_beautify(shared);
                }
                self.toggle_smart_redact(ctx, shared);
            }
            if k.scan {
                self.toggle_scan(ctx, shared);
            }
            if k.beautify {
                self.toggle_beautify(shared);
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
            if k.paste {
                self.paste_shapes(ctx.pixels_per_point());
            }
            if k.duplicate {
                self.duplicate_selected(ctx.pixels_per_point());
            }
            if k.order != 0 {
                self.reorder(k.order);
            }
            if k.bold {
                self.toggle_bold();
            }
            if k.eyedropper {
                self.toggle_eyedropper();
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

        // Arrows: nudge the selected shapes, otherwise the selection rectangle.
        if k.arrows != (0, 0) && self.sel.is_some() && matches!(self.drag, Drag::None) {
            let (dx, dy) = k.arrows;
            let step = if k.shift { 10 } else { 1 };
            if self.annotating() && !self.selected.is_empty() {
                let pre = self.doc.clone();
                // One screen px per press, whatever the zoom.
                let k_img = 1.0 / (self.scale * self.view.zoom).max(0.01);
                let (ix, iy) = (
                    dx as f32 * step as f32 * k_img,
                    dy as f32 * step as f32 * k_img,
                );
                for id in self.selected.clone() {
                    if let Some(s) = self.doc.get_mut(id) {
                        shapes::translate(&mut s.kind, ix, iy);
                    }
                }
                self.undo.push(&pre);
            } else if self.sel_editable() {
                let s = step;
                let nudge = |r: PxRect, b: PxRect| {
                    if k.ctrl {
                        selection::grow_rect(r, dx * s, dy * s, b)
                    } else {
                        selection::move_rect(r, dx * s, dy * s, b)
                    }
                };
                if matches!(self.kind, Kind::Annotate)
                    && let Some(img) = self.image_bounds()
                {
                    // The crop moves by whole image px (a move keeps its size).
                    self.set_crop(nudge(self.crop, img));
                } else {
                    let sel = self.sel.unwrap_or_default();
                    let new = nudge(sel, self.sel_bounds());
                    self.set_sel(new);
                }
            }
        } else if k.arrows != (0, 0) && self.sel.is_none() {
            // Arrows with nothing selected confirm the hover highlight, so keyboard-only
            // users can nudge it next.
            if let Some(h) = self.hover
                && !matches!(self.kind, Kind::Select(_))
            {
                self.accept_region(h, shared);
            }
        }
        let _ = inp;
    }

    /// Screen px point zoom keys zoom around: the pointer if it is on screen, else the
    /// selection centre.
    fn zoom_anchor(&self) -> (f32, f32) {
        if let Some(p) = self.last_screen {
            return p;
        }
        let s = self
            .sel
            .map(|s| self.canvas_rect_screen(s))
            .unwrap_or_default();
        ((s.x + s.w / 2) as f32, (s.y + s.h / 2) as f32)
    }

    /// Apply a new view, keeping the selection on screen.
    fn set_view(&mut self, v: View) {
        let v = match self.sel {
            Some(s) if !v.is_identity() => {
                let mon = self.monitor_for(self.canvas_rect_screen(s));
                view::clamp_pan(
                    v,
                    (s.x as f32, s.y as f32, s.w as f32, s.h as f32),
                    (mon.x as f32, mon.y as f32, mon.w as f32, mon.h as f32),
                    64.0,
                )
            }
            _ => v,
        };
        self.view = v;
    }

    fn on_escape(&mut self, shared: &mut Shared) {
        if !matches!(self.drag, Drag::None | Drag::Ignore) {
            // Abort the gesture in progress; undo any live edit.
            match std::mem::replace(&mut self.drag, Drag::None) {
                Drag::MoveShapes { pre, .. } | Drag::ResizeShape { pre, .. } => {
                    self.doc = pre;
                    self.release_live();
                }
                Drag::ResizeSel { start_sel, .. } | Drag::MoveSel { start_sel, .. } => {
                    self.set_sel(start_sel)
                }
                Drag::Marquee { before, .. } => self.selected = before,
                _ => {}
            }
            self.cancel_add();
            self.guides.clear();
            return;
        }
        if self.help_open {
            self.help_open = false;
            return;
        }
        if self.picking {
            self.picking = false;
            return;
        }
        if self.smart.take().is_some() {
            return;
        }
        if self.scan.take().is_some() {
            return;
        }
        if self.beautify_open {
            self.close_beautify(shared);
            return;
        }
        if self.popup.is_some() {
            self.popup = None;
            return;
        }
        if self.magnifier_source.take().is_some() {
            return;
        }
        if self.annotating() && (!self.selected.is_empty() || self.tool != Tool::Select) {
            self.selected.clear();
            self.set_tool(Tool::Select);
            return;
        }
        if self.remove_active_region() {
            return;
        }
        let now = Instant::now();
        if esc_needs_confirm(self.has_unsaved_edits(), self.discard_armed, now) {
            self.discard_armed = Some(now);
            return;
        }
        self.cancel(shared);
    }

    /// Annotations the user drew or changed in this session (lost on cancel).
    fn has_unsaved_edits(&self) -> bool {
        self.annotating() && !self.doc.shapes.is_empty() && self.undo.can_undo()
    }

    /// True while the discard prompt is showing.
    fn discard_prompt_active(&self) -> bool {
        self.discard_armed
            .is_some_and(|t| t.elapsed() < DISCARD_WINDOW)
    }

    fn cancel(&mut self, shared: &mut Shared) {
        // Beautify choices are remembered even when the capture is dropped.
        self.persist_beautify(shared);
        shared.outbox.push(Command::Cancelled);
        self.closed = true;
    }

    /// C: copy the color under the cursor in the picker's format (hex by default).
    /// Feedback stays inside the overlay (loupe plate, or the hint bar when the loupe is
    /// hidden) instead of a toast.
    fn copy_hex(&mut self, shared: &mut Shared) {
        let Some(p) = self.last_pointer else { return };
        let c = self.sample_at(p);
        self.copy_color(shared, c);
    }

    /// Copied-color feedback text while it is fresh.
    fn hex_flash_text(&self) -> Option<(String, bool)> {
        let f = self.hex_flash.as_ref()?;
        if f.at.elapsed() >= HEX_FLASH {
            return None;
        }
        Some(if f.ok {
            (tr!("overlay.notice.copied_hex", hex = f.hex), true)
        } else {
            (tr!("overlay.notice.clipboard_busy_c").to_string(), false)
        })
    }

    /// Show `text` in the hint bar for a few seconds.
    fn flash_notice(&mut self, text: &str) {
        self.notice = Some(Notice {
            text: text.to_string(),
            until: Some(Instant::now() + NOTICE_SECS),
        });
    }

    /// The current hint-bar notice, if still due.
    fn notice_text(&self) -> Option<&str> {
        let n = self.notice.as_ref()?;
        let live = match n.until {
            Some(t) => Instant::now() < t,
            None => self.sel.is_none(),
        };
        live.then_some(n.text.as_str())
    }

    /// The window is up and visible (uncloaked after its first frames).
    pub fn revealed(&self) -> bool {
        self.window.is_some() && self.reveal.is_none()
    }

    /// Take over from the quick overlay shown during a cold start
    /// (`stinkshot_platform::quick_overlay`): Esc there cancels, a drag still in progress
    /// continues here, a finished drag becomes the selection, a plain click takes the window
    /// under it.
    pub fn take_quick_input(
        &mut self,
        inp: stinkshot_platform::quick_overlay::QuickInput,
        shared: &mut Shared,
    ) {
        if inp.cancelled {
            self.cancel(shared);
            return;
        }
        if self.sel.is_some() || !matches!(self.drag, Drag::None) {
            return;
        }
        if let (false, Some(click)) = (inp.down, inp.clicked) {
            // The quick overlay highlighted the whole window under the cursor; a click there
            // takes that window, as a click on the hover target does here.
            let hit = hover::hover_target(&self.layers, click)
                .and_then(|t| hover::virtual_to_local(t.frame, self.bounds));
            if let Some(r) = hit.filter(|r| r.w >= 1 && r.h >= 1) {
                self.accept_region(r, shared);
            }
            return;
        }
        let end = if inp.down { inp.cursor } else { inp.release };
        let (Some(press), Some(cursor)) = (inp.press, end) else {
            return;
        };
        let local = |p: PxPoint| ((p.x - self.bounds.x) as f32, (p.y - self.bounds.y) as f32);
        let (start, cur) = (local(press), local(cursor));
        let moved = (start.0 - cur.0).abs().max((start.1 - cur.1).abs()) >= CLICK_SLOP_PT;
        if inp.down {
            self.hover = None;
            self.drag = Drag::Selecting {
                anchor: start,
                start,
                cur,
                moved,
            };
        } else if moved {
            let r = selection::drag_rect(
                (start.0 as i32, start.1 as i32),
                (cur.0 as i32, cur.1 as i32),
                self.local_bounds(),
                self.effective_aspect(false),
            );
            if r.w >= 1 && r.h >= 1 {
                self.accept_region(r, shared);
            }
        }
    }

    /// Nothing has happened in this overlay yet (no selection, no gesture, no edits): it can
    /// be dropped without losing anything.
    pub fn is_untouched(&self) -> bool {
        matches!(self.kind, Kind::Capture)
            && self.sel.is_none()
            && matches!(self.drag, Drag::None)
            && self.doc.shapes.is_empty()
    }

    /// The editor color memory to save: (global color, global width, per-tool styles,
    /// recent colors).
    fn tool_style_config(&self) -> std::collections::BTreeMap<String, ToolStyle> {
        self.tool_styles
            .iter()
            .map(|(t, (c, w))| {
                (
                    t.config_key().to_string(),
                    ToolStyle {
                        color: to_hex_color(*c),
                        width: *w,
                    },
                )
            })
            .collect()
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
        // Created hidden, at its final physical rect: winit creates the window on the primary
        // monitor and egui converts points with that monitor's scale, so dividing by it lands
        // on the physical px. It used to be created visible with physical px as points, which
        // showed an oversized window (8000x2700 for a 6400x2160 desktop at 125 %) until
        // `on_created` resized it, and the tool-window restyle then hid and re-showed it.
        // `prepare_window` styles the hidden window and shows it.
        let s = self.create_scale;
        egui::ViewportBuilder::default()
            .with_title(self.title())
            .with_decorations(false)
            .with_resizable(false)
            .with_transparent(false)
            .with_taskbar(false)
            .with_has_shadow(false)
            .with_always_on_top()
            .with_active(true)
            .with_visible(false)
            .with_position(Pos2::new(
                self.bounds.x as f32 / s,
                self.bounds.y as f32 / s,
            ))
            .with_inner_size(Vec2::new(
                self.bounds.w as f32 / s,
                self.bounds.h as f32 / s,
            ))
    }

    fn on_created(&mut self, hwnd: Hwnd, shared: &Shared) {
        let _ = shared;
        // Normally done already by `show` on the first frame.
        self.prepare_window(hwnd);
    }

    fn after_paint(&mut self) {
        let Some((hwnd, presented)) = &mut self.reveal else {
            return;
        };
        *presented += 1;
        crate::timing::point(format_args!("overlay: presented {presented} (cloaked)"));
        if *presented >= REVEAL_AFTER_PRESENTS {
            let hwnd = *hwnd;
            self.reveal = None;
            crate::sysutil::set_cloaked(hwnd, false);
            window_util::force_foreground(hwnd);
            crate::timing::point(format_args!("overlay: uncloaked"));
        }
    }

    fn wants_frame_now(&self) -> bool {
        self.reveal.is_some()
    }

    fn show(&mut self, ctx: &Context, shared: &mut Shared) -> SurfaceStatus {
        // The window exists (hidden) before the first frame's UI runs, so it is styled and
        // shown here and that same frame is painted onto it.
        if self.window.is_none()
            && let Some(hwnd) = crate::sysutil::find_own_window(&self.title())
        {
            self.prepare_window(hwnd);
        }
        if self.reveal.is_some() {
            ctx.request_repaint();
        }
        self.ensure_base_texture(ctx);
        self.frame(ctx, shared)
    }
}

impl Overlay {
    /// Style the hidden window and show it cloaked; `after_paint` uncloaks it once a frame
    /// is on it. Runs once.
    ///
    /// Still hidden here, so the tool-window restyle does not hide and re-show it, and
    /// `set_rect` changes nothing when the creation rect is already right (it is unless a DPI
    /// change on the way moved it). The cloak: a freshly shown window is composited white
    /// until DWM picks up a present, and the first present on a new swapchain is not always
    /// the one it shows (measured with the overlay made capturable: a white frame in 3 of 6
    /// opens when uncloaked after the first present, none in 16 after the second). Painting
    /// while still hidden instead stalled the next frame by about 100 ms.
    fn prepare_window(&mut self, hwnd: Hwnd) {
        if self.window.is_some() {
            return;
        }
        self.window = Some(hwnd);
        window_util::set_tool_window(hwnd, true);
        window_util::set_topmost(hwnd, true);
        window_util::set_rect(hwnd, self.bounds);
        window_util::exclude_from_capture(hwnd, true);
        crate::sysutil::set_cloaked(hwnd, true);
        crate::sysutil::show_window(hwnd);
        window_util::force_foreground(hwnd);
        self.reveal = Some((hwnd, 0));
        crate::timing::point(format_args!("overlay: window styled and shown (cloaked)"));
    }
}

#[cfg(test)]
mod esc_tests {
    use super::*;

    #[test]
    fn tray_double_click_needs_the_same_spot_and_quick_timing() {
        let at = (100, 1060);
        let soon = Duration::from_millis(150);
        assert!(is_tray_double_click(true, soon, at, (103, 1058)));
        assert!(!is_tray_double_click(false, soon, at, at));
        assert!(!is_tray_double_click(
            true,
            TRAY_DOUBLE_CLICK + Duration::from_millis(1),
            at,
            at
        ));
        assert!(!is_tray_double_click(true, soon, at, (140, 1060)));
    }

    #[test]
    fn snap_label_names_type_name_and_level() {
        assert_eq!(
            snap_label_text("Button", "Save", 0, 3),
            "Button \u{201c}Save\u{201d} \u{b7} Level 1 of 3"
        );
        assert_eq!(
            snap_label_text("Pane", "  ", 2, 3),
            "Pane \u{b7} Level 3 of 3"
        );
        let long = snap_label_text("Document", &"x".repeat(80), 1, 2);
        assert!(long.contains('\u{2026}'));
        assert!(long.chars().count() < 80);
        // Count never reads lower than the level.
        assert!(snap_label_text("Group", "", 4, 2).ends_with("Level 5 of 5"));
    }

    #[test]
    fn esc_with_edits_needs_a_second_press() {
        let now = Instant::now();
        assert!(!esc_needs_confirm(false, None, now));
        assert!(esc_needs_confirm(true, None, now));
        // Second Esc inside the window cancels.
        assert!(!esc_needs_confirm(
            true,
            Some(now),
            now + Duration::from_millis(500)
        ));
        // Too late: it asks again.
        assert!(esc_needs_confirm(
            true,
            Some(now),
            now + DISCARD_WINDOW + Duration::from_millis(1)
        ));
    }

    #[test]
    fn recent_colors_are_most_recent_first_without_duplicates() {
        let mut r = Vec::new();
        for c in [[1, 0, 0, 255], [2, 0, 0, 255], [1, 0, 0, 128]] {
            push_recent(&mut r, c);
        }
        assert_eq!(r, vec![[1, 0, 0, 255], [2, 0, 0, 255]]);
        for i in 0..10u8 {
            push_recent(&mut r, [i, 9, 9, 255]);
        }
        assert_eq!(r.len(), RECENT_COLORS);
        assert_eq!(r[0], [9, 9, 9, 255]);
    }

    #[test]
    fn redaction_boxes_default_to_black() {
        let orange = [255, 138, 61, 255];
        assert_eq!(
            default_tool_style(Tool::Redact, orange, 4.0),
            ([0, 0, 0, 255], 4.0)
        );
        assert_eq!(default_tool_style(Tool::Arrow, orange, 6.0), (orange, 6.0));
    }
}
