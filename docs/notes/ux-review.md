# UX review (Nov 6 2026)

Adversarial pass over every flow as an impatient Greenshot / Flameshot / ShareX user. Method:
read the code paths for each flow, run a `STINKSHOT_PROFILE=ux` debug build (History,
Settings, first launch) and capture what can be captured headlessly. Overlay, toasts and HUDs
are excluded from capture, so those were reviewed from code.

Scope: behavior, flow, copy and feedback. Purely visual styling (colors, spacing, icons) is the
visual polish pass's job and is not listed here.

Severity: **H** loses work or leaves the user stuck/confused, **M** friction or misleading
feedback a regular user hits, **L** polish.

## Findings

| # | Sev | Where | What is wrong | Fix | Status |
|---|---|---|---|---|---|
| 1 | H | First run | Double-clicking the exe shows nothing at all: no window, no toast, just a tray icon that Windows 11 usually hides in the overflow. A first-time user thinks it did not start and launches it again. | Plain launch (not `--tray`, no subcommand) shows a "StinkShot Is Running" toast naming the region hotkey and the tray icon, and that launching again opens History. Autostart (`--tray`) stays silent. | Fixed |
| 2 | H | History > Delete | Delete hard-deletes the file (`std::fs::remove_file`). No Recycle Bin, no undo; one Del + click loses a capture forever. Explorer users expect the Recycle Bin. | Files go to the Recycle Bin (`SHFileOperationW` + `FOF_ALLOWUNDO`); if Windows refuses, nothing is deleted and a toast says why. Modal copy now says "moves to the Recycle Bin"; Enter confirms, Esc keeps. | Fixed |
| 3 | H | Overlay / editor, Esc | Esc steps back once (deselect tool) and then cancels the whole editor with no confirmation, so two quick Escs throw away every annotation on a window capture or a re-edit. | With unsaved annotation edits, the final Esc arms a prompt ("Press Esc Again to Discard Your Annotations", 3 s) and only the second Esc cancels. No edits: one Esc cancels as before. | Fixed |
| 4 | M | Overlay hint bar | Idle hints advertised "Space Move" and "Arrows Nudge", which do nothing before a region exists. The wheel-to-parent feature was not mentioned in normal capture, and the record/OCR picker called it "Wheel Level" (jargon). After a selection the hint said "Esc Cancel" even when Esc would only deselect a tool. | Idle: Drag Region, Click Window or Element, Wheel Parent or Child, Enter Select Highlight, Esc. Selected: Enter Copy, Ctrl+S Save, Ctrl+Z Undo, Arrows Nudge, Esc. Esc reads Deselect / Discard / Cancel depending on what it will do. | Fixed |
| 5 | M | Settings > Privacy > Check Now | When no update exists, or the request fails, nothing happens. The helper text promises "A message appears when it answers." | A user-requested check always reports: "StinkShot Is Up to Date" (with version), the update toast (even if already known), or "Could Not Check for Updates" with the reason. Background checks stay silent. | Fixed |
| 6 | M | Busy clipboard (all copy paths) | When another app holds the clipboard the toast shows `OpenClipboard failed: Access is denied. (0x80070005)`. | Plain words: "Another app is using the clipboard. The capture is saved in History; copy it from there in a moment." (and a retry wording for text, history items and pins). Other errors keep their own message. | Fixed |
| 7 | M | No OCR language | Four different wordings across the overlay hotkey path, the worker, History and Settings ("Time & language", "Time and language, then Language and region", ...). None says the language needs text recognition. | One constant (`workers::NO_OCR_TITLE` / `NO_OCR_HELP`) used everywhere: "Add a language with text recognition in Windows Settings > Time & language > Language & region." | Fixed |
| 8 | M | Overlay action bar without OCR | The Copy Text button looks live; clicking it finishes the capture and then errors. | Disabled when no OCR language is installed, with a tooltip saying why. | Fixed |
| 9 | M | Toast (messages) | Message bodies were cut at 2 lines with an ellipsis, so error reasons and hotkey conflict lists were unreadable and there is no other place to see them. | Message toasts grow to 3-4 body lines for long text (still within the stack slot). | Fixed |
| 10 | M | Recording HUD toggles | Mic, System Audio and Webcam toggles can be switched "on" mid-recording even when that source was off in Settings, so the recorder never opened it. The HUD claims it is recording audio that it is not. | Sources not opened at start are disabled with a tooltip "Microphone Is Off in Settings > Recording". | Fixed |
| 11 | M | Recording HUD summary (GIF) | GIF recordings showed "GIF · H.264 · 60 FPS · System Audio": wrong codec, wrong frame rate, and GIFs have no audio. | GIF: "GIF · 15 FPS · Up to 960 px Wide · No Audio" from the GIF settings. MP4 unchanged. | Fixed |
| 12 | M | Settings > Capture > Freeze Screen | The toggle does nothing (the shell always freezes; see app-shell notes). A setting that silently has no effect erodes trust in every other toggle. | Row removed from Settings; the config key stays for compatibility. | Fixed |
| 13 | M | Esc in History and Settings | Esc did nothing in Settings and only deselected / cleared search in History; there was no keyboard way to close either window. | Esc closes Settings (unless a hotkey field, text field or dropdown owns it) and closes History once there is nothing left to deselect or clear. | Fixed |
| 14 | M | History empty state | With the region hotkey cleared it read "Press Not Set to capture a region." (seen live in the ux profile). | Falls back to "Use the button below or the StinkShot tray icon to capture a region." | Fixed |
| 15 | L | Recording HUD Stop | No hint that the record hotkey also stops. | Stop tooltip "Stop and Trim (Ctrl+Shift+R)" with the real hotkey. | Fixed |
| 16 | L | Settings > Privacy footer | Hard-coded `%APPDATA%\StinkShot\config.toml` is wrong for dev profiles and for portable setups. | Shows the real `Config::path()`. | Fixed |
| 17 | L | Title Case | "GIF saved", "MP4 and GIF saved", "StinkShot vX is available", loupe "C to copy", HUD tooltips "Microphone on/off". | Title Cased per ARCHITECTURE.md. | Fixed |
| 18 | L | Toast body | "760 x 420" instead of the design's "760 × 420". | Uses × in capture and recording toasts. | Fixed |
| 19 | L | Toast thumbnail | Clicking the thumbnail opens the editor, but nothing says so. | Tooltip "Edit" (or "Play" for recordings). | Fixed |
| 20 | H | Default hotkeys | Ctrl+Shift+T (OCR) and Ctrl+Shift+R (record) are global, so they steal "reopen closed tab" and "hard reload" from every browser; Ctrl+Shift+S is Save As in many apps. Greenshot/ShareX users never lost those. | Move the non-PrtSc defaults to PrtSc combos or Ctrl+Alt letters (e.g. OCR Ctrl+Shift+PrtSc, Scrolling Ctrl+Alt+PrtSc, Record Ctrl+Alt+R / Ctrl+Alt+G), update Tray.dc.html. Product decision (the design mockup names the current keys) and only affects new configs. | Fixed: defaults are now Ctrl+Alt+S/T/R/G (user decision Nov 6) |
| 21 | H | History retention default | Captures are auto-deleted after 90 days or 5 GB by default. Greenshot and Flameshot never delete anything; a user discovers this only when an old screenshot is gone. The setting is in the design. | Default to Keep Forever (0 days, no size cap), or ask once on first run; at least mention it in the first-run toast. | Fixed: default is keep forever (user decision Nov 6) |
| 22 | M | Window / Full Screen capture | Both always open the editor and need an extra Enter before anything is copied. Greenshot users expect Alt+PrtSc to land on the clipboard immediately. | Setting "After Window Capture: Open Editor / Copy Right Away", or copy immediately and offer Edit on the toast. | Fixed: new Settings > Capture toggle "Open Editor After Window and Full-Screen Captures" (`capture.open_editor_after_capture`, default off). Off: the capture is finished at once (`FinishAction::Quick`: window fx, clipboard per Copy to Clipboard, history, toast whose Edit opens the editor). |
| 23 | M | Overlay, new region after annotating | Clicking outside the selection once annotations exist is silently ignored (by design, to not orphan them). Users think the overlay froze. | Show a short hint ("Esc to Discard and Start Over") on that click. | Fixed: that click now shows "Annotations Keep This Region: Press Esc to Discard and Start Over" in the hint bar for a few seconds. |
| 24 | M | Toast after a failed copy | When the clipboard copy fails the toast has Edit, Pin, Copy Text, Folder but no Copy Image, so retrying means opening History. | Add a Copy action (or swap Copy Text for Copy when the copy failed). | Fixed: capture toasts have Edit, Copy, Pin, Copy Text; recordings Play, Copy (copies the file). Show in Folder moved to an icon button beside Dismiss so the row keeps four 84 pt buttons. |
| 25 | M | Trim window close (X) | Closing the trim window asks "Discard This Recording?", but the app-shell notes say closing keeps the untrimmed MP4. Users who close to "save as is" must choose Discard or Keep Editing; neither saves. | Third button "Save Untrimmed" in that modal, or make X save the MP4 as the notes describe. | Fixed: X or Esc asks "Save This Recording?" with Keep Editing, Discard (goes through the existing confirm) and Save Untrimmed (whole recording in the format it was started for, MP4 or GIF, into History with the usual toast). app-shell notes updated. |
| 26 | M | Error bodies everywhere | Most failure toasts still show `format!("{e:#}")` chains (file paths, HRESULTs). | Map the common ones (disk full, access denied, path too long) to plain sentences like #6. | Fixed: `errors.rs` maps disk full, access denied, path too long, not found, file in use, network path, busy clipboard and offline to plain sentences with advice; anything else gets the call site's sentence plus the log hint (see the third pass below). Every Notify / inline error in crates/app goes through it; chains are logged with `{e:#}`. |
| 27 | M | Overlay discoverability | Tool letters and Ctrl+P/Ctrl+T are only in tooltips; Flameshot shows a help overlay. | "?" or F1 shows a cheat sheet over the frozen screen. | Fixed: F1 or ? toggles a cheat sheet (Select, Tools, Edit, Finish) built from `overlay/keys.rs`, the same table `read_keys` uses; tests tie toolbar tooltips and chord labels to it. Hint bar ends with "? Shortcuts". |
| 28 | L | Tray left click | Left click waits 300 ms (double-click detection) before the overlay starts, which feels laggy next to PrtSc. | Start the freeze immediately and cancel it on the double click, or make left click open the menu like Greenshot. | Fixed: a single click starts the overlay immediately (no 300 ms wait). A double click opens History and drops the overlay the first click opened, whether the second click reaches the tray icon (`Command::TrayDoubleClick`) or lands on the new overlay at the icon's spot within 500 ms. |
| 29 | L | Overlay wheel snapping | Wheel-cycling to the parent element has no label, so it is unclear which element is highlighted. | Show the element's control type / name next to the size chip. | Fixed: after wheel-cycling, a chip at the highlight reads e.g. `Pane “Files” · Level 2 of 4` (UIA control type + name; `uia::SnapResult.labels`, additive). |
| 30 | L | Settings hotkey labels | Settings says "Region Capture" / "Scrolling Capture" while the tray says "Region" / "Scrolling". | Pick one set of names for both surfaces. | Fixed: `actions.rs` is the one list of capture actions (names, descriptions, hotkey fields, commands, tray ids) used by Settings rows, the tray menu and hotkey errors. Names follow Tray.dc.html: Region, Window, Full Screen, Scrolling, Copy Text (OCR), Repeat Last Region, Record MP4, Record GIF. |
| 31 | L | Repeat Last Region with none stored | Shows an info toast and opens the overlay at the same time; the toast sits over the frozen screen. | Skip the toast; the overlay hint already says what to do. | Fixed: no toast; the region overlay opens with "No Region Remembered Yet: Select One" in the hint bar (and "Last Region Is Off Screen: Select a New One" for that case). |
| 32 | L | Copy Color (C in the overlay) | Pops a full toast on top of the overlay for a hex string. | Flash the loupe plate ("Copied") instead. | Fixed: C flashes "Copied #CE9178" in the loupe plate (or the hint bar when the loupe is hidden); a busy clipboard reads "Clipboard Busy, Press C Again". No toast. |

