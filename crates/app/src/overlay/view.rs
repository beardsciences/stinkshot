//! Editor zoom and pan: a pure transform between screen px (overlay-local physical px, what
//! the window shows) and canvas px (overlay-local physical px of the frozen screen, the space
//! the selection and every gesture work in). Identity is the normal, unzoomed view.
//!
//! `screen = off + zoom * canvas`.

/// Image zoom range (image px per screen px), 25 % to 800 %.
pub const MIN_IMAGE_ZOOM: f32 = 0.25;
pub const MAX_IMAGE_ZOOM: f32 = 8.0;
/// One Ctrl+wheel notch or Ctrl+= / Ctrl+- press.
pub const STEP: f32 = 1.25;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub zoom: f32,
    pub off: (f32, f32),
}

impl Default for View {
    fn default() -> Self {
        View::IDENTITY
    }
}

impl View {
    pub const IDENTITY: View = View {
        zoom: 1.0,
        off: (0.0, 0.0),
    };

    pub fn is_identity(self) -> bool {
        (self.zoom - 1.0).abs() < 1e-4 && self.off.0.abs() < 0.01 && self.off.1.abs() < 0.01
    }

    pub fn to_screen(self, p: (f32, f32)) -> (f32, f32) {
        (self.off.0 + p.0 * self.zoom, self.off.1 + p.1 * self.zoom)
    }

    pub fn to_canvas(self, s: (f32, f32)) -> (f32, f32) {
        (
            (s.0 - self.off.0) / self.zoom,
            (s.1 - self.off.1) / self.zoom,
        )
    }

    /// Change the zoom to `zoom`, keeping the canvas point under `anchor` (screen px) still.
    pub fn zoom_at(self, anchor: (f32, f32), zoom: f32) -> View {
        let c = self.to_canvas(anchor);
        View {
            zoom,
            off: (anchor.0 - c.0 * zoom, anchor.1 - c.1 * zoom),
        }
    }

    /// Move the view by a screen-px delta.
    pub fn pan(self, d: (f32, f32)) -> View {
        View {
            zoom: self.zoom,
            off: (self.off.0 + d.0, self.off.1 + d.1),
        }
    }

    /// Screen rect (x, y, w, h) of a canvas rect.
    pub fn rect_to_screen(self, r: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
        let (x, y) = self.to_screen((r.0, r.1));
        (x, y, r.2 * self.zoom, r.3 * self.zoom)
    }
}

/// Allowed view zoom range for an image shown at `scale` canvas px per image px: image zoom
/// 25 %..800 %, never cutting off the fitted view (a fit below 25 % stays reachable).
pub fn zoom_limits(scale: f32) -> (f32, f32) {
    let s = scale.max(1e-3);
    let min = (MIN_IMAGE_ZOOM / s).min(1.0);
    let max = (MAX_IMAGE_ZOOM / s).max(1.0);
    (min, max)
}

/// Zoom by `factor` around `anchor` within the limits for `scale`. Steps that land within 3 %
/// of the fitted view snap onto it, so wheeling back always returns to exactly "Fit".
pub fn zoom_by(v: View, anchor: (f32, f32), factor: f32, scale: f32) -> View {
    let (lo, hi) = zoom_limits(scale);
    let mut z = (v.zoom * factor).clamp(lo, hi);
    if (z - 1.0).abs() < 0.03 {
        z = 1.0;
    }
    if (z - 1.0).abs() < 1e-4 {
        // Back at fit size: return to the exact home position too.
        return View::IDENTITY;
    }
    v.zoom_at(anchor, z)
}

/// Ctrl+1: one image px per screen px, centred on `anchor` (screen px).
pub fn actual_size(v: View, anchor: (f32, f32), scale: f32) -> View {
    let z = 1.0 / scale.max(1e-3);
    if (z - 1.0).abs() < 1e-4 {
        return View::IDENTITY;
    }
    v.zoom_at(anchor, z)
}

