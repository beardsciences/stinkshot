//! Capture history. The folder of images/videos is the source of truth and is usable
//! without the app; `<folder>\.stinkshot\index.sqlite` is a rebuildable index (metadata +
//! FTS5 over OCR text, window title and app). Re-edit sidecars live in
//! `<folder>\.stinkshot\edit\` (owned by `stinkshot_core::annotate`); `delete` and retention
//! must remove them too, and thumbnails cached in `<folder>\.stinkshot\thumbs\<id>.png`.
//!
//! `History` is `Send + Sync` (internal `Mutex<Connection>`); share it as `Arc<History>`.
//! All methods are cheap enough to call from the UI thread except `rescan` and
//! `apply_retention`, which the app runs on a worker thread.

use std::path::PathBuf;

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

mod store;

pub use store::History;
