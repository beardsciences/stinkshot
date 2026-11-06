//! Capture history. The folder of images/videos is the source of truth and is usable
//! without the app; `<folder>\.stinkshot\index.sqlite` is a rebuildable index (metadata +
//! FTS5 over OCR text, window title and app). Re-edit sidecars live in
//! `<folder>\.stinkshot\edit\` (owned by `stinkshot_core::annotate`); `delete` and retention
//! must remove them too, and thumbnails cached in `<folder>\.stinkshot\thumbs\<id>.png`.
//!
//! `History` is `Send + Sync` (internal `Mutex<Connection>`); share it as `Arc<History>`.
//! All methods are cheap enough to call from the UI thread except `rescan` and
//! `apply_retention`, which the app runs on a worker thread.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

pub type ItemId = i64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemKind {
    Image,
    Video,
    Gif,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ItemMeta {
    pub id: ItemId,
    pub path: PathBuf,
    pub kind: ItemKind,
    pub created: DateTime<Local>,
    pub width: u32,
    pub height: u32,
    /// Process name of the source window, e.g. "firefox". Empty if unknown.
    pub app: String,
    pub window_title: String,
    pub starred: bool,
    /// None = not OCR'd yet.
    pub ocr_text: Option<String>,
    pub bytes: u64,
    /// Capture mode label: "region" | "window" | "fullscreen" | "scrolling" | "recording".
    pub mode: String,
}

#[derive(Clone, Debug, Default)]
pub struct NewItem {
    pub path: PathBuf,
    pub kind: Option<ItemKind>,
    pub width: u32,
    pub height: u32,
    pub app: String,
    pub window_title: String,
    pub mode: String,
}

#[derive(Clone, Debug, Default)]
pub struct Query {
    /// Free text; matched with FTS5 against ocr_text, window_title, app, file name.
    /// Prefix-matches the last word ("invo" finds "invoice").
    pub text: Option<String>,
    pub app: Option<String>,
    pub kind: Option<ItemKind>,
    pub starred_only: bool,
    pub since: Option<DateTime<Local>>,
    /// Newest first. 0 = no limit.
    pub limit: usize,
    pub offset: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub count: u64,
    pub starred: u64,
    pub recordings: u64,
    pub bytes: u64,
}

pub struct History {
    folder: PathBuf,
}

impl History {
    /// Creates the folder and index if needed; migrates schema.
    pub fn open(folder: &Path) -> anyhow::Result<History> {
        let _ = folder;
        todo!("history::History::open")
    }
    pub fn folder(&self) -> &Path {
        &self.folder
    }
    /// Index a file already written into the folder. Fills bytes/created from the file.
    pub fn add(&self, item: NewItem) -> anyhow::Result<ItemId> {
        let _ = item;
        todo!("history::History::add")
    }
    pub fn get(&self, id: ItemId) -> anyhow::Result<Option<ItemMeta>> {
        let _ = id;
        todo!("history::History::get")
    }
    pub fn find_by_path(&self, path: &Path) -> anyhow::Result<Option<ItemMeta>> {
        let _ = path;
        todo!("history::History::find_by_path")
    }
    pub fn query(&self, q: &Query) -> anyhow::Result<Vec<ItemMeta>> {
        let _ = q;
        todo!("history::History::query")
    }
    /// Distinct apps with item counts, most frequent first.
    pub fn apps(&self) -> anyhow::Result<Vec<(String, u64)>> {
        todo!("history::History::apps")
    }
    pub fn stats(&self) -> anyhow::Result<Stats> {
        todo!("history::History::stats")
    }
    pub fn set_starred(&self, id: ItemId, starred: bool) -> anyhow::Result<()> {
        let _ = (id, starred);
        todo!("history::History::set_starred")
    }
    pub fn set_ocr_text(&self, id: ItemId, text: &str) -> anyhow::Result<()> {
        let _ = (id, text);
        todo!("history::History::set_ocr_text")
    }
    /// File was re-saved (re-edit): refresh size/dimensions, clear OCR text so it re-indexes.
    pub fn touch(&self, id: ItemId, width: u32, height: u32) -> anyhow::Result<()> {
        let _ = (id, width, height);
        todo!("history::History::touch")
    }
    /// Images with ocr_text NULL, newest first.
    pub fn pending_ocr(&self, limit: usize) -> anyhow::Result<Vec<ItemMeta>> {
        let _ = limit;
        todo!("history::History::pending_ocr")
    }
    /// Deletes the file, its re-edit sidecar, thumbnail and index row.
    pub fn delete(&self, id: ItemId) -> anyhow::Result<()> {
        let _ = id;
        todo!("history::History::delete")
    }
    /// Remove non-starred items older than `keep_days` (0 = no age limit), then oldest
    /// non-starred until total bytes <= `max_bytes` (0 = no cap). Returns removed count.
    pub fn apply_retention(&self, keep_days: u32, max_bytes: u64) -> anyhow::Result<u32> {
        let _ = (keep_days, max_bytes);
        todo!("history::History::apply_retention")
    }
    /// Sync index with folder: add files the user dropped in (png/jpg/webp/mp4/gif, not
    /// under .stinkshot), drop rows whose files vanished. Returns (added, removed).
    pub fn rescan(&self) -> anyhow::Result<(u32, u32)> {
        todo!("history::History::rescan")
    }
    /// Cached thumbnail path (generate on first request; videos: None unless cached by
    /// `store_thumbnail`).
    pub fn thumbnail(&self, id: ItemId, max_size: u32) -> anyhow::Result<Option<PathBuf>> {
        let _ = (id, max_size);
        todo!("history::History::thumbnail")
    }
    pub fn store_thumbnail(&self, id: ItemId, thumb: &stinkshot_core::Rgba) -> anyhow::Result<()> {
        let _ = (id, thumb);
        todo!("history::History::store_thumbnail")
    }
}
