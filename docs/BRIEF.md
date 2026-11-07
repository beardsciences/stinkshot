# Stinkshot brief

A modern, local-first screenshot and screen-recording tool for Windows. Replaces Greenshot
(feels legacy: dated look, clunky editor, too many clicks, bad mixed-DPI behavior) and
Flameshot (missing features below).

Decided Nov 6 2026 through a Q&A session. Everything below is in scope for the first build.

## Platform and stack

- Windows only (Windows 11 primary). Uses Windows APIs directly.
- Rust + native Win32 via `windows-rs`. Direct2D / DirectComposition (or wgpu) for the overlay.
- Toolchain: rustc 1.95 stable-x86_64-pc-windows-msvc, MSVC 14.44 BuildTools.
- Portable single exe. Config in `%APPDATA%\Stinkshot\config.toml`; the settings UI writes the
  same file. Optional start-with-Windows toggle.
- Look: clean native Fluent / Windows 11 (Mica, rounded, dark and light). The humor lives only
  in the name and maybe the icon.

## Privacy stance

- Local only. No upload targets, no telemetry.
- The single network call is an opt-in update check against the Forgejo releases API.
- Label this clearly as a feature (README, settings, about box).

## Triggers

- Takes over PrintScreen (region overlay).
- Configurable global hotkeys: region, window, full screen, scrolling, record, OCR, repeat last
  region.
- Tray icon menu: capture modes, history, settings.
- CLI: `stinkshot.exe region|window|full|scroll|record|ocr ...` for scripts, AutoHotkey,
  Stream Deck.

## Region overlay

- Freezes the screen: snapshots all monitors instantly so menus/tooltips/video stay still.
- One selection can span all monitors at mixed DPI.
- Magnifier loupe with pixel coordinates and color picker (copy hex).
- Window and UI-element snapping on hover (UI Automation) for one-click capture.
- Aspect and size presets (16:9, 1:1, exact pixels); arrow keys nudge edges.

## Annotation (inline on the overlay, Flameshot style)

- Tools appear around the selection; annotate in place; Enter / Ctrl+C to finish.
- Basics: arrow, line, rectangle, ellipse, freehand pen, highlighter, text.
- Redaction: blur, pixelate, solid box. Real redaction: pixels destroyed in output.
- Step counters (auto-incrementing numbered circles).
- Magnifier inset, speech-bubble callout, spotlight (dim everything except an area).
- Annotations are live objects. Any history item can be reopened in the same overlay editor
  with annotations still editable (store annotation data alongside the image).

## After capture (defaults)

- Copy to clipboard.
- Toast with actions: Edit, Pin, Open folder, Copy text (OCR), etc.
- Every capture is saved to history (see below).

## Window capture

- Real Win11 rounded corners with alpha (PNG), optional drop shadow.
- Capture occluded windows (Windows Graphics Capture).
- Cursor hidden by default, toggle to include.
- Optional padded background frame for sharing.

## Scrolling capture

- Auto-scroll + stitch: select region, Stinkshot sends scroll input and stitches frames by
  image matching. Handle sticky headers/footers as well as practical.

## OCR

- Windows built-in `Windows.Media.Ocr`, behind an engine interface so a stronger engine can be
  added later.
- Select region -> text to clipboard.

## Pin to screen

- Float any capture always-on-top as a reference window.

## Screen recording

- MP4 with hardware encode (NVENC on RTX 3060 Ti via Media Foundation).
- GIF output with size-optimized palette.
- System audio and/or microphone.
- Trim start/end before saving.
- Click and key visualizer overlay.
- Pause/resume into one file.
- Webcam bubble overlay.
- 3-2-1 countdown.

## History

- Plain folder of image files plus a small index, usable without the app.
- Background local OCR of every capture; search history by text.
- Store source app, window title, timestamp; filter by them.
- Retention limits: auto-delete after N days or N GB, except starred items.

## Release

- Open source, GPL-3.0, hosted on the user's Forgejo (code.mccullough.dev).

## Defaults chosen without asking (change freely)

- Save format PNG; JPEG/WebP selectable. Filename pattern `Stinkshot_{yyyy-MM-dd_HH-mm-ss}`.
- Project lives at `C:\dev\stinkshot`.
