//! Multi-region capture: Shift+drag (or the Add Regions toggle) collects several regions
//! of the frozen screen; finishing or picking a tool combines them into one picture that
//! opens in the editor, so annotations work across all of them.
//!
//! Layout: the regions keep their relative positions on a transparent canvas, with empty
//! space between them squeezed to `GAP_PX`. Relative placement keeps what was left of /
//! above what (two parts of a form, a menu next to its dialog) and lets arrows point from
//! one region to another; squeezing keeps regions from opposite monitors from producing a
//! mostly empty picture. Transparent gaps paste cleanly into documents and chats (the
//! clipboard carries alpha; plain-DIB apps get white).

use std::sync::Arc;

use egui::{Pos2, Rect};
use stinkshot_core::annotate::Doc;
use stinkshot_core::{PxPoint, PxRect, Rgba};

use super::{Drag, Kind, Overlay, hover};

/// Empty space kept between regions in the combined picture, physical px.
pub const GAP_PX: i32 = 16;

/// Where each region lands in the combined picture: (source rect, destination top-left).
pub type Placement = (PxRect, (i32, i32));

/// Maps coordinates along one axis: every empty stretch between covered intervals
/// longer than `gap` shrinks to `gap`; the first covered pixel maps to 0.
fn squeeze_axis(spans: &[(i32, i32)], gap: i32) -> impl Fn(i32) -> i32 + use<> {
    let mut v: Vec<(i32, i32)> = spans.to_vec();
    v.sort_unstable();
    let mut merged: Vec<(i32, i32)> = Vec::new();
    for (a, b) in v {
        match merged.last_mut() {
            Some((_, e)) if a <= *e => *e = (*e).max(b),
            _ => merged.push((a, b)),
        }
    }
    // (start of a covered interval, total shift applied from there on)
    let mut shifts: Vec<(i32, i32)> = Vec::new();
    let mut shift = merged.first().map_or(0, |m| m.0);
    let mut prev_end: Option<i32> = None;
    for (a, b) in merged {
        if let Some(e) = prev_end {
            shift += (a - e - gap).max(0);
        }
        shifts.push((a, shift));
        prev_end = Some(b);
    }
    move |x: i32| {
        let i = shifts.partition_point(|(a, _)| *a <= x);
        let s = if i == 0 {
            shifts.first().map_or(0, |s| s.1)
        } else {
            shifts[i - 1].1
        };
        x - s
    }
}

/// Combines `regions` of `screen` into one transparent picture (see the module docs).
pub fn compose(screen: &Rgba, regions: &[PxRect], gap: i32) -> (Rgba, Vec<Placement>) {
    let regions: Vec<PxRect> = regions
        .iter()
        .filter_map(|r| r.intersect(&screen.bounds()))
        .filter(|r| r.w > 0 && r.h > 0)
        .collect();
    if regions.is_empty() {
        return (Rgba::new(0, 0), Vec::new());
    }
    let mx = squeeze_axis(
        &regions.iter().map(|r| (r.x, r.right())).collect::<Vec<_>>(),
        gap,
    );
    let my = squeeze_axis(
        &regions
            .iter()
            .map(|r| (r.y, r.bottom()))
            .collect::<Vec<_>>(),
        gap,
    );
    let placements: Vec<Placement> = regions.iter().map(|r| (*r, (mx(r.x), my(r.y)))).collect();
    let w = placements
        .iter()
        .map(|(r, (x, _))| x + r.w)
        .max()
        .unwrap_or(0);
    let h = placements
        .iter()
        .map(|(r, (_, y))| y + r.h)
        .max()
        .unwrap_or(0);
    let mut out = Rgba::new(w.max(1) as u32, h.max(1) as u32);
    let ow = out.width as usize;
    let sw = screen.width as usize;
    for (r, (dx, dy)) in &placements {
        for row in 0..r.h as usize {
            let s = ((r.y as usize + row) * sw + r.x as usize) * 4;
            let d = ((*dy as usize + row) * ow + *dx as usize) * 4;
            let n = r.w as usize * 4;
            out.data[d..d + n].copy_from_slice(&screen.data[s..s + n]);
        }
    }
    (out, placements)
}

