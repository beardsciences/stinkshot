//! Placement of the combined tool bar + action bar ("the toolbar block") once the user drags
//! it by its grip. Pure geometry plus a session-wide memory of where it was left.
//!
//! - Docked (default): `selection::layout_bars` places the bars around the selection and
//!   they follow it.
//! - Undocked: the user dragged the grip (moved it past `UNDOCK_THRESHOLD_PX`; a press that
//!   does not move leaves it docked). The block keeps its own origin (top-left of the tool
//!   bar) and no longer follows the selection. It is clamped to the monitor it is on (it can
//!   be dragged to another monitor, never off-screen). Double-clicking the grip docks it
//!   again.
//! - The undocked origin is remembered in virtual-desktop px for the rest of the session, so
//!   the next capture opens with the bar where it was left (not across restarts). A later
//!   overlay uses it only while the selection is on the same monitor (else the bar docks), and
//!   the editor (Annotate) always opens docked.
//!
//! All values are physical px; overlay-local unless the name says virtual.

use std::sync::Mutex;

use stinkshot_core::PxRect;

/// Undocked block origin in virtual-desktop px for this session (None = docked).
static REMEMBERED: Mutex<Option<(i32, i32)>> = Mutex::new(None);

/// Where the bar was left in an earlier overlay of this session (virtual px).
pub fn remembered() -> Option<(i32, i32)> {
    REMEMBERED.lock().map(|g| *g).unwrap_or(None)
}

/// Remember (Some) or forget (None, docked) the undocked origin for later overlays.
pub fn remember(origin_virtual: Option<(i32, i32)>) {
    if let Ok(mut g) = REMEMBERED.lock() {
        *g = origin_virtual;
    }
}

/// Offset of the action bar from the block origin and the block's size. Side by side when
/// both fit in `avail_w`, otherwise the action bar is stacked under the tool bar.
pub fn block_layout(
    tool: (f32, f32),
    action: (f32, f32),
    gap: f32,
    avail_w: f32,
) -> ((f32, f32), (f32, f32)) {
    let side_w = tool.0 + gap + action.0;
    if side_w <= avail_w {
        ((tool.0 + gap, 0.0), (side_w, tool.1.max(action.1)))
    } else {
        (
            (0.0, tool.1 + gap),
            (tool.0.max(action.0), tool.1 + gap + action.1),
        )
    }
}

/// The monitor a block at `origin` with `size` belongs to: the one holding its centre, else
/// the one it overlaps most, else the nearest one. None only for an empty list.
pub fn monitor_for_block(
    monitors: &[PxRect],
    origin: (f32, f32),
    size: (f32, f32),
) -> Option<PxRect> {
    let c = (origin.0 + size.0 / 2.0, origin.1 + size.1 / 2.0);
    let contains = |m: &PxRect| {
        c.0 >= m.x as f32 && c.0 < m.right() as f32 && c.1 >= m.y as f32 && c.1 < m.bottom() as f32
    };
    if let Some(m) = monitors.iter().find(|m| contains(m)) {
        return Some(*m);
    }
    let overlap = |m: &PxRect| {
        let w = (origin.0 + size.0).min(m.right() as f32) - origin.0.max(m.x as f32);
        let h = (origin.1 + size.1).min(m.bottom() as f32) - origin.1.max(m.y as f32);
        w.max(0.0) * h.max(0.0)
    };
    let best = monitors
        .iter()
        .map(|m| (overlap(m), *m))
        .filter(|(a, _)| *a > 0.0)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, m)| m);
    if best.is_some() {
        return best;
    }
    let dist = |m: &PxRect| {
        let dx = (m.x as f32 - c.0).max(c.0 - m.right() as f32).max(0.0);
        let dy = (m.y as f32 - c.1).max(c.1 - m.bottom() as f32).max(0.0);
        dx * dx + dy * dy
    };
    monitors
        .iter()
        .min_by(|a, b| dist(a).total_cmp(&dist(b)))
        .copied()
}

/// Keep a block of `size` at `origin` fully inside `mon` with `pad` px of margin. A block
/// bigger than the monitor sticks to its left / top edge.
pub fn clamp_origin(origin: (f32, f32), size: (f32, f32), mon: PxRect, pad: f32) -> (f32, f32) {
    let clamp_axis = |v: f32, len: f32, lo: f32, hi: f32| {
        let max = hi - pad - len;
        let min = lo + pad;
        if max < min { min } else { v.clamp(min, max) }
    };
    (
        clamp_axis(origin.0, size.0, mon.x as f32, mon.right() as f32),
        clamp_axis(origin.1, size.1, mon.y as f32, mon.bottom() as f32),
    )
}

