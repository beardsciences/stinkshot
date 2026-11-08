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

## Adaptive settle, lazy loading and web pages (Nov 8 2026)

Scrolling capture no longer needs Scroll Delay tuned per page. The setting is now "Minimum
Delay Between Scrolls" (default 120 ms, unchanged); anything longer happens on its own.

### Driver (`drive.rs`)

- **Settle** before every push: capture every 40 ms until two consecutive frames are identical
  (any changed pixel resets it, so smooth-scroll animations, spinners and shimmer keep it waiting)
  and `Stitcher::assess` finds no loading placeholders among the rows the frame would add, or
  until `settle_max` (2.5 s, `DriveParams::new`). The first frame is checked too.
- **Permanent animation** (video, carousel): two settles in a row that time out with only a small
  area changing (bounding box <= 2% of the frame) switch small changes to "still" for the rest
  of the capture, so such a page costs two 2.5 s waits, not one per step.
- **Retry on mismatch**: a push that finds no overlap is captured again after 160 ms and 320 ms
  (`retries`, 2) before the capture ends with what it has.
- **End grace**: after a step that did not move, the next capture waits at least 700 ms, so an
  infinite-scroll page that appends content at the bottom continues instead of ending.
- `DriveParams::new(logical_height, delay)` holds the defaults (`max_steps` 800, `settle_max`
  2.5 s, `retries` 2). `DriveOutcome` now reports settle times, timeouts, placeholder waits,
  retries and captures (the harness prints them).

### Placeholders (`settle.rs`, `Stitcher::assess`)

- A placeholder row has at most 6 color runs (tolerance 10) and a block (run >= 10% of the
  compared width) in a color that is not a page background color: skeleton bars on the page,
  flat gray panels, stripes. Text, photos and horizontal gradients have many runs and never
  qualify; a blank row in a background color does not either.
- Background colors: colors of fully flat rows (>= 98% one run over the compared middle 90%, so a
  scrollbar beside a blank line does not spoil it) on >= 1% of the first frame's rows; the most
  common long-run color when there are none. A skeleton bar never fills a row, so a page that is
  still loading does not teach its placeholder color.
- A frame looks unloaded when >= max(8, 5% of the new rows) are placeholder rows. When a settle
  times out while still seeing them, their block colors are accepted as page colors
  (`accept_colors`): a gray sidebar or a colored banner costs one 2.5 s wait, not one per step.

### Stitcher repairs (`lib.rs`)

When the normal match fails after the first step (`repair`):

1. Fixed rows are detected again between the two frames at the same screen position, and the
   offset is searched with that layout (a sticky header that collapsed or a bar that appeared).
2. Otherwise a relaxed search (`find_offset_relaxed`) accepts an offset when at least a third of
   the overlap matches (and >= min overlap) with >= 8 distinctive matching rows. A run of
   mismatching rows at the top of the overlap is a sticky bar that grew when the previous frame
   had detailed content there, or when those rows have the color of the bar above them (a header
   that changed height); otherwise mismatching rows are **stale**: rows of the previous frame that
   changed in place after it was captured (lazy content that finished loading). Stale rows are
   overwritten in the stitched body with this frame's version.
3. The first scroll (layout not known yet) tries the same relaxed search after the two plain ones.

New content of a step now starts right under the previous frame's band and ends at this frame's
footer, so a footer that grows or shrinks mid-capture neither duplicates nor drops rows.

### Scrollbar crop (`settle::scrollbar_width`, `StitchOptions::trim_scrollbar`, default on)

At `finish` the first and last frames are compared at the right edge: track-like columns (few
color runs top to bottom) whose only change is the thumb (one or two runs of rows, the same in
neighbouring columns), plus plain track-colored columns beside them, are cropped. Windows 11's
idle scrollbar is a 2 px thumb line in a 17 px track: 16 px are cropped (the 1 px lighter edge
column reads as page and stays). Without thumb movement nothing is cropped.

### Web pages

The web-page path is the ordinary scrolling capture: Ctrl+Alt+S (or `stinkshot scroll`), hover
the page and click. The overlay starts on the outermost UIA `Document` of the element under the
cursor (`hover::scroll_target_level`), which in Chrome, Edge and Firefox is the page without the
tab strip, address bar and window frame; the wheel walks out to the window. Sticky headers (also
ones that collapse once scrolled), smooth scrolling, lazy images and skeleton screens, and the
scrollbar are handled as above. No browser extension, no DevTools protocol, no network: only
wheel input to the page and screen capture, like any other window.