/// Pieces of `outer` not covered by any of `holes` (for dimming around several regions).
pub fn dim_except(outer: Rect, holes: &[Rect]) -> Vec<Rect> {
    let holes: Vec<Rect> = holes
        .iter()
        .map(|h| h.intersect(outer))
        .filter(|h| h.is_positive())
        .collect();
    if holes.is_empty() {
        return vec![outer];
    }
    let mut ys: Vec<f32> = vec![outer.min.y, outer.max.y];
    for h in &holes {
        ys.push(h.min.y);
        ys.push(h.max.y);
    }
    ys.sort_by(f32::total_cmp);
    ys.dedup();
    let mut out = Vec::new();
    for band in ys.windows(2) {
        let (y0, y1) = (band[0], band[1]);
        if y1 <= y0 {
            continue;
        }
        let mut xs: Vec<(f32, f32)> = holes
            .iter()
            .filter(|h| h.min.y <= y0 && h.max.y >= y1)
            .map(|h| (h.min.x, h.max.x))
            .collect();
        xs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut x = outer.min.x;
        for (a, b) in xs {
            if a > x {
                out.push(Rect::from_min_max(Pos2::new(x, y0), Pos2::new(a, y1)));
            }
            x = x.max(b);
        }
        if x < outer.max.x {
            out.push(Rect::from_min_max(
                Pos2::new(x, y0),
                Pos2::new(outer.max.x, y1),
            ));
        }
    }
    out
}

impl Overlay {
    /// Regions to combine: the earlier ones, then the active selection.
    pub(super) fn regions(&self) -> Vec<PxRect> {
        let mut v = self.extra_regions.clone();
        v.extend(self.sel);
        v
    }

    /// A press or hover outside the selection adds a region (Shift or the toggle), in the
    /// screen overlay only.
    pub(super) fn add_armed(&self, shift: bool) -> bool {
        matches!(self.kind, Kind::Capture)
            && self.sel.is_some()
            && self.edit.is_none()
            && (shift || self.add_regions)
    }

    pub(super) fn region_at(&self, p: (f32, f32)) -> Option<usize> {
        let pt = PxPoint {
            x: p.0.floor() as i32,
            y: p.1.floor() as i32,
        };
        self.extra_regions.iter().rposition(|r| r.contains(pt))
    }

    /// Press outside the selection in the screen overlay: make an earlier region the
    /// active one, or start adding a region. True when the press was handled.
    pub(super) fn press_regions(&mut self, p: (f32, f32), shift: bool) -> bool {
        let Some(sel) = self.sel else { return false };
        if !matches!(self.kind, Kind::Capture) {
            return false;
        }
        if let Some(i) = self.region_at(p) {
            if !self.doc.shapes.is_empty() {
                self.flash_notice("Annotations stay on the active region.");
                self.drag = Drag::Ignore;
                return true;
            }
            let r = self.extra_regions.remove(i);
            self.extra_regions.push(sel);
            self.set_sel(r);
            self.drag = Drag::Ignore;
            return true;
        }
        if !self.add_armed(shift) {
            return false;
        }
        if !self.doc.shapes.is_empty() {
            self.flash_notice("Add regions before annotating.");
            self.drag = Drag::Ignore;
            return true;
        }
        self.extra_regions.push(sel);
        self.adding_region = true;
        self.selected.clear();
        self.drag = Drag::Selecting {
            anchor: p,
            start: p,
            cur: p,
            moved: false,
        };
        true
    }

    /// The add gesture ended without a region: the active selection stays as it was.
    pub(super) fn cancel_add(&mut self) {
        if std::mem::take(&mut self.adding_region) {
            self.extra_regions.pop();
        }
    }

    /// Esc with several regions: drop the active one, the previous one becomes active.
    pub(super) fn remove_active_region(&mut self) -> bool {
        if !matches!(self.kind, Kind::Capture) || !self.doc.shapes.is_empty() {
            return false;
        }
        let Some(r) = self.extra_regions.pop() else {
            return false;
        };
        self.set_sel(r);
        true
    }

