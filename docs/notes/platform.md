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
| `hdr` | Advanced color state + SDR white level per display (DisplayConfig), FP16 scRGB to sRGB8 converter with highlight roll-off. See `docs/notes/hdr.md`. |
| `dpi` | `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)`; failure (already set by manifest) is logged at debug. |
| `monitors` | `EnumDisplayMonitors` + `GetMonitorInfoW` (EX) + `GetDpiForMonitor(EFFECTIVE)`. `list()` is sorted primary first, then left-to-right. |
| `capture` | DXGI Desktop Duplication per output with one cached session (see below), BGRA/RGBA8/R10G10B10A2/FP16 conversion, GDI `BitBlt(CAPTUREBLT)` fallback, WGC window capture, PrintWindow fallback, cursor drawing. |
| `windows_enum` | `EnumWindows` z-order, `DWMWA_CLOAKED` filter, `DWMWA_EXTENDED_FRAME_BOUNDS`, process name from `QueryFullProcessImageNameW`. |
| `uia` | `ElementSnapper`: dedicated MTA worker, lazily built per-window node tree with cached `BoundingRectangle`/`IsOffscreen` (CacheRequest through the ControlView walker), coalesced requests. |
| `clipboard` | CF_DIBV5 (BITMAPV5HEADER, BI_BITFIELDS, straight alpha, bottom-up) + registered "PNG" + CF_DIB (composited on white), CF_UNICODETEXT, CF_HDROP + "Preferred DropEffect". `OpenClipboard` retried for about 0.5 s. |
| `ocr` | `Windows.Media.Ocr` behind `OcrEngine` trait. 2x CatmullRom upscale when the longest side is < 1600 px, padding to 48 px minimum, vertical tiling (3000 px, 96 px overlap) with centre-based de-duplication, per-line rects from word boxes mapped back to input pixel space; `OcrLine::words` keeps each word's text and box (Smart Redact boxes words, not lines). |
| `input` | `cursor_pos`, `scroll_at` (SetCursorPos, N x SendInput wheel, restore), `InputHook` (WH_MOUSE_LL / WH_KEYBOARD_LL on a private message-loop thread, key-repeat suppressed), `key_label`. |
| `system` | Run-key autostart, PrintScreen takeover with remembered previous value, `reveal_in_explorer`, `open_path`, dark mode + accent, single-instance mutex, `WM_COPYDATA` IPC through message-only window `StinkShot.IPC`, `start_session_watcher` (unlock, display change and resume events for the capture re-warm-up), `play_capture_sound`. |
| `snip` | Replace Snipping Tool: `ChordFilter` (pure Win+Shift+S / Win+Shift+R state machine) and `SnipHook` (`WH_KEYBOARD_LL` on a private thread, Start-menu mask key, 30 s re-install). See `docs/notes/snipping-tool.md`. |
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
  trusted: the shadow texture is seeded from a GDI capture of the output, and real frames
  replace it. When that first frame only arrives after the seed (the warm-up's 300 ms wait for
  it timed out), it stands for every change since the duplication was made, so the shadow is
  seeded again (`needs_seed`); before Nov 7 2026 it was dropped silently and the warm-up
  picture stayed until the next change on screen. The shadow/staging textures are re-created
  whenever a frame's size or format differs from them.
- Rotated outputs and software-composed outputs use GDI instead. GDI is also the fallback for
  any duplication error (secure desktop, driver reset, no adapters).
- SDR outputs are duplicated as BGRA8. HDR outputs are duplicated as FP16 scRGB and converted
  with the display's SDR white level (`hdr` module); an 8-bit duplication of an HDR output is
  clipped, not tone-mapped, and looked blown out. See `docs/notes/hdr.md`.
- Conversion is split across up to 8 threads by row bands.
- Timing on this box (2 monitors, 6400x2160 virtual desktop): `freeze_all` about 40 to 55 ms
  with an optimized build once the session exists (Nov 7 2026, under 20-45 % background load:
  median 50-54 ms, was 68-70 ms before outputs converted straight into the result image); **the first capture takes 0.3 to 0.6 s**
  (device + duplication creation + GDI seed). Call the new `capture::warm_up()` from a worker
  thread at startup. A debug build without optimization is about 1 s per freeze because the
  conversion loops are unoptimized.
