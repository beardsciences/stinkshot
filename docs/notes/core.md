# core (stinkshot-core)

All `todo!()` bodies in `crates/core` are implemented. `cargo test -p stinkshot-core` (98 tests) and
`cargo clippy -p stinkshot-core --all-targets -- -D warnings` are clean.

## What was built

- `image`: PNG / JPEG / lossless WebP encode, decode, `save` (atomic temp + rename), `thumbnail`
  (Lanczos3, premultiplied when alpha is present, never upscales). JPEG composites over white.
  PNG uses the png crate's fast (fdeflate) compression with adaptive filters since Nov 7 2026:
  a real 3840x2160 screenshot encodes in 47-120 ms instead of 1.1-1.9 s (zlib level 6), files
  are about 17 % larger (7.75 MB vs 6.65 MB; 6400x2160: 8.73 vs 7.36 MB). `Up` filtering alone
  was 20 % faster again but 40 % larger. `examples/encode_bench` compares the settings on any
  PNG. `thumbnail` averages k x k blocks first when shrinking by 4x or more (keeping twice the
  target size for the Lanczos pass): 27 ms instead of 75 ms for 3840x2160, 62 instead of 144 ms
  for 6400x2160.
- `config`: `dir()` (`%APPDATA%\Stinkshot`), `load()` (writes defaults when missing, backs up a
  malformed file as `config.toml.bad-<yyyymmdd-hhmmss>` then writes fresh defaults), `load_from`,
  atomic `save_to`, `history_folder()`, `parse_hex_color`.
- `naming`: all tokens, `sanitize`, `unique_path`.
- `redact`: `blur`, `pixelate`, `fill`.
- `frame`: `round_corners`, `apply_window_fx`.
- `fonts`: `load_system`, `from_bytes`, `measure`.
- `annotate`: `hit_test`, `shape_bounds`, `translate_shape`, `render`, `render_subset`, sidecars.
  Drawing lives in `annotate/render.rs`, tests in `annotate/tests.rs`.

## Design decisions

- **Redaction strength.** Blur sigma is `max(radius, 4)`: three box passes (gaussian
  approximation) on premultiplied floats, clamped at the rect edge so nothing outside the rect
  leaks in. Deterministic per-pixel noise (+-7 levels) is added before blurring. Pixelate block
  is `max(block, 4)`; cells are aligned to the rect origin, and partial edge cells are averaged.
  `fill` overwrites (never blends), and `Redact` shapes always force alpha 255.
- **Render order** follows the doc comment exactly. The magnifier samples the step-2 image
  (post-redaction, pre-spotlight, pre-annotation), so it can never show redacted pixels no matter
  its z-order relative to the redaction shape. Tested for Redact, Blur and Pixelate.
- **Spotlight** with several shapes: clear area is the union; the dim amount is the max of the
  `dim` values. Dimming is drawn under all vector shapes.
- **Highlighter** is a normal-blend stroke at 40% of the style color's alpha (a true multiply
  would vanish on dark UIs). Round caps.
- **Text** top-left anchored; baseline = line top + (1.25*size - size)/2 + ascent. A background
  pill pads the text box by 0.4*size horizontally and 0.2*size vertically (pad grows outward from
  `pos`, so `shape_bounds` is larger than the raw text box when a background is set).
- **Step** number is bold, ink-centered, black or white by luminance of the style color; a thin
  white ring sits inside the disc edge.
- **Callout**: body color = `style.fill` or else `style.color`; text color is picked for contrast
  against the body; when `style.fill` is Some, `style.color` outlines the bubble with `width`.
  Text word-wraps to the rect width and is vertically centered. The tail attaches to the side
  facing the tail point, at the intersection of the center-to-tail line (clamped away from the
  corners).
- **Magnifier**: circular insets use the inscribed circle of `dest` (zoom stays uniform); rounded
  boxes use radius `clamp(0.12*min(w,h), 4, 24)`. White ring `max(0.75*width, 2.5)`. Source
  areas outside the image repeat the edge pixels. `translate_shape` moves only `dest`, never
  `source`.
- **Shadows** (`Style::shadow`) are a single offset dark copy (0, ~0.35*width, clamped 1..3 px),
  not a blurred shadow, to keep per-shape cost tiny.
- **Hit testing**: stroke shapes hit within `width/2 + tolerance`; filled rect/ellipse hit
  inside; unfilled rect/ellipse only near the outline; arrow head counts; Blur/Pixelate/Redact/
  Magnifier hit inside; spotlight only hits near its border (so it does not swallow clicks on
  shapes it frames); callout hits its body or tail.
- **Window fx**: shadow reach is `1.5*blur` (3 sigma, sigma = blur/2) plus the `blur/3` y offset.
  With a background the canvas margin is exactly `padding` (the shadow is clipped to the canvas if
  padding is smaller than its reach). With no background the transparent margin is exactly what
  the shadow needs. With neither, the result is the rounded window at its original size.
- **Fonts**: regular = `SegUIVar.ttf` (Segoe UI Variable) -> `segoeui.ttf` -> `arial.ttf`; bold =
  `segoeuib.ttf` -> `arialbd.ttf` -> regular. Variable-font weight axes are not used, so the
  variable face renders at its default instance.
