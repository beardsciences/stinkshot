# app-shell notes

Files: `crates/app/src/{main,shell,cli,tray,hotkeys,update,theme,state}.rs`, new
`capture_ops, filedialog, icon_gen, logger, sysutil, wake, workers`, `build.rs`,
`assets/stinkshot.manifest`.

## What was built

- **shell**: eframe 0.37 (wgpu) app. `ui()` shows every surface with
  `show_viewport_immediate` keyed by `ViewportId::from_hash_of(title)`; HWND found by title
  once, then `on_created` (plus `force_foreground` for overlay/history/settings/trim). Outbox
  drained after each frame. Full command routing per the shell.rs doc: capture
  region/window/fullscreen/scrolling/ocr/repeat-last, CaptureDone (clipboard, native Save As,
  pin, OCR copy, history + editable sidecar + OCR queue + toast, `last_region`), scrolling
  loop with ScrollHud then Annotate, recording flow (Countdown -> Recorder -> Frame+HUD ->
  stop -> TrimWindow -> TrimDone export -> history + toast), EditItem/PinItem/Copy/Reveal/
  Open/Delete/Star, stacked toasts, config save + apply (theme, autostart, hotkeys, history
  folder, tray labels, update check).
- **workers**: capture save, window capture, OCR, scroll capture, recorder start/stop,
  export, item ops, plus a maintenance thread (rescan + retention at start and hourly, OCR
  indexing of `pending_ocr`, abandoned temp recordings removed after 7 days).
- **tray**: tray-icon with generated icon (DPI-sized 16/20/24/32), native menu with hotkey
  labels (tab-aligned, names from `actions.rs`), left click = region started immediately
  (`Command::TrayClick`), double click = History (`Command::TrayDoubleClick` drops the
  untouched overlay the first click opened; a second click that lands on that overlay at the
  icon's spot within 500 ms does the same from inside the overlay).
- **hotkeys**: global-hotkey, bridge thread wakes the UI, conflict and parse errors reported,
  PrintScreen takeover handled in `reload`/`shutdown`. Replace Snipping Tool installs the`
  platform `SnipHook` and routes Win+Shift+S / Win+Shift+R (`snip_routes`,`
  docs/notes/snipping-tool.md).
- **cli**: clap; headless `full|window --out`, `ocr --file`; other subcommands forward over
  IPC. Release builds attach to the parent console so stdout/--help work.
- **update**: ureq GET (10 s timeout), `parse_release`, `is_newer`. Only network code.
- **theme**: install (fonts, Fluent-ish visuals/styles), all widgets, vector icons for every
  `Icon` variant (SVG path interpreter incl. arcs/cubics, shapes from the design SVGs).
- **build.rs**: generates the .ico (BMP frames), embeds manifest (PerMonitorV2, Common
  Controls 6, asInvoker, long paths, UTF-8) + version info via embed-resource (needs the
  Windows SDK `rc.exe`).
- Logger at `%APPDATA%\Stinkshot\stinkshot.log`, rotated at 5 MB, panic hook.

## Design decisions / deviations

- **Root viewport is not literally hidden.** It is a 1x1 transparent click-through
  taskbar-less tool window at (-32000,-32000). A truly hidden root makes eframe throttle all
  wakeups to 100 ms (hotkey latency) and needs `RequestPaintWhileHidden` tricks. Verified with
  a throwaway probe that this keeps the loop alive, child immediate viewports render, and
  exit is clean. `set_tool_window` + `exclude_from_capture` are applied to it.
- Surfaces closing: shell requests one extra repaint so egui destroys their windows.
- OCR engine is constructed on a scratch thread (avoid COM apartment clash with winit).
- Copy action always copies; Save As/Pin also copy when `general.copy_to_clipboard`.
- Save sidecar only when the doc has shapes; Save As writes an extra copy, history copy stays.
- Closing the Trim window (X or Esc) asks "Save This Recording?": Save Untrimmed exports the
  whole recording in the format it was started for (MP4 or GIF) into History with the usual
  toast, Discard goes through the Discard confirm, Keep Editing returns. If the window ever
  goes away without a choice, the shell still saves it untrimmed in that format (safety net).
  Discard (TrimDone with neither format, or RecordCancel while trimming) deletes the raw file.
- Window and full-screen captures open the editor only with
  `capture.open_editor_after_capture`; otherwise they finish at once as
  `FinishAction::Quick` (clipboard per `general.copy_to_clipboard`, history, toast with Edit).
- Repeat Last Region with nothing stored (or off screen) opens the region overlay with an
  inline hint-bar notice (`OverlayRequest::notice`) instead of a toast over the frozen screen.
- User-facing failure text goes through `errors.rs` (plain sentence + advice); raw chains
  are logged only.
- Raw recordings live in `<history>\.stinkshot\tmp`.
- Second plain launch (no args, no `--tray`) opens History in the running instance.
- Update found: toast + an "Update available" tray item that opens the release page.
- `freeze_screen=false` is ignored (the overlay always needs a frozen frame).

## Contract changes (additive, `state.rs`)