Not handled: overlay scrollbars that stay visible while still (they are captured as content at
the right edge), pages that scroll only inside an inner element smaller than the selection
(select that element instead), and content that keeps changing everywhere (a full-page video
background; each step then waits 2.5 s). Not verified on a real browser: this session sent no
input to other apps.

### Shakedown (Nov 8 2026)

`scroll_harness` gained `lazy` (each 100-row block shows a skeleton of three gray bars plus a
spinner for 400-1200 ms, random per block, after it first comes into view; `NO_SPINNER=1` makes
the skeleton static so only the placeholder check can catch it) and `web` (smooth scrolling, a
100 px header that collapses to 60 px once scrolled, lazy blocks; truth is the 100 px header +
document + footer). It runs `DriveParams::new` with the delay given (default 120 ms = the app
default), prints settle statistics and stitch repairs, and `STITCH_LOG=1` prints the driver and
stitcher debug lines. Release build, displays off (GDI capture), quiet machine:

| Mode | Delay | Result vs truth | Took | Settle median / max | Held for placeholders | Repairs |
|---|---|---|---|---|---|---|
| instant | 120 ms (2 runs) | 6100 rows identical (max diff 0), scrollbar cropped (800 -> 784 px) | 4.7 s | 53-54 / 295-296 ms | 0 | 0 |
| smooth | 120 ms (2 runs) | identical | 7.4 s | 198-199 / 245-253 ms | 0 | 0 |
| slow (static stripes, 400 ms) | 120 ms (2 runs) | identical | 10.4-10.5 s | 359-361 / 372-386 ms | 19 | 0 |
| lazy (skeleton + spinner, 0.4-1.2 s) | 120 ms (2 runs) | identical | 22.5 s | 989-1019 / 1182-1185 ms | 0 (the spinner keeps frames changing) | 0 |
| lazy, static skeleton | 120 ms | identical | 22.5 s | 997 / 1160 ms | 20 (first frame included) | 0 |
| web (smooth + collapsing header + lazy) | 120 ms (2 runs) | 6140 rows identical | 23.4-23.5 s | 1033-1037 / 1202-1204 ms | 0 | 1 (header 24 -> 60 rows on the first scroll) |
| web, static skeleton | 120 ms | identical | 23.6 s | 1051 / 1218 ms | 20 | 1 |
| all five modes | 30 ms (the minimum) | all identical | 2.9-23.3 s | | slow 19 | web 1 |

Every run ended through End with no settle timeout and no retry. capture_rect median 6.4-8.5 ms
(GDI with the displays off), max about 250 ms (the first call). During development, runs that
overlapped other sessions' builds (capture_rect median up to 210 ms, single captures up to 6 s)
still matched in every mode; they only took longer (up to 42 s for `lazy`).

Before (Nov 7): `slow` at 120 ms failed (placeholder stripes stitched in, then no overlap) and
needed 500 ms.

## Contract changes (Nov 8 2026)

- `DriveParams` gained `settle_max` and `retries` (struct literals must add them; use
  `DriveParams::new`); `DriveOutcome` gained `settle_ms`, `settle_timeouts`,
  `placeholder_waits`, `retried`, `captures`.
- `StitchOptions` gained `trim_scrollbar` (default true).
- New: `pub mod settle`, `Assessment`, `Stitcher::assess`, `Stitcher::accept_colors`,
  `Stitcher::repairs`.

## Contract changes

- Added `pub mod drive` (`drive`, `DriveParams`, `DriveControl`, `DriveOutcome`,
  `initial_notches`). No existing signature changed.

## Contract requests

None.

## Known limitations

- Vertical scroll down only. Scrolling up or horizontal scrolling is not handled (Failed).
- A sticky header/footer that keeps changing between frames (clock, animation) is not detected
  as fixed; the relaxed search then needs a third of the overlap to match elsewhere. One that
  changes height once (collapsing header, appearing bar) is handled (`repair`).
- A bar that appears at the bottom between two frames is first taken as stale rows (written into
  the body once), then detected as a footer on the next step (by design; not in the harness).
- Placeholders are recognised by shape (few runs, block in a new color). A placeholder that looks
  like content (a blurred low-res image that sharpens later) is not; it is fixed only when it
  sharpens while still inside the next frame's overlap (stale-row repair).
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
5. Window with smooth-scroll animation at the default delay: stitches (settle waits).
6. Long web page in Chrome or Edge: Ctrl+Alt+S, hover the page (the highlight is the page, not
   the tab strip), click. Sticky nav once at the top, lazy images loaded, no scrollbar stripe at
   the right edge, no browser chrome.
7. Infinite-scroll page: continues past the first bottom, ends at Maximum Height or Stop.