- `draw_cursor` recovers straight alpha by rendering the cursor with `DrawIconEx` once over
  black and once over white, which handles both alpha and legacy mask cursors.
- **One duplication per output per process** (measured Nov 7 2026): a second
  `DuplicateOutput(1)` of an output this process already duplicates fails with
  `E_INVALIDARG` (another process duplicating the same output is fine). The warm session
  holds every output, so the recorder takes `capture::lend_duplication()` before
  duplicating: the cached session is dropped, `freeze_all` / `capture_rect` use GDI for
  every output while any `DuplicationLease` lives, `warm_up` does nothing, and dropping the
  last lease discards the GDI-only session and warms a duplication session up again on a
  worker thread (only if one was warm before).
- **DPI awareness**: `DuplicateOutput1` returns `DXGI_ERROR_UNSUPPORTED` to a DPI-unaware
  caller on a display scaled above 100% (the 4K HDR monitor at 125%); at 100% it works, which
  is why the recorder tests passed earlier. Duplication runs inside
  `dpi::ThreadPerMonitorAware` (a guard that sets Per-Monitor-V2 on the current thread and
  restores it), so test binaries and tools without a manifest work too. The guard is held for
  the whole of `capture_region` and `warm_up` (session creation, frame acquisition and the
  GDI fallback), not just around `DuplicateOutput1`: a duplication made from an aware thread
  delivers no frames to an unaware one, and GDI coordinates are rescaled for unaware callers.
- **Silent duplications**: while the displays are idle-off, DWM feeds no frames to any
  duplication (0 frames in 2 s on both outputs with a window animating), but GDI still sees
  windows repaint. A cached session then returns its last picture forever. `capture_rect`
  (scrolling, "live") therefore uses GDI for the region unless the duplication delivered a
  frame within the last 20 ms (`LIVE_MAX_AGE`, i.e. during this very call). `freeze_all` and
  `freeze_rect` use GDI while the displays are off (see "Stale frames" below).
  `examples/display_state` in the record crate prints the console display state (0 off, 1 on).

### Stale frames and save speed (Nov 7 2026)

Benchmark report: `stinkshot full` through a running instance returned the screen as it was
when the instance started (a window open for 15 s missing, an old toast still there); the
headless capture in the same trial was correct.

**Reproduced deterministically** with `cargo run -p stinkshot-platform --example stale_probe
--release -- cycle 5`: drop the session, warm it up, open a solid marker window on each
monitor (this process's own, no input), wait, capture. Old code: `freeze_all` missed the
markers in 10 of 10 checks (it returned the warm-up picture every time), `capture_rect` was
right (its GDI live fallback); `latefirst 8` gave the same 16 of 16. The bench harness (`fullcap`) reproduced it too: every instance
capture had the same byte size and failed the swatch check (max error 194).

**Root cause 1, displays idle-off (reproduced).** The console display state was 0 (off) during
all of these runs (the benchmark ran unattended, so the displays had gone to sleep). DWM feeds
duplications nothing then, and a cached session treats "no frame" as "unchanged", so every
freeze returned the warm-up picture. The later full benchmark run that did not reproduce it
most likely ran with the displays on.

**Root cause 2, late first frame (found in review, not reproducible here).** The first frame of
each duplication is skipped as untrusted. If the warm-up's wait for it timed out (busy
start-up), the first frame acquired later was the one carrying every change since, and it was
dropped; with a static screen afterwards the warm-up seed stayed forever.

Fixes (`capture.rs`, new `display_power.rs`):

- `display_power`: `PowerSettingRegisterNotification(GUID_CONSOLE_DISPLAY_STATE)` with a
  callback (no window); Windows reports the current state on registration. `is_off()`, an
  `epoch` bumped on every change, and a wake hook. Registered lazily on the first capture or
  warm-up; unknown state is treated as on (old behavior).