- **Sidecars**: written base-first, then JSON, each atomically. `load_editable` returns `None`
  unless both files exist; corrupt JSON is an error.
- **Config**: `load_from` normalizes hand-edited junk (jpeg_quality clamped to 1..=100, empty
  filename pattern or palette restored, non-positive stroke/font size restored, fps 0 restored).
  A leading UTF-8 BOM is tolerated. `history.folder` may contain `%VAR%` references.
- **Naming**: unknown or unterminated `{tokens}` are kept literally; `{title}` is sanitized then
  cut to 40 chars; `{app}` has `.exe` stripped; the final stem is sanitized, capped at 150 chars,
  and reserved device names (`CON`, `NUL`, `COM1`...) get a trailing `_`; an empty result becomes
  `Stinkshot`.

## Contract changes (additive)

- `image::write_atomic(path, bytes)` is public (also used by config and sidecars).
- `fonts::LINE_HEIGHT` constant (1.25), `Fonts::line_width`, `Fonts::ascent`,
  `Fonts::outline_line` (outlined glyph layout) are public.
- `redact::MIN_BLUR_SIGMA` and `redact::MIN_PIXELATE_BLOCK` constants are public.
- `Config` and `Privacy` now `#[derive(Default)]` instead of hand-written impls (same values).
- `config::Capture::open_editor_after_capture` (default false; `#[serde(default)]` on the
  table, so older configs load with it off).
- `config::Hotkeys::replace_snipping_tool` (default false, Nov 7 2026; older configs load
  with it off). See `docs/notes/snipping-tool.md`.
- Editor polish (Nov 7 2026), all additive with serde defaults:
  - `ShapeKind::Arrow` gains `ctrl: Option<PtF>` (quadratic control point, curved arrow),
    `double: bool` (head at both ends) and `head: ArrowHead` (`Notched` default, `Triangle`,
    `Open`). `ShapeKind::Text` gains `bold: bool`. Missing fields load as straight / single /
    notched / regular, and default values are not written (`skip_serializing_if`), so a doc
    without the new features serializes exactly as before. Tests:
    `old_sidecar_json_loads_with_defaults`, `new_arrow_and_text_fields_round_trip`.
  - Shared geometry made public so the editor preview matches the renderer:
    `arrow_parts` (+ `ArrowParts`, `ArrowGeom`), `arrow_centerline`, `arrow_mid`,
    `ctrl_through`, `follow_ctrl`, `quad_points`, `callout_radius`, `callout_tail_base`,
    `text_layout` (+ `TextLine`, byte ranges per laid-out line, used for the caret).
  - `render_region(work, doc, fonts, region)`: the `region` crop of `render(base, doc)` drawn
    over `work = bake_destructive(base, doc)`; equal to the full render within 2 levels
    (`region_render_matches_full_render`). The editor's render worker uses it for dirty
    rectangles.
  - `render_shape_patch(shape, fonts)`: one vector shape on a small transparent canvas (the
    editor's live text while typing).
  - Plain text whose color is dark gets a light halo instead of the black drop shadow.
  - Callout text wrapping keeps spaces between words exactly as typed (the old wrap dropped a
    leading space); render and caret share `text_layout`.
  - `config::Editor::recent_colors` (hex, newest first) and `config::Editor::tool_styles`
    (`BTreeMap<String, ToolStyle { color, width }>`), both empty by default.
- No signatures changed.

## Contract requests

None.

## Known limitations

- `Doc::hit_test` has no access to fonts (signature fixed), so text hit areas use an estimate of
  0.52*size per character. `shape_bounds` is exact; the editor should prefer it for text
  selection boxes if it needs precision.
- Callout tails are straight triangles attached to one side of the bubble. If the tail point lies
  inside the bubble rect, no tail is drawn.
- WebP output is lossless only (the pure-Rust encoder in `image` 0.25 has no lossy mode), so
  `jpeg_quality` is ignored for WebP.
- Sidecar folder `.stinkshot` is a dot-folder but is not given the Windows hidden attribute
  (no Win32 calls in core). History/platform may set it.
- Segoe UI Variable bold is not used; step digits use Segoe UI Bold.

## Manual verification checklist (needs a real desktop)

1. Run `STINKSHOT_SHOWCASE_OUT=C:\temp\showcase.png cargo test -p stinkshot-core showcase -- --ignored`
   and eyeball: arrow heads, rounded rect, translucent ellipse, smooth pen, highlighter over text,
   pill label, step circles, callout with tail, circular and rounded magnifier, dimmed spotlight.
2. On a machine with the Segoe UI Variable font removed or a different `%WINDIR%`, confirm
   `Fonts::load_system` still falls back (Segoe UI, then Arial).
3. Redaction: screenshot real text (small UI font), apply Blur radius 12 and Pixelate 12, and
   confirm it cannot be read at 400% zoom, in the flattened PNG and in the JPEG/WebP encodes.
4. Window fx on a real WGC window capture: rounded corner radius 8 at 100/125/150% scale, shadow
   on a dark and a light wallpaper, padded background.
5. Config: delete `%APPDATA%\Stinkshot\config.toml` and call `Config::load()` (defaults are
   written); corrupt the file and confirm a `.bad-<timestamp>` copy appears next to it.
6. Open a saved capture's sidecar from a path on a mapped/network drive and on a path with
   non-ASCII characters.
