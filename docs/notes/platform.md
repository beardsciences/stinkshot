# Platform layer notes (`stinkshot-platform`)

Everything in `crates/platform` is implemented against `windows` 0.62. No `todo!()` is left.
Desktop integration tests live in `crates/platform/tests/desktop.rs` (`#[ignore]`), plus an
optional visual dump in `tests/visual.rs`.

Run: `cargo test -p stinkshot-platform -- --ignored --test-threads=1`
(some tests touch the real clipboard and the single-instance mutex; `capture_a_window` takes
about 5 s here because of the WGC hang described below).

## What was built

| Module | Notes |
|---|---|
| `dpi` | `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)`; failure (already set by manifest) is logged at debug. |
| `monitors` | `EnumDisplayMonitors` + `GetMonitorInfoW` (EX) + `GetDpiForMonitor(EFFECTIVE)`. `list()` is sorted primary first, then left-to-right. |
| `capture` | DXGI Desktop Duplication per output with one cached session (see below), BGRA/RGBA8/R10G10B10A2/FP16 conversion, GDI `BitBlt(CAPTUREBLT)` fallback, WGC window capture, PrintWindow fallback, cursor drawing. |
| `windows_enum` | `EnumWindows` z-order, `DWMWA_CLOAKED` filter, `DWMWA_EXTENDED_FRAME_BOUNDS`, process name from `QueryFullProcessImageNameW`. |
| `uia` | `ElementSnapper`: dedicated MTA worker, lazily built per-window node tree with cached `BoundingRectangle`/`IsOffscreen` (CacheRequest through the ControlView walker), coalesced requests. |
| `clipboard` | CF_DIBV5 (BITMAPV5HEADER, BI_BITFIELDS, straight alpha, bottom-up) + registered "PNG" + CF_DIB (composited on white), CF_UNICODETEXT, CF_HDROP + "Preferred DropEffect". `OpenClipboard` retried for about 0.5 s. |
| `ocr` | `Windows.Media.Ocr` behind `OcrEngine` trait. 2x CatmullRom upscale when the longest side is < 1600 px, padding to 48 px minimum, vertical tiling (3000 px, 96 px overlap) with centre-based de-duplication, per-line rects from word boxes mapped back to input pixel space. |
| `input` | `cursor_pos`, `scroll_at` (SetCursorPos, N x SendInput wheel, restore), `InputHook` (WH_MOUSE_LL / WH_KEYBOARD_LL on a private message-loop thread, key-repeat suppressed), `key_label`. |
| `system` | Run-key autostart, PrintScreen takeover with remembered previous value, `reveal_in_explorer`, `open_path`, dark mode + accent, single-instance mutex, `WM_COPYDATA` IPC through message-only window `StinkShot.IPC`, `play_capture_sound`. |
| `window_util` | `find_own_window`, `set_rect` (move then resize, so DPI changes settle first), topmost, `WDA_EXCLUDEFROMCAPTURE`, Mica + immersive dark + rounded corners + glass frame, `force_foreground`, `WS_EX_TOOLWINDOW`. |

## Design decisions and findings

### Desktop Duplication
- One process-wide session, cached behind a mutex, rebuilt when the monitor layout changes or
  on `DXGI_ERROR_ACCESS_LOST`. `freeze_all` and `capture_rect` share it. `AcquireNextFrame`
  uses a 2 ms timeout on a cached session (the duplication always holds the whole desktop
  image, so a timeout simply means "unchanged").
- Each output keeps a GPU-side shadow texture that is refreshed on every real frame, so static
  screens still return full images. Region reads use `CopySubresourceRegion` into a staging
  texture, so `capture_rect` of a small area is cheap (about 3 to 12 ms for 400x300 on this PC).
- **The first frame of a new duplication is a placeholder** (observed: an uninitialized
  `R16G16B16A16_FLOAT` surface even on SDR content, later frames are BGRA8). It is never
  trusted: the shadow texture is seeded once from a GDI capture of the output, and real frames
  replace it. The shadow/staging textures are re-created whenever a frame's size or format
  differs from them.
- Rotated outputs and software-composed outputs use GDI instead. GDI is also the fallback for
  any duplication error (secure desktop, driver reset, no adapters).