- `Surface::after_paint`, `Surface::wants_frame_now` (Nov 7 2026, see "Idle memory, start-up
  and overlay open").
- `Command::OpenUrl(String)` (http/https only).
- `Command::SetHotkeysSuspended(bool)`: settings window should send `true` while capturing a
  combo and `false` after.
- `Command::TrayClick`, `Command::TrayDoubleClick` (tray icon clicks, see tray above).
- `FinishAction::Quick` (finished without the editor).
- `OverlayRequest::notice` (hint-bar notice) and `OverlayRequest::from_tray_click`.
- `OverlayRequest::layers`: full Z-ordered occlusion stack (`windows_enum::layers()`), filled
  by `open_overlay` for Capture and SelectRegion modes (empty for Annotate).

## Contract requests

- settings_ui: use `SetHotkeysSuspended` (nothing suspends hotkeys otherwise).
- record: `Recorder` must be `Send` (moved to workers for `stop`/`cancel`/start result).
- toast: no link action exists; update availability is surfaced via the tray menu instead.

## Known limitations

- Nothing was run end to end: platform/record/core/history were stubs in this worktree. Built,
  unit tested (26 tests) and clippy clean for this crate (`--no-deps`; remaining dead-code
  errors are the toast stub fields). Theme/widgets/icons and the root-window mechanism were
  verified visually with a temporary probe (removed).
- Rounded window corners are applied on any OS (Win10 corners are square).
- Scroll step size adapts from `added_rows`; very smooth-scrolling pages may need a larger
  `scroll_delay_ms`.
- Hotkeys that were taken over from Snipping Tool are only restored if this process changed
  them (a crash while taken over leaves it taken over).

## Manual verification checklist

1. Start `stinkshot.exe`: tray icon appears, no taskbar button, no window; log file created.
2. Tray menu matches Tray design, labels follow hotkeys; left click = region at once; double
   click = History with no capture left open.
3. PrintScreen opens the overlay (Snipping Tool does not); each default hotkey works; change one
   in Settings and confirm re-register; conflict shows an error.
4. Region/Window/Full/Repeat: clipboard has image, toast appears, file in history folder, re-edit works.
5. Save As dialog opens, remembers folder, format follows extension.
6. Pin and Copy text (OCR) actions; OCR region copies text.
7. Scrolling capture on a browser page: HUD updates, Stop/Cancel work, result opens in editor.
8. Record MP4: countdown, frame, HUD toggles, pause/resume, stop, trim, Save MP4/GIF; closing
   the trim window asks (Save Untrimmed / Discard / Keep Editing); Discard deletes.
9. History: OCR indexing progresses, retention removes old items, star keeps them.
10. `stinkshot full -o a.png`, `window -o b.png`, `ocr --file x.png`, `region` forwarded to the
    running instance; second launch with no args opens History; `--version` prints.
11. Theme: switch dark/light/system and accent; autostart toggle writes HKCU Run entry.
12. Update check (opt-in): only then any network traffic; newer tag adds tray item.
13. Quit from tray removes the icon and restores the PrintScreen setting.

## Name spelling: Stinkshot (Nov 6 2026)

User request: the product is "Stinkshot", not "StinkShot", everywhere it can be. Every
user-visible string now reads Stinkshot: History / Settings / Trim window titles, tray tooltip
and header, toasts, first-run text, About, error bodies and the fatal start-up box, the Save As
stem fallback, the default filename pattern (`Stinkshot_{yyyy}-...`), the naming fallback stem,
the update check's User-Agent, VERSIONINFO ProductName / FileDescription (what Task Manager and
Settings > Apps > Startup show), README and docs.

New installs get `%APPDATA%\Stinkshot` and `Pictures\Stinkshot`. Windows paths are
case-insensitive, so an existing `StinkShot` folder is the same folder. Checked: no code compares
these paths with Rust `Path` equality or prefixes; the history index's `path` column is
`COLLATE NOCASE` and `rescan` keys on lowercase paths, so old rows (spelled `StinkShot`) and new
ones mix without duplicates. A config that still holds the untouched old default pattern
(`StinkShot_{yyyy}-{MM}-{dd}_{HH}-{mm}-{ss}`) is respelled on load; a pattern the user edited
is kept as written.

Kept as `StinkShot` on purpose (internal identifiers, renaming would break existing installs or
running instances, and none of them is shown to users):

- Window titles used only to find HWNDs: `StinkShot.Root`, `StinkShot.Overlay`,
  `StinkShot.Pin.<id>`, `StinkShot.Toast.<id>`, `StinkShot.Countdown`, `StinkShot.RecordFrame`,
  `StinkShot.RecordHud`, `StinkShot.ScrollHud`. All are tool windows (`set_tool_window` /
  `apply_hud_style`, `with_taskbar(false)`): not in Alt+Tab or the taskbar.
- `StinkShot.IPC` / `StinkShot.IPC.Window` (message-only window), mutex `Local\StinkShot.Instance`:
  an old and a new build must still find each other.
