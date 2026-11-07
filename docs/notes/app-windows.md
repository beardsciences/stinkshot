# app-windows notes

Area: toast, pin, history, settings, recording HUDs (countdown, frame, HUD, trim, scroll HUD).
Files: `crates/app/src/{toast,pin,history_ui,settings_ui,record_ui}.rs` plus new
`ui_common.rs` and `ui_hotkey.rs` (see "Integration").

## What was built

- **Toast** (`toast.rs`): bottom-right of the work area of the monitor under the cursor at
  creation, stacked upward by `stack_index` on a fixed slot pitch (short message toasts are
  bottom aligned in their slot). Opaque card in the flyout color, DWM rounded corners through
  `apply_mica`, `with_active(false)` plus WS_EX_NOACTIVATE, tool window, topmost, excluded from
  capture, exact physical rect applied in `on_created`. Auto-dismiss after
  `general.toast_seconds` (errors at least 8 s); the timer does not run while the pointer is
  over the toast. Thumbnail click = Edit (captures) or Open (recordings). Actions in design
  order: Edit, Pin, Copy text, Folder (recordings: Play, Folder; messages: none). Copy text is
  disabled when `shared.ocr` is None. Actions push `EditItem` / `PinItem` / `CopyItemText` /
  `RevealItem` / `OpenItem` and close the toast.
- **Pin** (`pin.rs`): borderless transparent topmost window showing the image at 1:1 physical
  size. Drag with our own loop (see "Pin drag" below), wheel zoom 10%..800% anchored on the cursor (window resized with
  `set_rect` in physical px), Ctrl+wheel opacity 20%..100% (`ViewportCommand::Transparent` +
  painter tint alpha), hover accent outline and mini toolbar (opacity cycle, copy, close),
  double-click = `EditItem` when the pin came from history, Ctrl+C copies via
  `platform::clipboard::set_image` on a worker, Esc closes, right-click menu. A small chip
  shows the zoom or opacity for 0.9 s. Images wider or taller than the GPU texture limit are
  downscaled for the texture only; the window still reports the true size.
- **History** (`history_ui.rs`): sidebar (All, Starred, Recordings, source apps with counts,
  retention summary and bytes used), search box with 150 ms debounce into `History::query`
  (FTS), date filter, Open folder, virtualized card grid (only visible rows plus one overscan
  row are laid out), thumbnails loaded by one worker via `History::thumbnail` -> `Rgba::load`
  -> `ColorImage`, newest-request-first queue, at most 6 texture uploads per frame, LRU of 300
  textures keyed by (id, file size). Star toggle on the card, context menu and bottom action
  bar for the selected item (Edit/Open, Pin, Copy image, Copy text, Show in folder, Star,
  Delete), delete confirmation modal, keyboard: arrows/Home/End move, Enter = Edit (Open for
  video/GIF), Delete, Ctrl+C copy image, Ctrl+F focus search, Esc deselect / clear search.
  Paging: 400 items at a time, more loaded when the last rows scroll into view. Empty states:
  no captures yet (with the region hotkey and a Capture button), nothing matches (with Clear
  filters), and a permanent notice when no OCR language is installed.
- **Settings** (`settings_ui.rs`): General, Hotkeys, Capture, Editor, Recording, History,
  Privacy, About. Every `Config` field except `capture.last_region` and
  `privacy.last_update_check` is editable. Hotkey capture widget (click, press combo; Esc
  cancels, Backspace clears), duplicate-combo warning, shell registration errors shown inline
  and in a banner. Palette editor (edit, add, remove, reset), default color swatches, accent
  presets + custom + "Windows accent", folder picker (native IFileOpenDialog on a worker),
  retention controls, microphone/webcam dropdowns (device lists fetched on a worker, refresh
  button, missing configured device kept in the list as "not found"), Privacy page with the
  design's promise copy, update toggle and Check now (enabled only when the toggle is on, so
  the "off until you turn it on" promise holds), About with version, GPL-3.0-or-later and
  repo URL.
- **record_ui.rs**: `Countdown`, `RecordFrame`, `RecordHud`, `TrimWindow`, `ScrollHud` as
  specified (details below).

## Cross-window links and search box (Nov 6 2026)