    /// Combines the regions into one picture and switches to the editor on it. No-op with
    /// a single region.
    pub(super) fn combine_regions(&mut self) {
        if self.extra_regions.is_empty() || !matches!(self.kind, Kind::Capture) {
            return;
        }
        let regions = self.regions();
        let (img, placements) = compose(&self.frozen_image, &regions, GAP_PX);
        if img.width == 0 || placements.is_empty() {
            self.extra_regions.clear();
            return;
        }
        let union = regions.iter().skip(1).fold(regions[0], |a, r| a.union(r));
        let source = self.source_info(regions[regions.len() - 1]);
        log::info!(
            "overlay: combined {} regions into {}x{} (union {:?})",
            regions.len(),
            img.width,
            img.height,
            hover::local_to_virtual(union, self.bounds)
        );
        let mut doc = Doc::new(img.width, img.height);
        doc.base_width = img.width;
        doc.base_height = img.height;
        self.extra_regions.clear();
        self.add_regions = false;
        self.adding_region = false;
        self.kind = Kind::Annotate;
        self.combined = true;
        self.annotate_meta = Some(("region", None, source));
        self.doc = doc;
        self.undo = super::undo::UndoStack::new();
        self.selected.clear();
        self.hover = None;
        self.view = super::view::View::IDENTITY;
        self.bar_origin = None;
        self.layout_annotate(&img);
        let img = Arc::new(img);
        self.orig = Some(img.clone());
        self.base = Some(img);
        // Uploaded on the next frame (`ensure_base_texture`).
        self.base_tex = None;
        self.clear_render_cache();
    }

    /// Upload the editor's picture once it exists (after `combine_regions`).
    pub(super) fn ensure_base_texture(&mut self, ctx: &egui::Context) {
        if matches!(self.kind, Kind::Annotate)
            && self.base_tex.is_none()
            && let Some(b) = &self.orig
        {
            self.base_tex = Some(super::render::TiledTexture::upload(
                ctx, "ov-base", b, false,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Rgba {
        let (w, h) = (400u32, 300u32);
        let mut s = Rgba::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                s.data[i..i + 4].copy_from_slice(&[x as u8, y as u8, 7, 255]);
            }
        }
        s
    }

    #[test]
    fn keeps_relative_positions_and_squeezes_gaps() {
        let s = screen();
        let a = PxRect::new(10, 20, 50, 40);
        let b = PxRect::new(300, 200, 60, 30);
        let (img, pl) = compose(&s, &[a, b], 16);
        // x: a spans 10..60, b 300..360 -> gap 240 squeezed to 16
        assert_eq!(pl[0].1, (0, 0));
        assert_eq!(pl[1].1, (50 + 16, 40 + 16));
        assert_eq!((img.width, img.height), (50 + 16 + 60, 40 + 16 + 30));
        // pixels copied 1:1, the gap is transparent
        let px = |x: u32, y: u32| &img.data[((y * img.width + x) * 4) as usize..][..4];
        assert_eq!(px(0, 0), &[10, 20, 7, 255]);
        assert_eq!(px(66, 56), &[44, 200, 7, 255]);
        assert_eq!(px(60, 50)[3], 0);
    }

    #[test]
    fn overlapping_and_side_by_side_regions() {
        let s = screen();
        // side by side with a small gap (kept), overlapping on y
        let a = PxRect::new(0, 0, 100, 100);
        let b = PxRect::new(110, 50, 100, 100);
        let (img, pl) = compose(&s, &[a, b], 16);
        assert_eq!(pl[1].1, (110, 50));
        assert_eq!((img.width, img.height), (210, 150));
        // a region partly off screen is clipped
        let (img, _) = compose(&s, &[PxRect::new(350, 250, 100, 100)], 16);
        assert_eq!((img.width, img.height), (50, 50));
        assert_eq!(compose(&s, &[], 16).0.width, 0);
    }

    #[test]
    fn dim_covers_everything_but_the_holes() {
        let outer = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(100.0, 100.0));
        let holes = [
            Rect::from_min_max(Pos2::new(10.0, 10.0), Pos2::new(40.0, 40.0)),
            Rect::from_min_max(Pos2::new(30.0, 30.0), Pos2::new(70.0, 60.0)),
        ];
        let parts = dim_except(outer, &holes);
        let area: f32 = parts.iter().map(|r| r.area()).sum();
        // union of holes = 900 + 1200 - 100 overlap
        assert!((area - (10000.0 - 2000.0)).abs() < 0.01, "{area}");
        for p in &parts {
            for h in &holes {
                assert!(!p.intersect(*h).is_positive(), "{p:?} overlaps {h:?}");
            }
        }
        assert_eq!(dim_except(outer, &[]), vec![outer]);
    }
}
