//! Post-processing for window captures: Win11 rounded corners with real alpha, drop shadow,
//! optional padded background. Applied after capture, before annotation.

use crate::image::Rgba;

#[derive(Clone, Debug, PartialEq)]
pub struct WindowFx {
    /// Corner radius in physical px (8 * monitor scale on Win11). 0 = none.
    pub corner_radius: f32,
    pub shadow: bool,
    /// Shadow blur radius in px; offset is (0, blur/3).
    pub shadow_blur: f32,
    pub shadow_opacity: f32,
    /// Padding around the window (px). Only used when `background` is Some.
    pub padding: u32,
    /// Solid padded background; None = transparent margin just big enough for the shadow.
    pub background: Option<[u8; 4]>,
}

/// Returns a new, larger image when shadow/padding apply.
pub fn apply_window_fx(window: &Rgba, fx: &WindowFx) -> Rgba {
    let _ = (window, fx);
    todo!("core::frame::apply_window_fx")
}

/// Zero alpha outside a rounded rectangle covering the whole image (anti-aliased edge).
pub fn round_corners(img: &mut Rgba, radius: f32) {
    let _ = (img, radius);
    todo!("core::frame::round_corners")
}