/// Undocked placement: the monitor the block is on, its layout there, and the clamped
/// positions. Returns (tool bar pos, action bar pos, monitor), all overlay-local px.
pub fn undocked_layout(
    origin: (f32, f32),
    tool: (f32, f32),
    action: (f32, f32),
    gap: f32,
    monitors: &[PxRect],
    fallback: PxRect,
) -> ((f32, f32), (f32, f32), PxRect) {
    // Pick the monitor with the block as it would be laid out on a monitor as wide as the
    // fallback, then lay it out for the chosen monitor's width and clamp.
    let (_, rough) = block_layout(tool, action, gap, fallback.w as f32 - 2.0 * gap);
    let mon = monitor_for_block(monitors, origin, rough).unwrap_or(fallback);
    let (off, size) = block_layout(tool, action, gap, mon.w as f32 - 2.0 * gap);
    let o = clamp_origin(origin, size, mon, gap);
    (o, (o.0 + off.0, o.1 + off.1), mon)
}

/// Whether a spot remembered from an earlier overlay should be used for a selection on
/// `sel_mon`: only when the block would land on that same monitor. Otherwise the bar docks
/// (a spot on another monitor would leave the tools a screen away from the selection).
pub fn remembered_applies(
    origin: (f32, f32),
    tool: (f32, f32),
    action: (f32, f32),
    gap: f32,
    monitors: &[PxRect],
    sel_mon: PxRect,
) -> bool {
    let (_, rough) = block_layout(tool, action, gap, sel_mon.w as f32 - 2.0 * gap);
    monitor_for_block(monitors, origin, rough) == Some(sel_mon)
}

/// Pointer travel (physical px) before a grip press counts as a drag that undocks the bar.
/// egui also reports a long press without movement as a drag start.
pub const UNDOCK_THRESHOLD_PX: f32 = 3.0;

/// A grip drag in progress: where the pointer and the block origin were when it started, and
/// whether the pointer has moved far enough to undock the bar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GripDrag {
    pub start_pointer: (f32, f32),
    pub start_origin: (f32, f32),
    pub moved: bool,
}

impl GripDrag {
    pub fn new(start_pointer: (f32, f32), start_origin: (f32, f32)) -> Self {
        Self {
            start_pointer,
            start_origin,
            moved: false,
        }
    }

    /// Mark the drag as moved once the pointer is more than `threshold` px from its start.
    /// Returns whether it has moved (now or earlier).
    pub fn track(&mut self, pointer: (f32, f32), threshold: f32) -> bool {
        if !self.moved {
            let dx = pointer.0 - self.start_pointer.0;
            let dy = pointer.1 - self.start_pointer.1;
            self.moved = dx * dx + dy * dy > threshold * threshold;
        }
        self.moved
    }

    /// Unclamped origin for the current pointer (clamping happens at layout, so dragging
    /// past an edge and back does not drift).
    pub fn target(&self, pointer: (f32, f32)) -> (f32, f32) {
        (
            self.start_origin.0 + pointer.0 - self.start_pointer.0,
            self.start_origin.1 + pointer.1 - self.start_pointer.1,
        )
    }
}

/// Convert between overlay-local and virtual px (`bounds` = the overlay's virtual rect).
pub fn to_virtual(local: (f32, f32), bounds: PxRect) -> (i32, i32) {
    (
        local.0.round() as i32 + bounds.x,
        local.1.round() as i32 + bounds.y,
    )
}

pub fn to_local(virt: (i32, i32), bounds: PxRect) -> (f32, f32) {
    ((virt.0 - bounds.x) as f32, (virt.1 - bounds.y) as f32)
}

