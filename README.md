<p align="center"><img src="docs/logo-256.png" width="128" height="128" alt="Stinkshot logo: a glossy green gel orb"></p>

# Stinkshot

A local-first screenshot and screen-recording tool for Windows 11. It freezes the screen, lets you
select and annotate in place, and saves every capture to a plain folder you can use without the app.

## Features

- Region capture on a frozen snapshot of all monitors, one selection across mixed-DPI displays.
- Magnifier loupe with pixel coordinates and a hex color picker.
- Window and UI-element snapping through UI Automation. Aspect and size presets, arrow-key nudging.
- Inline annotation: arrow, line, rectangle, ellipse, pen, highlighter, text, step counters,
  callout, magnifier inset, spotlight.
- Real redaction: blur, pixelate and solid box destroy the pixels in the output.
- Annotations stay editable. Any history item reopens in the same editor.
- Window capture with Windows 11 rounded corners and alpha, optional drop shadow and padded
  background, cursor hidden by default.
- Scrolling capture: auto-scroll and stitch, with sticky header and footer handling.
- OCR with Windows.Media.Ocr: select a region, get the text on the clipboard.
- Pin any capture to the screen as an always-on-top reference.
- Screen recording to MP4 (hardware H.264 where available, AAC audio) or size-optimized GIF.
  System audio and microphone, pause/resume, click and key visualizer, webcam bubble,
  3-2-1 countdown, trim before saving.
- History: plain image files plus a small index, background OCR, full-text search, filters by app,
  window title and date, retention by age or size (starred items are kept).
- Fluent look with dark and light themes, tray icon, portable single exe.

## Privacy

Everything stays on your machine. There are no upload targets and no telemetry.

The only network call Stinkshot can make is an update check against the releases API of the
project's Forgejo instance. It is off until you turn it on in Settings, Privacy, and you can run it
manually with "Check now". Nothing about you or your captures is sent.

## Hotkeys

These are the defaults. All can be changed in Settings.

| Action | Default |
| --- | --- |
| Region capture | PrintScreen |
| Window capture | Alt+PrintScreen |
| Full screen | Ctrl+PrintScreen |
| Repeat last region | Shift+PrintScreen |
| Scrolling capture | Ctrl+Alt+S |
| OCR region | Ctrl+Alt+T |
| Record MP4 | Ctrl+Alt+R |
| Record GIF | Ctrl+Alt+G |

Taking over PrintScreen from Windows is a setting and is restored when you turn it off.

## CLI

With no arguments Stinkshot starts in the tray. Subcommands act on the running instance and start
it if needed.

```
stinkshot region
stinkshot window [--out FILE] [--cursor]
stinkshot full [--out FILE] [--monitor N] [--cursor]
stinkshot scroll
stinkshot ocr [--file IMAGE]
stinkshot repeat
stinkshot record
stinkshot gif
stinkshot history
stinkshot settings
```

`window --out`, `full --out` and `ocr --file` run headless and exit. The output format follows the
file extension (png, jpg, webp). The others are handy for scripts, AutoHotkey and Stream Deck.

## Files and config

- Config: `%APPDATA%\Stinkshot\config.toml` (the settings window writes the same file).
- Log: `%APPDATA%\Stinkshot\stinkshot.log`.
- Cache: `%LOCALAPPDATA%\Stinkshot\cache` holds the About page's pre-rendered logo animation
  (a few MB, safe to delete; it is rebuilt in the background).
- Captures: the history folder set in Settings. Annotation data lives beside each image in a
  `.stinkshot` subfolder, together with the search index and thumbnails.

## Privacy

Everything stays on your PC; the only network call is the optional update check, which is off by
default.

Redaction (Blur, Pixelate and Redact boxes) destroys the pixels in the exported image. However,
with Keep Editable Originals on (the default), Stinkshot also keeps the full, unannotated original
of every annotated capture in `<history folder>\.stinkshot\edit\<file name>.base.png` so you can
re-edit it later. That original still contains whatever you redacted. If you share or sync your
history folder, or copy the `.stinkshot` folder, you share those originals too. To prevent this,
turn off Settings > History > Keep Editable Originals (existing originals can be deleted by
removing the `edit` folder), and never share the `.stinkshot` folder.

## Build

Requirements: Windows 10 or 11, Rust 1.95 or newer (MSVC toolchain), Visual Studio Build Tools with
the C++ workload and a Windows SDK.

```
cargo build --release
```

The binary is `target\release\stinkshot.exe`. Tests:

```
cargo test --workspace
cargo test --workspace -- --ignored --test-threads=1
```

The second command runs desktop integration tests that capture the real screen, use the clipboard
and install input hooks, so run it on an interactive session.

The workspace is split into `core` (image, annotation render, config), `stitch`, `history`,
`platform` (Windows APIs), `record` and `app` (UI). See `docs/ARCHITECTURE.md`.

## License

GPL-3.0-or-later. See `LICENSE`.
