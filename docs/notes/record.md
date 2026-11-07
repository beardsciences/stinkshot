# record (crates/record)

## What was built

All public items of the stub contract are implemented: `Recorder` (start, pause, resume,
status, mutes, webcam visibility, overlay toggles, stop, cancel), `probe`, `thumbnails`,
`frame_at`, `trim`, `to_gif`, `list_microphones`, `list_webcams`.

Modules (all private except the re-exports in `lib.rs`):

| Module | Role |
|---|---|
| `timeline` | `Timeline` (pause/resume offsets), `FrameScheduler` (CFR: duplicate when late, skip after a 2 s stall), `pts_hns` |
| `capture` | WGC per monitor (free-threaded pools, polled), DXGI Desktop Duplication fallback, region crop into one staging texture |
| `convert` | BGRA -> NV12 (BT.709 limited, multi-threaded bands), box/bilinear scaler |
| `overlay` | tiny-skia click ripples, key chip (ab_glyph), webcam circle with white 2 px ring |
| `encoder` | `IMFSinkWriter` MP4: H.264 (hardware MFT first, software fallback) + AAC 192 kbps |
| `audio`, `dsp` | WASAPI loopback + mic capture threads, sinc resampler to 48 kHz stereo, mixer, PCM16 |
| `webcam` | `MFEnumDeviceSources`, `IMFSourceReader` thread keeping the latest frame |
| `mfread`, `media` | source-reader decoding; probe / thumbnails / frame_at / trim |
| `gifenc` | MP4 -> GIF: per-frame imagequant palette + dithering, frame-diff transparency |

## Design decisions

- **Video path**: capture -> GPU `CopySubresourceRegion` into one staging texture (only the
  region; several monitors are composited by copying each intersection) -> map -> overlays on a
  tiny-skia pixmap only when something is visible -> CPU BGRA->NV12 -> sink writer takes NV12.
  Pictures are reused (no re-map, no re-convert) while the screen is static and no overlay is live.
- **tiny-skia channel order**: the overlay pixmap holds BGRA bytes; every colour is passed with
  R and B swapped so the bytes land in BGRA order. Frames are opaque so premultiplied == straight.
- **Clock**: video frame `n` is stamped `n/fps`; frames are written at their deadline, duplicating
  the last picture when WGC delivers nothing. Audio is produced by a mixer thread pulling from
  per-source queues at the same recording clock (silence fills gaps, so a quiet loopback device
  that delivers no packets stays in sync). A source that runs ahead by > 100 ms is trimmed.
- **Pause**: the timeline freezes; both threads stop writing; audio queues are cleared so resume
  starts clean. Held keys are released in the overlay on pause.
- **Mute** writes silence for that source (queue is still drained to keep sync).
- **Temp file**: `<stem>.recording.tmp.mp4` next to the output, renamed on stop; deleted on cancel,
  on drop without stop/cancel, and when no frame was captured.
- **Encoder name** comes from the real MFT behind the writer (`MFT_FRIENDLY_NAME_Attribute`), with
  "(hardware)" when it is async / has a hardware URL. On the dev box: "NVIDIA H.264 Encoder MFT
  (hardware)".
- **Fatal errors** after start are stored in `Status.error`; `stop()` still finalizes and returns
  the file when at least one frame was written.
- **Input hook** is installed lazily and only while click or key overlay is enabled (so no keyboard
  hook exists when the user does not use the key chip). A missing/unimplemented hook is logged and
  ignored (`catch_unwind`); overlays then simply never appear.
- **Fonts**: key chip tries `stinkshot_core::fonts::Fonts::load_system()` (panic-safe) and falls back
  to reading Segoe UI / Arial from `%WINDIR%\Fonts`.
- **Monitors** are enumerated locally (`EnumDisplayMonitors` with a temporary per-monitor-v2 thread
  DPI context) so physical rectangles are right even if the calling process is not DPI aware, and so
  this crate does not depend on `platform::monitors` being implemented.
- Webcam / audio device that was explicitly requested but cannot be opened makes `Recorder::start`
  fail with a descriptive error (no silent downgrade).
- **trim** decodes to RGB32 (video processor), converts to NV12 and re-encodes; audio is decoded
  to 48 kHz PCM16 and re-encoded to AAC, head/tail trimmed on sample boundaries. Output bitrate is
  the source's average bitrate clamped to 2..60 Mbps.
