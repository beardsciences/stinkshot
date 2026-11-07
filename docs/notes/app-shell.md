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
  PrintScreen takeover handled in `reload`/`shutdown`.
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