- `OutputDup::prime` (used by captures and `warm_up`): displays off -> GDI serves the capture
  and the output is marked `needs_seed`; otherwise pick up a pending frame and re-seed from
  GDI when `must_seed` (no picture, late first frame, or the display epoch changed since the
  last seed or real frame). A real frame clears it (it is the whole desktop as composed now).
- `Session::capture` with the displays off: one GDI copy of the whole rect (median 89 ms for
  6400x2160 in `stale_probe timing`, against 143 ms per output).
- When the displays come back on, a `display-wake` thread re-seeds the warm session after
  500 ms (`revalidate_after_wake`), so the first PrtSc after waking the screen does not pay
  the seed. Not verified: the displays stayed off for this whole session, and the off -> on
  transition logic is covered by a unit test only.
- Regression tests (desktop, `#[ignore]`): `capture_is_fresh_after_warm_up` and
  `capture_is_fresh_when_the_first_frame_comes_late` (`warm_up_with(0)` skips the wait for the
  first frame) open marker windows after the warm-up and assert they appear in `freeze_all`,
  `freeze_rect` and `capture_rect`, and are gone after closing. Both pass; with the displays
  off they exercise the GDI path, the duplication path was not verified with the displays on
  (nobody at the PC during this work).
- Session rebuilds are now logged with their reason (`capture: building a duplication session
  (monitor layout changed | HDR state changed | no session)`, `access lost`) and creation time.

Cost: with the displays off a freeze takes the GDI path (about 90-150 ms for both monitors
instead of 40-60 ms from the stale shadow). Benchmarks that run unattended now measure that
path; the overlay opened in about 212 ms median there instead of 120 ms.

**Save speed.** `stinkshot full` via the instance took 1.35-1.45 s for one 4K monitor, headless
1.7-2.3 s. Almost all of it was PNG encoding at zlib level 6 with adaptive filters (1.1-1.9 s
for 3840x2160 depending on load). Changes:

- `Rgba::encode` PNG: `CompressionType::Fast` (fdeflate) + adaptive filters (core.md).
- `freeze_all` composites without the 55 MB black fill and per-output tiles (`grab_into` /
  `read_back_into` convert straight into the result; gaps between monitors are made opaque
  only when the outputs do not cover the rect).
- `freeze_rect(rect)`: the freeze rules for one rect. Full-screen capture with the editor off
  reads back only the monitor under the cursor (`capture_fullscreen_quick`), and `finish_quick`
  no longer clones the image for an unused `base`.
- Headless `full --out` uses `capture_rect_once` (GDI for that monitor, no duplication session:
  building one cost 0.2-1 s, a GDI copy of the 4K monitor about 60 ms).

Results (release, `C:\dev\stinkshot-bench\harness-capture` = the bench harness pointed at its own
target dir and the `capbench` profile, `BENCH_ONLY=stinkshot fullcap 10`, displays idle-off,
20-45 % background load from parallel builds):

| | before | after |
|---|---|---|
| `stinkshot full` via instance, 3840x2160 (median, min-max) | 1446 ms (1416-1928) | 182 ms (165-198) |
| headless `full --out --monitor 1` | 2332 ms (2319-2815) | 166 ms (153-181) |
| `repeat` with last region = whole 6400x2160 desktop, via instance | not measured | 290 ms (277-308) |
| PNG size, 3840x2160 desktop | 7.17 MB | 8.40 MB |
| PNG size, 6400x2160 | 7.36 MB | 8.73 MB (bench sample) / 9.55 MB (this desktop) |
| instance capture shows the current screen | no (swatch error 194) | yes (exact) |

The `repeat` figure includes PowerShell's `Start-Process` overhead (scratch script, not the
harness).

**Overlay slow mode (from the capture side).** `freeze_all` inside the instance over the 30-run
overlay test (before the fix): 38-90 ms, no session rebuild, while the overlay itself took
340-518 ms in 4 of 30 runs. The slow mode is therefore not in the capture: the instance logs
show IPC received to overlay window created in about 170 ms on every run, so the extra time is
in the forwarding process before `main` (process creation, image load) or after the window
exists (show/resize). `stale_probe timing 50` on a warm session (duplication path forced with
`PROBE_ASSUME_ON=1`): 32-110 ms, median 50-54 ms. The GPU part of a 4K FP16 read-back is
itself bimodal (copy + map 2.7 ms or 20.6 ms, apparently GPU clock state), worth at most about
18 ms. One instance crashed during the overlay test in `egui-wgpu` 0.37 `renderer.rs:1033`
("Failed to create staging buffer for index data"): a lost wgpu device, see app-shell.md
("GPU device loss, memory and cold start").

