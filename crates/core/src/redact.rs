//! Destructive pixel operations. All operate in place on the given rect (clipped).
//! These must destroy information: blur must be strong enough (and pixelate coarse enough)
//! that text is unrecoverable; blur additionally adds slight noise before blurring so
//! deconvolution can't reconstruct it.

use crate::geom::PxRect;
use crate::image::Rgba;

pub fn blur(img: &mut Rgba, rect: PxRect, radius: f32) {
    let _ = (img, rect, radius);
    todo!("core::redact::blur")
}

pub fn pixelate(img: &mut Rgba, rect: PxRect, block: u32) {
    let _ = (img, rect, block);
    todo!("core::redact::pixelate")
}

pub fn fill(img: &mut Rgba, rect: PxRect, color: [u8; 4]) {
    let _ = (img, rect, color);
    todo!("core::redact::fill")
}