- `DuplicateOutput1` is asked for BGRA8 only, so HDR outputs are tone-mapped to SDR by the
  system. The float/10-bit converters are there as a safety net (linear scRGB to sRGB with a
  4096-entry LUT).
- Conversion is split across up to 8 threads by row bands.
- Timing on this box (2 monitors, 6400x2160 virtual desktop): `freeze_all` about 40 to 55 ms
  with an optimized build once the session exists; **the first capture takes 0.3 to 0.6 s**
  (device + duplication creation + GDI seed). Call the new `capture::warm_up()` from a worker
  thread at startup. A debug build without optimization is about 1 s per freeze because the
  conversion loops are unoptimized.
- `draw_cursor` recovers straight alpha by rendering the cursor with `DrawIconEx` once over
  black and once over white, which handles both alpha and legacy mask cursors.

### Window capture (WGC) and a hang found on this machine
- `capture_window` runs `Windows.Graphics.Capture` on a private MTA thread with a 5 s timeout
  (the caller's apartment does not matter, so it is safe from the UI thread).
- **On this PC (Windows 11 build 26200) `Direct3D11CaptureFramePool::CreateCaptureSession`
  never returns**, for window and monitor items, `CreateFreeThreaded` and `Create` pools,
  MTA and STA, with and without a dispatcher queue or a window, on either adapter. The privacy
  consent keys are all `Allow`. It was reproduced in a 40-line standalone program. Cause not
  found. The WGC code path (`wgc_capture`) is complete (frame pool, cursor/border flags, staging
  copy, premultiplied alpha fix) but **could not be verified end to end here**.
- Because of that, `capture_window` falls back to `PrintWindow(PW_RENDERFULLCONTENT)` on any WGC
  failure or timeout, and remembers a hang for 5 minutes so later calls skip WGC immediately
  (`Windows.Graphics.Capture disabled after an earlier hang` in the log). The fallback captures
  occluded windows, crops to the DWM frame, and applies Win11 rounded-corner alpha (8 px at
  100% scale, anti-aliased) for non-maximized windows. This was verified visually (the Podman
  Desktop window came out correct and complete while partially covered).
- The `record` crate also needs WGC; it will hit the same hang on this machine, so check this
  early and decide whether it needs its own fallback (for example Desktop Duplication cropped to
  the window rect).

### Window list
- Untitled windows are skipped. Click-through layered windows and non-activating layered tool
  windows are skipped too: the NVIDIA GeForce overlay is a full-screen CEF widget with those
  styles and otherwise wins `window_at` for every point on the primary monitor.
- Taskbars (`Shell_TrayWnd`, `Shell_SecondaryTrayWnd`) are included with `title == "taskbar"`
  and `process == "taskbar"`.

### UI Automation
- Electron/Chromium apps (Claude, Discord, VS Code) expose no UIA children until an assistive
  technology flag is set, so snapping falls back to the window rect for them. Native and
  Firefox windows produce inner rects (verified: 3 nested rects for Firefox). Nothing to fix in
  this crate; the overlay can still snap to the window.

### OCR
- One `OcrEngine` instance rejects overlapping `RecognizeAsync` calls ("Another RecognizeAsync
  operation is already running"), so `WindowsOcr` serializes them with a mutex; sharing one
  engine across threads is safe. 20 to 40 ms for a short line.
- The task asked for the OCR test to render text with `stinkshot_core` fonts and annotate
  render. Those are still `todo!()` stubs in this worktree, so the test renders black-on-white
  text with GDI instead (Segoe UI at 18 and 40 px, plus a 900x7000 image to exercise tiling).

### Clipboard
- Uses the `image` crate (PNG) for the registered "PNG" format instead of `Rgba::to_png`, so the
  crate does not depend on another area's implementation state. This adds `image` to the
  platform crate's dependencies (workspace version, png only). `Cargo.lock` changes accordingly.

### System / IPC
- IPC payload is UTF-8 args joined by `\0`, tagged with `dwData = 0x53544B53`; an empty
  `Vec<String>` is delivered for an empty payload (the app can treat it as "show").
  `send_to_running_instance` uses `SendMessageTimeoutW` (3 s, abort-if-hung).
- `reveal_in_explorer` runs the shell COM work on a private thread so a UI-thread (STA) caller
  is not blocked and its apartment is untouched; falls back to `explorer.exe /select,`.
- `take_over_printscreen` stores the previous value in `HKCU\Software\StinkShot\PrevPrintScreenSnipping`
  (`0xFFFFFFFF` = value did not exist). Taking over twice is idempotent and never overwrites the
  saved original.
- `play_capture_sound` plays the `Notification.Default` alias with `SND_NODEFAULT` (silent if the
  alias is missing, never the default beep).

### COM
- `util::ensure_com()` initializes MTA on the calling thread once and ignores
  `RPC_E_CHANGED_MODE`. It is called on platform-created worker threads and from `WindowsOcr`.
  **Do not call platform OCR/UIA setup from the UI thread before winit has initialized it**,
  or the thread would become MTA. The app normally runs these on workers anyway.

## Contract changes (all additive)

- `capture::warm_up()`: create the duplication session ahead of time.
- `capture::release_cached_capture()`: drop the cached session.
- `clipboard::get_text()`, `get_image()`, `has_dibv5()`: read-back helpers (used by tests).
- `input::key_label(vk, scan, extended)`: public so the visualizer can label arbitrary keys.
- `Cargo.toml`: added `image` dependency and extra `windows` features.
- Private `util` module (COM init, wide strings, registry helpers).

## Contract requests

- None blocking. `stinkshot-core` clippy currently fails (`-D warnings`) on its own lints
  (doc list indentation, derivable `Default` impls), which is why platform is linted with
  `cargo clippy -p stinkshot-platform --all-targets --no-deps -- -D warnings`.

## Known limitations

- WGC unverified on this machine (see above); fallback is opaque apart from the synthesized
  corner alpha, and cannot reproduce window drop shadows.
- Rotated monitors take the (slower, about 60 to 100 ms per 4K output) GDI path.
- Protected (DRM) content is black in every capture path, as usual.
- `scroll_at` blocks for about 40 ms plus 12 ms per call for the pointer to settle.
- `InputHook` key labels are plain per-key labels; the visualizer composes combos from the
  down/up stream (auto-repeat is already suppressed).
- UIA trees are cached for the snapper's lifetime; create a new snapper per overlay session.

## Manual verification checklist (real desktop)

- [ ] `capture::warm_up()` then `freeze_all` on a mixed-DPI, multi-monitor setup: size equals
      `virtual_bounds`, gaps are black, no seam at monitor borders.
- [ ] Rotate a monitor to portrait: capture is correctly oriented (GDI path).
- [ ] Play a full-screen video and an HDR video: freeze looks right and not washed out; with
      HDR on, the image is SDR tone-mapped, not blown out.
- [ ] Lock screen / UAC prompt during capture: falls back without panicking or hanging.
- [ ] `capture_window` on a covered window, a maximized window, a Win11 window with rounded
      corners (alpha corners), an elevated app, a minimized window (clean error), and a window
      with `WDA_EXCLUDEFROMCAPTURE`. Check the log line to see whether WGC or PrintWindow ran.
      On a machine where WGC works, confirm no yellow border and cursor on/off.
- [ ] `ElementSnapper` over Explorer, Settings, Notepad, Firefox: nested rects, cycling outward
      ends at the window frame.
- [ ] Clipboard: paste into Paint (opaque), Discord / browser (alpha PNG), Word (DIBV5),
      Explorer (file paste with `set_file_and_image`).
- [ ] OCR with a non-English profile language and with no OCR language installed (error text).
- [ ] `scroll_at` against a browser page: scrolls, cursor returns to its position.
- [ ] `InputHook`: click/key events while a game or elevated window has focus (low-level hooks
      do not see higher-integrity input unless StinkShot is elevated).
- [ ] PrintScreen takeover: toggle on, press PrintScreen (no Snipping Tool), toggle off and
      confirm the original value returned.
- [ ] Autostart toggle shows up in Task Manager > Startup; second launch forwards its args to the
      running instance via IPC.
- [ ] `window_util` on an egui window: Mica visible, dark title bar, topmost, excluded from a
      screen recording, tool-window hides it from Alt+Tab, `set_rect` across a mixed-DPI border
      ends at exactly the requested physical rect.
