//! The mouse pointer as an object (Settings > Capture > Cursor as Object).
//!
//! The freeze keeps the pointer image apart from the screen pixels. In the region overlay it
//! joins the document once a region touches it; in the full-screen editor the shell puts it
//! in the document up front. Either way it starts selected, so Del removes it, but while that
//! automatic selection is untouched the keys keep their usual meaning: Enter and Ctrl+C copy
//! the image, arrows move the region, Esc cancels, brackets change the stroke width.

use stinkshot_core::annotate::{self, CursorImage};

use super::*;

/// Overlay-side state of the captured pointer.
#[derive(Default)]
pub(super) struct CursorObj {
    /// The pointer image and its rect on the canvas (overlay-local px) at freeze time.
    pub shot: Option<(PxRect, CursorImage)>,
    /// Not in the document yet: added once a region touches it.
    pub pending: bool,
    /// The pointer shape selected automatically, until the user picks something.
    pub auto: Option<ShapeId>,
}

impl CursorObj {
    pub fn new(frozen: &stinkshot_platform::capture::Frozen, enabled: bool) -> Self {
        let shot = frozen
            .cursor_shot
            .as_ref()
            .filter(|_| enabled && !frozen.cursor_drawn)
            .map(|c| {
                let r = c.rect().translate(-frozen.bounds.x, -frozen.bounds.y);
                (r, CursorImage::new(c.image.clone()))
            });
        Self {
            pending: shot.is_some(),
            shot,
            auto: None,
        }
    }
}

/// Where a pointer at canvas rect `r` lands in a multi-region picture: the placement that
/// shows most of it. None when no region shows any of it.
pub(super) fn place_in_combined(r: PxRect, placements: &[multi::Placement]) -> Option<(i32, i32)> {
    placements
        .iter()
        .filter_map(|(src, (dx, dy))| {
            let i = r.intersect(src)?;
            Some((
                i.w as i64 * i.h as i64,
                (r.x - src.x + dx, r.y - src.y + dy),
            ))
        })
        .max_by_key(|(area, _)| *area)
        .map(|(_, p)| p)
}

fn is_cursor(s: &Shape) -> bool {
    matches!(s.kind, ShapeKind::Cursor { .. })
}

impl Overlay {
    /// The automatically selected pointer is the whole selection and nothing else happened.
    pub(super) fn cursor_auto_selected(&self) -> bool {
        self.cursor.auto.is_some_and(|id| self.selected == [id])
    }

    /// Select the pointer shape `id` automatically and say so in the hint bar.
    pub(super) fn auto_select_cursor(&mut self, id: ShapeId) {
        self.selected = vec![id];
        self.cursor.auto = Some(id);
        self.flash_notice(tr!("overlay.notice.cursor_kept"));
    }

    /// The user picked something (a press, a tool): the automatic selection is theirs now.
    pub(super) fn forget_cursor_auto(&mut self) {
        self.cursor.auto = None;
    }

    /// Region overlay: add the pending pointer once the region touches it. The first time,
    /// with nothing else drawn, it starts selected. No undo step: it is part of the capture.
    pub(super) fn place_pending_cursor(&mut self) {
        if !self.cursor.pending
            || !matches!(self.kind, Kind::Capture)
            || !matches!(self.drag, Drag::None)
            || self.edit.is_some()
        {
            return;
        }
        let (Some(sel), Some((r, image))) = (self.sel, self.cursor.shot.clone()) else {
            return;
        };
        if r.intersect(&sel).is_none() {
            return;
        }
        self.cursor.pending = false;
        let fresh = self.doc.shapes.is_empty()
            && self.selected.is_empty()
            && self.tool == Tool::Select
            && !self.undo.can_undo();
        let pos = PtF::new((r.x - sel.x) as f32, (r.y - sel.y) as f32);
        let (kind, style) = annotate::cursor_shape(pos, image);
        let id = self.doc.add(kind, style);
        if fresh {
            self.auto_select_cursor(id);
        }
    }

    /// The document holds only the pointer, as captured (not moved or resized).
    pub(super) fn only_untouched_cursor(&self) -> bool {
        matches!(self.doc.shapes.as_slice(), [s] if is_cursor(s)) && !self.undo.can_undo()
    }

    /// Multi-region capture: before adding or dropping a region, take the untouched pointer
    /// back out of the document (it is placed again on the new active region, or in the
    /// combined picture). True when the document is empty afterwards.
    pub(super) fn take_back_cursor(&mut self) -> bool {
        if self.only_untouched_cursor() && self.cursor.shot.is_some() {
            self.doc.shapes.clear();
            self.selected.clear();
            self.cursor.auto = None;
            self.cursor.pending = true;
        }
        self.doc.shapes.is_empty()
    }