- History and Settings each end their sidebar with a link to the other window
  (`ui_common::sidebar_link`, same rect in both: 36 pt row 12 pt above the bottom, hairline
  above, icon, label, shortcut right-aligned): "Open Settings" (gear, `Icon::Settings`) in
  History, "Open History" (clock) in Settings. Tooltips "Open Settings (Ctrl+,)" and "Open
  History (Ctrl+H)". The shortcuts work from the window: Ctrl+, in History (also while typing
  in the search box, not while the delete dialog is up), Ctrl+H in Settings (not while a
  hotkey field is recording). They push `OpenSettings` / `OpenHistory`; the shell focuses the
  window if it is open (restoring it when minimized), otherwise opens it. The retention
  summary in History now sits above the link.
- Search box: the `TextEdit` was laid out top-aligned inside a 30 pt row offset 1 pt up, so
  hint and typed text sat about 4-5 pt above the box centre. It now fills the box height with
  zero margin and `align(LEFT_CENTER)` (hint and typed text share it). Measured: at 125 % and
  100 %, dark and light, the text baseline matches the "Any Time" combo and "Open Folder"
  button next to it to the pixel; the search icon and clear button are within 1 px of the box
  centre. 150 % was not available on this desktop (the 4K monitor is at 125 %); the layout is
  point-based with egui pixel snapping, so it is expected to hold within 1 px there too.

## Pin drag (Nov 6 2026)

User report: dragging a pin between monitors, or sometimes within one, wiggles. Causes:

1. `ViewportCommand::StartDrag` only fired after egui's drag threshold, so the window jumped
   to catch up with the cursor.
2. `self.rect` was rewritten every frame from `outer_rect * native_pixels_per_point`, which is
   stale or mixed-scale mid-drag. Worse, `viewport_builder` reported `self.rect`, and egui
   turns every builder change into `OuterPosition` / `InnerSize` commands in points; winit
   converts those with the window's current scale, so every frame fought the OS drag (and on
   the other monitor used the wrong scale).
3. Crossing a 100 % / 125-150 % boundary, Windows resizes the window (WM_DPICHANGED), so the
   pin lost its 1:1 size.

Now: the builder always reports the creation rect (`initial`), so egui never sends move or
size commands. A primary press on the background (not the toolbar) records the global
physical cursor and the window rect; while the button is held (egui plus the async button
state) the window is placed with `window_util::set_rect` at press rect + cursor travel, at the
intended size (image size x zoom), once the cursor moved more than 4 px on either axis. The
size is re-asserted when Windows rescales it on a DPI change, during the drag and after it
(`size_fix`). A press without that travel stays a click: double-click opens the editor
(ignored for 400 ms after a drag) and right-click still opens the menu. Wheel zoom is ignored
mid-drag. Tests: `drag_threshold_is_per_axis_and_exclusive`,
`drag_moves_by_cursor_travel_and_keeps_size`, `size_fix_restores_intended_size_only`.

Verified live with synthetic input on the dev build: a 400 x 300 pin pressed and moved 3 px
did not move; dragged from the 100 % monitor onto the 125 % monitor and back it followed the
cursor at a constant offset on every sample and stayed 400 x 300 physical px throughout.
Double-click-to-edit and the context menu were checked by reading the code only.

## Design decisions

- All windows draw through `ui_common::root_ui`, a manual top-level `Ui` over the whole
  viewport, instead of egui panels. egui 0.37 panels need a `Ui`, but `Surface::show` receives a
  `Context`. Layout is rect based, so it works at any viewport size.
- Dirty tracking in settings: the config is cloned at the start of each frame and compared at
  the end. A change while the pointer is held (slider drags, color pickers) sets
  `config_dirty` when the pointer is released, so the shell saves once per gesture. Text
  fields edit a buffer and commit on Enter / focus loss, with validation (hex colors,
  non-empty file name pattern). Pending changes are flushed when the window closes.
- Toast, HUD, scroll HUD and trim use `ui_common::rect_button` (same palette, 6 px radius) for
  buttons that must fill an exact grid cell. Settings and History use `theme::widgets`
  (`setting_row`, `toggle`, `button`, `accent_button`, `icon_button`, `paint_icon`) because
  their layout is free-flow. HUD toolbar buttons use `widgets::icon_button` placed at exact
  rects.
