//! UI-element snapping via UI Automation.
//!
//! Our overlay window covers the screen, so `ElementFromPoint` would hit the overlay.
//! Instead: start from `ElementFromHandle(target top-level window)` (picked from the
//! z-ordered snapshot taken before the overlay appeared) and descend with a cached
//! ControlView TreeWalker choosing the child whose BoundingRectangle contains the point,
//! up to a depth limit. UIA calls can take 10-200 ms for some apps, so `ElementSnapper`
//! runs on its own MTA worker thread: the UI posts the latest point with `request` and
//! polls `latest` each frame; stale requests are coalesced. Results are cached per window
//! for the lifetime of the snapper (screen is frozen, so the tree is too).

use stinkshot_core::{PxPoint, PxRect};

use crate::Hwnd;

pub struct ElementSnapper {
    #[allow(dead_code)]
    inner: (),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SnapResult {
    pub point: PxPoint,
    /// Element rects from innermost to the top-level window, clipped to the window rect,
    /// deduplicated, each with at least 4x4 px. The UI shows `rects[0]` and lets the user
    /// cycle outward with the mouse wheel.
    pub rects: Vec<PxRect>,
}

impl ElementSnapper {
    pub fn new() -> anyhow::Result<ElementSnapper> {
        todo!("platform::uia::ElementSnapper::new")
    }
    /// Ask for elements under `p` inside top-level window `hwnd`. Non-blocking.
    pub fn request(&self, hwnd: Hwnd, p: PxPoint) {
        let _ = (hwnd, p);
        todo!("platform::uia::ElementSnapper::request")
    }
    /// Most recent completed result, if any. Non-blocking.
    pub fn latest(&self) -> Option<SnapResult> {
        todo!("platform::uia::ElementSnapper::latest")
    }
}