- Registry: Run value `StinkShot` (renaming would orphan existing autostart entries; Startup
  lists show the exe's FileDescription, not the value name), `HKCU\Software\StinkShot`.
- `StinkShot.App` manifest assembly identity, the `StinkShot.Semibold` egui font family name.
- Crate names, `stinkshot.exe`, `STINKSHOT_PROFILE`.

## First overlay after launch was slow (Nov 6 2026)

Report: right after start-up the first `stinkshot region` sometimes showed no overlay within
3 s while later ones did; once the client gave up with "another instance owns the mutex but
did not answer IPC".

`STINKSHOT_LOG=debug` (or `trace`) raises a release build to that level (Nov 7 2026).

Measured with `timing::point` trace lines (debug level, `grep timing: stinkshot.log`, time
since process start): mutex, IPC listener ready, eframe up, first frame, IPC received, IPC
command taken by the shell, dispatch, freeze (window list and `freeze_all` separately),
overlay constructed, overlay first frame, textures uploaded, overlay window created. The
client side logs which forwarding attempt succeeded. Reproduced in the `b4` dev profile with
a script that launches `--tray`, sends `region` 0.3 s later, waits for our own
`StinkShot.Overlay` window (enumerated by our pid) and closes it with `WM_CLOSE` (a window
message, no synthetic input).

Root causes (in order of cost):

1. **Cold DXGI duplication session.** The first `freeze_all` created the D3D11 device and the
   duplications and GDI-seeded every output's placeholder first frame: 787 ms vs 35-44 ms warm
   (optimized), 2164 ms vs 850 ms in the unoptimized dev build. `capture::warm_up()` existed
   for this (platform notes) but nothing called it, and it only probed a 16x16 rect, so the
   second monitor stayed cold.
2. **IPC listener started late.** The mutex was taken at process start but the listener only
   in `Shell::new`, after fonts, history, OCR and wgpu (about 0.75-1 s in a dev build, more on
   a cold boot). The client retried for 705 ms in the baseline run; past its 3 s budget the
   command was lost ("did not answer IPC").
3. **Root window restyle on the first frame.** `setup_root_window` (tool-window style, which
   hides and re-shows the root) ran first thing in `logic` and took about 150 ms on the first
   frame, delaying a command queued at launch by that much.
4. Dev-build only: platform and core were unoptimized, so every freeze converted the 6400x2160
   desktop (FP16 tone mapping, swizzles, `Rgba::filled`) at opt-level 0: about 0.85 s per
   freeze, which hid the real latency.

Not a cause: the first overlay viewport is not slower than later ones (first frame 82-106 ms
after `Overlay::new` on the first and on later overlays), so no wgpu pre-warm was added.

Fixes:

- `shell::run` starts the IPC listener and bridge (`start_ipc`) right after the mutex, before
  config/fonts/history/OCR/eframe. Commands queue in the channel (the bridge's `wake()` is a
  no-op until the shell registers its context) and the shell drains them on its first frame.
- A `capture-warmup` thread calls `capture::warm_up()` right after the mutex, beside eframe
  start-up. `warm_up` now primes every output (session, first frame past, shadow seeded)
  without the read-back and conversion of a real capture. A capture that arrives during the
  warm-up waits on the session lock, so it is never slower than before. GPU memory for the
  duplication textures is now held from start-up instead of from the first capture.
- Re-warm-up: a lock or unlock, UAC prompt, display change or sleep invalidates the session
  (DXGI access lost), and the next PrtSc would rebuild it cold. `system::start_session_watcher`
  runs a never-shown top-level tool window (`StinkShot.Session.Window<profile>`; a
  message-only window does not get the broadcast display and power messages) that reports
  `WTS_SESSION_UNLOCK` / `WTS_CONSOLE_CONNECT` (via `WTSRegisterSessionNotification`),
  `WM_DISPLAYCHANGE` and `PBT_APMRESUMEAUTOMATIC`. The `capture-rewarm` thread coalesces a
  burst, waits 1.5 s for the desktop to settle and calls `warm_up` again. `warm_up` rebuilds
  a lost session once instead of leaving it for the next capture.
- `setup_root_window` runs at the end of `ui()`, after the surfaces are shown.
- `Cargo.toml`: `stinkshot-platform` and `stinkshot-core` build at opt-level 2 in the dev
  profile (like `stinkshot-record`), so dev builds show release-like capture latency.

Timings (dev build, `b4` profile, 4K HDR + 1440p, region sent 0.3 s after launch unless noted;
"visible" = our overlay window exists and is visible, measured by the script from the moment
the client process is started):

| Run | First overlay | Later overlays | First `freeze_all` | Warm `freeze_all` |
|---|---|---|---|---|
| Before (unoptimized platform/core) | 3060 ms | 931-940 ms | 2164 ms | 843-855 ms |
| Before, warm-up off, optimized (region at 1.5 s) | 875 ms | 125 ms | 787 ms | 35 ms |
| After, region at 0.3 s (app still starting) | 799 ms (eframe up at +870 ms, dispatch +880 ms) | 123 ms | 172 ms (waited for the warm-up) | 44 ms |
| After, region at 1.5 s | 138 ms | - | 43 ms | - |

In the 0.3 s run the command now reaches the app at once (forwarding attempt 0, it was attempt
3-7) and is dispatched on the first frame (+0.1 ms after it, was +105-150 ms).

Remaining, not fixed: in a dev build the app crate's tile upload of the frozen image (per-pixel
`Color32` map, opt-level 0) takes about 600 ms per overlay before its first paint; release is
fast. Global hotkeys (PrtSc included) are registered in `Shell::new`, so a PrtSc pressed before
eframe is up still goes to Windows; after that the hotkey path is the same as IPC (bridge ->
`pending` -> `begin_overlay` -> `freeze_all`) and gets the warm session. The first PrtSc was
not pressed live (no synthetic input; hotkeys are disabled in the dev profile).

## Idle memory, start-up and overlay open (Nov 7 2026)

Follow-up to the benchmark (`docs/benchmark/`, raw data in `C:\dev\stinkshot-bench\results`):
620 MB idle private bytes, 843 ms launch to tray, an overlay that first appeared oversized and
then re-showed, and a slow mode in about 1 run in 5. Measured with a copy of the bench harness
(`C:\dev\stinkshot-bench\harness-app`: `SS_ROOT`, `SS_PROFILE`, `BENCH_ONLY`, `BENCH_OUT`,
`IDLE_SECS` select the build, profile, tools, output folder and idle wait), release builds in
`C:\dev\stinkshot-bench\target-app`, profile `appbench` (bench config, own history folder).
Same machine as the benchmark (4K HDR 125 % + 1440p 100 %, RTX 3060 Ti).

Diagnostics added: `timing::point` lines now carry the process's private bytes, the first one
says how long the kernel took from process creation to `main`, and `STINKSHOT_LOG=debug` (or `trace`,
case-insensitive; the same switch the capture track added)
turns the debug lines on in release builds (`grep timing: stinkshot.log`).

### Where the 620 MB went

Measured with timing points at each start-up step, and a scratch program for the capture
session alone:

| Piece | Private MB |
|---|---|
| Everything before the GPU (config, fonts, history, OCR engine, tray) | 10 |
| wgpu device, default setup (Vulkan picked on NVIDIA, implicit Vulkan layers from Overwolf, OBS, GOG loaded) | 332-360 |
| same, DX12 only | 337 |
| same, GL only | 66-74 |
| DX12 with `MemoryHints::MemoryUsage` | 139-149 |
| Capture warm session (D3D11 device, duplications, 4K FP16 shadow + staging, GDI seed) | about 250 |

The capture session numbers (scratch program, no wgpu): bare D3D11 device +52 MB; `warm_up`
+270 MB over process start; `release_cached_capture` gave back only 35 MB (the driver keeps
the rest); a second `warm_up` after the release added another 100 MB. So releasing the warm
session when idle would not save memory, it would only make the next capture slow. That part
belongs to `capture.rs` (capture track); the staging textures alone are 81 MB of CPU memory
(4K FP16 + 1440p BGRA).

### Changes