- Countdown, RecordFrame and the scroll HUD never take focus. Esc for the countdown and the
  scroll HUD is read with `GetAsyncKeyState` (edge detected, ignoring an Esc still held from
  the region selection) in addition to normal egui key events.
- Trim: closing the window or pressing Discard asks for confirmation ("It has not been saved
  yet"). Confirmed Discard pushes `Command::RecordCancel`; the documented meaning is "drop the
  untrimmed recording and close the trim window". Save GIF / Save MP4 push `TrimDone` with
  exactly one of `as_gif` / `as_mp4`. Preview frames are requested latest-wins on a worker,
  throttled to ~15/s with a trailing request so the final position is always rendered.
  Filmstrip count is derived from the strip size and video aspect.
- `RecordHud` shows the summary line inside the HUD card (the design places it outside on the
  desktop) so the HUD is one window with an opaque background. The bar can be dragged by its
  background.
- Pin starts at 1:1 unless the image does not fit the monitor's work area, in which case it
  starts at the largest zoom that fits (otherwise a 30000 px scroll capture would create an
  unusable window).

## Integration

- **New files**: `ui_common.rs` and `ui_hotkey.rs`. They are mounted without touching
  `main.rs`: `toast.rs` contains `#[path = "ui_common.rs"] pub mod ui_common;` and
  `settings_ui.rs` contains `#[path = "ui_hotkey.rs"] pub mod ui_hotkey;`. Other modules use
  `crate::toast::ui_common` and `crate::settings_ui::ui_hotkey`. If you prefer, add
  `mod ui_common; mod ui_hotkey;` to `main.rs`, delete the two `#[path]` lines and fix the
  `use` paths (a search and replace).
- Each of my files starts with `#![allow(dead_code)]` because nothing constructs the surfaces
  until the shell exists. Remove them once the shell uses everything.
- `crates/app/Cargo.toml` gained a `[target.'cfg(windows)'.dependencies] windows` entry
  (features Win32_Foundation, Win32_System_Com, Win32_UI_Input_KeyboardAndMouse,
  Win32_UI_Shell, Win32_UI_Shell_Common, Win32_UI_WindowsAndMessaging) for the folder picker,
  `GetAsyncKeyState` and WS_EX_NOACTIVATE.

### Contract changes (additive)

- `Toast::set_stack_index(&mut self, usize)`: re-stack when a toast below goes away.
- `SettingsWindow::is_capturing_hotkey(&self) -> bool`: true while a hotkey field is waiting for
  a key press.
- `RecordFrame`, `Countdown`, ... keep their exact signatures. `Pin.image` is now
  `Arc<Rgba>` internally (private).

### Contract requests

1. **Suspend global hotkeys while recording one.** The shell should call
   `Hotkeys::set_suspended(settings.is_capturing_hotkey())` every frame. Otherwise pressing the
   combo being recorded (especially PrintScreen) triggers the capture instead of reaching the
   settings window.
2. **`Command::RecordCancel` while no recorder is active** (it is what the trim window's
   Discard sends): delete the untrimmed source file and close the `TrimWindow`.
3. **`App::clear_color` must be fully transparent** (`[0.0; 4]`) for the Pin, Countdown and
   RecordFrame viewports. The default eframe clear color is semi-opaque dark and would show as a
   rectangle around the pin and the countdown circle.
4. **Surfaces return `SurfaceStatus::Close` on `viewport().close_requested()`** (History,
   Settings, Pin Esc/close button). The shell should drop the surface when it sees `Close`. The
   trim window instead cancels the OS close and asks for confirmation.
5. `TrimWindow::set_progress(Some(p))` should be called while a job runs and `set_progress(None)`
   when it ends; the window re-enables its buttons on the Some -> None transition. The shell
   normally closes the window when the export succeeds.
6. `theme::install` should register a semibold font if one is wanted: all text here uses
   `FontId::proportional` (no weight variants) via `ui_common::font`.
7. `platform::monitors::at_point`, `input::cursor_pos`, `window_util::{set_rect, set_topmost,
   set_tool_window, exclude_from_capture, apply_mica, force_foreground}` and
   `system::{open_path, system_accent}` are called from the UI; they must be real, not stubs.
   `system::open_path` is also used with an `https://` URL (About page "Open") and must hand
   non-file strings to ShellExecute.
8. `Config::history_folder`, `naming::expand_pattern` and `Rgba::{thumbnail, load}` are used
   during drawing (file name preview, thumbnails).

## Known limitations

- No window drop shadow for the pin or HUDs (windows are exactly their content size; DWM
  rounded corners only). The design's CSS shadows are not reproduced.
- The pin does not hold a window outline when not hovered, so a pin of a dark capture on a dark
  desktop has no visible edge until hovered (as specified).
- Autostart, PrintScreen takeover, theme and hotkey re-registration are applied by the shell
  after `config_dirty`; the settings window does not call the platform for them.
- History grid thumbnails for MP4 recordings are a type placeholder unless the shell stored a
  thumbnail through `History::store_thumbnail`.
- Hotkey capture sees PrintScreen through a release event or a `GetAsyncKeyState` poll, because
  Windows does not always deliver its key-down to the window.
- Nothing here was run on a desktop: the theme, platform and history crates are still
  `todo!()` stubs in this worktree, so only compile + unit tests could run.

## Manual verification checklist (needs a real desktop)

Toast
- [ ] Capture with the cursor on each monitor: toast appears bottom-right of that monitor's work
      area above the taskbar, correct size at 100/125/150/200% scaling.
- [ ] Take 3 captures quickly: toasts stack upward without overlap; closing the lowest one
      lets the others slide down (shell calls `set_stack_index`).
- [ ] Focus stays in the app being captured (type into it while a toast is up).
- [ ] Hover pauses the dismiss timer; leaving resumes it. Thumbnail click opens the editor.
- [ ] Toast does not appear in a screen recording or a following screenshot.

Pin
- [ ] Pin a region: the pixels line up 1:1 with the original (zoom both side by side).
- [ ] Drag, wheel zoom around the cursor, Ctrl+wheel opacity, toolbar buttons, Esc, Ctrl+C
      (paste into Paint), double-click opens the editor for a history item.
- [ ] Transparent window has rounded corners and no dark rectangle (clear color request).
- [x] Pin on a secondary monitor with a different scale keeps its physical size (Nov 6:
      dragged 100 % <-> 125 %, stays 400 x 300).

History
- [ ] 2,000+ items scroll smoothly; thumbnails fill in lazily and quickly after a fast scroll.
- [ ] Search narrows within ~150 ms of the last keystroke; filters and date range combine.
- [ ] Keyboard: arrows, Enter, Delete (confirm), Ctrl+C, Ctrl+F, Esc.
- [ ] Star from the card, the menu and the action bar; counts in the sidebar update.
- [ ] Empty library shows the hint with the real region hotkey; no OCR language shows the notice.

Settings
- [ ] Change one value per page and confirm `config.toml` updates once per gesture (slider drag
      writes once on release).
- [ ] Hotkeys: record PrintScreen, Alt+PrintScreen, Ctrl+Shift+S, a Win-key combo, F-keys; Esc
      cancels, Backspace clears; duplicate warning; registration error text appears.
- [ ] Folder picker opens in front of the window and updates the folder.
- [ ] Microphone and webcam lists populate without freezing the window; Refresh works.
- [ ] Privacy: Check now is disabled until updates are enabled, then triggers a check.

Recording
- [ ] Countdown circle is centered on the region, ticks 3-2-1, Esc cancels even though the
      window never has focus, and the circle is absent from the recording.
- [ ] Dashed frame follows the region, clicks pass through it, absent from the recording.
- [ ] HUD sits below the region, above it near the screen bottom, inside as a last resort;
      toggles reach the Recorder; Pause/Resume and Stop work; it is absent from the recording.
- [ ] Trim: filmstrip fills, handles drag and cannot cross, preview follows the active handle,
      arrow keys nudge, Save GIF / Save MP4 progress bar, Discard asks first.
- [ ] Scroll HUD shows frame count and size, progress follows height, Cancel / Stop here and Esc.