## Verification

- `cargo test --workspace` (new unit tests: welcome text, GIF summary, clipboard wording,
  Esc-to-discard timing, toast message rows, HUD toggle tips, History empty-state text).
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all`.
- Live in the `ux` profile: first launch, History and Settings windows render and close.
- Not verified live (excluded from capture, or would need synthetic input): the welcome toast,
  the overlay hint changes and double-Esc, HUD toggles, Recycle Bin delete. The Recycle Bin
  path fails safe: if `SHFileOperationW` refuses, the item is kept and a toast says so.

## Second pass (Nov 6 2026): items 22-32

All open findings fixed (notes in the table). Verification:

- `cargo test --workspace`: new tests for error classification and wording (`errors.rs`), the
  action list (`actions.rs`), hotkey error texts, tray ids, the trim close prompt and its
  untrimmed `TrimDone`, toast actions and button width, the overlay key table / cheat sheet,
  tray double-click detection, element labels, snap index mapping, UIA control type names
  and the new config flag loading from old configs.
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all`.
- Live checks in the `ux2` profile are listed in the return summary of that pass; overlay,
  toasts and HUDs are excluded from capture, so their visuals were checked from code.

## Third pass (Nov 6 2026): review of the 22-32 fixes

- Offline wording: `errors::classify_text` no longer returns `Offline`; only the update check
  uses `classify_network_text` / `report_network_str`. Local timeouts (recorder start, window
  capture, audio/webcam init) get their call site's sentence instead of "check your internet
  connection".
