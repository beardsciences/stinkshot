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
| 22 | M | Window / Full Screen capture | Both always open the editor and need an extra Enter before anything is copied. Greenshot users expect Alt+PrtSc to land on the clipboard immediately. | Setting "After Window Capture: Open Editor / Copy Right Away", or copy immediately and offer Edit on the toast. | Open |
| 23 | M | Overlay, new region after annotating | Clicking outside the selection once annotations exist is silently ignored (by design, to not orphan them). Users think the overlay froze. | Show a short hint ("Esc to Discard and Start Over") on that click. | Open |
| 24 | M | Toast after a failed copy | When the clipboard copy fails the toast has Edit, Pin, Copy Text, Folder but no Copy Image, so retrying means opening History. | Add a Copy action (or swap Copy Text for Copy when the copy failed). | Open |
| 25 | M | Trim window close (X) | Closing the trim window asks "Discard This Recording?", but the app-shell notes say closing keeps the untrimmed MP4. Users who close to "save as is" must choose Discard or Keep Editing; neither saves. | Third button "Save Untrimmed" in that modal, or make X save the MP4 as the notes describe. | Open |
| 26 | M | Error bodies everywhere | Most failure toasts still show `format!("{e:#}")` chains (file paths, HRESULTs). | Map the common ones (disk full, access denied, path too long) to plain sentences like #6. | Open |
| 27 | M | Overlay discoverability | Tool letters and Ctrl+P/Ctrl+T are only in tooltips; Flameshot shows a help overlay. | "?" or F1 shows a cheat sheet over the frozen screen. | Open |
| 28 | L | Tray left click | Left click waits 300 ms (double-click detection) before the overlay starts, which feels laggy next to PrtSc. | Start the freeze immediately and cancel it on the double click, or make left click open the menu like Greenshot. | Open |
| 29 | L | Overlay wheel snapping | Wheel-cycling to the parent element has no label, so it is unclear which element is highlighted. | Show the element's control type / name next to the size chip. | Open |
| 30 | L | Settings hotkey labels | Settings says "Region Capture" / "Scrolling Capture" while the tray says "Region" / "Scrolling". | Pick one set of names for both surfaces. | Open |
| 31 | L | Repeat Last Region with none stored | Shows an info toast and opens the overlay at the same time; the toast sits over the frozen screen. | Skip the toast; the overlay hint already says what to do. | Open |
| 32 | L | Copy Color (C in the overlay) | Pops a full toast on top of the overlay for a hex string. | Flash the loupe plate ("Copied") instead. | Open |

## Verification

- `cargo test --workspace` (new unit tests: welcome text, GIF summary, clipboard wording,
  Esc-to-discard timing, toast message rows, HUD toggle tips, History empty-state text).
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all`.
- Live in the `ux` profile: first launch, History and Settings windows render and close.
- Not verified live (excluded from capture, or would need synthetic input): the welcome toast,
  the overlay hint changes and double-Esc, HUD toggles, Recycle Bin delete. The Recycle Bin
  path fails safe: if `SHFileOperationW` refuses, the item is kept and a toast says so.