- `gpu.rs` (new): the wgpu instance, adapter and device are created on a `gpu-init` thread
  started right after the mutex, and handed to eframe as `WgpuSetup::Existing`. DX12 first
  (Vulkan, then GL as fallbacks; `WGPU_BACKEND` still overrides), `MemoryHints::MemoryUsage`,
  low-power adapter preference (`WGPU_POWER_PREF` overrides), FXC instead of "Auto" (Auto
  loaded the Vulkan SDK's `dxcompiler.dll` from `PATH`), and DirectComposition swapchains
  (`DxgiFromVisual`): the default HWND swapchain has no alpha, which would have turned the
  transparent pins, recording frame and countdown black (Vulkan had alpha; a pin was checked on
  screen with rounded transparent corners after the change).
- Tray icon and global hotkeys are created in `run_app` right after the config, before fonts,
  history and the GPU. Their hidden windows live on the main thread, so a click or a hotkey
  before the event loop runs waits in the thread's queue and is handled on the first frame,
  like IPC commands (`Boot` carries them into `Shell::new`). The OCR engine is created on its
  thread as before but joined after the GPU device, so the two overlap.
- Surfaces' HWNDs are found with `sysutil::find_own_window` (`FindWindowExW`, compares the
  stored caption) instead of `window_util::find_own_window`, which sends `WM_GETTEXT` with a
  100 ms timeout to every window of the process and can stall on helper windows of busy
  threads.
- `Surface::after_paint` and `Surface::wants_frame_now` (additive, `state.rs`): called after
  each painted frame / ask for another frame in the same shell pass. The overlay uses them to
  reveal its window (docs/notes/app-overlay.md, "Opening without a flash").

### Results (release, before = commit 3d3c876, after = this change)

| Metric | Before | After |
|---|---|---|
| Launch to tray icon (`tray_icon_app` window), 15 runs, median / max | 781 / 1184 ms | 41 / 58 ms |
| eframe up (`Shell::new`) | 912-1381 ms | 478-624 ms |
| Idle private bytes after 60 s | 620.6 MB | 400.2 MB |
| Idle working set | 258.5 MB | 200.1 MB |
| Idle handles | 920 | 714 |
| Private bytes after three region overlays | 654 MB | 483-491 MB |
| Region overlay settled at final size, 50 runs, median / p90 / max | 228 / 244 / 621 ms | 167 / 182 / 190 ms |
| Region overlay first visible (any size) | 120 ms (at 8000x2700) | 167 ms (at 6400x2160) |
| Rect changes after first visible | 1-2 per run | 0 |
| Peak private bytes during an overlay | 762 MB | 661 MB |
| Tool CPU per overlay open + close | 250 ms | 172 ms |

The overlay before/after pair was run back to back in the same session; an earlier baseline run
had the same median (220 ms settled) but 12 of 50 slow runs, see below.

