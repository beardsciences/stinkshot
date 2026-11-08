//! Platform-independent core of Stinkshot.
//!
//! Everything here is pure Rust and unit-testable without Windows APIs.
//! Pixel convention everywhere: physical pixels, `Rgba` is straight (non-premultiplied)
//! RGBA8, row-major, tightly packed (`data.len() == width * height * 4`).

pub mod annotate;
pub mod beautify;
pub mod codes;
pub mod color;
pub mod config;
pub mod fonts;
pub mod frame;
pub mod geom;
pub mod image;
pub mod naming;
pub mod png_opt;
pub mod redact;
pub mod sensitive;
pub mod table;

pub use geom::{PtF, PxPoint, PxRect, RectF};
pub use image::Rgba;
