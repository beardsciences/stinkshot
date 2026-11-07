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
as, Ctrl+P pin, Ctrl+T copy text, C copies the hex under the cursor, Esc steps back (abort drag
-> close popup -> cancel pending magnifier -> deselect tool/shape -> cancel overlay).

Shapes are selectable, movable, deletable, recolorable (palette popup or 1-8), resizable with
per-type handles: arrows/lines (two endpoints), rect-like (8 handles with flip, Shift = square),
pen/highlighter (bounding-box scale), text (corner scales font size), step (radius), callout
(8 handles + tail), magnifier (dest 8 handles + source 8 handles). Text and callouts edit
inline with a multi-line `egui::TextEdit` positioned over the shape (Enter = new line,
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
- Starting a brand new region after annotations exist is blocked (it would orphan them); resize
  or move the existing region instead.
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