- **to_gif**: target-fps sampling of the decoded stream; unchanged frames are merged into the
  previous frame's delay (cumulative centisecond rounding, no drift); a changed frame is cropped to
  the bounding box of changed pixels, unchanged pixels inside it become transparent, and the crop is
  quantized with imagequant (dithering 1.0). Change detection compares against the source value
  each pixel last changed to, so slow gradients do not freeze.
- **HDR monitors** (Nov 6 2026): captured as FP16 scRGB (DXGI `DuplicateOutput1` FP16, WGC
  `R16G16B16A16Float` pool) into a per-monitor FP16 staging texture, then converted on the CPU
  into the BGRA stage with the display's SDR white level (`stinkshot_platform::hdr`). An 8-bit
  duplication of an HDR monitor is clipped, not tone-mapped, and recorded blown out. Verified on
  the DXGI path only (WGC hangs here). See `docs/notes/hdr.md`.

## Contract changes (additive)

- `Recorder::capture_backend(&self) -> &'static str` ("Windows Graphics Capture" or
  "DXGI Desktop Duplication").
- `primary_monitor_rect() -> Option<PxRect>` free function (used by tests / tools).
- `Recorder` is now defined in `recorder.rs` and re-exported; its (private) field layout changed.
- Cargo features added to `windows`: `Win32_Graphics_Gdi`, `Win32_Media`, `Win32_UI_HiDpi`.
  Dev-dependency `gif` for the GIF assertions in the desktop test.

## Contract requests

- None blocking. Nice to have in the workspace root `Cargo.toml` (not editable from this crate):
  `[profile.dev.package.stinkshot-record] opt-level = 3`. In an unoptimized build the NV12
  conversion of a 4K frame is too slow for 30 fps (debug-mode `cargo test -- --ignored` passes but
  the 4K recording lags and records duplicates).
- `platform::input::InputHook` events must carry virtual-desktop **physical** pixel positions and
  `vk` codes that pair KeyDown/KeyUp; labels are expected as "Ctrl", "Alt", "Shift", "Win", "P", ...
  (modifiers are sorted first in the chip).

## Known limitations

- **WGC session creation never returned on the dev machine** (`CreateCaptureSession` blocks in an
  RPC wait, reproduced with a 30-line standalone program, for monitor and window items, hardware and
  WARP devices, STA and MTA). Because of that the WGC path is written against the API docs/sources but
  was **not exercised end to end**. `Capturer::new` creates WGC sessions on a helper thread with a 3 s
  timeout and then falls back to DXGI Desktop Duplication, which is what all desktop tests used here.
  After one failure the process skips WGC for later recordings (so only the first start pays 3 s).
  The Desktop Duplication fallback cannot include the mouse cursor (`include_cursor` is ignored there),
  duplicates only monitors on a single GPU, and ignores rotated outputs.
- No device-change handling for audio (the default render device is bound at start).
- Hardware encoder failures that surface only after the first frames (not at open) are not retried in
  software mid-recording.
- GIF encoding of a 4K source is single threaded; use `max_width` for speed.
- `Fonts::load_system` and `InputHook::install` were stubs in this worktree: the fallbacks above ran;
  the real ones are unexercised.

## Manual verification checklist (real desktop)

1. `cargo test -p stinkshot-record -- --ignored --test-threads=1 --nocapture` (release is ~35 s; use
   `--release`). Check the printed `capture backend:` line; on a machine where WGC works it should say
   "Windows Graphics Capture".
2. Play audio during a recording; confirm it is audible in the MP4 and that muting the system source
   silences it while video continues.
3. Speak into the mic with `microphone: true`; confirm level and sync with a clapped video.
4. Region spanning two monitors with different DPI: both halves present, no offset.
5. Toggle `set_overlays(true, true)` with the real input hook: ripples at the click positions, chip
   bottom-centre ("Ctrl + Shift + P"), fades 1.2 s after release.
6. Webcam bubble in each corner, `set_webcam_visible(false)` hides it.
7. Pause 5 s, resume: the file has no freeze/gap; audio stays in sync after resume.
8. Kill the app mid-recording: only `*.recording.tmp.mp4` is left (not a valid MP4); the final path
   never contains a partial file.
9. Trim then play: first frame is the requested start, audio aligned; GIF plays in a browser, loops.
10. Change display resolution during a recording: recording continues (WGC resizes its pool; the
    output size stays at the original region size).