- Log hint: `LOG_HINT` reads "Details are in the log (Settings > About > Open Log)." and
  Settings > About has a Log row whose Open Log button selects `stinkshot.log` (profile-aware,
  `logger::path`) in Explorer. `report_with` / `report_str_with` put the call site's closing
  sentence before the hint, so the hint always ends the body.
- Tray double click on the overlay: the overlay swallows the press and opens History on the
  release (`tray_double_pending`), and the shell ignores `TrayClick` for 700 ms after any
  handled double click (`tray_click_quiet_until`), so the button-up cannot start a second
  overlay over History.
- Copy from a toast or History: a failure in the clipboard step (context `CLIPBOARD_MARK`) reads
  as a clipboard problem, not "The file could not be read".
- Export that wrote nothing keeps (or reopens) the trim window so Save can be retried; the toast
  says so. Partial failures say what was saved is in History.
- Cheat sheet: `keys::sheet(select_only)`; Copy Text / record / scrolling overlays show only
  Select plus Finish ("Enter: Use This Region", Esc, F1 or ?). Edit lists Ctrl+Shift+Z, Del or
  Backspace and Shift+Drag (straight line, square or circle). Columns are sized to their text
  and wrap to 2x2 (or one column) when the monitor's logical width is too small
  (`ui::sheet_columns`).
- Settings > Hotkeys rows show a short error (`hotkeys::ErrorKind::row_text`) without the key,
  action or Settings path, skip the duplicate text when "Also used by" is shown, and
  `error_for` no longer prefix-matches "Alt+P" against "Alt+PrtSc".
- Error titles use "Could Not ...": Could Not Capture the Window, Could Not Finish Scrolling
  Capture, Could Not Save the File, Could Not Open Save As, Could Not Export the Recording,
  Could Not Read History, Could Not Find That Item, Could Not Recognize Text, Cannot Edit Yet.
  "Recording Problem" stays: the recording is stopped and kept, so it is a warning.
- Toast Copy tooltip: "Copy the Image to the Clipboard" / "Copy the File to the Clipboard".
- "Could Not Find That Item" body: "It may have been moved or deleted." (History has no
  refresh control).
