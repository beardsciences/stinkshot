//! Saving an editor's result into history. Shared by every editor host (the full-screen
//! overlay and the windowed editor): a host finishes by pushing `Command::CaptureDone` with
//! a `FinishAction`, and the capture worker writes history through `write_to_history` with
//! the item `history_target` picks. Hosts map Ctrl+S / Ctrl+Shift+S with `save_shortcut`.
//!
//! Re-editing a history item (`editing: Some(id)`):
//! - `Save` (Ctrl+S) writes the edits back to that item: the image file, its re-edit sidecar
//!   (which keeps the original unannotated base, so the item stays re-editable), the index
//!   row (size, bytes, OCR text reset) and the thumbnail (dropped, regenerated on demand).
//! - `SaveCopy` and `SaveAs` (file dialog) add a new history item and leave the original
//!   untouched: Save As asks for another file, so it must not replace this one, least of
//!   all when the dialog is cancelled.
//! - `Copy` and `Pin` also write back, as they always have.

use std::path::{Path, PathBuf};

use anyhow::Context;
use stinkshot_core::Rgba;
use stinkshot_core::annotate::{self, Doc};
use stinkshot_core::config::Config;
use stinkshot_core::naming::{self, NameParts};
use stinkshot_history::{History, ItemId, ItemKind, NewItem};

use crate::filedialog;
use crate::state::{FinishAction, SourceInfo};

/// The history item a finished edit overwrites; None adds a new item.
pub fn history_target(action: FinishAction, editing: Option<ItemId>) -> Option<ItemId> {
    match action {
        FinishAction::SaveCopy | FinishAction::SaveAs => None,
        _ => editing,
    }
}

/// Whether finishing with `action` also puts the image on the clipboard.
pub fn copies_image(action: FinishAction, copy_to_clipboard: bool) -> bool {
    match action {
        FinishAction::Copy => true,
        FinishAction::SaveAs
        | FinishAction::Save
        | FinishAction::SaveCopy
        | FinishAction::Pin
        | FinishAction::Quick => copy_to_clipboard,
        FinishAction::CopyText => false,
    }
}

/// The finish action for the save shortcut: Ctrl+S saves a re-edited history item in place
/// and Ctrl+Shift+S opens Save As; for a new capture both open Save As.
pub fn save_shortcut(editing: bool, shift: bool) -> FinishAction {
    if editing && !shift {
        FinishAction::Save
    } else {
        FinishAction::SaveAs
    }
}

/// Encode `img` in the format its extension names and write it atomically.
pub fn encode_save(img: &Rgba, path: &Path, cfg: &Config) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let bytes = img
        .encode(filedialog::format_for_path(path), cfg.capture.jpeg_quality)
        .with_context(|| format!("encode {}", path.display()))?;
    // A queued re-compression of this file must not swap in an older picture after this.
    let _g = crate::recompress::write_lock();
    stinkshot_core::image::write_atomic(path, &bytes)
        .with_context(|| format!("write {}", path.display()))
}

/// What `write_to_history` did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryWrite {
    pub id: ItemId,
    pub path: PathBuf,
    /// An existing item was overwritten (otherwise a new one was added).
    pub updated: bool,
}

/// One finished edit for `write_to_history`.
pub struct EditResult<'a> {
    /// Unannotated pixels (the original when re-editing).
    pub base: &'a Rgba,
    pub doc: &'a Doc,
    /// The exported image.
    pub flat: &'a Rgba,
    /// The export without its Beautify frame, when one was applied.
    pub unframed: Option<&'a Rgba>,
    pub source: &'a SourceInfo,
    pub mode: &'a str,
}

/// Write the flattened image (+ re-edit sidecar) into the history folder and index it.
/// `target` (from `history_target`) overwrites that item; None, or an item that no longer
/// exists, adds a new one. Background re-compression is up to the caller.
pub fn write_to_history(
    history: &History,
    cfg: &Config,
    r: &EditResult,
    target: Option<ItemId>,
) -> anyhow::Result<HistoryWrite> {
    let flat = r.flat;
    if let Some(id) = target
        && let Some(meta) = history.get(id)?
    {
        let path = meta.path;
        encode_save(flat, &path, cfg)?;
        write_sidecar(cfg, &path, r.base, r.doc, r.unframed, true);
        // Size, bytes, OCR text reset, and the cached thumbnail dropped (regenerated from
        // the new file on the next request).
        history.touch(id, flat.width, flat.height)?;
        return Ok(HistoryWrite {
            id,
            path,
            updated: true,
        });
    }
    // A picture too large for the chosen format (a long scrolling capture as WebP) is saved
    // as PNG instead of failing.
    let fmt = cfg.capture.format.for_size(flat.width, flat.height);
    let parts = NameParts {
        now: chrono::Local::now(),
        app: &r.source.app,
        title: &r.source.window_title,
        width: flat.width,
        height: flat.height,
    };
    let path = naming::unique_path(
        history.folder(),
        &cfg.capture.filename_pattern,
        &parts,
        fmt.extension(),
    );
    let t0 = std::time::Instant::now();
    encode_save(flat, &path, cfg)?;
    log::debug!(
        "saved {}x{} {:?} in {:.1} ms (encode + write)",
        flat.width,
        flat.height,
        fmt,
        t0.elapsed().as_secs_f64() * 1000.0
    );
    // `true`: a sidecar left under this name by a file deleted outside the app must go.
    write_sidecar(cfg, &path, r.base, r.doc, r.unframed, true);
    let id = history.add(NewItem {
        path: path.clone(),
        kind: Some(ItemKind::Image),
        width: flat.width,
        height: flat.height,
        app: r.source.app.clone(),
        window_title: r.source.window_title.clone(),
        mode: r.mode.to_string(),
    })?;
    Ok(HistoryWrite {
        id,
        path,
        updated: false,
    })
}

