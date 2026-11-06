//! Geometry types. `Px*` are integer physical pixels (virtual-desktop or image space,
//! documented at each use site). `PtF`/`RectF` are float image-space pixels used by annotations.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PxPoint {
    pub x: i32,
    pub y: i32,
}

/// Integer rectangle. `w`/`h` are always >= 0 for a normalized rect.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PxRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl PxRect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }
    /// Normalized rect spanning two corner points (inclusive of `a`, exclusive of `b`).
    pub fn from_points(a: PxPoint, b: PxPoint) -> Self {
        let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
        let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
        Self::new(x0, y0, x1 - x0, y1 - y0)
    }
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }
    pub fn contains(&self, p: PxPoint) -> bool {
        p.x >= self.x && p.y >= self.y && p.x < self.right() && p.y < self.bottom()
    }
    pub fn intersect(&self, o: &PxRect) -> Option<PxRect> {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = self.right().min(o.right());
        let y1 = self.bottom().min(o.bottom());
        (x1 > x0 && y1 > y0).then(|| PxRect::new(x0, y0, x1 - x0, y1 - y0))
    }
    pub fn union(&self, o: &PxRect) -> PxRect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        let x0 = self.x.min(o.x);
        let y0 = self.y.min(o.y);
        let x1 = self.right().max(o.right());
        let y1 = self.bottom().max(o.bottom());
        PxRect::new(x0, y0, x1 - x0, y1 - y0)
    }
    pub fn translate(&self, dx: i32, dy: i32) -> PxRect {
        PxRect::new(self.x + dx, self.y + dy, self.w, self.h)
    }
    pub fn area(&self) -> i64 {
        if self.is_empty() { 0 } else { self.w as i64 * self.h as i64 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PtF {
    pub x: f32,
    pub y: f32,
}

impl PtF {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl RectF {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    pub fn from_points(a: PtF, b: PtF) -> Self {
        let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
        let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
        Self::new(x0, y0, x1 - x0, y1 - y0)
    }
    pub fn contains(&self, p: PtF) -> bool {
        p.x >= self.x && p.y >= self.y && p.x <= self.x + self.w && p.y <= self.y + self.h
    }
    pub fn center(&self) -> PtF {
        PtF::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
    pub fn expand(&self, by: f32) -> RectF {
        RectF::new(self.x - by, self.y - by, self.w + 2.0 * by, self.h + 2.0 * by)
    }
    /// Smallest integer rect covering this one.
    pub fn to_px(&self) -> PxRect {
        let x0 = self.x.floor() as i32;
        let y0 = self.y.floor() as i32;
        let x1 = (self.x + self.w).ceil() as i32;
        let y1 = (self.y + self.h).ceil() as i32;
        PxRect::new(x0, y0, x1 - x0, y1 - y0)
    }
}
