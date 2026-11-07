//! The scrolling-capture driver loop, with capture, scrolling and control injected so the
//! app and test harnesses run exactly the same steps.

use std::time::Duration;

use stinkshot_core::Rgba;

use crate::{PushResult, Stitcher};

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
    /// Wait after each scroll step before the next capture.
    pub delay: Duration,
    /// Safety limit on capture steps.
    pub max_steps: u32,
}

#[derive(Debug, Default)]
pub struct DriveOutcome {
    pub cancelled: bool,
    /// Matching failed; the result holds what was stitched before.
    pub failed: bool,
    /// Capture error that ended the loop.
    pub error: Option<String>,
    /// Capture steps taken.
    pub steps: u32,
    /// Wheel notches sent, in order (negative = down).
    pub scrolls: Vec<i32>,
}

/// Wheel notches for the first step: about half a region per step keeps a healthy overlap
/// for matching (a notch is about 100 to 120 logical px in most apps).
pub fn initial_notches(logical_height: f32) -> i32 {
    (logical_height / 220.0).round().clamp(1.0, 8.0) as i32
}

/// Capture -> push -> scroll -> wait, until `End`, `Failed`, a capture error, `Stop`,
/// `Cancel` or `max_steps`. The step size adapts: fewer notches when a step moved more
/// than 85% of the frame (too little overlap), more when it moved less than 30%.
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
    for _ in 0..params.max_steps {
        match control() {
            DriveControl::Cancel => {
                out.cancelled = true;
                return out;
            }
            DriveControl::Stop => break,
            DriveControl::Run => {}
        }
        let frame = match capture() {
            Ok(f) => f,
            Err(e) => {
                out.error = Some(e);
                break;
            }
        };
        out.steps += 1;
        let first = stitcher.frames() == 0;
        match stitcher.push(&frame) {
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
            PushResult::Unchanged => {}
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

    #[test]
    fn drives_to_the_end_and_matches_the_document() {
        let (w, vh, dh) = (64u32, 40u32, 400u32);
        let doc: Vec<Vec<u8>> = (0..dh).map(|y| doc_row(y, w)).collect();
        let pos = std::cell::Cell::new(0u32);
        let mut st = Stitcher::new(StitchOptions::default());
        let out = drive(
            &mut st,
            &DriveParams {
                logical_height: 440.0, // 2 notches
                delay: Duration::ZERO,
                max_steps: 200,
            },
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

    #[test]
    fn cancel_and_stop() {
        let mut st = Stitcher::new(StitchOptions::default());
        let p = DriveParams {
            logical_height: 400.0,
            delay: Duration::ZERO,
            max_steps: 10,
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
