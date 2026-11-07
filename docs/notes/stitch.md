# stitch notes

## What was built

`crates/stitch/src/lib.rs`: full `Stitcher` (push / height / frames / finish).

- Matching uses the middle 90% of width only (5% ignored per side) so scrollbars do not matter.
  The output keeps the pixels of whichever frame contributed the row, so scrollbar columns in
  the result are whatever the contributing frame showed.
- First frame is stored whole. On the first frame pair that differs, fixed header/footer are
  detected as the leading/trailing rows identical (within tolerance) at the same position
  (each capped at 1/3 of the height). If matching fails with that layout, one retry is made
  with no header/footer. The layout is then frozen for the session.
- Offset search: exact RGB row hashes of distinctive rows near the top of the new band give
  candidate offsets; each candidate is verified over the whole overlap with per-channel
  tolerance (a row may differ in up to 1% of pixels; up to 3% of overlap rows may mismatch,
  to survive caret blink). Lowest mismatch wins; ties go to the offset closest to the previous
  step (or the largest on the first step). If no candidate verifies (noisy frames, blank-heavy
  content) every offset is scanned.
- Minimum overlap between consecutive frames is 10% of the band height (min 4 rows).
- Header stays once at the top (it is part of the first frame); footer is held separately,
  refreshed from the newest appended frame, and added once at `finish`.
- Unchanged: frames equal within allowance (max 2% of band rows differing) to the previous
  accepted frame. Two consecutive unchanged pushes return `End` (so: Appended, Unchanged,
  End). The previous frame is not replaced by an Unchanged frame, so drift cannot accumulate.
- max_height: counts the footer. The push that reaches the cap appends only what fits and
  returns `Appended { added_rows }` (possibly fewer than the scroll step); any later push
  returns `End`. If the first frame is taller than the cap it is cropped to the cap.
- Failed leaves state intact; the caller may keep pushing or `finish`. Zero-size, bad data
  length, or size different from the first frame all return `Failed` without panicking.
- `frames()` counts the first frame plus every `Appended`; Unchanged/Failed pushes are not counted.

## Performance

1920x1080 push with a 300 px scroll: about 8 ms in release (the test asserts < 30 ms in
release builds only).

## Driver loop (Nov 7 2026)

`drive::drive(stitcher, params, capture, scroll, control, progress)` is the scroll loop that used
to live in `app::workers::scroll_capture`: capture -> push -> scroll -> wait, until End, Failed,
a capture error, Stop, Cancel or `max_steps`; first step `initial_notches` (about half the region
in logical px / 220), fewer notches when a step moved > 85% of the frame, more when < 30%.
The app passes `capture::capture_rect` and `input::scroll_at`; harnesses pass their own scroll.

## Shakedown (Nov 7 2026, `cargo run -p stinkshot --example scroll_harness -- MODE DELAY_MS`)

Own window: 800x600 popup with a real scrollbar (783 px client), sticky 60 px header and 40 px
footer, 6000 px document of GDI text in several sizes, section rules, blank bands and gradient
images with noise, 100 px per wheel notch. The harness runs `drive` with `capture_rect` on the
window rect (scrollbar included) and posts WM_MOUSEWHEEL to that window only (no system input, the
pointer never moves). Ground truth is header + document + footer (6100 rows), compared on the
client columns.

| Mode | Delay | Steps / appended | Result vs truth |
|---|---|---|---|
| Instant scroll | 120 ms (default) | 22 / 20, notches 3 then 4 | 6100 rows, all identical (max diff 0) |
| Instant scroll | 30 ms (minimum) | 22 / 20 | identical |
| Smooth scroll (250 ms ease-out) | 120 ms | 24 / 22 | identical |
| Smooth scroll | 30 ms | 26 / 24, notches grow to 12 near the end | identical |
| Lazy content (placeholder for 400 ms) | 120 ms | 20 / 19, Failed | 6000 rows, placeholder stripes stitched in, then no overlap |
| Lazy content | 500 ms | 22 / 20 | identical |

End detection: every run that completed ended through End (two Unchanged pushes after the last
partial step), with the footer once at the bottom and the header once at the top. capture_rect
median 16 to 22 ms per step, first call about 0.5 s (session creation).

Defect found and fixed: with the displays idle-off the cached duplication session returned the
same picture every step (duplications receive no frames then), so the capture ended after one
frame (`Unchanged`, `Unchanged`, End). `capture_rect` now uses GDI unless the duplication
delivered a frame during the call (platform.md, "Silent duplications").

Not handled: lazy-loading content at the default delay. Placeholders look like content and are
stitched in; the next frame then no longer overlaps. Raise Scroll Delay for such pages.

## Contract changes

- Added `pub mod drive` (`drive`, `DriveParams`, `DriveControl`, `DriveOutcome`,
  `initial_notches`). No existing signature changed.

## Contract requests

None.

## Known limitations

- Vertical scroll down only. Scrolling up or horizontal scrolling is not handled (Failed).
- A sticky header/footer that changes between frames (clock, animation) is not detected as
  fixed; the no-header retry may then fail on tall changing bars.
- A scroll step moving fewer rows than ~2% of the band while only a few rows differ may be
  classed Unchanged. Perfectly periodic content (every row repeated) is ambiguous.
- Requires >= 10% overlap between consecutive frames; the driver should scroll by at most
  ~85% of the viewport.

## Manual verification (real desktop)

1. Scrolling capture of a long web page with a sticky nav bar: header once at top, no
   duplicate bars mid-image.
2. Page with a sticky footer/cookie bar: footer once at the bottom.
3. Page with a blinking caret or video thumbnail inside the region: still stitches.
4. Reaching the end of a page: final partial scroll lines up, capture ends via End.
5. Window with smooth-scroll animation: with too short a delay frames may mid-animate and
   fail; confirm `scroll_delay_ms` is adequate.
