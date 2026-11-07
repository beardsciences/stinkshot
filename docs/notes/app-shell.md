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
- Logger at `%APPDATA%\StinkShot\stinkshot.log`, rotated at 5 MB, panic hook.

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
