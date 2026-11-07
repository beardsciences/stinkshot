# Stinkshot architecture

Read `docs/BRIEF.md` first (what and why). Visual spec: `docs/design/*.dc.html` (HTML mockups
of every surface; colors/sizes there are the source of truth, mirrored in
`crates/app/src/theme.rs`).

## Workspace

| Crate | Path | Role |
|---|---|---|
| `stinkshot-core` | crates/core | Pure Rust: geometry, `Rgba`, config TOML, annotation model + flatten render (tiny-skia + ab_glyph), redaction, window frame fx, file naming. Unit tested. |
| `stinkshot-stitch` | crates/stitch | Scrolling-capture stitcher. Pure, unit tested with synthetic frames. |
| `stinkshot-history` | crates/history | Folder-of-files history + SQLite (bundled, FTS5) index, retention, thumbnails. Unit tested with temp dirs. |
| `stinkshot-platform` | crates/platform | Windows APIs via `windows` 0.62: monitors/DPI, DXGI/GDI freeze, WGC window capture, window enumeration, UIA element snapping, clipboard, Windows.Media.Ocr, SendInput scroll, LL input hooks, registry (autostart, PrintScreen takeover), single instance + IPC, window helpers (Mica, topmost, exclude-from-capture). |
| `stinkshot-record` | crates/record | WGC capture -> Media Foundation H.264 (hardware MFT preferred) + AAC; WASAPI loopback + mic mix; webcam bubble; click/key overlay; pause/resume; probe/thumbnails/trim; MP4 -> GIF (imagequant). |
| `stinkshot-logo` | crates/logo | Pure std logo renderer (ray-marched green gel orb): exe icon and tray bitmaps at build time (app `build.rs`), About page animation frames and particle field at runtime. Optimized even in dev builds. |
| `stinkshot` | crates/app | eframe/egui 0.37 app: shell (tray, hotkeys, CLI, IPC, workers, command routing), overlay editor, toast, pin, history, settings, recording HUDs, update check. |

Dependency direction: core <- stitch, history, platform <- record <- app (logo <- app, standalone). Never upward.

## Fixed contracts

The public items in each crate's stub files are the contract. They compile today with
`todo!()` bodies. Implementers fill the bodies. Rules:

1. Do not rename or remove public items, or change their signatures, in a crate you do not
   own. If you truly need a change in a crate you don't own, write it in
   `docs/notes/<your-area>.md` under "Contract requests" and work around it locally.
2. In your own crate you MAY add public items and private modules freely, and you may make
   additive, backward-compatible changes to your own signatures (new fields with defaults,
   new methods). Record every such change in your notes file.
3. `crates/app/src/state.rs` and the `Surface` trait are shared by three app areas: do not
   edit `state.rs` except the shell owner, and only additively.

## Conventions

- Pixels: physical px everywhere outside egui. Virtual-desktop coordinates for screen
  rects; image-space for annotation shapes. Convert to egui points only at draw time with
  `ctx.pixels_per_point()`.
- `Rgba` is straight alpha RGBA8. egui textures: `ColorImage::from_rgba_unmultiplied`.
- Errors: `anyhow` across crate boundaries; never `unwrap()`/`expect()` on fallible OS or
  IO calls in non-test code. UI shows failures through `Command::Notify { error: true }`.
  Bodies are plain sentences (what happened, what to do) from `app/src/errors.rs`; the raw
  chain goes to the log only.
- Logging: `log` macros; the app installs a file logger at `%APPDATA%\Stinkshot\stinkshot.log`
  (rotated at 5 MB).
- Threads: UI thread never does > ~8 ms of work. Capture flattening, OCR, encoding, history
  scans, thumbnails run on workers, posting results back through channels plus
  `ctx.request_repaint()`.
- COM: platform/record initialize COM per worker thread they create (MTA). The UI thread
  is STA (winit) and must not be blocked by COM calls.
- Network: none anywhere except `crates/app/src/update.rs`, behind the opt-in setting.
- Style: `cargo fmt`, `cargo clippy -p <crate> -- -D warnings` clean for your crate.
- No AI attribution in code, comments, commits or docs.
- UI copy: Title Case for every label (buttons, menu and tray items, window titles,
  section headings, setting row titles, tabs, chips, filter names, toast titles, HUD
  labels, tooltips): "Copy Text", "Check for Updates", "Repeat Last Region". Articles,
  short conjunctions and short prepositions (a, an, the, and, or, for, to, of, in, on, at,
  by, with) stay lowercase unless first. Sentence case only for descriptions, helper text,
  status sentences and error bodies.

## Overlay (app/src/overlay)

- Opening: shell freezes all monitors first (target: hotkey -> overlay visible < 150 ms),
  then creates the overlay viewport sized to `frozen.bounds` (physical) via `set_rect`.
- States: Hover (window/element highlight + loupe) -> Selecting (drag) -> Selected
  (handles, size label, presets; toolbars visible) -> Annotating (a tool active) -> Finish.
- Element snap: `ElementSnapper::request(hwnd_under_cursor, p)` on mouse move; draw
  `latest().rects[level]`; wheel changes `level`; falls back to window rect.
- Keys: Esc (cancel / deselect tool), Enter or Ctrl+C (copy), Ctrl+S (save as), Ctrl+P
  (pin), Ctrl+T (copy text), Ctrl+Z/Ctrl+Y, Del, tool shortcuts A arrow, L line, R rect,
  E ellipse, P pen, H highlighter, T text, N step, B blur, X pixelate, K redact box,
  M magnifier, O callout, S spotlight, V select; 1-8 palette colors; [ ] stroke width.
  F1 or ? toggles the shortcut cheat sheet. Key table: `overlay/keys.rs` (handling, hints
  and the cheat sheet all read it).
- Re-edit: `OverlayMode::Annotate` with the stored base and doc; finishing overwrites the
  history item (`CaptureOutput.editing`).

## Testing

- core/stitch/history: `cargo test -p ...` with real assertions (golden-ish pixel checks,
  stitch round-trips on synthetic scrolled images with sticky header/footer, FTS queries,
  retention edge cases).
- platform/record: `#[ignore]` integration tests that need a desktop session (capture a
  monitor, OCR a rendered text image, record 2 s to MP4 and probe it). Run them with
  `cargo test -p <crate> -- --ignored` on the dev box.
- app: build + `cargo clippy`; manual verification list in `docs/notes/app-*.md`.
