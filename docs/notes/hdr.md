# HDR displays

User report (Nov 6 2026): on one monitor the frozen overlay, and the captures taken from it,
looked brighter / washed out compared to the live screen.

## Cause

The 3840x2160 primary (`\\.\DISPLAY11`) has Windows HDR on with "SDR content brightness" at
480 nits; the 2560x1440 (`\\.\DISPLAY12`) is SDR. With HDR on, DWM composes the desktop in
linear scRGB FP16 (1.0 = 80 nits) and places SDR content at the SDR white level: an sRGB value
`v` becomes `srgb_decode(v) * white_nits / 80` (x6 here).

`DuplicateOutput1` was asked for `B8G8R8A8_UNORM`, on the assumption that the system tone-maps
HDR outputs to SDR. It does not: it clips the scRGB values to 0..1 without undoing the white
scale, so everything brighter than 80 nits blows out. GDI `BitBlt`, on the other hand, returns
correct SDR values. A cached duplication session seeds each output from GDI and then switches to
duplication frames as soon as the screen changes, which is why it looked fine sometimes and
blown out at other times.

Measured on the HDR monitor with a borderless window of three solid swatches (200x200 each):

| Path | #7FD36A | #808080 | #FFFFFF |
|---|---|---|---|
| GDI `BitBlt` (also the first capture of a fresh session) | 127,211,106 | 128,128,128 | 255,255,255 |
| Duplication BGRA8 (old warm session, old recorder fallback) | **255,255,239** | **255,255,255** | 255,255,255 |
| Duplication FP16 raw scRGB | 1.272, 3.908, 0.865 | 1.295 | 6.000 |
| FP16 converted with 480-nit white (new) | 127,211,106 | 128,128,128 | 255,255,255 |

On the SDR monitor every path returns the exact source colors, before and after.

## Fix

- `platform::hdr`: `displays()` reads every active display's advanced-color state
  (`DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO`, bit 1 = enabled) and SDR white level
  (`DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL`: `value / 1000 * 80` nits), keyed by GDI
  device name (`\\.\DISPLAYn`, same as `DXGI_OUTPUT_DESC::DeviceName` and `szDevice`).
  `display_for_monitor(hmonitor)` for callers that have an `HMONITOR`.
- `ScrgbToSrgb`: FP16 scRGB to 8-bit sRGB for one white level. Values are divided by the white
  level's scRGB value, so SDR content maps 1:1 to its original bytes, then sRGB-encoded.
  - Fast path: a 64 K table indexed by the half bit pattern (built once per white level and
    cached), used when all three channels are at or below SDR white. Positive halves are
    ordered like their bit patterns, so the check is three integer compares; negative values
    (out-of-gamut wide-color content) are negative as `i16` and clamp to 0.
  - Highlight path (`rolloff`): a color brighter than SDR white keeps its hue: it is scaled so
    its largest channel is 1.0 and blended toward white by `t = (m - 1) / (m - 1 + 3)` (`m` =
    largest channel relative to SDR white; half way to white at 4x). Brightness keeps rising
    with the input and hues do not skew the way a per-channel clip does (bright orange stays
    orange instead of turning yellow). The roll-off deliberately starts at SDR white, not
    below it: any knee under 1.0 would also dim SDR whites and break the 1:1 mapping.
  - Rows are split across up to 8 threads. 3840x2160 converts in 6.5 to 10 ms (release).
- `platform::capture`: each duplicated output knows its `DisplayColor`. HDR outputs are
  duplicated with `DuplicateOutput1([R16G16B16A16_FLOAT])` (no BGRA8 fallback: if that fails the
  output uses GDI, which is correct), the shadow/staging textures are FP16, and `read_back`
  converts with the output's white level. An HDR output that delivers anything other than
  FP16 frames falls back to GDI. The GDI seed for the first frame is lifted into scRGB with
  `srgb8_to_scrgb_half(white)` so it matches later frames exactly (round trip is lossless,
  unit tested). The session re-reads white levels at most once per second; an HDR on/off
  change rebuilds the session.
- `record::capture`: HDR monitors get an `HdrStage` (FP16 staging texture of the monitor's
  part of the region). DXGI duplication is FP16 on HDR monitors; WGC pools use
  `R16G16B16A16Float` on HDR monitors. New frames are copied into the FP16 stage and
  converted into the shared BGRA stage at the monitor's offset. SDR monitors are unchanged.
- The overlay needs nothing: egui-wgpu picks a `Bgra8Unorm` swapchain, which DWM composites as
  sRGB at the SDR white level, the same as every other SDR window. With correct captured
  pixels the frozen image shows at exactly the brightness of the live desktop.

## Verification (Nov 6 2026, this PC)

- `cargo run -p stinkshot-platform --example hdr_probe --release -- X Y W H` with the swatch
  window on each monitor (it toggles a 2x2 corner block every 250 ms so duplication keeps
  delivering frames). After the fix, `capture_rect` (first and warm calls) and `freeze_all`
  return 127,211,106 / 128,128,128 / 255,255,255 on both monitors; the log shows the HDR
  shadow as format 10 (`R16G16B16A16_FLOAT`) and the SDR one as 87 (BGRA8).
- `freeze_all` of the 6400x2160 desktop (4K HDR in FP16 + 1440p SDR) in a release build with a
  warm session and an idle CPU: 33 to 42 ms over 8 runs; the 4K HDR output's read-back is
  4 to 5 ms GPU copy + map and 5.6 to 9.8 ms conversion (trace log `read_back ...`). The first
  call of a new session takes 95 to 170 ms. While several other cargo builds saturated the CPU
  the same calls took 85 to 260 ms, with the plain BGRA8 swizzle of the SDR output slowed just
  as much, so that is CPU contention rather than the HDR path.
- `stinkshot.exe full --monitor 1|2 --out` (dev profile): exact colors before and after. The
  one-shot CLI never showed the bug because a fresh session always serves the GDI seed.
- Window capture of the swatch window (`hdr_probe window "HDR Swatch" 3`, PrintWindow fallback
  because WGC hangs on this PC): exact colors on both monitors.
- Recorder (`cargo run -p stinkshot-record --example hdr_record_probe --release -- X Y W H 3`,
  DXGI fallback): identical results on the HDR and SDR monitor (132,227,108 / 128,128,128 /
  255,255,253). The small green error is the H.264 YUV round trip and happens on SDR too.
- Unit tests: `hdr::tests` (half conversions, sRGB round trip, white level identity for SDR
  content at 80 to 480 nits, roll-off monotonic luminance / falling saturation / continuity /
  hue order, highlights not hard-clipped, parallel bands match serial, GDI seed round trip) and
  `capture::tests::fp16_uses_white_level`.

## Not verified

- WGC FP16 pools (recorder) on an HDR monitor: WGC session creation hangs on this PC, so only
  the DXGI path ran. `capture_window`'s WGC path still uses a BGRA8 pool (also unverifiable
  here; PrintWindow, which ran, is correct).
- Real HDR content (video, games) above SDR white: covered by unit tests only.
- Changing the SDR content brightness slider while StinkShot runs (not touched: it is a system
  setting). The code re-reads it at most once per second.
- Auto color management on an SDR display (advanced color on, SDR white reported as 80 nits):
  takes the FP16 path with scale 1.0, which should be exact, but no such display here.
- The overlay's on-screen brightness was reasoned from the swapchain format, not measured.
