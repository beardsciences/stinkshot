# Changelog

All notable changes to Stinkshot are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

`scripts/release.ps1` copies the section for the version it builds into the release notes, so
write entries for people who use Stinkshot, not for people who read the diff.

## [Unreleased]

### Added

- Box mode for the Highlighter. Pick Freehand or Box in the tool bar, or hold Shift while
  dragging to use the other one. A selected freehand stroke has a To Box button.
- The mouse pointer is kept as an object in the editor. It starts selected, so Del removes it.
  Turn it off with Settings > Capture > Cursor as Object.
- Settings > Hotkeys shows hotkeys another program holds, with a Check Again button.

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

[Unreleased]: https://code.mccullough.dev/christian/stinkshot/compare/v0.1.1...main
[0.1.1]: https://code.mccullough.dev/christian/stinkshot/releases/tag/v0.1.1
[0.1.0]: https://code.mccullough.dev/christian/stinkshot/releases/tag/v0.1.0
