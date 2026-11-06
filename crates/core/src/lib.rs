//! Platform-independent core of StinkShot.
//!
//! Everything here is pure Rust and unit-testable without Windows APIs.
//! Pixel convention everywhere: physical pixels, `Rgba` is straight (non-premultiplied)
//! RGBA8, row-major, tightly packed (`data.len() == width * height * 4`).

pub mod annotate;
pub mod config;
pub mod fonts;
pub mod frame;
pub mod geom;
pub mod image;
pub mod naming;
pub mod redact;

pub use geom::{PtF, PxPoint, PxRect, RectF};
pub use image::Rgba;