The idle target of 150 MB is not reachable from the app side alone: with the capture warm-up
skipped the app idles at about 150 MB (DX12 device about 140 of it), and the warm session adds
about 250 MB. (Superseded: a smaller DX12 descriptor heap, smaller allocator blocks and a
smaller warm session brought idle private bytes to 134 MB with the displays on; see "GPU device
loss, memory and cold start".) Dropping it would bring back the 0.8-1.2 s cold first capture,
and releasing it later frees only 35 MB. A smaller warm session (no full-size staging textures
until a capture needs them, or no FP16 duplication kept between captures) is the capture
track's call.

### The slow overlay mode (about 1 run in 5 in the benchmark) is process creation

With the timing trace on, every one of 59 overlays spent 98-181 ms inside the instance (IPC
received to window shown). For the slow harness runs the extra 236-587 ms was outside it:
before the forwarding `stinkshot region` client reached `main`. The same tail shows up for an
unrelated exe: 40 launches of `whoami.exe` took 55-71 ms 33 times and 138-485 ms 7 times;
`stinkshot --version` 103-121 ms 29 times and 167-571 ms 11 times. It comes and goes with the
machine's state (the back-to-back baseline run above had 1 slow run of 50, the earlier one 12).
Nothing in the app can remove it for CLI triggers; hotkeys and the tray do not start a process.
The client itself reaches `main` 27-46 ms after process creation.

### Known limitations / not verified

- A machine without DX12 (fallback to Vulkan, then GL) was not tested, nor the low-power
  adapter on a hybrid-GPU laptop.
- Tray clicks and hotkeys during the first 0.5 s (before the event loop runs) were not tried
  live (no synthetic input; hotkeys are off in the bench profile).
- After the first overlay about 85-90 MB stays committed (483-491 vs 397 MB; before: 654 vs
  619). It is the same with the GL backend and does not happen for full-screen captures, so it
  is tied to the overlay's viewport rather than wgpu's allocator; polling the device after the
  window closed did not release it. Cause not found. (Found later the same day: gpu-allocator
  keeps the last block of each memory type, a 64 MB write-combined upload block with the
  `MemoryUsage` hint; see "GPU device loss, memory and cold start".)
- Countdown and recording frame transparency were not looked at on screen (the pin was).

## Benchmark re-run (Nov 7 2026, evening)

Full re-run of the benchmark (`docs/benchmark/`, raw data and harness in
`C:\dev\stinkshot-bench`) for main at b5409a9, with the old build (3d3c876, profile
`benchbefore`) and the three competitors in the same session. Harness change: "overlay ready"
replaces "visible" / "settled" as the headline. It is the start of the last unbroken stretch in
which the tool's overlay is shown (visible, uncloaked, covering a monitor) at the rect it still
has 600 ms after it first became visible, so an oversized first show or a hide and re-show no
longer counts (`follow_overlay` in the harness). `verify-baseline.ps1` checks the Run/Keyboard
keys and the Flameshot config afterwards; `make-results.py` builds `results.json`.

Overlay ready, median / p90, 30 runs: main 220 / 547 (4 slow runs of 30), old build 238 / 254
(1 of 30), ShareX 644 / 1028, Greenshot 382 / 400 (window message, no process start), Flameshot
357 / 387 (its monitor picker). Launch to tray 786 -> 32 ms; full-screen via the instance
1460 -> 168 ms, and the old build's instance capture was stale in 3 of 3 checks. Idle private
bytes read 164 MB, but that was with the displays off (the warm session got no frames); the same
b5409a9 build idled at 399 MB with the displays on (next section), so 164 MB was neither its
real idle figure nor a lower bound for it.

Not verified / open:

- The displays were off for the whole run, so main took the GDI freeze path and its warm
  session got no frames. Overlay ready and idle memory with the displays on were not measured
  in this run (399 MB idle private bytes with the displays on, measured in the next section;
  the third benchmark run, below, has displays-on numbers for f334f45).
- The slow overlay runs (0.5-0.95 s) are still there for CLI triggers; hotkey and tray
  triggers were not measured (no synthetic input).

## GPU device loss, memory and cold start (Nov 7 2026, late)

Follow-up to the benchmark: one instance had panicked in `egui-wgpu` 0.37 `renderer.rs:1033`
("Failed to create staging buffer for index data") during repeated overlays; idle private bytes
were about 400 MB with the displays on (ShareX 113, Greenshot 45); a cold launch took 0.7-1.0 s
to the first overlay. Harness: `C:\dev\stinkshot-bench\harness-mem` (copy of the bench harness;
`SS_ROOT`, `SS_PROFILE`, `BENCH_OUT`, `IDLE_SECS`, `SETTLE_SECS`, `KEEP`; new commands `cycles N`
(overlay open + `WM_CLOSE` N times in one instance, memory after each), `cold N DELAY` (launch ->
first overlay ready, the region command sent 30 ms after launch or DELAY ms after the tray is
up) and `regions PID` (committed private regions by allocation)). `results-mem\run-set.sh`
runs a set; raw results in `C:\dev\stinkshot-bench\results-mem\{before,after}`. Release builds
in `C:\dev\stinkshot-bench\stinkshot-target`, profile `bench`. **Displays on** for every
before/after number below (console display state 1, logged before each step); background CPU
outside the tool 13-36 % (the PC was in use).

### The crash: a lost wgpu device

`Queue::write_buffer_with` returns None without reporting an error when the device is lost
(wgpu 30: `DeviceError::Lost` maps to `ErrorType::DeviceLost`, which the error sink swallows
because it "will be surfaced via callback"), and egui-wgpu panics on None. wgpu-core loses the
device on any hal `OutOfMemory`, `Lost` (`DXGI_ERROR_DEVICE_REMOVED` / `RESET`) or `Unexpected`
(any other failed HRESULT in a D3D12 call: Present, fence signal, `SetMaximumFrameLatency`,
swapchain `GetBuffer`). An out-of-memory error would have panicked earlier with "wgpu error: Out
of Memory" through wgpu's default error handler, so the device was lost through a removed /
reset device or an unexpected HRESULT. Neither memory hint sets a memory budget
(`memory_budget_thresholds` stays default), so the hint did not make allocations fail.

Not reproduced naturally: 300 open/close cycles in the old build with the displays on ran
without a crash and without growth (private 650-670 MB median per 50 cycles, flat). The crash
happened in an unattended run with the displays off; the log of that instance was not kept, so
the failing call is unknown. wgpu-hal logs it (`... failed: <HRESULT>`, target `wgpu_hal`, error
level, which the file logger keeps), and the new handlers log the loss, so a next occurrence
names it.

Reproduced exactly with a test hook: `STINKSHOT_TEST_GPU_LOSS=3` destroys the device while the
third overlay paints -> the same panic at `renderer.rs:1033`.

Fix (`gpu.rs`, `shell.rs`): a lost device cannot be revived and eframe 0.37 cannot swap its
device, so the process restarts itself.

- `install_handlers`: `set_device_lost_callback` and `on_uncaptured_error` (wgpu's default
  handler panics on every uncaptured error: validation errors are now logged, out-of-memory
  marks the device lost).
- Loss between frames: `logic()` sees `gpu::lost()` before anything paints, stops a recording
  in progress (the recorder does not use wgpu, the file is kept), drops tray and hotkeys,
  starts `stinkshot --tray` with `STINKSHOT_RESTART_AFTER=<pid>` and exits.
- Loss inside a paint (the reported case): winit hands the panic back out of its event loop,
  `run_app` catches it around `eframe::run_native` (`catch_unwind`), recognizes it
  (`gpu::lost()` or the panic text, `is_device_panic`) and restarts the same way. Other panics
  are re-raised.
- The new instance waits (up to 10 s) for the old process to exit before taking the
  single-instance mutex (`wait_for_predecessor`).
- Verified: `STINKSHOT_TEST_GPU_LOSS=3` (in-paint) -> panic logged, "restarting after GPU device
  loss", new instance up in 0.2 s and answering `region`; `idle:6` (device destroyed between
  frames) -> restart. An overlay that was open when the device went is lost (it cannot paint).

### Memory: 399 -> 134 MB idle private bytes

Where it went, from timing points and `bench regions` (committed private regions):

| Piece | Before | After |
|---|---|---|
| App before the GPU (config, fonts, history, tray) | 12 MB | 12 MB |
| D3D11 capture device (driver) | 52 MB | 52 MB |
| Capture shadows (4K FP16 + 1440p BGRA) | 78 MB | 0 when idle (released after 30 s) |
| Capture staging textures (4K FP16 + 1440p BGRA) | 77 MB | none kept |
| Driver upload buffers left by the GDI seed | about 80 MB | trimmed |
| D3D12 CBV/SRV/UAV descriptor heap (1 M x 32 B) | 30.5 MB | 2 MB (64 K) |
| gpu-allocator blocks kept after the first overlay | 64 MB WC upload block | 8 MB |

