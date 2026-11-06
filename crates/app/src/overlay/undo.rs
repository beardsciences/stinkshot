//! Snapshot undo/redo stack. Each entry is a full `Doc` clone: annotation docs are small
//! (dozens of shapes), so snapshots are simpler and more robust than operation logs.

use stinkshot_core::annotate::Doc;

const MAX_DEPTH: usize = 200;

#[derive(Default)]
pub struct UndoStack {
    past: Vec<Doc>,
    future: Vec<Doc>,
}

impl UndoStack {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `before` (the state prior to a mutation). Clears the redo branch. Consecutive
    /// identical snapshots are collapsed.
    pub fn push(&mut self, before: &Doc) {
        if self.past.last().is_some_and(|d| d == before) {
            return;
        }
        self.past.push(before.clone());
        if self.past.len() > MAX_DEPTH {
            self.past.remove(0);
        }
        self.future.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    /// Undo: returns the document to restore, remembering `current` for redo.
    pub fn undo(&mut self, current: &Doc) -> Option<Doc> {
        let prev = self.past.pop()?;
        self.future.push(current.clone());
        Some(prev)
    }

    pub fn redo(&mut self, current: &Doc) -> Option<Doc> {
        let next = self.future.pop()?;
        self.past.push(current.clone());
        Some(next)
    }

    /// Apply `f` to every stored snapshot (used when the selection origin moves so that
    /// history stays in the same coordinate space as the live document).
    pub fn map_docs(&mut self, mut f: impl FnMut(&mut Doc)) {
        self.past
            .iter_mut()
            .chain(self.future.iter_mut())
            .for_each(&mut f);
    }

    pub fn clear(&mut self) {
        self.past.clear();
        self.future.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stinkshot_core::RectF;
    use stinkshot_core::annotate::{ShapeKind, Style};

    fn add(doc: &mut Doc, x: f32) {
        doc.add(
            ShapeKind::Redact {
                rect: RectF::new(x, 0.0, 5.0, 5.0),
            },
            Style::default(),
        );
    }

    #[test]
    fn undo_redo_round_trip() {
        let mut doc = Doc::new(100, 100);
        let mut st = UndoStack::new();
        assert!(!st.can_undo());
        st.push(&doc);
        add(&mut doc, 1.0);
        st.push(&doc);
        add(&mut doc, 2.0);
        assert_eq!(doc.shapes.len(), 2);

        doc = st.undo(&doc).unwrap();
        assert_eq!(doc.shapes.len(), 1);
        doc = st.undo(&doc).unwrap();
        assert_eq!(doc.shapes.len(), 0);
        assert!(st.undo(&doc).is_none());

        doc = st.redo(&doc).unwrap();
        assert_eq!(doc.shapes.len(), 1);
        doc = st.redo(&doc).unwrap();
        assert_eq!(doc.shapes.len(), 2);
        assert!(st.redo(&doc).is_none());
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut doc = Doc::new(100, 100);
        let mut st = UndoStack::new();
        st.push(&doc);
        add(&mut doc, 1.0);
        doc = st.undo(&doc).unwrap();
        assert!(st.can_redo());
        st.push(&doc);
        add(&mut doc, 9.0);
        assert!(!st.can_redo());
    }

    #[test]
    fn map_docs_touches_history() {
        let mut doc = Doc::new(100, 100);
        let mut st = UndoStack::new();
        add(&mut doc, 1.0);
        st.push(&doc);
        add(&mut doc, 2.0);
        st.map_docs(|d| d.base_width = 7);
        let prev = st.undo(&doc).unwrap();
        assert_eq!(prev.base_width, 7);
    }

    #[test]
    fn identical_snapshots_collapse_and_depth_is_bounded() {
        let doc = Doc::new(10, 10);
        let mut st = UndoStack::new();
        st.push(&doc);
        st.push(&doc);
        assert_eq!(st.past.len(), 1);
        let mut d = doc.clone();
        for i in 0..(MAX_DEPTH + 20) {
            st.push(&d);
            add(&mut d, i as f32);
        }
        assert_eq!(st.past.len(), MAX_DEPTH);
        st.clear();
        assert!(!st.can_undo() && !st.can_redo());
    }
}