/// X of a popup of `width` centred on `anchor_cx`, kept inside [`left`, `right`] with `pad`.
pub fn popup_x(anchor_cx: f32, width: f32, left: f32, right: f32, pad: f32) -> f32 {
    let half = width / 2.0;
    let min = left + pad + half;
    let max = right - pad - half;
    if max < min {
        min
    } else {
        anchor_cx.clamp(min, max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEFT: PxRect = PxRect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1080,
    };
    const RIGHT: PxRect = PxRect {
        x: 1920,
        y: -200,
        w: 2560,
        h: 1440,
    };

    #[test]
    fn block_side_by_side_or_stacked() {
        let (off, size) = block_layout((700.0, 46.0), (280.0, 46.0), 8.0, 1900.0);
        assert_eq!(off, (708.0, 0.0));
        assert_eq!(size, (988.0, 46.0));
        let (off, size) = block_layout((700.0, 46.0), (280.0, 46.0), 8.0, 900.0);
        assert_eq!(off, (0.0, 54.0));
        assert_eq!(size, (700.0, 100.0));
    }

    #[test]
    fn monitor_by_centre_then_overlap_then_nearest() {
        let mons = [LEFT, RIGHT];
        assert_eq!(
            monitor_for_block(&mons, (100.0, 100.0), (500.0, 50.0)),
            Some(LEFT)
        );
        // Centre on the right monitor.
        assert_eq!(
            monitor_for_block(&mons, (1800.0, 100.0), (500.0, 50.0)),
            Some(RIGHT)
        );
        // Centre in the gap above the left monitor (y < 0): the right monitor overlaps.
        assert_eq!(
            monitor_for_block(&mons, (1700.0, -150.0), (400.0, 40.0)),
            Some(RIGHT)
        );
        // Far away: nearest.
        assert_eq!(
            monitor_for_block(&mons, (-900.0, 500.0), (100.0, 40.0)),
            Some(LEFT)
        );
        assert_eq!(monitor_for_block(&[], (0.0, 0.0), (1.0, 1.0)), None);
    }

    #[test]
    fn clamp_keeps_block_on_its_monitor() {
        let size = (988.0, 46.0);
        assert_eq!(clamp_origin((-50.0, -30.0), size, LEFT, 8.0), (8.0, 8.0));
        assert_eq!(
            clamp_origin((1500.0, 1070.0), size, LEFT, 8.0),
            (1920.0 - 8.0 - 988.0, 1080.0 - 8.0 - 46.0)
        );
        assert_eq!(
            clamp_origin((300.0, 400.0), size, LEFT, 8.0),
            (300.0, 400.0)
        );
        // Bigger than the monitor: left / top edge.
        let small = PxRect::new(0, 0, 600, 40);
        assert_eq!(clamp_origin((100.0, 100.0), size, small, 8.0), (8.0, 8.0));
    }

    #[test]
    fn undocked_moves_to_other_monitor_and_never_off_screen() {
        let mons = [LEFT, RIGHT];
        let tool = (700.0, 46.0);
        let act = (280.0, 46.0);
        // Dragged mostly onto the right monitor: clamped into it, not left straddling.
        let (t, a, m) = undocked_layout((1700.0, 300.0), tool, act, 8.0, &mons, LEFT);
        assert_eq!(m, RIGHT);
        assert_eq!(t, (1928.0, 300.0));
        assert_eq!(a, (1928.0 + 708.0, 300.0));
        // Dragged past the bottom right corner of everything.
        let (t, _, m) = undocked_layout((9000.0, 9000.0), tool, act, 8.0, &mons, LEFT);
        assert_eq!(m, RIGHT);
        assert_eq!(
            t,
            (1920.0 + 2560.0 - 8.0 - 988.0, -200.0 + 1440.0 - 8.0 - 46.0)
        );
        // A narrow monitor stacks the action bar under the tool bar.
        let narrow = PxRect::new(0, 0, 800, 600);
        let (t, a, _) = undocked_layout((10.0, 10.0), tool, act, 8.0, &[narrow], narrow);
        assert_eq!(t, (10.0, 10.0));
        assert_eq!(a, (10.0, 64.0));
    }

    #[test]
    fn grip_drag_target_is_unclamped_delta() {
        let d = GripDrag::new((100.0, 100.0), (40.0, 900.0));
        assert_eq!(d.target((100.0, 100.0)), (40.0, 900.0));
        assert_eq!(d.target((-500.0, 130.0)), (-560.0, 930.0));
    }

    #[test]
    fn grip_undocks_only_after_real_movement() {
        let mut d = GripDrag::new((100.0, 100.0), (40.0, 900.0));
        // A long press without movement, or jitter within the threshold.
        assert!(!d.track((100.0, 100.0), UNDOCK_THRESHOLD_PX));
        assert!(!d.track((102.0, 101.0), UNDOCK_THRESHOLD_PX));
        assert!(d.track((104.0, 100.0), UNDOCK_THRESHOLD_PX));
        // Stays moved even when the pointer comes back.
        assert!(d.track((100.0, 100.0), UNDOCK_THRESHOLD_PX));
    }

    #[test]
    fn remembered_spot_only_on_the_selection_monitor() {
        let mons = [LEFT, RIGHT];
        let tool = (700.0, 46.0);
        let act = (280.0, 46.0);
        assert!(remembered_applies(
            (100.0, 900.0),
            tool,
            act,
            8.0,
            &mons,
            LEFT
        ));
        assert!(!remembered_applies(
            (100.0, 900.0),
            tool,
            act,
            8.0,
            &mons,
            RIGHT
        ));
        assert!(remembered_applies(
            (2200.0, 500.0),
            tool,
            act,
            8.0,
            &mons,
            RIGHT
        ));
    }

    #[test]
    fn virtual_round_trip_and_session_memory() {
        let bounds = PxRect::new(-1920, -200, 4480, 1440);
        let v = to_virtual((2000.5, 300.0), bounds);
        assert_eq!(v, (81, 100));
        assert_eq!(to_local(v, bounds), (2001.0, 300.0));
        remember(Some(v));
        assert_eq!(remembered(), Some(v));
        remember(None);
        assert_eq!(remembered(), None);
    }

    #[test]
    fn popups_stay_on_screen() {
        assert_eq!(popup_x(500.0, 272.0, 0.0, 1920.0, 8.0), 500.0);
        assert_eq!(popup_x(20.0, 272.0, 0.0, 1920.0, 8.0), 144.0);
        assert_eq!(popup_x(1910.0, 272.0, 0.0, 1920.0, 8.0), 1776.0);
    }
}
