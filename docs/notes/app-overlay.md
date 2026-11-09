# app-overlay notes

Area: `crates/app/src/overlay/**`. Region overlay and inline annotation editor.

## What was built

All three modes of `OverlayMode`:

- **Capture**: hover highlight (UIA element via `ElementSnapper`, wheel cycles element -> parent
  -> window, falls back to `windows_enum::window_at` over `req.windows`, then the monitor rect),
  click snaps, drag draws a free region (Space moves the region while dragging, Shift forces
  1:1, a chosen aspect preset constrains it). Selected state has 8 handles, Space+drag or
  Select-tool drag on empty space moves, arrows move the region 1 px (Shift+arrows resize
  the right/bottom edge, Ctrl makes either 10 px; shapes keep Shift = 10 px). Arrows with
  no region start one (keyboard-only path, see accessibility.md), size label + presets chip (Free, 16:9, 4:3, 1:1, Exact W x H input),
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
   moves; arrows move; Shift+arrows resize; Ctrl+arrows 10 px steps.
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

Review fixes (Nov 7 2026):

- IME: while typing, the overlay sets `PlatformOutput.ime` every frame (caret rect from
  `caret_pos` through `img_to_pt`; `Overlay::ime_output`), because egui-winit only enables
  the OS IME when an `IMEOutput` is present. `Event::Ime(Commit)` is typed like
  `Event::Text`; `Preedit` is kept in `EditState::preedit` and shown in place of the
  selection, underlined (`textedit::with_preedit`), never in the buffer. While composing,
  keys, clipboard events, Esc and Ctrl+Enter go to the IME; finishing drops an unfinished
  composition. Caveat: the render fonts (Segoe UI Variable / Segoe UI / Arial) have no CJK
  glyphs, so composed CJK text renders as missing-glyph boxes until a fallback font exists.
- Ctrl+Z / Ctrl+Y / Ctrl+Shift+Z undo the typing itself (`textedit::TextHistory`: a run of
  typed characters is one step, a caret move ends the run, delete / paste / cut / Enter are
  one step each). The editor's history still records the whole edit as one step at the end.
- The size chip (ratio presets, Exact) ends typing first, and while the Exact field is open
  `text_keys` does not run, so keys typed there no longer land in the text shape.

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
  re-based when the region's origin moves; magnifier sources move with them now). The re-base
  (`Overlay::rebase`) also covers the shape clipboard (Ctrl+C copies paste back onto the
  same spot of the picture) and the pre-edit document of text being typed. A new region
  can be dragged outside the old one even with shapes; shapes outside it are kept but clipped.
- Capture mode: expanding pulls more pixels from the frozen screen (the base is re-cropped).
- Annotate mode (window capture, History Edit): the whole picture stays visible, the part
  outside the crop dimmed like the screen; the selection handles crop inward
  (`Overlay::crop`, image px; `set_sel` converts canvas px to a crop inside the image, so it
  can never grow past the picture). Finishing outputs the cropped base and the re-based doc.
  Arrow keys change the crop itself in whole image px (`set_crop`; Ctrl+arrows resize), so a
  nudge is 1 (Shift: 10) image px and moving never changes the crop size. Moving the region
  by drag does not exist in the editor (Space+drag pans there).
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

## Opening without a flash (Nov 7 2026)

Benchmark finding: the overlay first appeared at 8000x2700 (the 6400x2160 desktop scaled by
125 %), then `on_created` resized it, and the tool-window restyle hid and re-showed it 80-140 ms
later. The viewport builder passed physical px as points, and the window was created visible.

Now (`viewport_builder`, `Surface::show`, `prepare_window`, `after_paint`):

1. The viewport is created **hidden**, with position and size divided by the primary monitor's
   scale (`create_scale`): winit creates new windows on the primary monitor and egui converts
   points with that monitor's scale, so the window is born at the final physical rect.
