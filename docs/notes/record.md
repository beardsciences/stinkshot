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

## Shakedown (Nov 7 2026)

### Recording failed to start: root causes

1. **DPI awareness.** `DuplicateOutput1` returns `DXGI_ERROR_UNSUPPORTED` (0x887A0004) to a
   DPI-unaware caller on a display scaled above 100%. The 4K HDR monitor moved to 125%; the test
   binaries have no manifest, so the HDR duplication failed (the 100% SDR monitor still worked).
   `examples/dup_probe` shows it: unaware -> 0x887A0004, thread PMv2 context -> ok. Fix: every
   duplication call and the whole video thread run inside `dpi::ThreadPerMonitorAware`.
2. **The app's warm capture session.** A process may duplicate each output once; a second
   `DuplicateOutput(1)` in the same process fails with `E_INVALIDARG` (0x80070057). Since the
   start-up capture warm-up (76385a7) the app always held every output, so recording from the app
   could not duplicate at all. Fix: `capture::lend_duplication()` (see platform.md); the lease
   lives in the DXGI `Capturer`. Another process (the release build) duplicating the same
   outputs is fine (two recorder processes ran concurrently, 249 and 59 frames, no errors).
3. **Static screens recorded nothing.** Desktop Duplication delivers no frame until the screen
   changes, so recording an idle desktop failed with "no video frames were captured". Monitors
   without a frame after 150 ms are now seeded from GDI, and pointer-only updates
   (`LastPresentTime == 0`, which also covers the placeholder first surface) are not copied.

Transient duplication errors (`E_ACCESSDENIED`, `NOT_CURRENTLY_AVAILABLE`,
`SESSION_DISCONNECTED`, `E_INVALIDARG`) are retried 4 times 150 ms apart; failures name the
display and the cause ("this process already duplicates the display", "Desktop Duplication does
not support this display mode or a DPI-unaware caller", ...).

### Silent duplication and the GDI fallback

During this shakedown the displays were idle-off (console display state 0, last input 2.6 h
earlier, 15 min display timeout). DWM then feeds no frames to any duplication, while GDI still sees
windows repaint. The recorder now cross-checks a duplication that has been silent for 1 s with a
GDI grab every 500 ms; when two grabs differ it captures that monitor with GDI at the frame rate
until the duplication delivers again (`GdiWatch` in `capture.rs`, counts logged at the end:
"N pictures from the duplication, M from GDI"). On a static screen this costs one GDI grab of the
monitor's part of the region every 500 ms. All recording numbers below went through this GDI path,
not through duplication frames.

Review fixes (Nov 7 2026): change detection compares the whole previous grab (a memcmp; the
sampled hash read 2 of every 16 pixel columns on 1920/2560/3840-wide rows and missed a typed
character or a caret). While the fallback is active every grab is written. The first probe
of a silent period is written too, since the screen may have changed after the duplication's
last frame and the stage would otherwise keep that older picture. A failing grab (secure
desktop, mode change, wrong size) backs off for 500 ms and logs once per failure streak.
The decision is `gdi_step` (unit tested).

### Late frames were lost

A picture polled when no frame was due (between deadlines, or while paused) was never converted
until the next new picture: a single update (one typed character) could be missing until something
else changed, and resume showed the picture from before the pause. The new-picture flag is now
sticky until converted. In the A/V test this took duplicate frames from 2927 to 102 of 5400.

### A/V test (examples/av_window + av_harness)

Test window (640x400, on the SDR monitor): barcode with its render clock, frame counter, moving bar,
white flash locked to a 100 ms 1 kHz beep at -30 dBFS every 2 s; the beep is written into its own
WASAPI stream and the flash is driven by that stream's clock, and the window logs the QPC time each
beep actually played. The harness records it through `Recorder` (system audio on, 30 fps, 8 Mbps,
3 min + a 5 s pause at 90 s), decodes the file with Media Foundation (`decode_all`) and compares.

| Measure | Result |
|---|---|
| Encoder | NVIDIA H.264 Encoder MFT (hardware) |
| Frames | 5401 written, 0 dropped; probe 180.03 s, 30.0 fps |
| CFR | every pts step exactly 33.333 ms (0 off-grid), first pts 0 |
| Content pacing | 102 duplicates (first 0.8 s before the GDI fallback engages), 22 steps > 70 ms (max 124 ms, all in one 13 s stretch of system load) |
| A/V offset (audio minus video) | start -11.4 ms [-12.2..-10.5], middle -11.3 [-12.1..-10.8], end -11.4 [-12.5..-9.0], 90 beeps |
| Drift | -3 ms/min fit; audio placement after 45 beeps returns to 0.0 / 0.5 ms of the first beep |
| Transient | 10 beeps at 35 to 50 s had audio placed up to +24 ms late, then recovered (same stretch as the video hiccups) |
| Pause 5 s | pts continuous across the pause (content jumps 5030 ms in one frame step), audio has no gaps, offset after resume equals before |
| Trim 30..50 s | 1.5 to 1.7 s, 20.01 s output, same offset for the same beeps (0.0 ms change) |
| GIF of the trim | 640x400, 15 fps, 298 frames, 20.00 s, 521 KB, 3.6 s, barcode blocks clean in every frame |

A shorter run with sparser content updates gave -6.8 / -6.5 / -6.1 ms and 0.04 ms/min drift. The
audio is placed by the mixer at the recording clock from the queue fill, not by device timestamps,
so a stall of the capture or mixer thread can delay audio by a few tens of ms until the drift
correction (max 0.5%) consumes the surplus. Device-timestamp placement
(`IAudioCaptureClient::GetBuffer` QPC position) would remove that; not done.

Other checks:
- Microphone (C920), 3 s: audio track present, RMS -41 dBFS, peak -23 dBFS (room noise, not
  digital silence). File deleted.
- Webcam bubble (C920), 3 s: ring pixel (255,255,253), bubble drawn from live frames. File deleted.
  Desktop test `webcam_overlay_records_when_a_camera_exists` passes.
- HDR color check (`swatch_window` + `hdr_record_probe`): 132,227,108 / 128,128,128 / 255,255,253 on
  the HDR and the SDR monitor, identical to hdr.md. With the displays off this went through GDI.
- Click and key visualizer: unit tests only (`overlay::tests`). Driving it needs real input to a
  foreground window, and this session could not make its own window foreground (foreground lock),
  so no synthetic input was sent.
- Desktop tests: all 5 pass (`cargo test -p stinkshot-record --test desktop -- --ignored
  --test-threads=1`), including the new `records_while_the_capture_session_is_warm`.

## Contract changes (additive)

- `Recorder::capture_backend(&self) -> &'static str` ("Windows Graphics Capture" or
  "DXGI Desktop Duplication").
- `primary_monitor_rect() -> Option<PxRect>` free function (used by tests / tools).
- `decode_all(path, on_video, on_audio)`, `DecodedFrame`, `AudioSink` (doc hidden): decode a
  whole file for measurement tools.
- Examples: `av_window`, `av_harness` (A/V sync shakedown), `swatch_window` (HDR check),
  `rec_probe` (logged short recording; `MIC=1`, `CAM=1`), `dup_probe` (duplication errors per
  DPI context and frame delivery), `display_state`, `wake_display` (guarded: only when its own
  window is foreground under the pointer).
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