### Review fixes and a smaller warm session (Nov 7 2026, late)

**Capture source decision is a pure function now.** `prime_output(state, displays, source,
timeout)` holds every rule that decides whether an output's shadow can serve a capture: the
first-frame placeholder, mouse-only updates, real frames, display off, and the re-seed when the
shadow may predate the screen (`ShadowState::must_seed`: no picture, `needs_seed`, or the display
epoch moved). It talks to the world through two traits: `Displays` (`is_off`, `epoch`;
`SystemDisplays` reads `display_power`) and `FrameSource` (`acquire(timeout, placeholder)` ->
`Frame::{None, Placeholder, PointerOnly, Copied}`, and `seed`; `OutputDup` implements it).
`session_source(displays, any_dup)` is the `Session::capture` branch (displays off -> one GDI
copy of the whole rect). Unit tests with a fake display state and a scripted frame source
(`capture::tests`) cover both stale-frame causes: displays off (GDI while off, one re-seed after
the wake, also when they went off and on between two captures) and the late first frame
(placeholder after the warm-up's seed -> second seed, not copied), plus a static screen, real
frames, a failed seed and a released shadow.

**A warm-up never blocks a capture.** Warm-ups (start-up, re-warm-up, display wake) hold the
session through `WarmGuard`, which raises `WARMING`. A capture first `try_lock`s the session;
when that fails and a warm-up holds it (`on_lock_busy`), GDI serves the capture at once instead
of waiting up to several hundred ms (session creation, a 300 ms wait for each first frame, GDI
seeds). A capture that finds another capture in the lock still waits (it is quick, and the
session stays warm). `revalidate_after_wake` no longer waits for first frames: it uses the
cached 2 ms timeout (the duplications exist already) and skips the re-seed when a capture or
warm-up is running right then (that one validates the session itself).

**Display epoch** only moves on transitions into or out of Off. Dimming no longer invalidates
shadows (a dimmed display still gets frames); off -> dimmed and dimmed -> off still count.

**Gaps between monitors** are found with the union of the captured parts (`union_covers`, cuts
the rect at every part edge and checks each cell) instead of their summed area: overlapping
outputs (a duplicated display) could add up to the rect's area and leave a gap transparent.

**Memory of the warm session: 300 MB -> 133 MB, and 54 MB when idle.** Measured with
`examples/mem_probe` (private bytes after each step, `PROBE_LOG=trace` adds the session's own
`mem:` trace lines; displays on, 4K HDR + 1440p SDR):

| Step (fresh process) | Before | After |
|---|---|---|
| D3D11 device | 52 MB | 52 MB |
| 4K FP16 shadow / staging | +64 / +63 MB | +64 MB / none kept |
| HDR GDI seed (transient peak) | +158 MB | +40 MB |
| Warm session at rest | 286-300 MB | 133 MB |
| Warm session after 30 s idle (shadows released) | - | 54 MB |
| `freeze_all` right after the release | - | 38.5 ms (warm: 38-40 ms) |

- `IDXGIDevice3::Trim` after every warm-up and capture (`Session::trim`): the driver kept the
  GDI seed's upload buffers (about 80 MB) committed until trimmed.
- No persistent staging textures: `read_back_into` makes a staging texture of exactly the
  rect it reads and drops it (0.2-0.3 ms to create; the two full-output staging textures were
  81 MB for a few ms of use per capture). Shadows were already FP16 only for HDR outputs and
  BGRA8 for SDR ones.
- The HDR seed converts and uploads in 270-row bands (8 MB) instead of one 66 MB half-float
  buffer plus the runtime's 66 MB copy of it.
- `release_idle_shadows(max_idle)` drops the shadow textures (78 MB here) of a session unused
  for `max_idle`; the app calls it from housekeeping with 30 s. The device and duplications
  stay, so the next capture still takes the duplication path: its frame is the whole desktop
  as it is now and refills the shadow (`freeze_all` 37.6 ms on the first capture after a
  release, same as warm). Only when nothing changed on an output since the last frame is it
  seeded from GDI, like the first capture of a session: slower, never stale. Releasing the
  whole session instead was rejected: the D3D11 driver keeps most of it (release gave back
  35-65 MB) and the next capture would pay the 0.3-0.5 s rebuild or the GDI path (overlay above
  the 180 ms target).
- Desktop test `capture_is_fresh_after_idle_shadow_release` (`#[ignore]`) runs the marker
  checks with the shadows dropped before each check. It and the two older freshness tests
  pass with the displays on (Nov 7 2026), i.e. on the duplication path this time.

What is left: the D3D11 device itself (about 52 MB, driver) and, while warm, the shadows. An
8-bit HDR shadow would need the FP16 -> sRGB conversion on the GPU (a compute shader matching
`hdr::ScrgbToSrgb` exactly); not done.

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
  failure or timeout. The call runs through `hang::HangGate`: while a timed-out helper thread
  is still blocked, and for 5 minutes after a hang, later calls skip WGC immediately
  (`Windows.Graphics.Capture skipped (...)` in the log). Before the soak test (Nov 8) only the
  5-minute cooldown existed, so every capture after it started one more thread that never
  returned (see "Soak test" below). The fallback captures
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
- `visible_windows()` is the list of capture *targets*. For hit testing it is not enough: it
  drops untitled windows (popup menus `#32768`, Electron/tool surfaces, flyouts) and every
  window of this process, so anything behind such a window "showed through" in the overlay.
  `windows_enum::layers()` is the full Z-ordered occlusion stack: every visible, non-cloaked,
  non-minimized top-level window including untitled ones and our own capturable windows
  (History, Settings; `Layer::own`, titles never read). Skipped: Progman/WorkerW,
  click-through and GPU-overlay styles (same rule as above, `is_overlay_style`), layered
  windows set to alpha 0 (`LWA_ALPHA`), and own windows with a display affinity (overlay,
  toasts, HUDs are not in a frozen image). `cargo test -p stinkshot-platform dump_layers --
  --ignored --nocapture` prints the live stack for diagnosis.
- `occlusion` (pure, unit tested): `top_layer_at`, `occluders_above`, `visible_rect_around`
  (biggest rect around the point that no window above overlaps; greedy, largest overlap
  first), `uncovered_area`, `largest_uncovered_rect` (exact, histogram over the compressed
  grid), `window_levels` (stable clipped window rect when the point is in it, then the whole
  frame), and `clip_chain` (clips inner UIA levels to the band around the point, drops those
  with under 30 % uncovered area, ends with `window_levels`, dedups after clipping).

### UI Automation
- Electron/Chromium apps (Claude, Discord, VS Code) expose no UIA children until an assistive
  technology flag is set, so snapping falls back to the window rect for them. Native and
  Firefox windows produce inner rects (verified: 3 nested rects for Firefox). Nothing to fix in
  this crate; the overlay can still snap to the window.
- Element rects are clipped to their (clipped) parent, not only to the window
  (`clip_to_ancestors`): items of a scrolled list or a clipped panel report rects that run past
  their viewport, where something else is drawn.

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

- `capture::warm_up()`: create the duplication session ahead of time. Since Nov 6 2026 it
  primes every output (first frame past, shadow seeded) without a read-back, and the app calls
  it from a `capture-warmup` thread right after taking the single-instance mutex (see
  app-shell.md, "First overlay after launch was slow").
- `capture::release_cached_capture()`: drop the cached session.
- `capture::freeze_rect(rect, cursor)` (freeze rules for one rect), `capture::capture_rect_once`
  (GDI only, no session; one-shot processes), `capture::warm_up_with(ms)` (`#[doc(hidden)]`,
  tests and probes). `display_power` module (`state`, `is_off`, `#[doc(hidden)]
  override_state` for probes). Feature `Win32_System_Power` added.
- `hdr` module (Nov 6 2026): `displays()`, `display_for_device()`, `display_for_monitor()`,
  `ScrgbToSrgb`, half/sRGB helpers. Feature `Win32_Devices_Display` added.
- `clipboard::get_text()`, `get_image()`, `has_dibv5()`: read-back helpers (used by tests).
- `input::key_label(vk, scan, extended)`: public so the visualizer can label arbitrary keys.
- `Cargo.toml`: added `image` dependency and extra `windows` features.
- Private `util` module (COM init, wide strings, registry helpers).
- `uia::SnapResult.labels: Vec<ElementLabel>` (same order as `rects`; UIA control type and
  name, cached with the rect), `uia::ElementLabel`, `uia::control_type_name(id)`.
- `occlusion` module (`Layer`, hit-test and clipping helpers) and `windows_enum::layers()`
  (see "Window list").
- `window_util::get_rect(hwnd)` (GetWindowRect, physical px), `input::try_cursor_pos()`
  (None instead of 0,0 on failure) and `input::primary_button_down()` (async state, honours
  swapped buttons). Used by the pin's drag loop.
- Perf track (Nov 8 2026, docs/notes/app-shell.md "Perf track"):
  - `files` module: `replace_if_unchanged(original, replacement, len, modified) -> Replace`
    (`Replaced` / `InUse` / `Changed`) and `background_thread()`. The original is held open
    without read or write sharing while its size and time are checked, then the replacement
    is renamed over it with `FileRenameInfoEx` (replace + POSIX semantics, NTFS on Windows 10
    1809+): atomic, no other program can open it in between, a reader with delete sharing
    keeps the old data. Creation and modification times are carried over. Other volumes
    report `InUse` (nothing replaced). Unit test with a real temp folder.
  - `quick_overlay` module: `show(frozen, bounds, window_rects, accent) -> QuickOverlay`,
    `QuickOverlay::input() -> QuickInput { press, cursor, down, cancelled }`, closed on drop.
    A plain Win32/GDI topmost window (two DIB sections: dimmed and clear copies of the
    frozen screen, filled on up to 4 threads) that shows the start-up freeze before the GPU
    is up. Excluded from capture like the overlay.
  - `system::trim_working_set()` (`SetProcessWorkingSetSizeEx(-1, -1)`).

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
      HDR on, SDR content matches the screen and HDR highlights roll off toward white.
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
      do not see higher-integrity input unless Stinkshot is elevated).