Capture side: platform.md, "Review fixes and a smaller warm session". App side (`gpu.rs`):
`max_non_sampler_bindings` 65,536 (wgpu-hal sizes the shader-visible descriptor heap from that
limit; the default 1,000,000 was 30.5 MB committed), `MemoryHints::Manual` with 4-16 MB device
blocks (4-8 MB host): gpu-allocator keeps the last block of each memory type, and with
`MemoryUsage` that was the 64 MB upload block holding the overlay's texture upload, which is the
85-90 MB that stayed committed after the first overlay (with the descriptor heap's share
gone the rest is about 19 MB now: a bigger driver region and an 8 MB block). The shell calls
`capture::release_idle_shadows(30 s)` from housekeeping (and schedules its next wake-up from the
answer).

Releasing the whole warm session when idle and re-warming on a hint was measured and rejected:
the D3D11 driver keeps most of the memory after the session is dropped (35-65 MB came back), a
re-warm takes 0.3-0.5 s, and a hotkey or CLI trigger gives no lead time, so the first overlay
after idle would take the GDI path (about 250 ms, above the 180 ms target). Dropping only the
shadows keeps the duplication path: the first capture after the release took 37.6 ms in
`freeze_all`, the same as a warm one.

### Cold start: 807 -> 650 ms

Timeline of a cold launch with `region` sent 30 ms after it (`STINKSHOT_LOG=debug`): GPU adapter
at +310-375 ms (D3D12 driver load; wgpu also creates a WARP device while enumerating, which it
offers no way to skip), device +45-75 ms, eframe +110-145 ms (root window 35 ms, egui's two
shaders), then the overlay about 105 ms. Changes:

- `instance_flags`: without `VALIDATION_INDIRECT_CALL` (wgpu's release default; egui never draws
  indirect) the device no longer compiles two compute shaders (about 30 ms); with `DEBUG` on
  DX12 FXC skips optimizing egui's shaders (eframe start 132 -> 114 ms average over 6 launches).
- `prefreeze_while_gpu_starts`: the main thread only waits for the GPU there, so it takes
  forwarded commands off the IPC channel meanwhile and freezes the screen at once for one that
  starts with a freeze (region, scrolling, OCR, window). The freeze runs during the warm-up, so
  it takes the GDI path (about 100 ms), off the critical path; `Shell::freeze` uses it once
  (within 3 s). Hotkeys and tray clicks queued before the event loop are not seen there.
- A capture never waits for a warm-up any more (GDI instead, platform.md), so a capture that
  arrives while the session is being built is not held up by it.
- Measured and not done: starting the capture warm-up after the GPU device (GPU ready only
  25-40 ms sooner, first freeze then 200 ms).

### Results (release, displays on, before = b5409a9 build, after = this change)

| Metric | Before | After |
|---|---|---|
| Idle private bytes after 60 s, 2 runs | 399.3 / 398.8 MB | 134.2 / 134.3 MB |
| Idle working set | 200 MB | 124 MB |
| Overlay ready, 30 runs, median / p90 | 154 / 630 ms | 154 / 213 ms |
| Peak private bytes during an overlay (median) | 659 MB | 476 MB |
| Cold launch -> first overlay ready, region at +30 ms (41-51 ms measured), 8 runs, median (range) | 807 ms (721-968) | 650 ms (609-811) |
| Same, region 1 ms after the tray is up | 714 ms (679-853) | 636 ms (610-699) |
| 300 overlay cycles: crash / private median per 50 cycles (250 ms after each close) | none / 647-677 MB | none / 347-354 MB (drift about 6 MB over 300) |
| Private bytes 5 s after the 300 cycles (start) | 506 MB (399) | 305 MB (212, shadows still held at both points) |

The overlay p90 before includes 11 slow runs (0.5-1.2 s) during heavier background load; the
median is what compares. The slow mode outside the instance (process creation of the forwarding
client, "The slow overlay mode") is unchanged.

### Not verified / open

- The natural cause of the device loss (see above); the recovery was tested with the hook only.
- Hybrid-GPU laptops and machines without DX12 (the descriptor-heap limit and memory hint also
  apply to the Vulkan and GL fallbacks; there the limit only caps bind groups).
- Hotkey and tray triggers during start-up still freeze after eframe is up (the early freeze
  only sees IPC commands).
- An idle desktop where nothing at all changes between two captures takes the GDI seed after
  the shadows were released (slower, correct); how often that happens was not measured.

## Benchmark third run (Nov 7 2026, 22:32-23:00)

`docs/benchmark/` re-run for f334f45 against ShareX, Greenshot and Flameshot, all four
interleaved in one batch, **displays on** for every step (console display state 1 in all 324
five-second samples and before every step; `display-state.log` in the raw results). Harness
`C:\dev\stinkshot-bench\harness-mem` (`run-final.ps1`, `make-results.py`); the second run's
results moved to `C:\dev\stinkshot-bench\results-second-run`.

