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
about 250 MB. Dropping it would bring back the 0.8-1.2 s cold first capture, and releasing it
later frees only 35 MB. A smaller warm session (no full-size staging textures until a capture
needs them, or no FP16 duplication kept between captures) is the capture track's call.

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
  window closed did not release it. Cause not found.
- Countdown and recording frame transparency were not looked at on screen (the pin was).