    /// Pointer shapes to carry into a combined multi-region picture, as canvas rects: the
    /// ones in the document (relative to the active region) and the pending one.
    pub(super) fn cursors_for_combine(&self) -> Vec<(PxRect, RectF, CursorImage)> {
        let sel = self.sel.unwrap_or_default();
        let mut out: Vec<(PxRect, RectF, CursorImage)> = self
            .doc
            .shapes
            .iter()
            .filter_map(|s| match &s.kind {
                ShapeKind::Cursor { rect, image } => {
                    let canvas =
                        RectF::new(rect.x + sel.x as f32, rect.y + sel.y as f32, rect.w, rect.h);
                    Some((canvas.to_px(), canvas, image.clone()))
                }
                _ => None,
            })
            .collect();
        if self.cursor.pending
            && let Some((r, image)) = &self.cursor.shot
        {
            let f = RectF::new(r.x as f32, r.y as f32, r.w as f32, r.h as f32);
            out.push((*r, f, image.clone()));
        }
        out
    }

    /// Put `cursors` (from `cursors_for_combine`) into the combined picture's document and
    /// select the first one automatically.
    pub(super) fn add_combined_cursors(
        &mut self,
        cursors: Vec<(PxRect, RectF, CursorImage)>,
        placements: &[multi::Placement],
    ) {
        self.cursor.pending = false;
        let mut first = None;
        for (r, f, image) in cursors {
            let Some((x, y)) = place_in_combined(r, placements) else {
                continue;
            };
            // Keep the sub-pixel part and size of a moved or resized pointer.
            let rect = RectF::new(
                x as f32 + (f.x - r.x as f32),
                y as f32 + (f.y - r.y as f32),
                f.w,
                f.h,
            );
            let style = annotate::cursor_shape(PtF::new(0.0, 0.0), image.clone()).1;
            let id = self.doc.add(ShapeKind::Cursor { rect, image }, style);
            first.get_or_insert(id);
        }
        if let Some(id) = first {
            self.auto_select_cursor(id);
        }
    }

    /// Finishing: pointer shapes that ended up entirely outside the picture are dropped
    /// (they would only bloat the edit sidecar).
    pub(super) fn drop_offscreen_cursors(doc: &mut Doc) {
        let (w, h) = (doc.base_width as f32, doc.base_height as f32);
        doc.shapes.retain(|s| match &s.kind {
            ShapeKind::Cursor { rect, .. } => {
                rect.x < w && rect.y < h && rect.x + rect.w > 0.0 && rect.y + rect.h > 0.0
            }
            _ => true,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stinkshot_core::annotate::cursor_shape;

    fn img() -> CursorImage {
        CursorImage::new(Rgba::filled(4, 6, [0, 0, 0, 255]))
    }

    #[test]
    fn combined_placement_picks_the_region_showing_most() {
        let placements: Vec<multi::Placement> = vec![
            (PxRect::new(0, 0, 100, 100), (0, 0)),
            (PxRect::new(500, 0, 100, 100), (116, 0)),
        ];
        // Inside the second region at (510, 20): 116 + 10.
        assert_eq!(
            place_in_combined(PxRect::new(510, 20, 4, 6), &placements),
            Some((126, 20))
        );
        // Straddling the first region's right edge: mostly inside it.
        assert_eq!(
            place_in_combined(PxRect::new(98, 0, 4, 6), &placements),
            Some((98, 0))
        );
        assert_eq!(
            place_in_combined(PxRect::new(300, 300, 4, 6), &placements),
            None
        );
    }

    #[test]
    fn offscreen_cursors_are_dropped_on_finish() {
        let mut d = Doc::new(50, 50);
        let (k, s) = cursor_shape(PtF::new(10.0, 10.0), img());
        let keep = d.add(k, s);
        let (k, s) = cursor_shape(PtF::new(-10.0, 10.0), img());
        let gone = d.add(k, s);
        let (k, s) = cursor_shape(PtF::new(-2.0, 48.0), img());
        let edge = d.add(k, s);
        d.add(
            ShapeKind::Redact {
                rect: RectF::new(80.0, 80.0, 5.0, 5.0),
            },
            Style::default(),
        );
        Overlay::drop_offscreen_cursors(&mut d);
        assert!(d.get(keep).is_some());
        assert!(d.get(gone).is_none());
        assert!(d.get(edge).is_some());
        // Other shapes are never dropped here.
        assert_eq!(d.shapes.len(), 3);
    }
}
