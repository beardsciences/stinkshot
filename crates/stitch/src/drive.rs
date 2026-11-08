//! The scrolling-capture driver loop, with capture, scrolling and control injected so the
//! app and test harnesses run exactly the same steps.

use std::time::{Duration, Instant};

use stinkshot_core::Rgba;

use crate::{PushResult, Stitcher, settle};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveControl {
    Run,
    /// Stop scrolling and keep what was captured.
    Stop,
    /// Abandon the capture.
    Cancel,
}

#[derive(Clone, Debug)]
pub struct DriveParams {
    /// Capture region height in logical px (physical / monitor scale): sets the first
    /// scroll step at about half the region.
    pub logical_height: f32,
    /// Minimum wait after each scroll step before the first capture of the next step.
    pub delay: Duration,
    /// Safety limit on capture steps.
    pub max_steps: u32,
    /// Longest wait for a step to settle (frames stop changing and no loading
    /// placeholders are left). Zero captures once per step after `delay`.
    pub settle_max: Duration,
    /// Captures of the same step that failed to match before the capture stops.
    pub retries: u32,
}

impl DriveParams {
    /// Defaults that need no tuning per page: smooth scrolling, spinners and lazy-loading
    /// content up to about 2.5 s per step are waited out.
    pub fn new(logical_height: f32, delay: Duration) -> Self {
        Self {
            logical_height,
            delay,
            max_steps: 800,
            settle_max: Duration::from_millis(2500),
            retries: 2,
        }
    }
}

#[derive(Debug, Default)]
pub struct DriveOutcome {
    pub cancelled: bool,
    /// Matching failed; the result holds what was stitched before.
    pub failed: bool,
    /// Capture error that ended the loop.
    pub error: Option<String>,
    /// Capture steps taken (settled frames pushed).
    pub steps: u32,
    /// Wheel notches sent, in order (negative = down).
    pub scrolls: Vec<i32>,
    /// Time each settle took, ms (one per push, retries included).
    pub settle_ms: Vec<u32>,
    /// Settles that hit `settle_max` while the page was still changing or loading.
    pub settle_timeouts: u32,
    /// Settles held back because new rows looked like loading placeholders.
    pub placeholder_waits: u32,
    /// Failed matches that a later capture of the same step fixed.
    pub retried: u32,
    /// Captures taken in total.
    pub captures: u32,
}

/// Wheel notches for the first step: about half a region per step keeps a healthy overlap
/// for matching (a notch is about 100 to 120 logical px in most apps).
pub fn initial_notches(logical_height: f32) -> i32 {
    (logical_height / 220.0).round().clamp(1.0, 8.0) as i32
}

/// Interval between captures while waiting for a step to settle.
const POLL: Duration = Duration::from_millis(40);
/// After a step that did not move, wait at least this long before the next capture
/// (infinite-scroll pages load more content at the bottom).
const END_GRACE: Duration = Duration::from_millis(700);

/// Why `settle` returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Settled {
    Stable,
    /// `settle_max` ran out; `small` when only a small area kept changing.
    Timeout {
        small: bool,
    },
}

struct SettledFrame {
    frame: Rgba,
    how: Settled,
    /// New rows looked like loading placeholders at some point.
    held: bool,
}

/// Captures until two consecutive frames match and the new rows do not look like loading
/// placeholders, or `max` runs out. Small changing areas count as stable once the page has
/// shown that it keeps a permanent animation (`ignore_small`).
fn settle(
    stitcher: &Stitcher,
    max: Duration,
    ignore_small: bool,
    capture: &mut impl FnMut() -> Result<Rgba, String>,
    control: &mut impl FnMut() -> DriveControl,
    out: &mut DriveOutcome,
) -> Result<SettledFrame, String> {
    let t0 = Instant::now();
    out.captures += 1;
    let mut prev = capture()?;
    let mut held = false;
    if max.is_zero() {
        return Ok(SettledFrame {
            frame: prev,
            how: Settled::Stable,
            held,
        });
    }
    loop {
        if control() != DriveControl::Run {
            return Ok(SettledFrame {
                frame: prev,
                how: Settled::Stable,
                held,
            });
        }
        std::thread::sleep(POLL);
        out.captures += 1;
        let cur = capture()?;
        let change = settle::diff(&prev, &cur, 6);
        let small = change.is_some_and(|c| c.bbox_area_share(cur.width, cur.height) <= 0.02);
        // Still means no pixel changed: a spinner arc moves only a few pixels per poll, and a
        // blinking caret rarely changes between two polls 40 ms apart.
        let still = change.is_some_and(|c| c.pixels == 0) || (ignore_small && small);
        if still {
            if stitcher.assess(&cur).looks_unloaded() {
                held = true;
            } else {
                return Ok(SettledFrame {
                    frame: cur,
                    how: Settled::Stable,
                    held,
                });
            }
        }
        if t0.elapsed() >= max {
            return Ok(SettledFrame {
                frame: cur,
                how: Settled::Timeout {
                    small: still || small,
                },
                held,
            });
        }
        prev = cur;
    }
}