2. The window exists before the first frame's UI runs, so `show` finds it (no-message
   `FindWindowExW`) and `prepare_window` sets tool window, topmost, the exact rect (no-op unless
   a DPI change moved it), exclude-from-capture, then **cloaks** it (DWM) and shows it. Still
   hidden at the restyle, so nothing is hidden and re-shown.
3. That first frame is painted onto the shown, cloaked window; `wants_frame_now` makes the
   shell paint a second frame in the same pass, and `after_paint` uncloaks after the second
   present and brings the window to the foreground.

Why the cloak and two presents: checked by temporarily making the overlay capturable and
grabbing the screen right after it became visible. Shown uncloaked after a frame painted while
hidden: a white frame in 3 of 4 opens (that hidden present is not kept). Uncloaked after the
first present on the shown window: white in 3 of 6. After the second present: no white frame
in 16 opens. Waiting for the second frame in the next shell pass instead cost 35-117 ms (the
next pass started late); painting it in the same pass costs about 1 ms. Shown to uncloaked is
about 28 ms (first frame's UI, tile upload, two presents).

Result (50 runs, release, back to back with the old build): settled at final size 228 -> 167 ms
median, p90 244 -> 182 ms; no size change after the window becomes visible (was 1-2 per open).
The harness's "visible" moved from 120 ms (oversized window) to 167 ms because the window is now
only visible once correct. Not verified: a layout whose primary monitor's DPI differs from the
monitor holding most of the desktop (the hidden window is corrected by `set_rect` before it is
shown, but that path was not exercised on this machine).

## Multi-region capture (Nov 8 2026)

`overlay/multi.rs`. In the screen overlay (Capture mode), with a region selected:

- **Shift+drag outside the selection** adds a region; the previous one keeps an outline and a
  number badge. Shift+click on a highlighted window or element adds it (hover snapping runs
  while Shift is held outside the regions). The **Add Regions** toggle (first button of the
  action bar, two-rectangles icon, screen overlay only) does the same without Shift.
- During an add gesture Shift does not mean 1:1 (an aspect preset still applies).
- Clicking an earlier region makes it the active one (handles, nudging, size chip). Esc with
  several regions removes the active one and activates the previous. The size chip shows
  "N Regions"; the hint bar says Enter copies all.
- Picking any tool other than Select, any finish action (Enter, Copy, Save, Pin, Copy Text) or
  Find Sensitive Info combines the regions (`combine_regions`) and switches the overlay to the
  editor (`Kind::Annotate`) on the combined picture, so annotations, redaction and crop work
  across all regions. The result is a normal new capture (`mode` "region", not an edit).
- Regions are added only while the active region has no annotations (otherwise a notice:
  "Add regions before annotating.").

Layout decision: regions keep their **relative positions** on a **transparent** canvas, and any
empty stretch between them (per axis) is squeezed to 16 px (`GAP_PX`). Relative placement keeps
what was beside or above what (a menu and its dialog, two parts of a form) and lets an arrow
point from one region to another; squeezing stops two regions on opposite monitors from making a
mostly empty picture. Stacking was the alternative: simpler to read in a chat, but it loses the
spatial relation, which is the reason to take several regions of one screen at once. The
transparent gaps paste cleanly (the clipboard carries alpha; the plain-DIB fallback composites
on white); in the editor they are drawn over the window background color (`combined`), not over
the frozen screen.

Tests: `multi::tests` (squeezed gaps and 1:1 pixels, overlap/side by side/clipping, dimming
around several holes), `hover::tests::scrolling_starts_at_the_outer_document`.

Not verified: the gestures themselves on a real desktop. This session sent no synthetic input
(the displays were off and input would wake them), so Shift+drag, the toggle, Esc removal and
the combine-into-editor path are covered by unit tests and a build only. Manual checks:

1. Region, then Shift+drag two more on another monitor: three outlines with badges 1-3, size
   chip "3 Regions"; Enter copies one picture with the regions in place and 16 px gaps.
2. Shift+click a window outside the selection: it is added at its window rect.
3. Toggle Add Regions, drag without Shift: adds; toggle off: a drag replaces the active region.
4. Click region 1: it gets the handles; resize it; Esc removes it.
5. With three regions pick Arrow: the editor opens on the combined picture; draw an arrow from
   region 1 to region 3; Ctrl+S saves a PNG with transparent gaps.

## Scrolling capture picks the page (Nov 8 2026)

In the scrolling-capture overlay the first highlight on a new element chain is the outermost
UIA `Document` in it (`hover::scroll_target_level`), so one click on a browser page selects the
web page without the tab strip, address bar or window frame; the wheel still walks the chain.
Not verified on a real browser (no input to other apps in this session).

## Editor tools: codes, tables, color picker, measure, beautify, CJK (Nov 8 2026)

Six features, all offline. No network code was added; nothing is uploaded or opened.

### Codes and Text panel (Q, QR button in the action bar)

`scan.rs`. On a worker: `stinkshot_core::codes::decode` (rxing, pure Rust zxing port; QR,
Micro QR, Data Matrix, Aztec, PDF417, EAN / UPC, Code 128 / 39 / 93, ITF, Codabar, GS1
DataBar), then OCR when a language is installed. The panel (same spot as Find Sensitive Info;
the three panels are exclusive) lists each code with its format and text and a Copy button,
"Copy Link" for http(s) text. Links are only copied, never opened. Found codes are outlined
on the canvas. Below: a 3-line text preview, Copy Text, and Copy as Table (enabled when
`stinkshot_core::table::detect` finds columns; tooltip "N rows x M columns, tab-separated"
or "No columns found"). A failed OCR says "Could not read text. Try a larger selection."
instead of "No text found." (`ScanResult::ocr_failed`, review fix). Copy shows "Copied" on the button for 1.5 s. Results drop when the
selection changes (same rule as Find Sensitive Info). Esc closes.

Decode time (dev profile, deps at opt-level 2, busy synthetic background): 800x600 208 ms
with nothing found (includes the 2x retry pass), 112 ms with a QR; 1920x1080 186 / 322 ms;
3840x2160 574 / 992 ms. Worker only, spinner meanwhile.

### Copy as Table

`stinkshot_core::table`: words are regrouped into rows by vertical overlap (Windows OCR splits
one row into several "lines" at wide gaps), a gap over 1.1 x the text height starts a cell,
columns are x ranges covered by most multi-cell rows (a spanning title row does not merge
them), each cell goes to the first column it overlaps. Tabular = 2+ columns and 2+ rows using
2+ of them, at least half of all rows. Output is TSV (tabs / newlines in cells become spaces).
Tests: grid, right-aligned numbers with a missing cell, jitter and shuffled input, prose (not
a table), one row (not a table), spanning title row, OCR lines split per cell.

### Color picker

- Standalone: `Action::PickColor` ("Pick Color", tray item, hotkey `[hotkeys] pick_color`,
  empty by default so it never collides; set it in Settings > Hotkeys; CLI
  `stinkshot.exe pick-color`) -> `Command::PickColor` -> overlay
  `SelectRegion(RegionPurpose::PickColor)`. The screen is frozen and not dimmed, the loupe is
  always on, no window / element snapping. Click (or Enter) copies the pixel in the chosen
  format and closes; a "Color Copied" toast shows the value. Tab cycles Hex / RGB / HSL. A
  panel under the hint bar has the format chips and up to 8 recent picks (click one: copied
  again, closes). Esc cancels.
- In every overlay, C now copies in the picker format (Hex by default, so unchanged unless
  the user switched) and adds to the recent picks. The editor's eyedropper (I) is unchanged.
- Config: `[editor] picker_format` ("hex" / "rgb" / "hsl"), `[editor] picker_recent`.
  Formats in `stinkshot_core::color` (hex upper case as before, `rgb(30, 144, 255)`,
  `hsl(210, 100%, 56%)`).

### Measure tool (U, ruler in the tool bar)

`measure.rs`. Drag inside the selection (anywhere on the picture in the editor): a ruler with
end ticks and dashed legs, and a label "240 x 120 px · 268.3 px · 26.6°" plus a logical line
"160 x 80 · 178.9 at 150%" when the capture's scale is not 100 %: on the frozen screen the
monitor under the selection; in the editor the scale carried in `SourceInfo::scale` (window,
full-screen and scrolling captures); a history Edit has no recorded scale, so no logical line
(review fix: it used the monitor the editor was on). Units are
image px, which are physical px of the capture (also in the scaled-down editor view). Ends
snap to the strongest luminance edge (step >= 28) within 6 px on each axis, searching the
second axis along the first axis' edge so box corners snap, when Settings > Capture > Snap to
Elements is on; Alt flips it while held (hint "Alt  No Edge Snapping", or "Alt  Snap to Edges"
with the setting off; review fix: the setting was ignored); Shift keeps 0 /
45 / 90 degrees. Nothing is added to the doc; the ruler shows while the tool is active and
clears when the region is re-based. Tests: length / angle, labels at 100 and 150 %, Shift
constraint, edge snap on a synthetic box (edge to edge = box width), faint steps ignored.

### Beautify (F, frame button in the action bar)

`beautify_panel.rs` + `stinkshot_core::beautify`. Apply on Export toggle, a live preview
(296 x 170, annotations included once the render worker has them), Background Gradient /
Solid / Blurred with 6 preset swatches each (blurred = the capture scaled to cover, blurred
and darkened), Padding 0-256 px, Corners 0-48 px, Shadow, Aspect Free / 16:9 / 1:1 / 4:5 (the
canvas grows, nothing is cropped). Applied on the finishing worker to Copy, Save As, Pin and
Quick exports, never to Copy Text. The history file is framed; its re-edit sidecar keeps
the picture without the frame (fixed Nov 8: before, an unannotated capture, or any capture
with Keep Editable Originals off, kept no sidecar, so a re-edit framed the frame and
overwrote the file). With annotations and Keep Editable Originals on the sidecar is the base +
doc as usual; otherwise it is the flattened, unframed picture with no annotations, which holds
nothing the export does not show (redactions already burned in). So a re-edit is unframed
and gets the frame again on export. The canvas is capped at 100 MP
(`beautify::MAX_CANVAS_PIXELS`): past it the aspect is dropped (padding only, the panel says
so in one line), and past it even padded the capture is exported unframed; the canvas
allocation is checked first, so a tall scrolling capture can no longer abort the process
(1920 x 30000 in 16:9 asked for 6.45 GB). Settings live in `[editor.beautify]`,
saved when the panel closes, on finish and on cancel (not per slider frame). The frame
button is highlighted while Beautify is on, tooltip "Beautify (F) · On".

`beautify::apply` cost (dev profile, core at opt-level 2): 1920x1080 solid 27 ms, gradient
26 ms, blurred 82 ms; 3840x2160 63 / 73 / 262 ms (first version 210-1012 ms; now LUT
gradient, fixed-point bilinear, shadow only within reach and not under the opaque capture,
opaque rows copied). The preview uses `beautify::preview_thumbnail` (3 x 3 sampled, cheap
enough for the UI thread).

### CJK and emoji text

Core `Fonts` falls back per character (see core.md). egui gets the same faces through a
`FontProvider` registered in `theme::install`, asked only for characters the UI font lacks,
so the editor UI (and every other window) shows CJK / emoji as well. Since text annotations
are typed straight onto the image with core's renderer, the caret, wrapping and output agree.
The IME caveat above (CJK showing as boxes) is fixed.

### Contract changes (additive)

`state.rs`: `Command::PickColor`, `RegionPurpose::PickColor` (shell: freezes first, opens the
overlay, `region_chosen` ignores it). `actions.rs`: `Action::PickColor` (Settings row and tray
item come from `Action::ALL`; `hotkeys::bindings` and `settings_ui::HOTKEY_FIELDS` now size
from `Action::ALL.len()`). `cli.rs`: `pick-color`. `theme.rs`: `Icon::Ruler`, `Icon::Qr`,
`Icon::Frame`, and the font provider. `ui::action_bar` takes a `PanelState` instead of
`smart_open`.

### Not verified live

No overlay was opened on the desktop (it covers every monitor while the user works, and is
excluded from capture so screenshots would not show it). Verified by build, clippy, unit
tests and rendered PNGs (mixed-script text, three Beautify variants). Manual checks: Q on a
screenshot with a QR code (and a barcode, and a dark-mode inverted QR), Copy / Copy Link,
Copy as Table from a spreadsheet screenshot pasted into Excel; Pick Color from the tray after
binding a hotkey, Tab, recent picks after a restart; U across a button (edges snap, Alt off,
Shift straight) on a 150 % monitor; F with each background and aspect, then Copy / Save /
Pin, and a cancelled capture keeps the settings; type Chinese / Japanese / Korean / emoji with
the IME in a text annotation and in the History search box.

## Localization (Nov 8 2026)

See docs/notes/l10n.md for the framework.


- Every overlay string is in `l10n/en/overlay.rs`; the checker reports 0 for `overlay/`.
- Keyboard key names (Ctrl, Shift, Enter, Esc, Tab, F1, letters) stay as typed in code and are
  not translated. Mouse gestures in the hint bar and cheat sheet are keys
  (`overlay.keys.k.drag`, `overlay.keys.k.wheel`, ...), so "Drag" can be translated.
- Shortcut labels are placeholders: `"Pin to Screen ({key})"` with `key = keys::PIN.label`.
- `Tool::name()` is the translated tool name; `Tool::label()` appends the key from
  `keys::TOOL_KEYS` ("Arrow (A)") and returns a `String`.
- Core enums keep their English `label()` for other callers; the overlay maps the words it
  shows to its own keys (`beautify_panel::aspect_label` for Free, `smart_redact::kind_label`
  for the sensitive-info kinds). Format names stay English: barcode symbologies from
  `stinkshot_core::codes` ("QR Code"), color formats (Hex, RGB, HSL), aspect ratios.
- UIA control types in the element chip ("Pane", "Button") come from Windows in English and
  are not translated; only the "Level {level} of {count}" frame is.
- Pseudo renders without opening an overlay: `overlay/pseudo_shots.rs` (ignored test) draws the
  bars, hint bar, loupe, cheat sheet and the Beautify, Codes and Text, Find Sensitive Info and
  picker panels offscreen with wgpu and saves PNGs:
  `$env:STINKSHOT_SHOTS_DIR='C:\dev\stinkshot-bench\ux-shots\pseudo-overlay'; cargo test -p stinkshot pseudo_shots -- --ignored`.
  It switches the global language, so run it alone.
- Found with it: long sensitive-kind names ran into the masked preview in Find Sensitive Info;
  the name now truncates with an ellipsis before the preview. Still to watch: the hint bar is
  one line (~1300 px in pseudo with the discard prompt), and the cheat sheet wraps to two rows
  of three at 1920 px in pseudo (same height as English on 1080p).

## Box highlighter (Forgejo #1, Nov 9 2026)

- The Highlighter has two modes, Freehand and Box. While the tool is active, two mode buttons
  follow the width button at the end of the tool bar (tips "Freehand (Hold Shift for Box)" /
  "Box (Hold Shift for Freehand)"); they sit last so showing them never moves the tool
  buttons. The mode is sticky (`ShapeOptions::highlight_box`) and saved as
  `editor.highlighter_box` in the config.
- Holding Shift while dragging draws the other mode (`shapes::highlight_box_mode`); the
  preview follows Shift live and the state at release wins. Box drags do not snap.
- Box highlights are `ShapeKind::HighlightBox { rect }` with the highlighter style (alpha
  0x99, width max(14, 4 x stroke)), filled with `annotate::highlight_color`, no outline. They
  hit anywhere inside, have 8 rect handles, recolor and resize like rects, and the live
  preview paints exactly the renderer's color.
- A selected freehand stroke gets a context bar with To Box ("Replace with a Box
  Highlight"): the stroke becomes the box over its painted extent (points' bounds grown by
  half the marker width, which is its selection bounds), same id, style and z-order, one
  undo step.
- Cheat sheet: Shift+Drag "Other Highlighter Mode".
- Not verified live: drawing both modes, Shift flip mid-drag, To Box and undo, reopening a
  saved capture with box highlights.

## Editor Window (Forgejo #4, #5, #9, Nov 9 2026)

`overlay/window_host.rs`. The annotation editor in a normal top-level window (taskbar, Task
View, movable, resizable, title "Stinkshot Editor", or "file.png - Stinkshot Editor" for a
history item; a second untitled window is "Stinkshot Editor (2)"). Several can be open at once,
at most one per history item.

- How: the same `Overlay` runs in Annotate mode with `windowed` set. Its `bounds` are the
  client rect (origin = the client's screen position, so pins open where the picture was) and
  `monitors` is that one rect with the window's scale, so every bar, chip, popup and panel is
  laid out and clamped inside the window. Each frame `EditorWindow::sync_layout` re-fits the
  picture (`window_host::fit`: never scaled up, 16 pt sides, 88 pt on top for the hint bar
  and size chip, one or two bar rows under it) when the client size, DPI or bar rows change;
  the crop, annotations (image px) and history are untouched, a zoomed view is re-clamped.
  Overlay hooks for `windowed`: no frozen screen (window background instead, dimming only the
  cropped-away picture), close button handled by the host, toolbar spot not remembered for
  the screen overlay, no Edit in Window button, no UIA snapper (now skipped for every
  Annotate overlay).
- Opening: created hidden on the primary monitor with its client size in points (the picture
  at 100 % plus margins, at least 640 x 420 pt, at most 90 % of the work area), then cloaked,
  shown and moved to the monitor under the cursor; Windows rescales it for that monitor's DPI;
  once its rect is stable for a frame (2-20 frames) it is centred on that work area and
  uncloaked. Moving it to another monitor later is plain Windows behavior (egui follows the
  DPI, the picture re-fits).
- History > Edit, the toast's Edit and a pin's Open in Editor (`Command::EditItem`) open an
  Editor Window instead of freezing the screen; if that item is already open its window comes
  to the front. Works during a recording, a scrolling capture or beside an open overlay.
- Edit in Window (new action bar button, window icon, between Save and Cancel, screen overlay
  and fullscreen editor): commits text, combines multi-regions, pushes
  `Command::OpenEditor(EditorRequest)` with the cropped picture and its annotations, and
  closes the overlay (the screen unfreezes). A fresh region keeps its virtual rect
  (`fixed_rect`), so the finished capture still sets repeat-last-region and a pin opens on the
  original spot; the measure tool keeps the monitor's scale.
- Settings > Capture > After Selecting a Region (`[capture] after_region`): Annotate in Place
  (default, `"annotate"`) or Open Editor Window (`"window"`). With the window choice Enter on
  a selected region does Edit in Window (hint bar "Enter Edit in Window", "Ctrl+C Copy";
  tooltips "Edit in Window (Enter)" and "Copy to Clipboard (Ctrl+C)"); Ctrl+C still copies
  and closes. Window and full-screen captures still open in place.
- Finishing goes through the same `Command::CaptureDone` path as the overlay
  (`edit_save.rs`, #6). For a history item, Save (Ctrl+S) writes back to it and the window
  stays open (`window_host::stays_open`; the saved doc becomes the clean state, so closing
  afterwards does not ask). Save as Copy, Save As (Ctrl+Shift+S), Copy, Pin and Copy Text
  close the window, like the full-screen editor.
  Closing with unsaved annotations (edits here, or annotations carried over from the overlay
  that were never saved) asks "Discard Changes?" with Keep Editing / Discard (Esc keeps
  editing); Esc's own discard prompt is unchanged. A close while a finish renders waits for it.
  The `Command::Cancelled` an Editor Window pushes is dropped (it is not a screen capture).

Tests: `overlay::window_host::tests` (fit at 100 % and scaled, tiny window, bar rows, initial
size and limits, unique titles, centring), core `config::tests` (after_region default and
round trip).

Not verified live: no window was opened on the desktop in this session (the user's own copy
runs with its hotkeys). Manual checks: History > Edit on two items (two windows, Task View
shows both, Edit again focuses), resize and maximize (picture re-fits, bars stay inside),
drag the window to a monitor with another scale, annotate and close (prompt), Ctrl+wheel zoom
and pan, Save As / Pin from a window (pin opens over the picture), After Selecting a Region =
Open Editor Window then Enter on a region (screen unfreezes, window opens with any
annotations), Edit in Window button with three regions, and repeat-last-region afterwards.

## Text boxes, Select Text, snapped regions (Forgejo #10, #12, #13, Nov 9 2026)

- **#10 Text box size first.** Text tool: a click still makes auto-size text; a drag at least
  12 pt (and 16 image px) wide draws a fixed box (dashed preview), text wraps at spaces to its
  width and the height follows the lines. `ShapeKind::Text { width: Option<f32> }` (serde
  default, skipped when None, so old sidecars load as auto). Wrapping is one algorithm
  (`annotate::wrap_with` / `text_line_ranges`) used by core's render, `text_layout` (caret)
  and `text_rect`, and by the editor's `shapes::text_rect` through its `Measure`, so export
  and the on-screen render match. A word wider than the box keeps its own line and widens the
  bounds. Text now has side handles (Left / Right) that set the box width (on auto text a side
  drag fixes the width); the corner still scales the font. The live egui preview (only while a
  shape is moved) wraps with egui's layout, an approximation. No setting was added.
- **#12 Select Text tool (W, I-beam icon, last tool).** On first use per capture
  (`select_text.rs`), `Shared::ocr` reads the base image on a worker ("Reading Text..." in
  the hint bar); words come back in OCR reading order (line order, then words). All words get
  a faint outline, the hovered word an accent outline; drag (or Shift+click) selects words
  from the anchor to the word nearest the pointer (nearest line, then nearest word), Ctrl+A
  selects all, Ctrl+C or the Copy Text button under the last selected word copies (spaces in a
  line, newlines between lines), Esc clears the selection. The cache follows the base `Arc`:
  a new region or crop reads again, switching tools keeps it. Notices: "No Text Found",
  "Could Not Read Text", "Select Text Needs an OCR Language in Windows Settings".
- **#13 Snapped regions stay on their element.** A region made by clicking a highlighted
  window or element (first or added) gets a `selection::SnapTie` with the hover chain
  (`Overlay::hover_chain`, innermost first, canvas px). Moving and resizing it stay inside the
  chain's root (the outermost rect: the top-level window, e.g. the taskbar for a taskbar
  button), and resize handles snap to every chain edge within `snap::SNAP_PT`. Alt drags are
  free (hint "Alt  Free Resize", cheat sheet "Alt+Drag"); an Alt drag that leaves the root,
  or any other change (arrows, Exact size, presets), ends the tie. Ties match their region
  by rect, so they follow region switching in multi-region capture; combining clears them.
  Free drags on empty space are unchanged.
- Not verified live (no synthetic input): text box drag and wrap, the OCR word selection and
  Copy Text on a real capture, tied resize on the taskbar.
