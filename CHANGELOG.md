# Changelog

All notable changes to Stinkshot are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

`scripts/release.ps1` copies the section for the version it builds into the release notes, so
write entries for people who use Stinkshot, not for people who read the diff.

## [Unreleased]

## [0.1.0] - Unreleased

First public release.

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

### Fixed

- TODO: resource leaks (GPU, handle and thread growth over long sessions in capture,
  recording and History). Replace with the merged leak fixes.

### Security

- No upload targets, accounts or telemetry. The update check is the only network request and
  is off by default.

[Unreleased]: https://code.mccullough.dev/beard/stinkshot/compare/v0.1.0...main
[0.1.0]: https://code.mccullough.dev/beard/stinkshot/releases/tag/v0.1.0