/// Capture -> push -> scroll -> wait, until `End`, `Failed`, a capture error, `Stop`,
/// `Cancel` or `max_steps`. Each step waits until the page settles (`settle_max`): frames
/// stop changing (smooth scrolling, spinners) and the new rows hold no loading
/// placeholders. A step that fails to match is captured again (`retries`) before giving
/// up. The step size adapts: fewer notches when a step moved more than 85% of the frame
/// (too little overlap), more when it moved less than 30%.
pub fn drive(
    stitcher: &mut Stitcher,
    params: &DriveParams,
    mut capture: impl FnMut() -> Result<Rgba, String>,
    mut scroll: impl FnMut(i32),
    mut control: impl FnMut() -> DriveControl,
    mut progress: impl FnMut(&Stitcher, &Rgba),
) -> DriveOutcome {
    let mut out = DriveOutcome::default();
    let mut notches = initial_notches(params.logical_height);
    // Consecutive steps that timed out with only a small area changing: the page has a
    // permanent animation (video, carousel), which then no longer holds steps back.
    let mut small_timeouts = 0u32;
    let mut grace = false;
    'steps: for _ in 0..params.max_steps {
        match control() {
            DriveControl::Cancel => {
                out.cancelled = true;
                return out;
            }
            DriveControl::Stop => break,
            DriveControl::Run => {}
        }
        if grace {
            std::thread::sleep(END_GRACE.saturating_sub(params.delay));
        }
        let mut attempt = 0;
        let (frame, result) = loop {
            let t = Instant::now();
            let s = match settle(
                stitcher,
                params.settle_max,
                small_timeouts >= 2,
                &mut capture,
                &mut control,
                &mut out,
            ) {
                Ok(s) => s,
                Err(e) => {
                    out.error = Some(e);
                    break 'steps;
                }
            };
            out.settle_ms.push(t.elapsed().as_millis() as u32);
            log::debug!(
                "scroll: settled in {} ms ({:?}, placeholders seen {}), capture {}",
                t.elapsed().as_millis(),
                s.how,
                s.held,
                out.captures
            );
            out.placeholder_waits += s.held as u32;
            match s.how {
                Settled::Stable => small_timeouts = 0,
                Settled::Timeout { small } => {
                    out.settle_timeouts += 1;
                    small_timeouts = if small { small_timeouts + 1 } else { 0 };
                    if s.held {
                        stitcher.accept_colors(&s.frame);
                    }
                }
            }
            let first = stitcher.frames() == 0;
            let r = stitcher.push(&s.frame);
            if r == PushResult::Failed
                && attempt < params.retries
                && !first
                && control() == DriveControl::Run
            {
                attempt += 1;
                log::debug!("scroll: step did not match, capturing it again ({attempt})");
                std::thread::sleep(POLL * 4 * attempt);
                continue;
            }
            if attempt > 0 && r != PushResult::Failed {
                out.retried += 1;
            }
            break (s.frame, r);
        };
        out.steps += 1;
        let first = stitcher.frames() == 1 && matches!(result, PushResult::Appended { .. });
        grace = false;
        match result {
            PushResult::Appended { added_rows } => {
                if !first {
                    let ratio = added_rows as f32 / frame.height.max(1) as f32;
                    if ratio > 0.85 {
                        notches = (notches - 1).max(1);
                    } else if ratio < 0.3 && notches < 12 {
                        notches += 1;
                    }
                }
            }
            PushResult::Unchanged => grace = true,
            PushResult::End => break,
            PushResult::Failed => {
                out.failed = true;
                break;
            }
        }
        progress(stitcher, &frame);
        scroll(-notches);
        out.scrolls.push(-notches);
        std::thread::sleep(params.delay);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StitchOptions;
    use std::cell::{Cell, RefCell};

    /// A 40-row viewport over a 400-row document with random-ish rows; each notch scrolls
    /// 10 rows.
    fn doc_row(y: u32, w: u32) -> Vec<u8> {
        let mut r = Vec::with_capacity(w as usize * 4);
        let mut s = y.wrapping_mul(2_654_435_761).wrapping_add(12345);
        for _ in 0..w {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            r.extend_from_slice(&[(s & 0xFF) as u8, (s >> 8) as u8, (s >> 16) as u8, 255]);
        }
        r
    }

    fn quick(h: f32) -> DriveParams {
        DriveParams {
            max_steps: 200,
            settle_max: Duration::ZERO,
            ..DriveParams::new(h, Duration::ZERO)
        }
    }

    fn no_trim() -> StitchOptions {
        StitchOptions {
            trim_scrollbar: false,
            ..StitchOptions::default()
        }
    }

    #[test]
    fn drives_to_the_end_and_matches_the_document() {
        let (w, vh, dh) = (64u32, 40u32, 400u32);
        let doc: Vec<Vec<u8>> = (0..dh).map(|y| doc_row(y, w)).collect();
        let pos = Cell::new(0u32);
        let mut st = Stitcher::new(no_trim());
        let out = drive(
            &mut st,
            &quick(440.0), // 2 notches
            || {
                let mut img = Rgba::new(w, vh);
                for y in 0..vh {
                    let src = &doc[(pos.get() + y) as usize];
                    img.data[(y * w * 4) as usize..((y + 1) * w * 4) as usize].copy_from_slice(src);
                }
                Ok(img)
            },
            |n| pos.set((pos.get() as i32 - n * 10).clamp(0, (dh - vh) as i32) as u32),
            || DriveControl::Run,
            |_, _| {},
        );
        assert!(
            !out.failed && !out.cancelled && out.error.is_none(),
            "{out:?}"
        );
        let img = st.finish();
        assert_eq!(img.height, dh);
        for y in 0..dh {
            assert_eq!(
                &img.data[(y * w * 4) as usize..((y + 1) * w * 4) as usize],
                &doc[y as usize][..],
                "row {y}"
            );
        }
    }

    /// Lazy page: rows newly scrolled into view show a flat gray placeholder until three
    /// captures later. Waiting for settled, placeholder-free frames gives the true page.
    #[test]
    fn waits_out_placeholders() {
        let (w, vh, dh) = (64u32, 60u32, 600u32);
        let doc: Vec<Vec<u8>> = (0..dh).map(|y| doc_row(y, w)).collect();
        let pos = Cell::new(0u32);
        // Capture count at which each row became visible.
        let seen: RefCell<Vec<Option<u32>>> = RefCell::new(vec![None; dh as usize]);
        let n = Cell::new(0u32);
        let mut st = Stitcher::new(no_trim());
        let out = drive(
            &mut st,
            &DriveParams {
                max_steps: 200,
                settle_max: Duration::from_secs(5),
                ..DriveParams::new(440.0, Duration::ZERO)
            },
            || {
                n.set(n.get() + 1);
                let mut seen = seen.borrow_mut();
                let mut img = Rgba::new(w, vh);
                for y in 0..vh {
                    let dy = (pos.get() + y) as usize;
                    let at = *seen[dy].get_or_insert(n.get());
                    let dst = &mut img.data[(y * w * 4) as usize..((y + 1) * w * 4) as usize];
                    if n.get() - at >= 3 || at == 1 {
                        dst.copy_from_slice(&doc[dy]);
                    } else {
                        for px in dst.chunks_exact_mut(4) {
                            px.copy_from_slice(&[200, 200, 204, 255]);
                        }
                    }
                }
                Ok(img)
            },
            |k| pos.set((pos.get() as i32 - k * 10).clamp(0, (dh - vh) as i32) as u32),
            || DriveControl::Run,
            |_, _| {},
        );
        assert!(!out.failed && out.error.is_none(), "{out:?}");
        assert!(out.placeholder_waits > 0, "{out:?}");
        let img = st.finish();
        assert_eq!(img.height, dh);
        for y in 0..dh {
            assert_eq!(
                &img.data[(y * w * 4) as usize..((y + 1) * w * 4) as usize],
                &doc[y as usize][..],
                "row {y}"
            );
        }
    }

    #[test]
    fn cancel_and_stop() {
        let mut st = Stitcher::new(StitchOptions::default());
        let p = DriveParams {
            max_steps: 10,
            ..quick(400.0)
        };
        let out = drive(
            &mut st,
            &p,
            || Ok(Rgba::new(8, 8)),
            |_| {},
            || DriveControl::Cancel,
            |_, _| {},
        );
        assert!(out.cancelled);
        assert_eq!(out.steps, 0);
        let out = drive(
            &mut st,
            &p,
            || Err("boom".into()),
            |_| {},
            || DriveControl::Run,
            |_, _| {},
        );
        assert_eq!(out.error.as_deref(), Some("boom"));
    }

    #[test]
    fn initial_step_is_about_half_a_region() {
        assert_eq!(initial_notches(100.0), 1);
        assert_eq!(initial_notches(1080.0), 5);
        assert_eq!(initial_notches(5000.0), 8);
    }
}