- [ ] PrintScreen takeover: toggle on, press PrintScreen (no Snipping Tool), toggle off and
      confirm the original value returned.
- [ ] Autostart toggle shows up in Task Manager > Startup; second launch forwards its args to the
      running instance via IPC.
- [ ] `window_util` on an egui window: Mica visible, dark title bar, topmost, excluded from a
      screen recording, tool-window hides it from Alt+Tab, `set_rect` across a mixed-DPI border
      ends at exactly the requested physical rect.

## Text selection (`text_selection.rs`, #14)

`selected_text(fallback_hwnd, paragraph)` reads where the focused app's text selection is:
the focused element's TextPattern (then up to 6 ancestors, then the text elements of
`fallback_hwnd`), `GetSelection`, and each range's `GetBoundingRectangles`. Paragraph mode
expands clones of the ranges with `TextUnit_Paragraph` (a bare caret counts as its paragraph),
so the selection, focus and clipboard never change and no input is sent. Call it off the UI
thread: it sets a 2 s UIA transaction and connection timeout. `capture_rect` unions the line
rects, pads them, keeps them inside the text element and clips to the monitors (unit tested).
Additive: new public module only. Core gained `Hotkeys::selected_text` and
`selected_paragraph` (empty by default).

- [ ] Notepad, WordPad / Word, Edge / Firefox page text, VS Code: Capture Selected Text and
      Capture Selected Paragraph frame the right area; nothing selected shows the toast.
- [ ] Tray menu path (focus moved to the tray): the fallback window search finds the selection.
- `cargo test -p stinkshot-platform -- --ignored reads_selection_of_own_richedit` checks
  an Edit control the test creates (shown without activation).
