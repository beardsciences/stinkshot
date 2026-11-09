//! Run an OS call that may never return on a helper thread, with a timeout, without piling
//! up blocked threads when it hangs every time.
//!
//! Found by the soak test (docs/notes/platform.md, "Soak test"): on machines where
//! `Windows.Graphics.Capture` session creation never returns, every window capture and every
//! recording after the 5-minute cooldown started one more helper thread that stayed blocked
//! forever, each holding a D3D11 device. A [`HangGate`] remembers helpers that timed out and
//! are still blocked, and skips the call while any is, so a hang costs one thread per process.

use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant};

const RUNNING: u8 = 0;
const DONE: u8 = 1;
const ABANDONED: u8 = 2;

/// Why [`HangGate::run`] gave no result.
#[derive(Debug)]
pub enum Hang {
    /// Not attempted: an earlier call is still blocked, or one hung within the cooldown.
    Skipped(&'static str),
    /// No result within the timeout; the helper thread is left blocked and counted.
    TimedOut,
    /// The helper thread panicked.
    Panicked,
    /// The helper thread could not be started.
    Spawn(std::io::Error),
}

impl std::fmt::Display for Hang {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Hang::Skipped(why) => write!(f, "skipped: {why}"),
            Hang::TimedOut => write!(f, "timed out"),
            Hang::Panicked => write!(f, "helper thread panicked"),
            Hang::Spawn(e) => write!(f, "spawn helper thread: {e}"),
        }
    }
}

impl std::error::Error for Hang {}

pub struct HangGate {
    blocked: AtomicUsize,
    last_hang: Mutex<Option<Instant>>,
    cooldown: Duration,
}

impl HangGate {
    /// `cooldown`: after a hang, how long calls stay skipped even once the blocked helper
    /// has returned.
    pub const fn new(cooldown: Duration) -> HangGate {
        HangGate {
            blocked: AtomicUsize::new(0),
            last_hang: Mutex::new(None),
            cooldown,
        }
    }

    /// Helper threads that timed out and have not returned yet.
    pub fn blocked(&self) -> usize {
        self.blocked.load(Ordering::Acquire)
    }

    /// Why a call would be skipped right now, if it would.
    pub fn skip_reason(&self) -> Option<&'static str> {
        if self.blocked() > 0 {
            return Some("an earlier call is still blocked");
        }
        let last = *self
            .last_hang
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        last.is_some_and(|t| t.elapsed() < self.cooldown)
            .then_some("an earlier call hung recently")
    }

    /// Run `f` on a new thread named `name` and wait up to `timeout` for its result.
    pub fn run<T, F>(&'static self, name: &str, timeout: Duration, f: F) -> Result<T, Hang>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        if let Some(why) = self.skip_reason() {
            return Err(Hang::Skipped(why));
        }
        let state = Arc::new(AtomicU8::new(RUNNING));
        let (tx, rx) = mpsc::channel();
        let st = state.clone();
        std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                // Mark done even if `f` panics, so a late panic still releases the count.
                struct Finish(&'static HangGate, Arc<AtomicU8>);
                impl Drop for Finish {
                    fn drop(&mut self) {
                        if self.1.swap(DONE, Ordering::AcqRel) == ABANDONED {
                            self.0.blocked.fetch_sub(1, Ordering::AcqRel);
                        }
                    }
                }
                let _finish = Finish(self, st);
                let _ = tx.send(f());
            })
            .map_err(Hang::Spawn)?;
        match rx.recv_timeout(timeout) {
            Ok(v) => Ok(v),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(Hang::Panicked),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if state
                    .compare_exchange(RUNNING, ABANDONED, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
                {
                    self.blocked.fetch_add(1, Ordering::AcqRel);
                    *self
                        .last_hang
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
                    Err(Hang::TimedOut)
                } else {
                    // It finished between the timeout and the check: take the result.
                    rx.recv().map_err(|_| Hang::Panicked)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate(cooldown: Duration) -> &'static HangGate {
        Box::leak(Box::new(HangGate::new(cooldown)))
    }

    #[test]
    fn returns_the_result() {
        let g = gate(Duration::from_secs(60));
        assert_eq!(g.run("t", Duration::from_secs(5), || 7).unwrap(), 7);
        assert_eq!(g.blocked(), 0);
        assert!(g.skip_reason().is_none());
    }

    #[test]
    fn a_hang_is_counted_once_and_later_calls_do_not_start_threads() {
        let g = gate(Duration::ZERO);
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let started = Arc::new(AtomicUsize::new(0));
        let s = started.clone();
        let r = g.run("t", Duration::from_millis(50), move || {
            s.fetch_add(1, Ordering::SeqCst);
            let _ = release_rx.recv();
            1
        });
        assert!(matches!(r, Err(Hang::TimedOut)));
        assert_eq!(g.blocked(), 1);
        for _ in 0..5 {
            let s = started.clone();
            let r = g.run("t", Duration::from_millis(50), move || {
                s.fetch_add(1, Ordering::SeqCst);
                2
            });
            assert!(matches!(r, Err(Hang::Skipped(_))));
        }
        assert_eq!(started.load(Ordering::SeqCst), 1);
        // The blocked call returns: the count drops and calls run again (no cooldown here).
        release_tx.send(()).unwrap();
        let t = Instant::now();
        while g.blocked() > 0 && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(g.blocked(), 0);
        assert_eq!(g.run("t", Duration::from_secs(5), || 3).unwrap(), 3);
    }

    #[test]
    fn cooldown_skips_after_the_blocked_call_returned() {
        let g = gate(Duration::from_secs(600));
        let r = g.run("t", Duration::from_millis(20), || {
            std::thread::sleep(Duration::from_millis(200));
        });
        assert!(matches!(r, Err(Hang::TimedOut)));
        let t = Instant::now();
        while g.blocked() > 0 && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(g.blocked(), 0);
        assert!(matches!(
            g.run("t", Duration::from_secs(1), || ()),
            Err(Hang::Skipped(_))
        ));
    }

    #[test]
    fn a_panic_is_reported_and_not_counted() {
        let g = gate(Duration::from_secs(60));
        let r: Result<(), Hang> = g.run("t", Duration::from_secs(5), || panic!("boom"));
        assert!(matches!(r, Err(Hang::Panicked)));
        assert_eq!(g.blocked(), 0);
        assert!(g.skip_reason().is_none());
    }
}