/// The re-edit sidecar: the unannotated base and the document when Keep Editable Originals
/// is on and there are annotations. Otherwise, for a framed export (Beautify), the picture
/// without the frame and no annotations: it holds nothing the export does not show
/// (redactions are already in it), and a re-edit starts from it instead of framing the frame.
pub(crate) fn write_sidecar(
    cfg: &Config,
    path: &Path,
    base: &Rgba,
    doc: &Doc,
    unframed: Option<&Rgba>,
    drop_stale: bool,
) {
    let flat_doc;
    let keep = if cfg.history.keep_editable && !doc.shapes.is_empty() {
        Some((base, doc))
    } else if let Some(u) = unframed {
        flat_doc = Doc::new(u.width, u.height);
        Some((u, &flat_doc))
    } else {
        None
    };
    if let Some((b, d)) = keep {
        if let Err(e) = annotate::save_editable(path, b, d) {
            log::warn!(
                "could not save editable sidecar for {}: {e:#}",
                path.display()
            );
        }
    } else if drop_stale && let Err(e) = annotate::delete_editable(path) {
        log::warn!(
            "could not remove stale sidecar for {}: {e:#}",
            path.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stinkshot_core::annotate::{ShapeKind, Style};
    use stinkshot_core::geom::RectF;
    use stinkshot_history::Query;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Tmp {
            let p = std::env::temp_dir().join(format!(
                "stinkshot-edit-save-{tag}-{}-{}",
                std::process::id(),
                chrono::Local::now().format("%H%M%S%f")
            ));
            std::fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn doc_with(n: usize) -> Doc {
        let mut d = Doc::new(8, 8);
        for i in 0..n {
            d.add(
                ShapeKind::Rect {
                    rect: RectF {
                        x: i as f32,
                        y: 1.0,
                        w: 3.0,
                        h: 3.0,
                    },
                    corner_radius: 0.0,
                },
                Style::default(),
            );
        }
        d
    }

    fn cfg() -> Config {
        let mut c = Config::default();
        c.capture.format = stinkshot_core::image::ImageFormat::Png;
        c.history.keep_editable = true;
        c
    }

    fn px(img: &Rgba) -> [u8; 4] {
        [img.data[0], img.data[1], img.data[2], img.data[3]]
    }

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];

    #[test]
    fn targets_and_shortcuts() {
        assert_eq!(history_target(FinishAction::Save, Some(4)), Some(4));
        assert_eq!(history_target(FinishAction::Copy, Some(4)), Some(4));
        assert_eq!(history_target(FinishAction::SaveAs, Some(4)), None);
        assert_eq!(history_target(FinishAction::SaveCopy, Some(4)), None);
        assert_eq!(history_target(FinishAction::Save, None), None);
        assert_eq!(save_shortcut(true, false), FinishAction::Save);
        assert_eq!(save_shortcut(true, true), FinishAction::SaveAs);
        assert_eq!(save_shortcut(false, false), FinishAction::SaveAs);
        assert_eq!(save_shortcut(false, true), FinishAction::SaveAs);
        assert!(copies_image(FinishAction::Copy, false));
        assert!(!copies_image(FinishAction::Save, false));
        assert!(copies_image(FinishAction::SaveCopy, true));
        assert!(!copies_image(FinishAction::CopyText, true));
    }

    #[test]
    fn save_writes_back_and_save_copy_adds_new_item() {
        let tmp = Tmp::new("back");
        let history = History::open(&tmp.0).unwrap();
        let cfg = cfg();
        let source = SourceInfo {
            app: "notepad".into(),
            window_title: "notes".into(),
            scale: None,
        };
        let base = Rgba::filled(8, 8, RED);

        // The first capture: a new item with a sidecar keeping the base.
        let doc1 = doc_with(1);
        let flat1 = Rgba::filled(8, 8, BLUE);
        let first = write_to_history(
            &history,
            &cfg,
            &EditResult {
                base: &base,
                doc: &doc1,
                flat: &flat1,
                unframed: None,
                source: &source,
                mode: "region",
            },
            None,
        )
        .unwrap();
        assert!(!first.updated);
        assert!(history.thumbnail(first.id, 64).unwrap().is_some());

        // Re-edit, Save: same item, same file, new pixels, original base kept.
        let doc2 = doc_with(2);
        let flat2 = Rgba::filled(10, 6, GREEN);
        let target = history_target(FinishAction::Save, Some(first.id));
        let saved = write_to_history(
            &history,
            &cfg,
            &EditResult {
                base: &base,
                doc: &doc2,
                flat: &flat2,
                unframed: None,
                source: &source,
                mode: "region",
            },
            target,
        )
        .unwrap();
        assert_eq!(
            saved,
            HistoryWrite {
                id: first.id,
                path: first.path.clone(),
                updated: true
            }
        );
        assert_eq!(history.stats().unwrap().count, 1);
        let meta = history.get(first.id).unwrap().unwrap();
        assert_eq!((meta.width, meta.height), (10, 6));
        assert_eq!(meta.bytes, std::fs::metadata(&first.path).unwrap().len());
        assert_eq!(meta.mode, "region");
        assert_eq!(px(&Rgba::load(&first.path).unwrap()), GREEN);
        let (side_base, side_doc) = annotate::load_editable(&first.path).unwrap().unwrap();
        assert_eq!(px(&side_base), RED);
        assert_eq!(side_doc.shapes.len(), 2);
        // The thumbnail follows the new file.
        let thumb = history.thumbnail(first.id, 64).unwrap().unwrap();
        assert_eq!(px(&Rgba::load(&thumb).unwrap()), GREEN);

        // Save as Copy: a new item; the original stays as saved.
        let doc3 = doc_with(3);
        let flat3 = Rgba::filled(8, 8, BLUE);
        let target = history_target(FinishAction::SaveCopy, Some(first.id));
        let copy = write_to_history(
            &history,
            &cfg,
            &EditResult {
                base: &base,
                doc: &doc3,
                flat: &flat3,
                unframed: None,
                source: &source,
                mode: "region",
            },
            target,
        )
        .unwrap();
        assert!(!copy.updated);
        assert_ne!(copy.id, first.id);
        assert_ne!(copy.path, first.path);
        let all = history
            .query(&Query {
                limit: 0,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(px(&Rgba::load(&first.path).unwrap()), GREEN);
        assert_eq!(
            annotate::load_editable(&first.path)
                .unwrap()
                .unwrap()
                .1
                .shapes
                .len(),
            2
        );
        let (copy_base, copy_doc) = annotate::load_editable(&copy.path).unwrap().unwrap();
        assert_eq!(px(&copy_base), RED);
        assert_eq!(copy_doc.shapes.len(), 3);
        assert_eq!(history.get(copy.id).unwrap().unwrap().app, "notepad");
    }

    #[test]
    fn saving_a_deleted_item_adds_it_again() {
        let tmp = Tmp::new("gone");
        let history = History::open(&tmp.0).unwrap();
        let base = Rgba::filled(4, 4, RED);
        let doc = Doc::new(4, 4);
        let w = write_to_history(
            &history,
            &cfg(),
            &EditResult {
                base: &base,
                doc: &doc,
                flat: &base,
                unframed: None,
                source: &SourceInfo::default(),
                mode: "region",
            },
            Some(12345),
        )
        .unwrap();
        assert!(!w.updated);
        assert!(w.path.is_file());
    }

    #[test]
    fn removing_every_annotation_drops_the_sidecar() {
        let tmp = Tmp::new("clear");
        let history = History::open(&tmp.0).unwrap();
        let cfg = cfg();
        let base = Rgba::filled(8, 8, RED);
        let r = |doc: &Doc, flat: &Rgba, target| {
            write_to_history(
                &history,
                &cfg,
                &EditResult {
                    base: &base,
                    doc,
                    flat,
                    unframed: None,
                    source: &SourceInfo::default(),
                    mode: "region",
                },
                target,
            )
            .unwrap()
        };
        let first = r(&doc_with(1), &Rgba::filled(8, 8, BLUE), None);
        assert!(annotate::load_editable(&first.path).unwrap().is_some());
        let again = r(&Doc::new(8, 8), &base, Some(first.id));
        assert!(again.updated);
        assert!(annotate::load_editable(&first.path).unwrap().is_none());
    }
}