Stinkshot: overlay ready 134 / 289 ms (median / p90, 4 slow runs of 30), launch to tray 34 ms
median, cold launch to first overlay 593 ms (region about 40 ms after launch) / 568 ms (region right after the
tray), idle 120 MB working set / 134-135 MB private, peak private during an overlay 414 / 477
MB, headless full-screen of the 4K monitor to PNG 156 / 465 ms. The idle figure matches the
134.2 / 134.3 MB measured in the previous section. Competitors and caveats (CLI vs
`WM_HOTKEY` triggers, Flameshot's monitor picker, capture scope, PNG size) are in the
benchmark README.

## Review of f334f45 / 0794b65 (Nov 7 2026, 23:05-23:25)

Skeptical read of the GPU-loss fix, the capture changes and the benchmark docs, with spot
re-runs. **Displays on** for every check (console display state 1 before and after each).

Verdicts:

- **Crash fix: sound for the reported panic, verified with the test hook only.** The 300-cycle
  runs (before and after, displays on) show no crash and no growth, which rules out a leak but
  says nothing about the fix: the natural loss never happened in them. Re-ran both hook paths
  on this review's build: `STINKSHOT_TEST_GPU_LOSS=2` -> the same `renderer.rs:1033` panic,
  caught, new instance 72 ms later; `idle:6` -> loss logged, restart 17 ms later. Release
  profile keeps `panic = "unwind"`, which `catch_unwind` needs.
- **Gaps found and fixed in the restart** (`gpu.rs`, `shell.rs`):
  - Both restart paths called `process::exit` at once, cutting off saves, exports and a
    recording's stop worker still running (`quit` waits up to 30 s for those). They now wait up
    to 15 s for `workers::jobs_in_flight()` to reach zero first.
  - In the in-paint path the shell is dropped while unwinding, and dropping an active
    `Recorder` aborts it and deletes the file; only the between-frames path kept recordings.
    `Drop for Shell` now finishes an active recording when it runs during a panic. (The file
    is kept on disk; it is not added to history, in either path.)
  - No limit on restarts: a loss, or a device panic during eframe's start-up (before
    `install_handlers` replaces wgpu's panicking default handler), that happens on every launch
    would chain processes forever. A fourth restart in a row, each within 60 s of its start, is
    now refused and logged (`STINKSHOT_GPU_RESTARTS`, unit tested).
- **Capture decision tests: they cover the rules, not the wiring.** `prime_output` and the
  tests match the two stale-frame causes, dimming no longer moving the epoch, and the released
  shadow; the mapping from DXGI results to `Frame` (`OutputDup::acquire`) and `prime`'s
  "no texture, no picture" override are covered only by the desktop tests below.
- **No HDR regression.** Swatch window on the 4K HDR monitor (480-nit SDR white), `hdr_probe`
  (new step: two `freeze_all` after `release_idle_shadows`): 127,211,106 / 128,128,128 /
  255,255,255 exact on the first (GDI seed, banded upload) and warm `capture_rect`, all
  `freeze_all` and both captures after the shadow release (34-39 ms, same as warm). The three
  `capture_is_fresh_*` desktop tests pass (release build).
- **Memory and overlay claims hold.** Spot re-run with the bench harness (Stinkshot only,
  bench release build of f334f45): overlay ready 10 runs 139-192 ms plus one 593 ms, median
  152 ms (the benchmark's 134 ms batch was a little faster; the memory work measured 154 ms);
  idle after 60 s 133.7 / 134.6 MB private, 124 MB working set, 0.13% of a core.
- **Docs:** honest about display state and about the unreproduced natural loss. Corrected
  here: the "region at +30 ms" cold launch sent the command 37-51 ms after the launch
  timestamp (30 ms sleep after `CreateProcess` returned). The misplaced doc comment of
  `release_cached_capture` (it had ended up on `release_idle_shadows`) is fixed.

Still open: a real (not hook-made) device loss; whether the restarted instance should add a
recording kept during a restart to history; the restart cap was not exercised live.

## Perf track (Nov 8 2026): idle memory frontier, smaller PNGs, cold start

Goals from the benchmark's measured losses: idle private bytes 134 MB (Greenshot 45,
Flameshot 8), PNGs 1.3-1.9x larger per pixel, cold launch to first overlay about 0.6 s.

**Displays off for every measurement in this section** (console display state 0 in every
harness log line, 00:30-01:55; nothing was done to wake them). With the displays off the
duplication session gets no frames, so every capture took the GDI path, and the warm
session's shadows were never filled. Relative numbers (before / after in the same
conditions) hold; the displays-on figures of the benchmark were not re-measured.

Tools: release builds in `C:\dev\stinkshot-bench\target-perf`, copied to `perf-base`
(f334f45, the benchmark's build) and `perf-v*`; profile `perf` (bench config, history in
`captures\perf`). A copy of the bench harness with one new command, `idleoverlay N GAP`
(start, settle 5 s, then N times: idle GAP s, record the footprint, open the region overlay,
measure ready), and a scratch probe that creates D3D11 / D3D12 / wgpu devices one step at a
time and prints private bytes and committed regions.

### Idle memory: the GPU driver sets the floor

Probe, one fresh process per line, private bytes (displays do not matter here):

| Step | Private |
|---|---|
| Process start | 1.3 MB |
| D3D11 device + duplication of both outputs (the capture session) | 50-53 MB |
| Same, then the device dropped | 17 MB |
| Raw D3D12 device on the RTX 3060 Ti | 60 MB |
| + one direct command queue | 81 MB (+21) |
| + shader-visible descriptor heaps (64 K views, 2 K samplers) | 83 MB |
| Raw D3D12 device, then dropped | 24 MB |
| wgpu DX12 device with the app's settings (`gpu.rs`) | 92-94 MB |
| D3D11 session + wgpu device (what the app holds) | 106-108 MB |
| Both, then wgpu dropped | 102 MB |
| Both, then D3D11 dropped | 105.7 MB |
| Both, then both dropped | 68 MB |
| D3D11On12 device on the D3D12 device + duplication | `DuplicateOutput` fails (E_INVALIDARG); 114 MB |

About a third of each device is not in any private region: it is the commit charge of the
NVIDIA user-mode driver's writable image sections (relocations, data), and it stays until the
DLL unloads. The two APIs share that driver, so the second device costs only 14 MB, and
dropping one of them gives back 2-6 MB. Even with both devices dropped the process keeps
68 MB. The app before any GPU work is 5 MB after this change (fonts mapped).

So the 60 MB target is not reachable in a process that has used the GPU once, whatever is
released after idle. The frontier, from the measurements above and the existing notes:

| Option | Idle private (displays on, est.) | First overlay after idle |
|---|---|---|
| Both devices warm (default, this change) | about 123 MB (displays off: 135.5 -> 124.7 MB) | unchanged, 134 ms median (benchmark) |
| Release the capture session after idle (measured Nov 7) | 134 - 14 to -35 MB | GDI path, about +100 ms; re-warm 0.3-0.5 s |
| Release the wgpu device after idle | about 128 MB (gives back 6 MB) | eframe 0.37 cannot swap its device: needs a full eframe restart, about +250 ms |
| Release both | about 75-80 MB | about +350 ms |
| GPU work in a child process that exits after idle | 5-15 MB (tray process only) | cold engine start: frozen screen about 0.25 s (quick overlay), full overlay about 0.7 s |

Nothing below the default stays within +30 ms, so the default stays: both devices warm.
What changed:

- Fonts are memory-mapped (`core::fonts`): 7 MB less private (app before the GPU 12 -> 5 MB).
- The working set is handed back after 60 s with nothing open (`trim_when_idle`,
  `system::trim_working_set`): Task Manager's figure (private working set) drops to about
  8 MB; private bytes (commit) do not change. Cost on the next overlay: see "Results".
  Windows does this anyway under memory pressure or with the displays off (the user's own
  instance showed 147 MB private / 22 MB working set this morning).
- Not done: a smaller descriptor heap (2 MB; a history grid with thousands of thumbnails
  could run out, which loses the device), and the separate-process design (the only way under
  60 MB; a large change with the latency cost above).

### Smaller PNGs: background re-compression

`recompress.rs` (new) + `core::png_opt` (new; algorithm in docs/notes/core.md) +
`platform::files::replace_if_unchanged`:

- The save path is unchanged except that opaque images are written as RGB (no alpha
  channel): the file exists at once for clipboard, toast and history.
- Each PNG saved into history (new capture or re-edit) and each Save As target is queued.
  One thread in Windows background mode (lowest CPU, very low I/O and memory priority) waits
  2 s, re-reads the file, optimizes it (miniz level 10; zopfli, 1 iteration, for images up to
  1 MP) and writes `<name>.<pid>.stinkshot-tmp` beside it (flushed).
- The swap happens only if the file still has the size and modification time it had when
  saved and nobody else has it open (opened without read/write sharing during the check, then
  an atomic POSIX rename over it); creation and modification times are kept. Moved,
  re-edited or deleted: dropped. Open in another program: retried once after 20 s, then
  dropped. The new bytes must decode to the same pixels (`png_opt` checks). Stinkshot's own
  writes to capture files take `recompress::write_lock`, so a re-edit cannot land between
  the check and the swap.
- Quitting does not wait for it; a temp file left by an exit or crash mid-write is deleted
  when the next instance opens history. The history index's size is refreshed
  (`History::refresh_bytes`); OCR text and thumbnail stay.
- The clipboard is not involved (it gets pixels, not the file). Separately, the clipboard's
  own PNG copy now uses the fast encoder and every payload is built before the clipboard is
  opened: the default-level encode took about 1.8 s for a 4K image with the clipboard held
  open, so a paste right after a capture could miss the image.
- Not re-compressed: the headless CLI (`stinkshot full --out`, the process exits at once),
  edit sidecars, JPEG and WebP.

### Cold start: a frozen screen at about 0.25 s

The GPU path cannot get under 0.4 s: the D3D driver load alone is 0.25-0.35 s (any API),
then eframe 0.12 s and the overlay window 0.13 s. The screen is frozen with GDI within about
0.15 s of the launch (`prefreeze_while_gpu_starts`), so that picture now goes on display at
once in a plain GDI window (`platform::quick_overlay`): dimmed like the overlay, the window
under the cursor clear, crosshair cursor, topmost, excluded from capture. It takes the first
input so nothing reaches the apps underneath: a drag draws a clear rectangle with the accent
border; Esc cancels. When the region overlay is uncloaked the shell hands over
(`settle_quick_overlay`, `Overlay::take_quick_input`): a drag still in progress continues
in the overlay, a finished one becomes the selection, Esc cancels; a plain click is dropped.
The quick overlay never stays longer than 10 s. Only region, scrolling and OCR commands that
arrive while the GPU starts get it (window capture does not show the frozen screen).

### Results

Release builds, displays off, profile `perf`. Before = `perf-base` (f334f45, the benchmark's
build), after = this change (`perf-v3`; the cold rows `perf-v4`, which adds the same-pass
hand-over and fewer fill threads).

| Metric | Before | After |
|---|---|---|
| Overlay ready, 30 runs, median / p90 (GDI freeze: displays off) | 218 / 249 ms | 228 / 248 ms |
| Cold launch, region about 40 ms later, 8 runs: frozen screen shown (first window) | 563 ms; again 548 ms (the overlay) | **204 ms** (201-206, one 587) |
| Same: region overlay ready | 563 / 548 ms | 592 ms (579-612, one 1001) |
| Idle 60 s, 2 runs: private bytes | 135.2 / 135.8 MB | 124.5 / 124.9 MB |
| Same: working set at 60 s / at 120 s | 124 / 124 MB | 116 / **8** MB |
| First overlay after 70 s idle, 6 runs, median (range) | - | trimmed 276 ms (253-403), trimming off 282 ms (249-301) |
| Private bytes at idle after one overlay | not compared | 154-157 MB (trim on or off) |

- Cold start: the frozen screen is up 0.35 s sooner (well under 0.4 s); the full overlay
  comes about 45 ms later than before (contention from the quick overlay's 110 MB of DIBs and
  their fill while the driver loads; it was 75 ms later before the hand-over moved into the
  same pass and the fill went to 4 threads).
- The trim has no measurable cost on the next overlay (medians 276 vs 282 ms, within the
  noise). The first sample after start-up (403 ms) was the slowest of either set.
- Private bytes after the first overlay stay about 30 MB above the never-used figure in both
  trim settings (egui and allocator state); the benchmark's idle figure is the never-used one.
- PNG sizes and times: docs/benchmark/README.md, "Follow-up: perf track". In the app, a
  full-screen capture through the running instance (displays off, so a mostly dark picture)
  was re-compressed 940,494 -> 393,710 bytes 2 s after the save (711 ms of work), swapped in
  with its modification time kept, no temp file left.

### Not verified / open

- Displays-on numbers for everything here.
- The quick overlay's look and the drag handover were not seen on screen: the displays were
  off, the window is excluded from capture, and no synthetic input may be sent. The handover
  code paths were exercised only by the harness (open, `WM_CLOSE`).
- Re-compression with another program holding the file, on a non-NTFS history folder, and
  racing a re-edit were covered by unit tests of `replace_if_unchanged` only.
- Laptops (battery, hybrid GPUs): the background pass costs about 1 s of one core per
  megapixel (zopfli about 13 s per megapixel up to 1 MP).
- The separate-process design that could reach the 60 MB target was not built (see the
  frontier table); it is the remaining lever.
