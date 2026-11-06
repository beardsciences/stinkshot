use stinkshot_core::{PxPoint, PxRect};

#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    /// HMONITOR value.
    pub handle: isize,
    /// Full monitor rect, physical px, virtual-desktop coords.
    pub rect: PxRect,
    /// Work area (excludes taskbar).
    pub work: PxRect,
    /// DPI scale, 1.0 = 96 dpi.
    pub scale: f32,
    /// Device name like "\\.\DISPLAY1".
    pub name: String,
    pub primary: bool,
}

pub fn list() -> Vec<Monitor> {
    todo!("platform::monitors::list")
}

/// Bounding rect of all monitors.
pub fn virtual_bounds() -> PxRect {
    todo!("platform::monitors::virtual_bounds")
}

pub fn at_point(p: PxPoint) -> Option<Monitor> {
    let _ = p;
    todo!("platform::monitors::at_point")
}
