# app-overlay notes

Area: `crates/app/src/overlay/**`. Region overlay and inline annotation editor.

## What was built

All three modes of `OverlayMode`:

- **Capture**: hover highlight (UIA element via `ElementSnapper`, wheel cycles element -> parent
  -> window, falls back to `windows_enum::window_at` over `req.windows`, then the monitor rect),
  click snaps, drag draws a free region (Space moves the region while dragging, Shift forces
  1:1, a chosen aspect preset constrains it). Selected state has 8 handles, Space+drag or
  Select-tool drag on empty space moves, arrows nudge 1 px (Shift 10 px, Ctrl resizes the
  right/bottom edge), size label + presets chip (Free, 16:9, 4:3, 1:1, Exact W x H input),
  tool bar and action bar placed per `selection::layout_bars` (below, else above, else inside;
  stacked when too wide for the monitor).
- **SelectRegion(purpose)**: no annotation bars. The purpose label ("Select area to record",
  "Select area to copy text", "Select area to scroll-capture", "Select area to record as GIF")
  leads the hint bar. Releasing a drag or clicking a snapped element/window confirms immediately
  and pushes `Command::RegionChosen`. Enter confirms the current hover rect.
- **Annotate**: the pre-captured image is centered on the monitor under the frozen cursor over a
  dimmed frozen screen, scaled down (never up) to 90% of the usable area when too big. The
  selection is fixed to the image; annotation coordinates stay in base-image px regardless of
  the display scale (`selection::Mapping`).

