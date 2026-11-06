//! Pure helpers for hover/snapping and small formatting jobs.

use stinkshot_core::{PxPoint, PxRect};

use crate::state::RegionPurpose;

/// Choose the rectangle to highlight under the cursor.
///
/// `rects` are UIA element rects from innermost to the top-level window; `level` is the
/// wheel-cycling depth (0 = innermost). Rects that do not contain the cursor (stale
/// results) are skipped. Falls back to the window rect, then the monitor rect.
/// Returns the rect and the effective level used (clamped to the available rects).
pub fn pick_snap_rect(
    rects: &[PxRect],
    level: usize,
    p: PxPoint,
    window: Option<PxRect>,
    monitor: Option<PxRect>,
) -> Option<(PxRect, usize)> {
    let valid: Vec<PxRect> = rects
        .iter()
        .copied()
        .filter(|r| r.contains(p) && r.w >= 4 && r.h >= 4)
        .collect();
    if !valid.is_empty() {
        let l = level.min(valid.len() - 1);
        return Some((valid[l], l));
    }
    window.filter(|r| !r.is_empty()).or(monitor).map(|r| (r, 0))
}

/// Next wheel level: scroll up (positive delta) goes outward (parent), down goes inward.
pub fn cycle_level(level: usize, wheel_delta_y: f32, max_level: usize) -> usize {
    if wheel_delta_y > 0.0 {
        (level + 1).min(max_level)
    } else if wheel_delta_y < 0.0 {
        level.saturating_sub(1)
    } else {
        level
    }
}

pub fn purpose_label(p: RegionPurpose) -> &'static str {
    match p {
        RegionPurpose::Ocr => "Select area to copy text",
        RegionPurpose::Scrolling => "Select area to scroll-capture",
        RegionPurpose::RecordMp4 => "Select area to record",
        RegionPurpose::RecordGif => "Select area to record as GIF",
    }
}

pub fn hex_string(rgb: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
}

/// Black or white, whichever is more readable on `bg` (used for step numbers).
pub fn contrast_text(bg: [u8; 4]) -> [u8; 3] {
    let lum = 0.2126 * bg[0] as f32 + 0.7152 * bg[1] as f32 + 0.0722 * bg[2] as f32;
    if lum > 150.0 {
        [0x14, 0x14, 0x14]
    } else {
        [0xff, 0xff, 0xff]
    }
}

/// Convert an overlay-local rect to virtual-desktop coordinates.
pub fn local_to_virtual(r: PxRect, bounds: PxRect) -> PxRect {
    r.translate(bounds.x, bounds.y)
}

/// Convert a virtual-desktop rect to overlay-local, clipped to the overlay.
pub fn virtual_to_local(r: PxRect, bounds: PxRect) -> Option<PxRect> {
    r.translate(-bounds.x, -bounds.y)
        .intersect(&PxRect::new(0, 0, bounds.w, bounds.h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_prefers_valid_element_levels() {
        let rects = [
            PxRect::new(10, 10, 50, 20),
            PxRect::new(0, 0, 200, 100),
            PxRect::new(0, 0, 800, 600),
        ];
        let p = PxPoint { x: 20, y: 15 };
        assert_eq!(
            pick_snap_rect(&rects, 0, p, None, None),
            Some((rects[0], 0))
        );
        assert_eq!(
            pick_snap_rect(&rects, 1, p, None, None),
            Some((rects[1], 1))
        );
        assert_eq!(
            pick_snap_rect(&rects, 9, p, None, None),
            Some((rects[2], 2))
        );
    }

    #[test]
    fn snap_skips_stale_rects_and_falls_back() {
        let rects = [PxRect::new(500, 500, 50, 20)];
        let p = PxPoint { x: 20, y: 15 };
        let win = PxRect::new(0, 0, 300, 300);
        let mon = PxRect::new(0, 0, 1920, 1080);
        assert_eq!(
            pick_snap_rect(&rects, 0, p, Some(win), Some(mon)),
            Some((win, 0))
        );
        assert_eq!(pick_snap_rect(&[], 0, p, None, Some(mon)), Some((mon, 0)));
        assert_eq!(pick_snap_rect(&[], 0, p, None, None), None);
        // Tiny rects are ignored.
        let tiny = [PxRect::new(0, 0, 3, 3)];
        assert_eq!(
            pick_snap_rect(&tiny, 0, PxPoint { x: 1, y: 1 }, Some(win), None),
            Some((win, 0))
        );
    }

    #[test]
    fn wheel_cycles_and_clamps() {
        assert_eq!(cycle_level(0, 1.0, 3), 1);
        assert_eq!(cycle_level(3, 1.0, 3), 3);
        assert_eq!(cycle_level(2, -1.0, 3), 1);
        assert_eq!(cycle_level(0, -5.0, 3), 0);
        assert_eq!(cycle_level(1, 0.0, 3), 1);
    }

    #[test]
    fn labels_and_colors() {
        assert_eq!(
            purpose_label(RegionPurpose::RecordMp4),
            "Select area to record"
        );
        assert_eq!(hex_string([0xce, 0x91, 0x78]), "#CE9178");
        assert_eq!(contrast_text([255, 255, 255, 255]), [0x14, 0x14, 0x14]);
        assert_eq!(contrast_text([0, 0, 0, 255]), [255, 255, 255]);
    }

    #[test]
    fn local_virtual_round_trip() {
        let bounds = PxRect::new(-1920, 0, 3840, 1080);
        let local = PxRect::new(100, 100, 50, 50);
        let v = local_to_virtual(local, bounds);
        assert_eq!(v, PxRect::new(-1820, 100, 50, 50));
        assert_eq!(virtual_to_local(v, bounds), Some(local));
        assert_eq!(virtual_to_local(PxRect::new(5000, 0, 10, 10), bounds), None);
    }
}
