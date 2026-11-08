//! Pure helpers for hover/snapping and small formatting jobs.

use stinkshot_core::{PxPoint, PxRect};
use stinkshot_platform::Hwnd;
use stinkshot_platform::occlusion::{self, Layer};
use stinkshot_platform::windows_enum::WinInfo;

use crate::state::RegionPurpose;

/// The window that is actually visible under the cursor in the frozen screen.
#[derive(Clone, Debug, PartialEq)]
pub struct HoverTarget {
    /// Full frame of the topmost window at the point.
    pub frame: PxRect,
    /// Window-level rects at the point, innermost first (`occlusion::window_levels`): its
    /// largest uncovered part when the point is in it, then the whole frame.
    pub levels: Vec<PxRect>,
    /// Window to ask UI Automation about; None for this process's own windows.
    pub uia: Option<Hwnd>,
    /// Frames of the windows above it (they may cover parts of its elements).
    pub occluders: Vec<PxRect>,
}

/// Topmost window at `p` over the full Z-ordered stack, with its occluders.
pub fn hover_target(layers: &[Layer], p: PxPoint) -> Option<HoverTarget> {
    let i = occlusion::top_layer_at(layers, p)?;
    let l = layers[i];
    let occluders = occlusion::occluders_above(layers, i);
    Some(HoverTarget {
        frame: l.rect,
        levels: occlusion::window_levels(l.rect, p, &occluders),
        uia: (!l.own).then_some(l.hwnd),
        occluders,
    })
}

/// Occlusion stack from a target-window snapshot (fallback when no full stack was taken).
pub fn layers_from_windows(windows: &[WinInfo]) -> Vec<Layer> {
    windows
        .iter()
        .map(|w| Layer {
            hwnd: w.hwnd,
            rect: w.rect,
            own: false,
        })
        .collect()
}

/// Whether a UIA result belongs to the window under the cursor: its outermost rect (the
/// window frame) must essentially match `frame`.
pub fn result_matches_window(result_top: Option<&PxRect>, frame: PxRect) -> bool {
    let Some(top) = result_top else {
        return false;
    };
    let grown = PxRect::new(frame.x - 3, frame.y - 3, frame.w + 6, frame.h + 6);
    grown
        .intersect(top)
        .is_some_and(|i| i.area() * 10 >= top.area() * 9)
}

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
    let valid: Vec<PxRect> = valid_snap_indices(rects, p)
        .into_iter()
        .map(|i| rects[i])
        .collect();
    if !valid.is_empty() {
        let l = level.min(valid.len() - 1);
        return Some((valid[l], l));
    }
    window.filter(|r| !r.is_empty()).or(monitor).map(|r| (r, 0))
}

/// Indices into `rects` that `pick_snap_rect` considers (contain `p`, at least 4x4), in
/// level order; `valid[level]` is the element a level refers to.
pub fn valid_snap_indices(rects: &[PxRect], p: PxPoint) -> Vec<usize> {
    rects
        .iter()
        .enumerate()
        .filter(|(_, r)| r.contains(p) && r.w >= 4 && r.h >= 4)
        .map(|(i, _)| i)
        .collect()
}

/// Scrolling capture: the element level to highlight first, the outermost Document in the
/// chain (innermost first). In Chrome, Edge and Firefox that is the web page without the
/// browser's tabs and toolbar; an inner document (an iframe, an editor) would scroll only
/// itself.
pub fn scroll_target_level(labels: &[stinkshot_platform::uia::ElementLabel]) -> Option<usize> {
    labels.iter().rposition(|l| l.control == "Document")
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
        RegionPurpose::Ocr => "Select Area to Copy Text",
        RegionPurpose::Scrolling => "Select Area to Scroll-Capture",
        RegionPurpose::RecordMp4 => "Select Area to Record",
        RegionPurpose::RecordGif => "Select Area to Record as GIF",
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
    fn scrolling_starts_at_the_outer_document() {
        use stinkshot_platform::uia::ElementLabel;
        let l = |c: &'static str| ElementLabel {
            control: c,
            name: String::new(),
        };
        let chain = [
            l("Text"),
            l("Document"),
            l("Group"),
            l("Document"),
            l("Pane"),
            l("Window"),
        ];
        assert_eq!(scroll_target_level(&chain), Some(3));
        assert_eq!(scroll_target_level(&[l("Button"), l("Window")]), None);
    }

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
    fn valid_indices_line_up_with_levels() {
        let rects = [
            PxRect::new(500, 500, 50, 20),
            PxRect::new(10, 10, 50, 20),
            PxRect::new(0, 0, 2, 2),
            PxRect::new(0, 0, 800, 600),
        ];
        let p = PxPoint { x: 20, y: 15 };
        let v = valid_snap_indices(&rects, p);
        assert_eq!(v, vec![1, 3]);
        for (level, idx) in v.iter().enumerate() {
            assert_eq!(
                pick_snap_rect(&rects, level, p, None, None),
                Some((rects[*idx], level))
            );
        }
    }

    fn layer(hwnd: Hwnd, r: PxRect, own: bool) -> Layer {
        Layer { hwnd, rect: r, own }
    }

    #[test]
    fn hover_targets_the_topmost_window_and_clips_it() {
        // An untitled popup menu (1) over a browser (2); our History window (3) elsewhere.
        let layers = [
            layer(1, PxRect::new(100, 100, 200, 300), false),
            layer(3, PxRect::new(1000, 0, 400, 400), true),
            layer(2, PxRect::new(0, 0, 1920, 1080), false),
        ];
        // Over the popup: the popup itself, not the browser behind it.
        let t = hover_target(&layers, PxPoint { x: 150, y: 150 }).unwrap();
        assert_eq!(t.uia, Some(1));
        assert_eq!(t.levels, vec![PxRect::new(100, 100, 200, 300)]);
        // Over our own window: a window-level target, no UIA.
        let t = hover_target(&layers, PxPoint { x: 1100, y: 100 }).unwrap();
        assert_eq!(t.uia, None);
        assert_eq!(t.frame, PxRect::new(1000, 0, 400, 400));
        // Over the browser below the popup: browser, clipped so it never covers the popup,
        // then the whole browser window.
        let p = PxPoint { x: 50, y: 500 };
        let t = hover_target(&layers, p).unwrap();
        assert_eq!(t.uia, Some(2));
        assert_eq!(t.levels.len(), 2);
        let v = t.levels[0];
        assert!(v.contains(p));
        for o in &t.occluders {
            assert!(v.intersect(o).is_none());
        }
        assert_eq!(t.levels[1], t.frame);
        assert!(hover_target(&layers, PxPoint { x: 5000, y: 5000 }).is_none());
    }

    #[test]
    fn stale_results_from_another_window_are_rejected() {
        let frame = PxRect::new(1920, 0, 1920, 2100);
        assert!(result_matches_window(Some(&frame), frame));
        // DWM frame vs UIA rect off by a pixel or two still matches.
        assert!(result_matches_window(
            Some(&PxRect::new(1919, 0, 1922, 2100)),
            frame
        ));
        // The browser behind it (twice as wide) does not.
        assert!(!result_matches_window(
            Some(&PxRect::new(0, 0, 3840, 2100)),
            frame
        ));
        assert!(!result_matches_window(None, frame));
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
            "Select Area to Record"
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
