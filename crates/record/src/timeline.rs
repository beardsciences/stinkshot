//! Recording timeline (pause/resume offsets) and constant-frame-rate scheduling.
//! Pure logic, no OS calls.

use std::ops::Range;
use std::time::{Duration, Instant};

/// Maps wall-clock instants to recording time. While paused the clock is frozen, and the
/// paused span is subtracted from every later reading, so the output has no gap.
#[derive(Clone, Debug)]
pub struct Timeline {
    start: Instant,
    paused_at: Option<Instant>,
    paused_total: Duration,
}

impl Timeline {
    pub fn new(start: Instant) -> Self {
        Self {
            start,
            paused_at: None,
            paused_total: Duration::ZERO,
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    pub fn pause(&mut self, now: Instant) {
        if self.paused_at.is_none() {
            self.paused_at = Some(now);
        }
    }

    pub fn resume(&mut self, now: Instant) {
        if let Some(p) = self.paused_at.take() {
            self.paused_total += now.saturating_duration_since(p);
        }
    }

    /// Recording time at wall-clock `now`.
    pub fn elapsed_at(&self, now: Instant) -> Duration {
        let end = match self.paused_at {
            Some(p) => now.min(p),
            None => now,
        };
        end.saturating_duration_since(self.start)
            .saturating_sub(self.paused_total)
    }
}

/// Presentation time of frame `index` at `fps`, in 100 ns units.
pub fn pts_hns(index: u64, fps: u32) -> i64 {
    ((index as u128 * 10_000_000u128) / fps.max(1) as u128) as i64
}

/// Constant-frame-rate scheduler. Frame `n` is due at `n / fps`. Each call to `due` returns
/// the frame indices that should be written now: more than one means the source delivered
/// nothing new (or we were late) and the last picture is duplicated to hold the rate.
#[derive(Clone, Debug)]
pub struct FrameScheduler {
    fps: u32,
    next: u64,
    max_catchup: u64,
}

impl FrameScheduler {
    pub fn new(fps: u32) -> Self {
        let fps = fps.max(1);
        Self {
            fps,
            next: 0,
            max_catchup: fps as u64 * 2,
        }
    }

    #[cfg(test)]
    pub fn next_index(&self) -> u64 {
        self.next
    }

    /// Frames due at recording time `elapsed`. If we are more than two seconds late the
    /// surplus is skipped rather than duplicated (the timeline simply jumps).
    pub fn due(&mut self, elapsed: Duration) -> Range<u64> {
        let count = (elapsed.as_nanos() * self.fps as u128 / 1_000_000_000u128) as u64 + 1;
        if count <= self.next {
            return self.next..self.next;
        }
        let start = self.next.max(count.saturating_sub(self.max_catchup));
        self.next = count;
        start..count
    }

    /// Recording time at which the next frame becomes due.
    pub fn next_deadline(&self) -> Duration {
        Duration::from_nanos((self.next as u128 * 1_000_000_000u128 / self.fps as u128) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_without_pause() {
        let t0 = Instant::now();
        let tl = Timeline::new(t0);
        assert_eq!(
            tl.elapsed_at(t0 + Duration::from_secs(3)),
            Duration::from_secs(3)
        );
    }

    #[test]
    fn timeline_pause_freezes_and_resume_offsets() {
        let t0 = Instant::now();
        let mut tl = Timeline::new(t0);
        tl.pause(t0 + Duration::from_secs(2));
        assert!(tl.is_paused());
        assert_eq!(
            tl.elapsed_at(t0 + Duration::from_secs(10)),
            Duration::from_secs(2)
        );
        tl.resume(t0 + Duration::from_secs(5));
        assert_eq!(
            tl.elapsed_at(t0 + Duration::from_secs(6)),
            Duration::from_secs(3)
        );
    }

    #[test]
    fn timeline_multiple_pauses_and_idempotence() {
        let t0 = Instant::now();
        let mut tl = Timeline::new(t0);
        tl.pause(t0 + Duration::from_secs(1));
        tl.pause(t0 + Duration::from_secs(2)); // ignored
        tl.resume(t0 + Duration::from_secs(3));
        tl.resume(t0 + Duration::from_secs(4)); // ignored
        tl.pause(t0 + Duration::from_secs(5));
        tl.resume(t0 + Duration::from_secs(8));
        assert_eq!(
            tl.elapsed_at(t0 + Duration::from_secs(10)),
            Duration::from_secs(10 - 2 - 3)
        );
    }

    #[test]
    fn timeline_reading_before_pause_instant() {
        let t0 = Instant::now();
        let mut tl = Timeline::new(t0);
        tl.pause(t0 + Duration::from_secs(4));
        assert_eq!(
            tl.elapsed_at(t0 + Duration::from_secs(1)),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn pts_exact_for_30_and_60() {
        assert_eq!(pts_hns(0, 30), 0);
        assert_eq!(pts_hns(30, 30), 10_000_000);
        assert_eq!(pts_hns(1, 60), 166_666);
        assert_eq!(pts_hns(60, 60), 10_000_000);
        assert_eq!(pts_hns(3, 30), 1_000_000);
    }

    #[test]
    fn scheduler_first_frame_is_immediate() {
        let mut s = FrameScheduler::new(30);
        assert_eq!(s.due(Duration::ZERO), 0..1);
        assert_eq!(s.due(Duration::from_millis(10)), 1..1);
    }

    #[test]
    fn scheduler_steady_cadence() {
        let mut s = FrameScheduler::new(30);
        let mut total = 0;
        for ms in (0..1000).step_by(5) {
            total += s.due(Duration::from_millis(ms)).count();
        }
        assert_eq!(total, 30);
    }

    #[test]
    fn scheduler_duplicates_when_late() {
        let mut s = FrameScheduler::new(10);
        assert_eq!(s.due(Duration::ZERO), 0..1);
        assert_eq!(s.due(Duration::from_millis(450)), 1..5);
        assert_eq!(s.next_deadline(), Duration::from_millis(500));
    }

    #[test]
    fn scheduler_caps_catchup() {
        let mut s = FrameScheduler::new(10);
        s.due(Duration::ZERO);
        let r = s.due(Duration::from_secs(100));
        assert_eq!(r.end, 1001);
        assert_eq!(r.end - r.start, 20);
    }

    #[test]
    fn scheduler_with_pause_is_continuous() {
        let t0 = Instant::now();
        let mut tl = Timeline::new(t0);
        let mut s = FrameScheduler::new(20);
        let mut written = 0u64;
        for ms in (0..500).step_by(10) {
            written += s.due(tl.elapsed_at(t0 + Duration::from_millis(ms))).count() as u64;
        }
        tl.pause(t0 + Duration::from_millis(500));
        for ms in (500..2500).step_by(10) {
            written += s.due(tl.elapsed_at(t0 + Duration::from_millis(ms))).count() as u64;
        }
        tl.resume(t0 + Duration::from_millis(2500));
        for ms in (2500..3000).step_by(10) {
            written += s.due(tl.elapsed_at(t0 + Duration::from_millis(ms))).count() as u64;
        }
        assert_eq!(written, 20);
        assert_eq!(s.next_index(), 20);
    }
}