Tools (design toolbar order, all of ARCHITECTURE.md's shortcuts): Select V, Arrow A, Line L,
Rect R, Ellipse E, Pen P, Highlighter H, Text T, Step N, Blur B, Pixelate X, Redact K,
Magnifier M, Callout O, Spotlight S. 1-8 palette colors, `[`/`]` stroke width (or size of the
selected shape), Del deletes, Ctrl+Z / Ctrl+Y (or Ctrl+Shift+Z), Enter / Ctrl+C copy, Ctrl+S save
as, Ctrl+P pin, Ctrl+T copy text, Ctrl+R Find Sensitive Info (shield button first in the action
bar; see smart-redact.md), C copies the hex under the cursor, Esc steps back (abort drag
-> close shortcut sheet -> close the Find Sensitive Info panel -> close popup -> cancel pending
magnifier -> deselect tool/shape -> cancel overlay).

Shapes are selectable, movable, deletable, recolorable (palette popup or 1-8), resizable with
per-type handles: arrows/lines (two endpoints), rect-like (8 handles with flip, Shift = square),
pen/highlighter (bounding-box scale), text (corner scales font size), step (radius), callout
(8 handles + tail), magnifier (dest 8 handles + source 8 handles). Text and callouts edit
inline (since Nov 7 2026 typed straight onto the image, see Editor polish; originally a
multi-line `egui::TextEdit` positioned over the shape) (Enter = new line,
Esc or Ctrl+Enter or clicking elsewhere commits, empty text deletes the shape, double-click a
text shape or use the Text tool on it to re-edit). Step numbers use `Doc::next_step_number`
and `renumber_steps` on deletion. Magnifier is two-step: drag the source region, then click to
place the inset (a ghost follows the cursor, Esc cancels).

Undo/redo: snapshot stack of `Doc` (`undo.rs`), 200 deep. One entry per gesture (drag, text
edit, recolor, nudge). Moving the selection origin with annotations present re-bases the live
doc and the whole history so they stay aligned.

Rendering: committed shapes go through `annotate::render` on a coalescing worker thread
(`render.rs`) and are shown as a texture. The shape being dragged/edited is excluded from the
cached render and painted with the painter (`paint.rs`) until a render containing it arrives, so
there is no flash. Shapes the cached render does not show correctly yet (just created, render
in flight, render failed) are painted by the painter too, so the editor stays usable even if
the renderer panics (caught with `catch_unwind`, logged, and finishing then reports an error
instead of silently dropping annotations).

Textures are tiled into <= 2048 px tiles, so a multi-monitor desktop wider than the GPU's max
texture size still draws 1:1. All geometry is physical px; conversion to points happens only at
draw time with `ctx.pixels_per_point()`.

Finishing: Capture crops `frozen.image` to the selection (`base`), flattened = the cached
render when it matches the doc, else a worker-thread `annotate::render` (a small "Finishing..."
pill shows meanwhile). Pushes `Command::CaptureDone`. Remembers `capture.last_region`
(virtual px) and, if the user changed color or stroke width, writes them back to
`config.editor` (`config_dirty = true`). Cancel pushes `Command::Cancelled`.

## Design decisions and deviations

- Own hit-testing, bounds and translate helpers in `overlay/shapes.rs` instead of
  `annotate::hit_test` / `shape_bounds` / `translate_shape`: those are `todo!()` in the core stub
  and the overlay needs text extents anyway. Text size comes from `Fonts::measure` through a
  callback so the pure logic is unit tested with a fake measurer. Integration may swap to core's
  versions later; behavior only needs to match visually.
- Selection handles are interactive only while the Select tool is active (otherwise drawing near
  the selection edge would start a resize). Press V (or click Select) to adjust the region again.
- (Superseded Nov 7 2026, see Editor polish.) Starting a brand new region after annotations
  exist used to be blocked; now shapes are re-based and kept.
- Highlighter shapes are created with color alpha 0x99 and width = max(14, 4 x stroke width).
  If core's renderer applies its own translucency this will be a bit fainter than intended;
  tune `HIGHLIGHTER_ALPHA` in `shapes.rs`.
- Callouts are created with `style.fill = Some(color)`; recolor updates both. The inline editor
  and the painter preview pick black/white text by luminance; core's renderer should match.
- Capture mode reports `mode = "region"` and the source window under the selection center.
  Window-mode window fx is the shell's job (it opens Annotate for window/fullscreen captures).
- In Annotate mode `CaptureOutput.rect` is the on-screen rect of the displayed image (not
  meaningful for repeat-last-region) and `last_region` is not touched.
- Selecting a SelectRegion region by drag confirms on release (no adjustment stage), as the
  task text says "confirm with Enter or release".
- `monitors::list()` is read once in `Overlay::new` to place the loupe, hint bar and bars on the
  right monitor and to show the monitor DPI in the loupe ("@ 150%").
- The overlay window is excluded from capture (`exclude_from_capture(true)`), per the module
  docs. Remove that line in `on_created` if you need external screenshots of the overlay.
- `#![allow(dead_code)]` sits at the top of `overlay/mod.rs` only because the shell does not
  construct `Overlay` yet. Delete it after wiring.

## Snapping only to what is visible (Nov 6 2026)

User report: with windows in the foreground, the overlay still highlighted elements of windows
behind them. Causes found:

1. The hover target came from `window_at(req.windows)`, and `visible_windows()` leaves out
   untitled windows (context menus, popups, Electron/tool surfaces) and all of Stinkshot's own
   windows. Hovering an open menu or the History/Settings window picked the window behind it.
2. UIA element rects were clipped to their window only, so an element of a partly covered
   window was outlined right across the window on top of it (and children of scrolled lists
   ran past their viewport).
3. Nothing rejected an element that is mostly hidden.

Now: `OverlayRequest::layers` carries `windows_enum::layers()` (full Z-ordered stack, taken in
the same frame as the freeze; empty for Annotate mode, then built from `windows`).
`hover::hover_target` picks the topmost layer at the cursor; UIA is asked only about that
window (never our own windows: they are window-level targets). The UIA chain goes through
`occlusion::clip_chain` with the windows above as occluders: each inner rect is cut to the
biggest unoccluded rect around the cursor, levels whose real uncovered area is under 30 % are
dropped (measured on the whole element, so levels do not come and go as the cursor moves),
and duplicates after clipping collapse. The window level comes from `occlusion::window_levels`:
the window's largest uncovered rectangle (independent of the cursor) when the cursor is in it
and it holds at least 30 % of the visible area, then the whole frame as one more wheel-up
level. Before, the window level was the cursor-dependent band, so moving a few px on
Firefox's tab strip flipped it between a 208 px vertical sliver and a 200 px strip, and a
click captured only that. With no UIA chain (snapping off, own windows, result pending) the
window levels are the snap rects; the desktop still falls back to the whole monitor. Source info for a
selection uses the same topmost-layer rule. Tests: `hover::tests` (target choice, stale result
check) and `stinkshot_platform::occlusion::tests`.

Verified live (125 % 4K monitor, overlay temporarily made capturable): hovering History where
Settings covers part of it outlines only History's uncovered strip; hovering Settings outlines
Settings (no UIA); hovering Discord snaps to its element. Popup menus were not opened
(synthetic input), so the untitled-window case is covered by the unit tests and the stack
dump only.

## Contract changes

None. No public signature changed. Additive: `overlay` internals only; `Overlay::is_untouched`
(the shell drops an untouched overlay on a tray double click).

UX pass 2: `overlay/keys.rs` holds the tool letters, Ctrl chords and the F1 / ? cheat sheet
content (one table for key handling, hints and help). Hint-bar notices (`Notice`) replace
toasts over the frozen screen (no remembered region, copied color when the loupe is hidden,
ignored click outside an annotated selection). The wheel-snapping label comes from
`uia::SnapResult.labels`.

## Contract requests

- core: `annotate::hit_test`, `shape_bounds`, `translate_shape` could be wired in later, but are
  not required. `annotate::render` must handle `Doc.base_width/height == base.width/height`
  (the overlay always keeps them in sync) and `Highlighter` translucency (see above).
- shell: `theme::widgets::{icon_button, accent_button}` and `theme::palette` are called by the
  overlay toolbars; `icon_button` should be 34x34 as documented. The shell should call
  `Surface::on_created` right after the HWND exists (it does `set_rect`, topmost, tool window,
  exclude-from-capture, force-foreground).

## Known limitations

- The inline text editor uses egui's proportional font (the theme font) while the final render
  uses core's `Fonts`; glyph advance may differ slightly between preview and output.
- Blur/pixelate/magnifier appear as hatched placeholders only while a render is in flight or
  failed; the real pixels appear as soon as the worker finishes.
- Spotlight dims are drawn by the painter only for live shapes; the committed look is the
  renderer's.
- Tiles wider than 2048 px are positioned at fractional point offsets at some DPI scales, which
  may show a hairline seam on very wide multi-monitor selections.

## Tests

`cargo test -p stinkshot overlay`: 44 tests covering handle hit-testing, resize with and without
aspect, flip and clamp, nudge/grow, ratio presets, exact-size parsing, bar layout, mapping and
fit-scale, snap picking and wheel cycling, shape hit-testing per kind, handle sets, handle
drags (rect, arrow with angle constraint, pen scale, text size, step radius), creation from
drags, recolor/resize, smoothing, undo/redo stack, tile coverage. `cargo clippy -p stinkshot
--all-targets` has no warnings in `overlay/` (the crate still warns about other areas' unwired
stubs).

## Manual verification checklist (needs a real desktop and integrated shell)

1. Hotkey -> overlay appears over all monitors, frozen image aligned 1:1 with the desktop on
   every monitor, including a mixed-DPI pair (compare a pixel-exact UI feature across the seam).
2. Hover a window, a button inside it, then wheel up/down: highlight cycles element -> parent
   -> window. Click captures the highlighted rect.
3. Drag a region spanning two monitors; size label shows physical px; handles resize; Space
   moves; arrows nudge; Shift+arrows 10 px; Ctrl+arrows resize.
4. Presets: 16:9, 4:3, 1:1 snap the rect and constrain further resizes; Exact accepts
   "1280x720", Enter applies, Esc closes.
5. Loupe: 11x11 grid, coordinates are virtual-desktop px, hex matches the pixel (check against
   another picker), C copies, flips away from monitor edges.
6. Every tool: create, select, move, resize via handles, recolor (palette popup and 1-8),
   width ([ ] and popup), delete, undo/redo. Text: multi-line, re-edit by double-click. Step
   counters renumber after delete. Magnifier two-step. Callout tail handle. Spotlight dims.
7. Blur/pixelate/redact on text, copy, paste elsewhere: pixels in the output are destroyed.
8. Enter/Ctrl+C copy, Ctrl+S save as, Ctrl+P pin, Ctrl+T OCR copy, Esc ladder behaves as in
   the list above; Cancelled leaves nothing behind.
9. SelectRegion for OCR / record / scroll: label shows, release confirms, no annotation bars.
10. Annotate mode (window capture, history Edit): image centered on the cursor monitor, large
    images scaled to fit, annotations editable, finishing with `editing` set overwrites.
11. Large 2-4K selection with many shapes stays responsive while dragging (render is async).

## Movable toolbar (Nov 6 2026)

User request: the tool palette + action bar that appears with a selection (or in the editor)
should be movable. The tool bar now starts with a dotted grip (two columns of three dots,
`ui::grip`, tooltip "Drag to Move · Double-Click to Dock"). Dragging it moves both bars as one
block; the cursor is the move cursor on hover and while dragging, and both bars get a deeper,
softer "lifted" shadow while dragging (`bar_frame(pal, lifted)`).

- Docked (default): placed by `selection::layout_bars` and follows the selection as before.
- Undocked: once dragged (the pointer moved more than `toolbar::UNDOCK_THRESHOLD_PX`, 3 px;
  egui also reports a long press without movement as a drag start, and that must not undock
  silently), the block keeps its own origin (`Overlay::bar_origin`) and no longer
  follows the selection. Each frame `toolbar::undocked_layout` picks the monitor the block is
  on (centre, else largest overlap, else nearest), lays the bars out side by side or stacked
  for that monitor's width, and clamps the block inside it with the usual 8 px margin. It can
  be dragged onto another monitor but never off-screen. The drag target is unclamped
  (`GripDrag::target`, start origin + pointer delta), so pushing past an edge and coming back
  does not drift; the clamped spot is stored when the drag ends.
- Double-clicking the grip docks it again (and forgets the stored spot).
- Session memory: `toolbar::remember` keeps the undocked origin in virtual-desktop px in a
  process-wide static, and `Overlay::new` starts from it, so the next capture opens with the
  bar where it was left. Not saved to config, so a restart docks it. A remembered spot is used
  only while the selection is on the same monitor (`toolbar::remembered_applies`, via
  `Overlay::effective_bar_origin`), otherwise the bar docks; the editor (Annotate) always
  opens docked, since a remembered spot would land on the centred image or another monitor.
- Stacked layout (tool bar + action bar wider than the monitor): docked and undocked use the
  same block shape, action bar directly under the tool bar at the same x, so starting a grip
  drag from the docked spot does not make the action bar jump.
- Color and width popups anchor to the buttons' rects from this frame's tool bar, open on the
  bar's monitor (above when there is no room below) and are now also kept inside the monitor
  horizontally (`toolbar::popup_x`). The presets chip belongs to the size label above the
  selection and is unaffected.
- Hit-testing: the grip is an egui widget, so a press on it is "over UI" and the overlay
  ignores the gesture (`Drag::Ignore`) exactly like a press on any toolbar button. Keyboard
  handling is untouched.

Tests: `overlay::toolbar::tests` (side-by-side / stacked block, monitor choice, clamping,
moving to another monitor and past the far corner, unclamped drag target, virtual round trip
and session memory, popup x clamp). Verified from code and unit tests only; no synthetic input
was sent to the overlay.

## Editor polish (Nov 7 2026)

Shape editing, typing on the image, color tools, curved arrows, cropping after annotating,
zoom and a faster render path. Every new shortcut is in `overlay/keys.rs` (cheat sheet now
has Select, Tools, Edit, Arrange, View, Finish).

### Shape editing

- **Shift** keeps lines and arrows at 45 degree steps and rect-like shapes square or circular,
  while drawing and while resizing (corner handles: the larger side wins; edge handles: the
  other side follows, centred). Shift while moving keeps the drag on one axis.
- **Smart guides** (`snap.rs`, pure): moving shapes snaps the group box's edges and centre to
  the other shapes' edges and centres and to the image's edges and centre; resizing snaps the
  dragged handle on the axes it moves; drawing snaps both corners; line ends and the callout
  tail snap on both axes. Threshold 6 screen points (`snap::SNAP_PT`, converted through the
  zoom). Guides are 1 px accent lines spanning the lined-up boxes with small end dots. Alt
  turns snapping off. Shift's own angle constraint wins over snapping on line ends.
- **Selection is a list** (`Overlay::selected`): Shift+click adds or removes a shape; with
  the Select tool a drag on empty space draws a marquee (accent wash) that selects every shape
  it touches (Shift adds to the selection). In a fresh capture with no shapes that drag still
  moves the region (cursor shows a grab hand then, a crosshair once shapes exist). Moving,
  Del, recolor, width, `[ ]`, arrows, Ctrl+D, Ctrl+C and z-order act on all selected shapes.
  One selected shape shows its handles; several show dashed outlines plus an accent group
  box.
- **Ctrl+D** duplicates next to the originals (16 screen points down-right, kept on the image),
  selected. **Ctrl+C with shapes selected copies the shapes inside the editor**, not the image
  (the hint bar switches to "Enter Copy Image · Ctrl+C Copy Shapes · Ctrl+D Duplicate"); with
  nothing selected Ctrl+C copies the image as before; Enter always copies the image. **Ctrl+V**
  pastes them, each paste one step further. The shape clipboard lives in the overlay only.
  Find Sensitive Info still decides Enter / Ctrl+C first (`smart_redact::panel_key`), so a
  shape selection never lets Ctrl+C past an open panel. egui-winit turns Ctrl+V into a paste of
  whatever the system clipboard holds (text or image), so the editor treats `Event::Paste` /
  `Event::PasteImage` as the key; with an empty or file-only clipboard no event arrives
  (limitation).
- **Z-order**: Ctrl+] / Ctrl+[ one step, Ctrl+Shift+] / Ctrl+Shift+[ to front / back
  (`shapes::bring_forward` and friends, tested).
- **Handles**: the one under the pointer (or being dragged) grows and fills with the accent;
  box handles show resize cursors per direction, point handles (line ends, curve, tail) a grab
  hand (grabbing while dragged). Selection handles highlight the same way. Handles have a soft
  shadow so they read on white screenshots.

### Typing on the image

The floating `TextEdit` box is gone. While typing, the text shape is rendered by core
(`annotate::render_shape_patch`, a small canvas re-rendered only when the text changes, about a
millisecond) and shown exactly as it will be saved, with a caret (text color with a thin
contrasting edge, solid for 0.5 s after each key, then blinking) and an accent selection band.
`textedit.rs` (pure, tested) does the editing: typing replaces the selection, Backspace /
Delete (Ctrl = word), Left / Right (Ctrl = word, Shift extends; a selection collapses first),
Up / Down / Home / End by laid-out line, Ctrl+A, Ctrl+C / Ctrl+X / Ctrl+V on the selected text,
Enter = new line, Esc or Ctrl+Enter = done. Click inside the text places the caret, drag
selects, double click selects a word. Caret positions come from `annotate::text_layout`, the
same layout the renderer draws, so callout wrapping lines up too. After Esc the rendered patch
stays until the cached render includes the text, so nothing flashes.

A context bar floats above the text (below when there is no room): A− / size / A+ (steps
through 10..128 px), **B** (bold, also Ctrl+B) and **Background Pill**. With the pill on, the
pill takes the color and the text gets black or white ink by luminance (`shapes::contrast_ink`,
same threshold as core); recoloring pill text recolors the pill. Plain dark text gets a light
halo instead of a black shadow (core). Bold, pill and size are sticky for the next text.

### Color

- **Eyedropper**: I or the button next to the color swatch. The loupe shows while picking (in
  the editor it magnifies the picture itself, not the dimmed screen); a click applies the
  color (to the selected shapes too), Esc cancels. The hint bar says "Pick a Color from the
  Image".
- **Recent colors**: the color popup has a second row, "Recent", with the last 6 colors used
  (palette, eyedropper or recent), newest first, saved in `[editor] recent_colors`.
- **Per-tool memory**: each drawing tool remembers its own color and width
  (`[editor.tool_styles]`, keyed by `Tool::config_key`); picking a tool restores them, changing
  color or width with a tool active updates that tool. Tools without a memory start from the
  current color; the Solid Redact Box starts black. Saved on finish with the rest of the
  editor prefs. With one shape selected the swatch and width show that shape's values.

### Arrows and callouts

- A third arrow handle (white diamond) sits on the curve's middle; dragging it bends the arrow
  into a quadratic curve through that point (`annotate::ctrl_through`), Shift keeps the bend
  symmetric, dropping it within 4 px of the straight line (or double-clicking it, or the
  context bar's Straighten) straightens it. Heads follow the end tangent. Moving an end keeps
  the bend (`annotate::follow_ctrl`, a rotate + scale about the fixed end).
- Context bar for a selected arrow: One Head / Heads at Both Ends, and Notched / Triangle /
  Open head styles (sticky for new arrows).
- The callout tail handle existed; the live preview now draws the bubble and tail with core's
  geometry (`callout_tail_base`, `callout_radius`), so dragging it matches the result.

### Crop and resize after annotating

- The "Annotations Keep This Region" lock is gone. With the Select tool the region's handles
  work at any time; shapes keep their place on the picture (the doc and the undo history are
  re-based when the region's origin moves; magnifier sources move with them now). A new region
  can be dragged outside the old one even with shapes; shapes outside it are kept but clipped.
- Capture mode: expanding pulls more pixels from the frozen screen (the base is re-cropped).
- Annotate mode (window capture, History Edit): the whole picture stays visible, the part
  outside the crop dimmed like the screen; the selection handles crop inward
  (`Overlay::crop`, image px; `set_sel` converts canvas px to a crop inside the image, so it
  can never grow past the picture). Finishing outputs the cropped base and the re-based doc.
- Find Sensitive Info findings are dropped whenever the base changes (unchanged behaviour,
  now also on an editor crop).

### Zoom and pan

`view.rs` (pure, tested) maps screen px to canvas px: `screen = off + zoom * canvas`. All input
is converted once in `read_input` (`Input::pos` canvas, `Input::spos` screen), every
tolerance and handle radius goes through `pt_to_img` / `pt_to_canvas`, painting through
`img_to_pt` / `canvas_rect_pt`, and the bars, chips, loupe and popups are laid out in screen
px, so every tool works zoomed.

- Ctrl+wheel zooms around the cursor; Ctrl+= / Ctrl+- around the pointer (egui's own UI zoom
  shortcuts are consumed so the app's zoom factor never changes). Range 25 % to 800 % of image
  px (a fit below 25 % stays reachable); steps landing within 3 % of the fitted view snap back
  to it exactly.
- Ctrl+0 fits (the normal view), Ctrl+1 shows 100 % image px. The plain wheel scrolls a zoomed
  view (Shift+wheel sideways). Middle-drag pans; Space+drag pans when zoomed and always in the
  editor (unzoomed capture keeps Space+drag = move the region). Pans keep at least 64 px of the
  selection on its monitor.
- A zoom chip ("150%") next to the size chip, always in the editor; clicking it fits.

### Rendering performance

Measured with `cargo test -p stinkshot perf_4k -- --ignored --nocapture` (dev profile; core and
egui at opt-level 2, the app crate unoptimized), 3840x2160 capture with 50 shapes (arrows,
rects, a blur, text):

| | Before | After |
|---|---|---|
| Committed change (one arrow moved) on the worker | full render 116.5 ms | dirty-rect render + texel conversion 12.2 ms (271x809 px) |
| UI thread per result | 15.4 ms RGBA -> texture conversion of the whole image, plus a 33 MB upload | `set_partial` of 0.22 M texels (0.9 MB); full results arrive pre-converted |

How: the worker (`render.rs`) keeps `work` (the base with Blur / Pixelate / Redact baked in)
and the last full image. For each new doc it diffs against the last one (`render::dirty_rect`:
bounds of changed, added, removed and re-stacked shapes plus a shadow / glyph margin) and
renders only that rectangle with `annotate::render_region`, patches its copy, splits the patch
per 2048 px tile and sends texture-ready `ColorImage`s; the UI applies every result in order
with `TextureHandle::set_partial`. Full renders happen for a new base (region or crop change),
a changed destructive shape (Redact is in the baked set because magnifiers sample `work` and
must never show what a box hides), a spotlight change (it dims everything), a patch over half
the image, or when the UI has no texture to patch (`need_full`). Finishing reuses the worker's
image when it shows exactly the final doc (`Renderer::image_for`). The live painter preview of
the shapes being dragged is unchanged.

### Tests

`overlay::shapes` (z-order, copies and step renumbering, paste offset, marquee, Shift on edge
handles, curve handle bend / symmetric / follow / straighten, arrow options, pill ink, text
sizes, tool config keys, magnifier source on re-base), `overlay::snap`, `overlay::view`,
`overlay::textedit`, `overlay::render` (tile split, dirty rect, ignored `perf_4k`),
`overlay::keys` (new chords, sheet rows, Ctrl+C wording), `overlay::esc_tests` (recent colors,
black redact default), core `annotate::tests` (old sidecar JSON, round trip, curve helpers,
head tangent, curved hit / bounds, head styles render, bold / halo, text layout, region render
equals full render, shape patch), core `config::tests` (new editor fields).

### Not verified

No synthetic input was sent to a live overlay (it covers the whole desktop while the user is
working), so everything above was verified by building, clippy and unit tests only. Manual
checks: draw two rects and move one next to the other (guides, Alt off), Shift+resize a
rect from an edge, marquee three shapes and recolor / delete / Ctrl+D / Ctrl+] them, Ctrl+C
then Ctrl+V with a shape selected (and Ctrl+C with none: image copied), Find Sensitive Info open
+ Ctrl+C (held), type text with selection / word jumps / Ctrl+B / pill, eyedropper in a capture
and in the editor, recent colors after a restart, per-tool colors after a restart, bend an
arrow and switch head styles, crop a window capture inward and save, Ctrl+wheel zoom to 800 %
and draw / resize / type there, middle-drag and Space-drag pans, a 4K capture with many shapes
stays smooth while editing.
