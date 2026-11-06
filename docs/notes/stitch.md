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

## Contract changes

None. No public signatures changed.

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