/// Keep at least `keep` screen px of the canvas rect `r` (x, y, w, h) inside `screen`
/// (x, y, w, h), so panning can not lose the image.
pub fn clamp_pan(
    v: View,
    r: (f32, f32, f32, f32),
    screen: (f32, f32, f32, f32),
    keep: f32,
) -> View {
    let (x, y, w, h) = v.rect_to_screen(r);
    let keep_x = keep.min(w);
    let keep_y = keep.min(h);
    let mut dx = 0.0;
    let mut dy = 0.0;
    if x + w < screen.0 + keep_x {
        dx = screen.0 + keep_x - (x + w);
    } else if x > screen.0 + screen.2 - keep_x {
        dx = screen.0 + screen.2 - keep_x - x;
    }
    if y + h < screen.1 + keep_y {
        dy = screen.1 + keep_y - (y + h);
    } else if y > screen.1 + screen.3 - keep_y {
        dy = screen.1 + screen.3 - keep_y - y;
    }
    v.pan((dx, dy))
}

/// Text for the zoom chip: image zoom in percent.
pub fn percent(v: View, scale: f32) -> i32 {
    (v.zoom * scale * 100.0).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3
    }

    #[test]
    fn round_trip_and_identity() {
        let v = View {
            zoom: 2.5,
            off: (-300.0, 40.0),
        };
        let p = (123.0, 456.0);
        assert!(close(v.to_canvas(v.to_screen(p)), p));
        assert!(View::IDENTITY.is_identity());
        assert_eq!(View::IDENTITY.to_screen(p), p);
        assert!(!v.is_identity());
    }

    #[test]
    fn zoom_keeps_the_point_under_the_cursor() {
        let v = View::IDENTITY;
        let anchor = (800.0, 500.0);
        let before = v.to_canvas(anchor);
        let z = zoom_by(v, anchor, 2.0, 1.0);
        assert!((z.zoom - 2.0).abs() < 1e-5);
        assert!(close(z.to_canvas(anchor), before));
        let z2 = zoom_by(z, (100.0, 900.0), 1.5, 1.0);
        assert!(close(
            z2.to_canvas((100.0, 900.0)),
            z.to_canvas((100.0, 900.0))
        ));
    }

    #[test]
    fn zoom_is_limited_to_25_to_800_percent_of_image_px() {
        // Capture: canvas px are image px.
        let mut v = View::IDENTITY;
        for _ in 0..40 {
            v = zoom_by(v, (0.0, 0.0), STEP, 1.0);
        }
        assert!((v.zoom - 8.0).abs() < 1e-4);
        for _ in 0..80 {
            v = zoom_by(v, (0.0, 0.0), 1.0 / STEP, 1.0);
        }
        assert!((v.zoom - 0.25).abs() < 1e-4);
        assert_eq!(percent(v, 1.0), 25);
        // Editor showing a 4K image at 45 %: 800 % of image px is a view zoom of 17.8.
        let (lo, hi) = zoom_limits(0.45);
        assert!((hi - 8.0 / 0.45).abs() < 1e-3);
        assert!((lo - 0.25 / 0.45).abs() < 1e-3);
        // A fit below 25 % (huge image) keeps the fitted view reachable.
        assert_eq!(zoom_limits(0.1).0, 1.0);
    }

    #[test]
    fn wheeling_back_snaps_home() {
        let a = (640.0, 360.0);
        let v = zoom_by(View::IDENTITY, a, STEP, 1.0);
        let back = zoom_by(v, (10.0, 10.0), 1.0 / STEP, 1.0);
        assert_eq!(back, View::IDENTITY);
    }

    #[test]
    fn actual_size_shows_one_image_px_per_screen_px() {
        let v = actual_size(View::IDENTITY, (500.0, 300.0), 0.5);
        assert!((v.zoom * 0.5 - 1.0).abs() < 1e-5);
        assert_eq!(percent(v, 0.5), 100);
        assert_eq!(actual_size(v, (0.0, 0.0), 1.0), View::IDENTITY);
    }

    #[test]
    fn pan_is_clamped_to_keep_the_image_visible() {
        let screen = (0.0, 0.0, 1920.0, 1080.0);
        let img = (100.0, 100.0, 800.0, 600.0);
        let v = View::IDENTITY.pan((-5000.0, 3000.0));
        let c = clamp_pan(v, img, screen, 48.0);
        let (x, y, w, h) = c.rect_to_screen(img);
        assert!((x + w - 48.0).abs() < 1e-3, "{x} {w}");
        assert!((y - (1080.0 - 48.0)).abs() < 1e-3, "{y} {h}");
        // Inside: unchanged.
        let ok = View::IDENTITY.pan((10.0, 10.0));
        assert_eq!(clamp_pan(ok, img, screen, 48.0), ok);
    }
}
