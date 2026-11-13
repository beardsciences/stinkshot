# Changelog

All notable changes to Stinkshot are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

`scripts/release.ps1` copies the section for the version it builds into the release notes, so
write entries for people who use Stinkshot, not for people who read the diff.

## [Unreleased]

### Added

- Ctrl+V in the editor pastes a picture copied from any app as an image you can move and
  resize.
- Add to Canvas in History puts screenshots on a blank canvas to arrange side by side; each
  one added goes onto the open canvas.
- Cover What's Under It on text: the box takes the color of the page under it and the text
  the color of the words it covers, so you can type over text in one step.
- Delete on the capture toast moves a capture you don't want to the Recycle Bin.
- After Selecting a Region can be Open Editor Window Right Away: the screen unfreezes as soon
  as the region is chosen.
- Hide Stinkshot From Captures in Settings > General: turn it off to show the overlay, toasts
  and recording bar in screenshots and screen shares.

### Changed

- The region overlay no longer outlines the previous region.

### Fixed

- A region made by clicking a window or element can be dragged anywhere, not only inside
  that window.
- Dragging inside a region moves it again when the captured pointer is in it (with Cursor as
  Object on, it started a selection box instead).
- Text keeps its size and look while it is dragged.

## [0.1.3] - 2026-11-10

### Added

- Update Now: when an update is out, the toast, the tray menu and Settings > Privacy can
  download it, check it against the signed release info and install it, then restart on the new
  version. `stinkshot update` does the same from the command line. Copies that were not
  installed by the setup still get the download page.
- An "Updated to" message with What's New on the first start after an update.

### Changed

- The installer says Update, not Install, when Stinkshot is already installed, and skips the
  folder and options pages.
- The shortcut sheet (F1 or ?) in the editor lists only what works there, with the crop and
  view keys in their own groups. It is solid instead of see-through, centers on the editor
  window, and scrolls with the wheel when the window is too short for it.
- Toasts show the Stinkshot logo in the header.
- Settings > Privacy says when the last update check failed, and links to History instead of
  repeating its settings.
- Settings pages scroll to the bottom edge of the window, and several descriptions are shorter.

### Fixed

- Ctrl+V pastes into text fields (Settings search, file name pattern, text boxes), and Ctrl+C
  and Ctrl+X copy and cut from them.

## [0.1.2] - 2026-11-09

### Added

- Box mode for the Highlighter. Pick Freehand or Box in the tool bar, or hold Shift while
  dragging to use the other one. A selected freehand stroke has a To Box button.
- The mouse pointer is kept as an object in the editor. It starts selected, so Del removes it.
  Turn it off with Settings > Capture > Cursor as Object.
- Settings > Hotkeys shows hotkeys another program holds, with a Check Again button.
- Editor Window: editing from History, a toast or a pin opens a normal window instead of
  freezing the screen, and several can be open at once. Edit in Window on the action bar moves
  a fresh capture into one; Settings > Capture > After Selecting a Region can make that the
  Enter action.
- Save and Save as Copy when re-editing a History item (Ctrl+S saves back to the item).
- Pins show in Alt+Tab, Task View and the taskbar, and have a toolbar above the image. Both can
  be turned off in Settings > General > Pins.
- Text tool: drag to draw a fixed-width box; text wraps inside it. Click still makes self-sizing
  text.
- Select Text tool (W): select words in a capture and copy them.
- A region snapped to a window or element stays inside it when moved or resized. Hold Alt to
  resize freely.
- Settings search (Ctrl+F).
- The recording toolbar's Microphone button can switch the microphone on mid-recording when
  the recording has sound, even if it was off in Settings.
- Play and pause in the trim window (Space) to preview the clip before saving a GIF or MP4.
- Capture Selected Text and Capture Selected Paragraph: capture the text highlighted in the
  active app. No hotkey by default; set one in Settings > Hotkeys.

### Fixed

- A hotkey that another program held now starts working once that program closes, without
  restarting Stinkshot.
- GIFs (and trimmed MP4s) of a region whose width isn't a multiple of 16 came out as diagonal
  streaks.
- The star and trash buttons on History thumbnails flashed on hover and could miss clicks.

## [0.1.1] - 2026-11-09

First release with downloads. The full feature list is under 0.1.0.

### Added

- History cards show a Delete button next to the star on hover. Starred captures don't get one; delete them from the menu.

### Fixed

- Menus and dropdowns have even spacing and room around each item.
- The Beautify preview shows captures that have no annotations yet.
- The History star and Delete buttons stay readable on light thumbnails.

## [0.1.0] - 2026-11-08

Tagged in the repository; no downloads were published.

### Added

- Region capture on a frozen snapshot of all monitors, one selection across mixed-DPI displays,
  with a magnifier loupe, color picker, window and UI-element snapping, and size presets.
- Inline annotation: arrow, line, rectangle, ellipse, pen, highlighter, text, step counters,
  callout, magnifier inset, spotlight. Annotations stay editable from History.
- Real redaction (blur, pixelate, solid box) and Find Sensitive Info.
- Window capture with Windows 11 rounded corners, optional shadow and padded background.
- Multi-region capture: Shift+drag or Add Regions to combine several regions in one picture.
- Scrolling capture with sticky header and footer handling, lazy-loading pages and full web
  pages without the browser frame.
- QR and barcode reading, Copy as Table from OCR text, a Pick Color tool, a Measure ruler and
  Beautify frames (background, padding, corners, shadow).
- CJK and emoji text in annotations and the UI.
- OCR through Windows.Media.Ocr, offline.
- Pin to screen.
- Screen recording to MP4 (hardware H.264, AAC, 30/60/120 FPS) or GIF, with system audio, microphone,
  pause/resume, click and key visualizer, webcam bubble, countdown and trim.
- History folder with full-text search over OCR text, filters and retention limits.
- Optional Snipping Tool replacement (Win+Shift+S, Win+Shift+R, PrtSc).
- CLI for scripts: `stinkshot region|window|full|scroll|ocr|repeat|record|gif|pick-color|history|settings`,
  and `stinkshot quit` for installers and scripts.
- Per-user installer (no admin) and a portable zip.
- Opt-in update check that only trusts release info signed with the project's minisign key.
- Accessibility: Narrator and other screen readers can read and use every window, the capture
  overlay can be driven by keyboard alone (arrows start, move and resize a selection),
  Windows contrast themes are followed, and most text follows the Windows text size.
- A short first-run tour (Settings > About > Show Tour reopens it) and an offline Help page
  listing every hotkey and editor shortcut.
- All UI text is ready for translation. English is the only language so far.

### Security

- No upload targets, accounts or telemetry. The update check is the only network request and
  is off by default.

[Unreleased]: https://code.mccullough.dev/christian/stinkshot/compare/v0.1.2...main
[0.1.2]: https://code.mccullough.dev/christian/stinkshot/compare/v0.1.1...v0.1.2
[0.1.1]: https://code.mccullough.dev/christian/stinkshot/releases/tag/v0.1.1
[0.1.0]: https://code.mccullough.dev/christian/stinkshot/releases/tag/v0.1.0
