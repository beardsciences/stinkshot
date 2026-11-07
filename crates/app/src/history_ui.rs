//! History window (docs/design/History.dc.html): sidebar filters (All, Starred,
//! Recordings, by source app with counts, retention summary), search box (FTS over OCR
//! text/title/app, debounced 150 ms), date filter, virtualized thumbnail grid (thumbnails
//! loaded lazily on a worker via `History::thumbnail`), per-item actions: Edit, Pin, Copy
//! image, Copy text, Open, Show in folder, Star, Delete (with confirm). Enter = Edit.
//!
//! Thumbnails: only rows near the viewport are drawn and requested. A single worker thread
//! serves requests newest-first (so a fast scroll does not wait behind stale rows), decodes
//! the cached thumbnail PNG and hands back a `ColorImage`; the UI thread uploads at most a few
//! per frame into an LRU texture cache keyed by (id, file size) so re-edited items refresh.

use std::collections::HashSet;
use std::ops::Range;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Datelike, Local, TimeZone};
use egui::{
    Align2, Color32, CornerRadius, Key, Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind,
    UiBuilder, pos2, vec2,
};
use stinkshot_core::Rgba;
use stinkshot_history::{History, ItemId, ItemKind, ItemMeta, Query, Stats};
use stinkshot_platform::Hwnd;

use crate::state::{Command, Shared, Surface, SurfaceStatus};
use crate::theme::{self, Icon, widgets};
use crate::toast::ui_common::{self as uc, Debounce, Lru};

const SIDEBAR_W: f32 = 240.0;
const THUMB_H: f32 = 150.0;
const CARD_INFO_H: f32 = 54.0;
const CARD_H: f32 = THUMB_H + CARD_INFO_H;
const GAP: f32 = 16.0;
const MIN_CARD_W: f32 = 250.0;
const PAGE: usize = 400;
const TEX_CACHE: usize = 300;
const THUMB_MAX: u32 = 512;
const UPLOADS_PER_FRAME: usize = 6;
const MAX_QUEUED: usize = 48;
const STAR_COLOR: Color32 = Color32::from_rgb(0xff, 0xd1, 0x66);

// ---------------------------------------------------------------------------------------
// Pure logic
// ---------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Filter {
    All,
    Starred,
    Recordings,
    App(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateFilter {
    Any,
    Today,
    Week,
    Month,
    Year,
}

impl DateFilter {
    pub const ALL: [DateFilter; 5] = [
        DateFilter::Any,
        DateFilter::Today,
        DateFilter::Week,
        DateFilter::Month,
        DateFilter::Year,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DateFilter::Any => "Any Time",
            DateFilter::Today => "Today",
            DateFilter::Week => "Last 7 Days",
            DateFilter::Month => "Last 30 Days",
            DateFilter::Year => "Last Year",
        }
    }

    pub fn since(self, now: DateTime<Local>) -> Option<DateTime<Local>> {
        match self {
            DateFilter::Any => None,
            DateFilter::Today => {
                let midnight = now.date_naive().and_hms_opt(0, 0, 0)?;
                Local.from_local_datetime(&midnight).earliest()
            }
            DateFilter::Week => Some(now - chrono::Duration::days(7)),
            DateFilter::Month => Some(now - chrono::Duration::days(30)),
            DateFilter::Year => Some(now - chrono::Duration::days(365)),
        }
    }
}

/// Columns and card width for a grid of at least `min_card` wide cards separated by `gap`.
pub fn grid_layout(avail_w: f32, min_card: f32, gap: f32) -> (usize, f32) {
    let cols = (((avail_w + gap) / (min_card + gap)).floor() as usize).max(1);
    let card_w = ((avail_w - gap * (cols as f32 - 1.0)) / cols as f32).max(1.0);
    (cols, card_w)
}

/// Rows to draw for a scroll window, with `overscan` extra rows each side.
pub fn visible_rows(
    scroll_top: f32,
    view_h: f32,
    pitch: f32,
    nrows: usize,
    overscan: usize,
) -> Range<usize> {
    if nrows == 0 || pitch <= 0.0 {
        return 0..0;
    }
    let first = ((scroll_top / pitch).floor().max(0.0) as usize).saturating_sub(overscan);
    let last = (((scroll_top + view_h) / pitch).ceil().max(0.0) as usize + overscan).min(nrows);
    first.min(nrows)..last
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
}

/// Keyboard selection movement in a `cols`-wide grid of `len` items.
pub fn move_selection(cur: Option<usize>, nav: Nav, cols: usize, len: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let Some(c) = cur.filter(|c| *c < len) else {
        return Some(0);
    };
    let cols = cols.max(1);
    Some(match nav {
        Nav::Left => c.saturating_sub(1),
        Nav::Right => (c + 1).min(len - 1),
        Nav::Up => {
            if c >= cols {
                c - cols
            } else {
                c
            }
        }
        Nav::Down => {
            if c + cols < len {
                c + cols
            } else if c / cols < (len - 1) / cols {
                len - 1
            } else {
                c
            }
        }
        Nav::Home => 0,
        Nav::End => len - 1,
    })
}

/// "firefox.exe" -> "Firefox"; "" -> "Unknown".
pub fn pretty_app(app: &str) -> String {
    let t = app.trim();
    let t = t
        .strip_suffix(".exe")
        .or_else(|| t.strip_suffix(".EXE"))
        .unwrap_or(t);
    let mut chars = t.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Unknown".into(),
    }
}

/// Card title: "Firefox - Billing portal", falling back to the file name.
pub fn item_title(m: &ItemMeta) -> String {
    let app = m.app.trim();
    let title = m.window_title.trim();
    match (app.is_empty(), title.is_empty()) {
        (false, false) => format!("{} - {}", pretty_app(app), title),
        (true, false) => title.to_string(),
        (false, true) => pretty_app(app),
        (true, true) => m
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Capture".into()),
    }
}

/// "Today 9:14", "Yesterday 16:02", "Nov 3", "Nov 3 2025".
pub fn friendly_date(created: DateTime<Local>, now: DateTime<Local>) -> String {
    let days = now
        .date_naive()
        .signed_duration_since(created.date_naive())
        .num_days();
    let time = created.format("%-H:%M");
    match days {
        0 => format!("Today {time}"),
        1 => format!("Yesterday {time}"),
        _ if created.year() == now.year() => created.format("%b %-d").to_string(),
        _ => created.format("%b %-d %Y").to_string(),
    }
}

/// "Today 9:14 · 1280 × 720", with the capture mode when it is not a plain region grab.
pub fn item_meta_line(m: &ItemMeta, now: DateTime<Local>) -> String {
    let mut parts = vec![friendly_date(m.created, now)];
    if !m.mode.is_empty() && m.mode != "region" {
        parts.push(m.mode.clone());
    }
    if m.width > 0 && m.height > 0 {
        parts.push(uc::fmt_dims(m.width, m.height));
    }
    parts.join(" \u{b7} ")
}

fn fmt_gb(gb: f32) -> String {
    if (gb - gb.round()).abs() < 0.05 {
        format!("{} GB", gb.round() as i64)
    } else {
        format!("{gb:.1} GB")
    }
}

/// Sidebar footer text.
pub fn retention_summary(keep_days: u32, max_gb: f32, used_bytes: u64) -> String {
    let used = uc::fmt_bytes(used_bytes);
    let days = |d: u32| {
        if d == 1 {
            "1 day".to_string()
        } else {
            format!("{d} days")
        }
    };
    match (keep_days > 0, max_gb > 0.0) {
        (true, true) => format!(
            "Keeping {} or {}, starred kept forever. {used} used.",
            days(keep_days),
            fmt_gb(max_gb)
        ),
        (true, false) => format!(
            "Keeping {}, starred kept forever. {used} used.",
            days(keep_days)
        ),
        (false, true) => format!(
            "Keeping up to {}, starred kept forever. {used} used.",
            fmt_gb(max_gb)
        ),
        (false, false) => format!("Keeping everything. {used} used."),
    }
}

/// Results line above the grid.
pub fn results_line(count: usize, more: bool, search: &str) -> String {
    let n = if more {
        format!("{}+", uc::fmt_thousands(count as u64))
    } else {
        uc::fmt_thousands(count as u64)
    };
    let noun = if count == 1 && !more {
        "capture"
    } else {
        "captures"
    };
    let q = search.trim();
    if q.is_empty() {
        format!("{n} {noun}")
    } else if count == 1 && !more {
        format!("1 capture contains text matching \u{201c}{q}\u{201d}")
    } else {
        format!("{n} captures contain text matching \u{201c}{q}\u{201d}")
    }
}

/// Things you can do with an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Edit,
    Pin,
    CopyImage,
    CopyText,
    Open,
    Reveal,
    ToggleStar,
    Delete,
}

impl Act {
    pub const MENU: [Act; 8] = [
        Act::Edit,
        Act::Pin,
        Act::CopyImage,
        Act::CopyText,
        Act::Open,
        Act::Reveal,
        Act::ToggleStar,
        Act::Delete,
    ];

    pub fn label(self, m: &ItemMeta) -> &'static str {
        match self {
            Act::Edit => "Edit",
            Act::Pin => "Pin to Screen",
            Act::CopyImage => "Copy Image",
            Act::CopyText => "Copy Text",
            Act::Open => "Open",
            Act::Reveal => "Show in Folder",
            Act::ToggleStar => {
                if m.starred {
                    "Remove Star"
                } else {
                    "Star"
                }
            }
            Act::Delete => "Delete\u{2026}",
        }
    }

    /// Whether the action applies to this item.
    pub fn enabled(self, m: &ItemMeta, ocr_available: bool) -> bool {
        let image = m.kind == ItemKind::Image;
        match self {
            Act::Edit | Act::Pin | Act::CopyImage => image,
            Act::CopyText => {
                image
                    && ocr_available
                    && m.ocr_text.as_deref().is_some_and(|t| !t.trim().is_empty())
            }
            Act::Open | Act::Reveal | Act::ToggleStar | Act::Delete => true,
        }
    }

    pub fn disabled_reason(self, m: &ItemMeta, ocr_available: bool) -> Option<&'static str> {
        if self.enabled(m, ocr_available) {
            return None;
        }
        Some(match self {
            Act::CopyText if !ocr_available => "No OCR language is installed",
            Act::CopyText if m.kind == ItemKind::Image && m.ocr_text.is_none() => {
                "Text is still being indexed"
            }
            Act::CopyText if m.kind == ItemKind::Image => "No text found in this capture",
            _ => "Only for images",
        })
    }

    /// The shell command for an action (Delete is confirmed first, so it has none here).
    pub fn command(self, m: &ItemMeta) -> Option<Command> {
        Some(match self {
            Act::Edit => Command::EditItem(m.id),
            Act::Pin => Command::PinItem(m.id),
            Act::CopyImage => Command::CopyItemImage(m.id),
            Act::CopyText => Command::CopyItemText(m.id),
            Act::Open => Command::OpenItem(m.id),
            Act::Reveal => Command::RevealItem(m.id),
            Act::ToggleStar => Command::StarItem(m.id, !m.starred),
            Act::Delete => return None,
        })
    }
}

/// Default action for Enter / double click.
pub fn primary_act(m: &ItemMeta) -> Act {
    if m.kind == ItemKind::Image {
        Act::Edit
    } else {
        Act::Open
    }
}

// ---------------------------------------------------------------------------------------
// Thumbnail worker
// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ThumbKey {
    id: ItemId,
    bytes: u64,
}

struct ThumbResult {
    key: ThumbKey,
    image: Option<egui::ColorImage>,
}

struct Queue {
    reqs: Vec<ThumbKey>,
    closed: bool,
}

struct ThumbWorker {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    results: Receiver<ThumbResult>,
}

impl ThumbWorker {
    fn start(history: Arc<History>, ctx: egui::Context) -> Option<ThumbWorker> {
        let queue = Arc::new((
            Mutex::new(Queue {
                reqs: Vec::new(),
                closed: false,
            }),
            Condvar::new(),
        ));
        let (tx, rx): (Sender<ThumbResult>, Receiver<ThumbResult>) = channel();
        let q = Arc::clone(&queue);
        let ok = uc::spawn_worker("history-thumbs", move || {
            loop {
                let key = {
                    let (lock, cv) = &*q;
                    let Ok(mut g) = lock.lock() else { return };
                    loop {
                        if g.closed {
                            return;
                        }
                        // Newest request first.
                        if let Some(k) = g.reqs.pop() {
                            break k;
                        }
                        g = match cv.wait(g) {
                            Ok(g) => g,
                            Err(_) => return,
                        };
                    }
                };
                let image = load_thumb(&history, key.id);
                if tx.send(ThumbResult { key, image }).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
        ok.then_some(ThumbWorker { queue, results: rx })
    }

    /// Queue keys so that `keys[0]` is served first.
    fn enqueue(&self, keys: &[ThumbKey]) {
        let (lock, cv) = &*self.queue;
        if let Ok(mut g) = lock.lock() {
            for k in keys.iter().rev() {
                g.reqs.push(*k);
            }
            cv.notify_one();
        }
    }
}

impl Drop for ThumbWorker {
    fn drop(&mut self) {
        let (lock, cv) = &*self.queue;
        if let Ok(mut g) = lock.lock() {
            g.closed = true;
        }
        cv.notify_all();
    }
}

fn load_thumb(history: &History, id: ItemId) -> Option<egui::ColorImage> {
    let path = match history.thumbnail(id, THUMB_MAX) {
        Ok(Some(p)) => p,
        Ok(None) => return None,
        Err(e) => {
            log::debug!("thumbnail for {id} failed: {e:#}");
            return None;
        }
    };
    let img: Rgba = match Rgba::load(&path) {
        Ok(i) => i,
        Err(e) => {
            log::debug!("thumbnail decode {} failed: {e:#}", path.display());
            return None;
        }
    };
    if img.width == 0 || img.height == 0 {
        return None;
    }
    let img = if img.width > THUMB_MAX * 2 || img.height > THUMB_MAX * 2 {
        img.thumbnail(THUMB_MAX, THUMB_MAX)
    } else {
        img
    };
    Some(uc::rgba_to_color_image(&img))
}

// ---------------------------------------------------------------------------------------
// Window
// ---------------------------------------------------------------------------------------

pub struct HistoryWindow {
    filter: Filter,
    search: String,
    applied_search: String,
    debounce: Debounce<String>,
    date: DateFilter,
    items: Vec<ItemMeta>,
    limit: usize,
    more: bool,
    stats: Stats,
    apps: Vec<(String, u64)>,
    dirty: bool,
    requery_at: Option<Instant>,
    selected: Option<ItemId>,
    confirm_delete: Option<ItemId>,
    textures: Lru<ThumbKey, egui::TextureHandle>,
    requested: HashSet<ThumbKey>,
    failed: HashSet<ThumbKey>,
    worker: Option<ThumbWorker>,
    /// The history `worker` reads from; a different one in `Shared` means the folder was
    /// changed in Settings and everything keyed by item id is stale.
    worker_history: Option<Arc<History>>,
    scroll_to: Option<ItemId>,
    focus_search: bool,
    was_focused: bool,
    cols: usize,
    hwnd: Option<Hwnd>,
    load_error: Option<String>,
}

impl HistoryWindow {
    pub fn new(shared: &Shared) -> Self {
        let mut w = Self {
            filter: Filter::All,
            search: String::new(),
            applied_search: String::new(),
            debounce: Debounce::new(Duration::from_millis(150)),
            date: DateFilter::Any,
            items: Vec::new(),
            limit: PAGE,
            more: false,
            stats: Stats::default(),
            apps: Vec::new(),
            dirty: false,
            requery_at: None,
            selected: None,
            confirm_delete: None,
            textures: Lru::new(TEX_CACHE),
            requested: HashSet::new(),
            failed: HashSet::new(),
            worker: None,
            worker_history: None,
            scroll_to: None,
            focus_search: false,
            was_focused: true,
            cols: 3,
            hwnd: None,
            load_error: None,
        };
        w.refresh(shared);
        w
    }

    /// Shell calls this after history changes (new capture, OCR finished) to refresh.
    pub fn invalidate(&mut self) {
        self.dirty = true;
        self.failed.clear();
    }

    fn query_for(&self, now: DateTime<Local>) -> Query {
        let text = self.applied_search.trim();
        Query {
            text: if text.is_empty() {
                None
            } else {
                Some(text.to_string())
            },
            app: if let Filter::App(a) = &self.filter {
                Some(a.clone())
            } else {
                None
            },
            kind: None,
            starred_only: self.filter == Filter::Starred,
            since: self.date.since(now),
            limit: self.limit,
            offset: 0,
        }
    }

    fn refresh(&mut self, shared: &Shared) {
        self.dirty = false;
        self.requery_at = None;
        let now = Local::now();
        let base = self.query_for(now);
        let result = if self.filter == Filter::Recordings {
            let mut all = Vec::new();
            for kind in [ItemKind::Video, ItemKind::Gif] {
                match shared.history.query(&Query {
                    kind: Some(kind),
                    ..base.clone()
                }) {
                    Ok(v) => all.extend(v),
                    Err(e) => {
                        self.load_error = Some(format!("{e:#}"));
                        return;
                    }
                }
            }
            all.sort_by_key(|m| std::cmp::Reverse(m.created));
            all.truncate(self.limit);
            Ok(all)
        } else {
            shared.history.query(&base)
        };
        match result {
            Ok(v) => {
                self.more = self.limit > 0 && v.len() >= self.limit;
                self.items = v;
                self.load_error = None;
            }
            Err(e) => {
                log::warn!("history query failed: {e:#}");
                self.load_error = Some(format!("{e:#}"));
            }
        }
        if let Ok(s) = shared.history.stats() {
            self.stats = s;
        }
        if let Ok(a) = shared.history.apps() {
            self.apps = a;
        }
        if let Some(sel) = self.selected
            && !self.items.iter().any(|m| m.id == sel)
        {
            self.selected = None;
        }
    }

    fn selected_index(&self) -> Option<usize> {
        let id = self.selected?;
        self.items.iter().position(|m| m.id == id)
    }

    fn select_index(&mut self, i: usize) {
        if let Some(m) = self.items.get(i) {
            self.selected = Some(m.id);
            self.scroll_to = Some(m.id);
        }
    }

    fn filtering(&self) -> bool {
        self.filter != Filter::All
            || !self.applied_search.trim().is_empty()
            || self.date != DateFilter::Any
    }

    fn clear_filters(&mut self) {
        self.filter = Filter::All;
        self.search.clear();
        self.applied_search.clear();
        self.debounce.cancel();
        self.date = DateFilter::Any;
        self.limit = PAGE;
        self.dirty = true;
    }

    fn pump_thumbs(&mut self, ctx: &egui::Context) {
        let Some(w) = &self.worker else { return };
        let mut uploads = 0;
        loop {
            if uploads >= UPLOADS_PER_FRAME {
                ctx.request_repaint();
                break;
            }
            match w.results.try_recv() {
                Ok(res) => match res.image {
                    Some(img) => {
                        let tex = ctx.load_texture(
                            format!("hist_{}_{}", res.key.id, res.key.bytes),
                            img,
                            egui::TextureOptions::LINEAR,
                        );
                        if let Some((evicted, _)) = self.textures.insert(res.key, tex) {
                            self.requested.remove(&evicted);
                        }
                        uploads += 1;
                    }
                    None => {
                        self.failed.insert(res.key);
                    }
                },
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
    }

    fn do_act(&mut self, shared: &mut Shared, act: Act, id: ItemId) {
        let Some(idx) = self.items.iter().position(|m| m.id == id) else {
            return;
        };
        let meta = self.items[idx].clone();
        if !act.enabled(&meta, shared.ocr.is_some()) {
            return;
        }
        match act {
            Act::Delete => self.confirm_delete = Some(id),
            Act::ToggleStar => {
                let starred = !meta.starred;
                self.items[idx].starred = starred;
                self.stats.starred = if starred {
                    self.stats.starred + 1
                } else {
                    self.stats.starred.saturating_sub(1)
                };
                shared.outbox.push(Command::StarItem(id, starred));
                self.requery_at = Some(Instant::now() + Duration::from_millis(700));
            }
            other => {
                if let Some(c) = other.command(&meta) {
                    shared.outbox.push(c);
                }
            }
        }
    }

    fn confirm_delete_now(&mut self, shared: &mut Shared, id: ItemId) {
        shared.outbox.push(Command::DeleteItem(id));
        if let Some(idx) = self.items.iter().position(|m| m.id == id) {
            let m = self.items.remove(idx);
            self.stats.count = self.stats.count.saturating_sub(1);
            if m.starred {
                self.stats.starred = self.stats.starred.saturating_sub(1);
            }
            if m.kind != ItemKind::Image {
                self.stats.recordings = self.stats.recordings.saturating_sub(1);
            }
            // Keep the keyboard cursor near where the item was.
            self.selected = self
                .items
                .get(idx.min(self.items.len().saturating_sub(1)))
                .map(|m| m.id);
        }
        self.requery_at = Some(Instant::now() + Duration::from_millis(500));
    }
}

impl Surface for HistoryWindow {
    fn title(&self) -> String {
        "StinkShot History".into()
    }

    fn viewport_builder(&self, _shared: &Shared) -> egui::ViewportBuilder {
        egui::ViewportBuilder::default()
            .with_title(self.title())
            .with_inner_size(vec2(1280.0, 820.0))
            .with_min_inner_size(vec2(900.0, 560.0))
            .with_resizable(true)
    }

    fn on_created(&mut self, hwnd: Hwnd, shared: &Shared) {
        self.hwnd = Some(hwnd);
        stinkshot_platform::window_util::apply_mica(hwnd, shared.dark);
        stinkshot_platform::window_util::force_foreground(hwnd);
    }

    fn show(&mut self, ctx: &egui::Context, shared: &mut Shared) -> SurfaceStatus {
        let now_i = Instant::now();
        if self
            .worker_history
            .as_ref()
            .is_some_and(|h| !Arc::ptr_eq(h, &shared.history))
        {
            // Item ids, thumbnails and textures all belong to the old index.
            self.worker = None;
            self.worker_history = None;
            self.textures = Lru::new(TEX_CACHE);
            self.requested.clear();
            self.failed.clear();
            self.dirty = true;
        }
        if self.worker.is_none() {
            self.worker = ThumbWorker::start(Arc::clone(&shared.history), ctx.clone());
            self.worker_history = Some(Arc::clone(&shared.history));
        }
        self.pump_thumbs(ctx);

        // Refresh triggers: external invalidation, regained focus, debounced search, deferred
        // re-sync after local edits, paging.
        let focused = ctx.input(|i| i.viewport().focused).unwrap_or(true);
        if focused && !self.was_focused {
            self.dirty = true;
        }
        self.was_focused = focused;
        if let Some(text) = self.debounce.poll(now_i)
            && text != self.applied_search
        {
            self.applied_search = text;
            self.limit = PAGE;
            self.dirty = true;
        }
        if let Some(rem) = self.debounce.remaining(now_i) {
            ctx.request_repaint_after(rem);
        }
        if let Some(at) = self.requery_at {
            if now_i >= at {
                self.dirty = true;
            } else {
                ctx.request_repaint_after(at - now_i);
            }
        }
        if self.dirty {
            self.refresh(shared);
        }

        let p = uc::pal(shared);
        let accent = uc::accent(shared);
        let now = Local::now();
        let ocr_available = shared.ocr.is_some();
        let hotkey_hint = crate::settings_ui::ui_hotkey::display(&shared.config.hotkeys.region);
        let retention = retention_summary(
            shared.config.history.keep_days,
            shared.config.history.max_gb,
            self.stats.bytes,
        );
        let folder = shared.history.folder().to_path_buf();

        // Keyboard (not while typing in the search box or a dialog).
        let typing = ctx.egui_wants_keyboard_input();
        let mut want_act: Option<(Act, ItemId)> = None;
        if self.confirm_delete.is_none() {
            let (cmd_f, esc) = ctx.input(|i| {
                (
                    i.modifiers.command && i.key_pressed(Key::F),
                    i.key_pressed(Key::Escape),
                )
            });
            if cmd_f {
                self.focus_search = true;
            }
            if !typing {
                let nav = ctx.input(|i| {
                    [
                        (Key::ArrowLeft, Nav::Left),
                        (Key::ArrowRight, Nav::Right),
                        (Key::ArrowUp, Nav::Up),
                        (Key::ArrowDown, Nav::Down),
                        (Key::Home, Nav::Home),
                        (Key::End, Nav::End),
                    ]
                    .into_iter()
                    .find(|(k, _)| i.key_pressed(*k))
                    .map(|(_, n)| n)
                });
                if let Some(n) = nav
                    && let Some(i) =
                        move_selection(self.selected_index(), n, self.cols, self.items.len())
                {
                    self.select_index(i);
                }
                if let Some(idx) = self.selected_index() {
                    let id = self.items[idx].id;
                    let (enter, del, copy) = ctx.input(|i| {
                        (
                            i.key_pressed(Key::Enter),
                            i.key_pressed(Key::Delete),
                            i.events.iter().any(|e| matches!(e, egui::Event::Copy)),
                        )
                    });
                    if enter {
                        want_act = Some((primary_act(&self.items[idx]), id));
                    } else if del {
                        want_act = Some((Act::Delete, id));
                    } else if copy {
                        want_act = Some((Act::CopyImage, id));
                    }
                }
            }
            if esc {
                if self.selected.is_some() && !typing {
                    self.selected = None;
                } else if !self.search.is_empty() {
                    self.search.clear();
                    self.debounce.push(String::new(), now_i);
                }
            }
        }

        // Frame state captured by the drawing closure (disjoint fields, no `self` methods).
        let mut new_filter: Option<Filter> = None;
        let mut new_date: Option<DateFilter> = None;
        let mut clear_filters = false;
        let mut open_folder = false;
        let mut capture_region = false;
        let mut clicked: Option<ItemId> = None;
        let mut act_req: Option<(Act, ItemId)> = want_act;
        let mut to_request: Vec<ThumbKey> = Vec::new();
        let mut want_more = false;
        let mut search_changed = false;
        let focus_search = std::mem::take(&mut self.focus_search);
        let mut cols_out = self.cols;

        let filter = self.filter.clone();
        let stats = self.stats.clone();
        let total_empty = stats.count == 0 && !self.filtering();
        let load_error = self.load_error.clone();
        let selected = self.selected;
        let scroll_to = self.scroll_to.take();
        let date = self.date;
        let more = self.more;
        let applied = self.applied_search.clone();
        let queue_room =
            MAX_QUEUED.saturating_sub(self.requested.len().saturating_sub(self.textures.len()));
        let items = &self.items;
        let apps = &self.apps;
        let textures = &mut self.textures;
        let requested = &self.requested;
        let failed = &self.failed;
        let search = &mut self.search;

        uc::root_ui(ctx, p.bg, |ui, rect| {
            let sidebar = Rect::from_min_size(
                rect.min + vec2(0.0, 8.0),
                vec2(SIDEBAR_W, rect.height() - 8.0),
            );
            let main = Rect::from_min_max(
                pos2(rect.left() + SIDEBAR_W, rect.top() + 8.0),
                rect.right_bottom(),
            );
            ui.painter().rect_filled(
                main,
                CornerRadius {
                    nw: 8,
                    ne: 0,
                    sw: 0,
                    se: 0,
                },
                p.layer,
            );
            ui.painter().line_segment(
                [
                    main.left_top() + vec2(8.0, 0.5),
                    main.right_top() + vec2(0.0, 0.5),
                ],
                Stroke::new(1.0, p.stroke),
            );
            ui.painter().line_segment(
                [
                    main.left_top() + vec2(0.5, 8.0),
                    main.left_bottom() + vec2(0.5, 0.0),
                ],
                Stroke::new(1.0, p.stroke),
            );

            // ---------------- Sidebar ----------------
            let sel_fill = if shared.dark {
                Color32::from_white_alpha(18)
            } else {
                Color32::from_black_alpha(14)
            };
            let hover_fill = if shared.dark {
                Color32::from_white_alpha(10)
            } else {
                Color32::from_black_alpha(8)
            };
            let mut y = sidebar.top() + 8.0;
            let nav_row = |ui: &mut egui::Ui,
                           y: &mut f32,
                           h: f32,
                           label: &str,
                           count: Option<u64>,
                           active: bool,
                           id: String|
             -> bool {
                let r =
                    Rect::from_min_size(pos2(sidebar.left() + 12.0, *y), vec2(SIDEBAR_W - 24.0, h));
                *y += h + 2.0;
                let resp = ui.interact(r, ui.scope_id().with(("hist_nav", id)), Sense::click());
                if active {
                    ui.painter().rect_filled(r, CornerRadius::same(6), sel_fill);
                    ui.painter().rect_filled(
                        Rect::from_min_size(
                            pos2(r.left() + 2.0, r.center().y - 8.0),
                            vec2(3.0, 16.0),
                        ),
                        CornerRadius::same(2),
                        accent,
                    );
                } else if resp.hovered() {
                    ui.painter()
                        .rect_filled(r, CornerRadius::same(6), hover_fill);
                }
                let mut job = egui::text::LayoutJob::single_section(
                    label.to_string(),
                    egui::TextFormat::simple(uc::font(13.0), p.text),
                );
                job.wrap = egui::text::TextWrapping {
                    max_width: r.width() - 20.0 - 52.0,
                    max_rows: 1,
                    break_anywhere: true,
                    overflow_character: Some('\u{2026}'),
                };
                let g = ui.painter().layout_job(job);
                ui.painter().galley(
                    pos2(r.left() + 12.0, r.center().y - g.size().y / 2.0),
                    g,
                    p.text,
                );
                if let Some(c) = count {
                    ui.painter().text(
                        pos2(r.right() - 10.0, r.center().y),
                        Align2::RIGHT_CENTER,
                        uc::fmt_thousands(c),
                        uc::font(13.0),
                        p.text_tertiary,
                    );
                }
                resp.clicked()
            };
            if nav_row(
                ui,
                &mut y,
                36.0,
                "All captures",
                Some(stats.count),
                filter == Filter::All,
                "all".into(),
            ) {
                new_filter = Some(Filter::All);
            }
            if nav_row(
                ui,
                &mut y,
                36.0,
                "Starred",
                Some(stats.starred),
                filter == Filter::Starred,
                "starred".into(),
            ) {
                new_filter = Some(Filter::Starred);
            }
            if nav_row(
                ui,
                &mut y,
                36.0,
                "Recordings",
                Some(stats.recordings),
                filter == Filter::Recordings,
                "rec".into(),
            ) {
                new_filter = Some(Filter::Recordings);
            }
            y += 14.0;
            ui.painter().text(
                pos2(sidebar.left() + 22.0, y),
                Align2::LEFT_TOP,
                "Source app",
                uc::font(12.0),
                p.text_tertiary,
            );
            y += 24.0;

            // Footer first so the app list knows how much room it has.
            let footer_g = ui.painter().layout(
                retention.clone(),
                uc::font(12.0),
                p.text_tertiary,
                SIDEBAR_W - 44.0,
            );
            let footer_h = footer_g.size().y + 24.0;
            let footer_top = sidebar.bottom() - footer_h;
            ui.painter().galley(
                pos2(sidebar.left() + 22.0, footer_top + 12.0),
                footer_g,
                p.text_tertiary,
            );
            let list_rect = Rect::from_min_max(
                pos2(sidebar.left(), y),
                pos2(sidebar.right(), (footer_top - 4.0).max(y)),
            );
            ui.scope_builder(UiBuilder::new().max_rect(list_rect), |ui| {
                ScrollArea::vertical()
                    .id_salt("hist_apps")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let mut ly = ui.cursor().top();
                        let _ = &mut ly;
                        for (name, count) in apps.iter() {
                            let active = matches!(&filter, Filter::App(a) if a == name);
                            let r = Rect::from_min_size(
                                pos2(sidebar.left() + 12.0, ui.cursor().top()),
                                vec2(SIDEBAR_W - 24.0 - 6.0, 34.0),
                            );
                            ui.allocate_rect(r, Sense::hover());
                            let resp = ui.interact(
                                r,
                                ui.scope_id().with(("hist_app", name.as_str())),
                                Sense::click(),
                            );
                            if active {
                                ui.painter().rect_filled(r, CornerRadius::same(6), sel_fill);
                                ui.painter().rect_filled(
                                    Rect::from_min_size(
                                        pos2(r.left() + 2.0, r.center().y - 8.0),
                                        vec2(3.0, 16.0),
                                    ),
                                    CornerRadius::same(2),
                                    accent,
                                );
                            } else if resp.hovered() {
                                ui.painter()
                                    .rect_filled(r, CornerRadius::same(6), hover_fill);
                            }
                            ui.painter().text(
                                pos2(r.left() + 12.0, r.center().y),
                                Align2::LEFT_CENTER,
                                pretty_app(name),
                                uc::font(13.0),
                                p.text,
                            );
                            ui.painter().text(
                                pos2(r.right() - 10.0, r.center().y),
                                Align2::RIGHT_CENTER,
                                uc::fmt_thousands(*count),
                                uc::font(13.0),
                                p.text_tertiary,
                            );
                            if resp.clicked() {
                                new_filter = Some(Filter::App(name.clone()));
                            }
                            ui.add_space(2.0);
                        }
                    });
            });

            // ---------------- Main: header ----------------
            let inner = main.shrink2(vec2(22.0, 18.0));
            let header = Rect::from_min_size(inner.min, vec2(inner.width(), 36.0));
            let folder_w = 108.0;
            let date_w = 130.0;
            let search_rect = Rect::from_min_max(
                header.min,
                pos2(header.right() - folder_w - date_w - 20.0, header.bottom()),
            );
            let search_id = ui.scope_id().with("hist_search");
            let search_focused = ui.memory(|m| m.has_focus(search_id));
            ui.painter()
                .rect_filled(search_rect, CornerRadius::same(6), p.bg);
            ui.painter().rect_stroke(
                search_rect,
                CornerRadius::same(6),
                Stroke::new(1.0, p.stroke),
                StrokeKind::Inside,
            );
            ui.painter().rect_filled(
                Rect::from_min_max(
                    pos2(search_rect.left() + 3.0, search_rect.bottom() - 2.0),
                    pos2(search_rect.right() - 3.0, search_rect.bottom()),
                ),
                CornerRadius::same(1),
                if search_focused || !search.is_empty() {
                    accent
                } else {
                    p.stroke
                },
            );
            widgets::paint_icon(
                ui.painter(),
                Rect::from_center_size(
                    pos2(search_rect.left() + 20.0, search_rect.center().y),
                    vec2(16.0, 16.0),
                ),
                Icon::Search,
                p.text_secondary,
            );
            let te_rect = Rect::from_min_max(
                pos2(search_rect.left() + 38.0, search_rect.top() + 2.0),
                pos2(search_rect.right() - 34.0, search_rect.bottom() - 4.0),
            );
            let te = ui.scope_builder(
                UiBuilder::new()
                    .max_rect(te_rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                |ui| {
                    ui.add(
                        egui::TextEdit::singleline(search)
                            .id(search_id)
                            .frame(egui::Frame::NONE)
                            .hint_text("Search captures")
                            .font(uc::font(14.0))
                            .desired_width(te_rect.width()),
                    )
                },
            );
            if focus_search {
                te.inner.request_focus();
            }
            if te.inner.changed() {
                search_changed = true;
            }
            if !search.is_empty() {
                let cr = Rect::from_center_size(
                    pos2(search_rect.right() - 18.0, search_rect.center().y),
                    vec2(24.0, 24.0),
                );
                let cresp =
                    ui.interact(cr, ui.scope_id().with("hist_search_clear"), Sense::click());
                if cresp.hovered() {
                    ui.painter()
                        .rect_filled(cr, CornerRadius::same(5), hover_fill);
                }
                widgets::paint_icon(
                    ui.painter(),
                    Rect::from_center_size(cr.center(), vec2(12.0, 12.0)),
                    Icon::Close,
                    p.text_secondary,
                );
                if cresp.on_hover_text("Clear search").clicked() {
                    search.clear();
                    search_changed = true;
                }
            }
            let date_rect = Rect::from_min_size(
                pos2(search_rect.right() + 10.0, header.top()),
                vec2(date_w, 36.0),
            );
            ui.scope_builder(
                UiBuilder::new()
                    .max_rect(date_rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                |ui| {
                    let mut d = date;
                    egui::ComboBox::from_id_salt("hist_date")
                        .selected_text(d.label())
                        .width(date_w - 28.0)
                        .show_ui(ui, |ui| {
                            for opt in DateFilter::ALL {
                                ui.selectable_value(&mut d, opt, opt.label());
                            }
                        });
                    if d != date {
                        new_date = Some(d);
                    }
                },
            );
            let folder_rect = Rect::from_min_size(
                pos2(date_rect.right() + 10.0, header.top()),
                vec2(folder_w, 36.0),
            );
            if uc::rect_button(
                ui,
                "open_folder",
                folder_rect,
                "Open folder",
                uc::BtnKind::Neutral,
                true,
                p,
                accent,
            )
            .clicked()
            {
                open_folder = true;
            }

            // ---------------- Main: status line + notices ----------------
            let mut top = header.bottom() + 14.0;
            if !total_empty {
                let line = results_line(items.len(), more, &applied);
                ui.painter().text(
                    pos2(inner.left(), top),
                    Align2::LEFT_TOP,
                    line,
                    uc::font(13.0),
                    p.text_secondary,
                );
                top += 24.0;
            }
            if !ocr_available {
                let msg = "No OCR language is installed, so search only covers window titles and app names. Add one in Windows Settings under Time and language, then Language and region.";
                let g = ui.painter().layout(
                    msg.to_string(),
                    uc::font(12.0),
                    p.text_secondary,
                    inner.width() - 28.0,
                );
                let r = Rect::from_min_size(
                    pos2(inner.left(), top),
                    vec2(inner.width(), g.size().y + 20.0),
                );
                uc::card(ui, r, 8, p.card, p.stroke);
                ui.painter()
                    .galley(r.min + vec2(14.0, 10.0), g, p.text_secondary);
                top = r.bottom() + 12.0;
            }
            if let Some(err) = &load_error {
                let g = ui.painter().layout(
                    format!("Could not read the history index: {err}"),
                    uc::font(12.0),
                    p.danger,
                    inner.width() - 28.0,
                );
                let r = Rect::from_min_size(
                    pos2(inner.left(), top),
                    vec2(inner.width(), g.size().y + 20.0),
                );
                ui.painter()
                    .rect_filled(r, CornerRadius::same(8), uc::with_alpha(p.danger, 28));
                ui.painter().galley(r.min + vec2(14.0, 10.0), g, p.danger);
                top = r.bottom() + 12.0;
            }

            // ---------------- Main: action bar (bottom) ----------------
            let sel_meta = selected.and_then(|id| items.iter().find(|m| m.id == id));
            let bar_h = 56.0;
            let bar_rect = Rect::from_min_size(
                pos2(inner.left(), inner.bottom() - bar_h),
                vec2(inner.width(), bar_h),
            );
            let grid_bottom = if sel_meta.is_some() {
                bar_rect.top() - 12.0
            } else {
                inner.bottom()
            };
            let grid_rect = Rect::from_min_max(
                pos2(inner.left(), top),
                pos2(inner.right(), grid_bottom.max(top + 40.0)),
            );

            // ---------------- Main: grid / empty states ----------------
            if items.is_empty() {
                let c = Rect::from_center_size(
                    grid_rect.center(),
                    vec2(grid_rect.width().min(460.0), 220.0),
                );
                let (icon, title, body) = if total_empty {
                    (
                        Icon::History,
                        "No captures yet".to_string(),
                        format!(
                            "Press {hotkey_hint} to capture a region. Everything you capture is saved here, with searchable text."
                        ),
                    )
                } else {
                    (
                        Icon::Search,
                        "Nothing matches".to_string(),
                        "Try different words, another date range, or clear the filters."
                            .to_string(),
                    )
                };
                widgets::paint_icon(
                    ui.painter(),
                    Rect::from_center_size(pos2(c.center().x, c.top() + 20.0), vec2(40.0, 40.0)),
                    icon,
                    p.text_tertiary,
                );
                ui.painter().text(
                    pos2(c.center().x, c.top() + 62.0),
                    Align2::CENTER_TOP,
                    title,
                    uc::font(18.0),
                    p.text,
                );
                let g = ui
                    .painter()
                    .layout(body, uc::font(13.0), p.text_secondary, c.width());
                let gh = g.size().y;
                let gw = g.size().x;
                ui.painter().galley(
                    pos2(c.center().x - gw / 2.0, c.top() + 92.0),
                    g,
                    p.text_secondary,
                );
                let b = Rect::from_center_size(
                    pos2(c.center().x, c.top() + 92.0 + gh + 34.0),
                    vec2(180.0, 34.0),
                );
                if total_empty {
                    if uc::rect_button(
                        ui,
                        "empty_capture",
                        b,
                        "Capture a region",
                        uc::BtnKind::Accent,
                        true,
                        p,
                        accent,
                    )
                    .clicked()
                    {
                        capture_region = true;
                    }
                } else if uc::rect_button(
                    ui,
                    "empty_clear",
                    b,
                    "Clear filters",
                    uc::BtnKind::Neutral,
                    true,
                    p,
                    accent,
                )
                .clicked()
                {
                    clear_filters = true;
                }
            } else {
                let now_local = now;
                ui.scope_builder(UiBuilder::new().max_rect(grid_rect), |ui| {
                    ScrollArea::vertical()
                        .id_salt("hist_grid")
                        .auto_shrink([false, false])
                        .show_viewport(ui, |ui, viewport| {
                            let avail_w = ui.available_width();
                            let (cols, card_w) = grid_layout(avail_w, MIN_CARD_W, GAP);
                            cols_out = cols;
                            let nrows = items.len().div_ceil(cols);
                            let pitch = CARD_H + GAP;
                            let total_h = nrows as f32 * pitch;
                            let (full, _) =
                                ui.allocate_exact_size(vec2(avail_w, total_h), Sense::hover());
                            let cell = |i: usize| {
                                let (row, col) = (i / cols, i % cols);
                                Rect::from_min_size(
                                    full.min
                                        + vec2(col as f32 * (card_w + GAP), row as f32 * pitch),
                                    vec2(card_w, CARD_H),
                                )
                            };
                            if let Some(id) = scroll_to
                                && let Some(i) = items.iter().position(|m| m.id == id)
                            {
                                ui.scroll_to_rect(cell(i).expand(GAP / 2.0), None);
                            }
                            let rows =
                                visible_rows(viewport.min.y, viewport.height(), pitch, nrows, 1);
                            if rows.end >= nrows.saturating_sub(1) && more {
                                want_more = true;
                            }
                            let mut room = queue_room;
                            for row in rows {
                                for col in 0..cols {
                                    let i = row * cols + col;
                                    let Some(m) = items.get(i) else { break };
                                    let r = cell(i);
                                    if !ui.is_rect_visible(r) {
                                        continue;
                                    }
                                    let key = ThumbKey {
                                        id: m.id,
                                        bytes: m.bytes,
                                    };
                                    let tex = textures.get(&key).map(|t| (t.id(), t.size()));
                                    if tex.is_none()
                                        && !requested.contains(&key)
                                        && !failed.contains(&key)
                                        && m.kind != ItemKind::Video
                                        && room > 0
                                    {
                                        to_request.push(key);
                                        room -= 1;
                                    }
                                    let resp = ui.interact(
                                        r,
                                        ui.scope_id().with(("hist_card", m.id)),
                                        Sense::click(),
                                    );
                                    let hovered = resp.hovered();
                                    paint_card(
                                        ui,
                                        r,
                                        m,
                                        tex,
                                        p,
                                        accent,
                                        selected == Some(m.id),
                                        hovered,
                                        now_local,
                                        failed.contains(&key),
                                    );
                                    // Star button.
                                    let star_r = Rect::from_center_size(
                                        pos2(r.right() - 24.0, r.top() + 24.0),
                                        vec2(30.0, 30.0),
                                    );
                                    if m.starred || hovered {
                                        let sresp = ui.interact(
                                            star_r,
                                            ui.scope_id().with(("hist_star", m.id)),
                                            Sense::click(),
                                        );
                                        let (icon, col) = if m.starred {
                                            (Icon::StarFilled, STAR_COLOR)
                                        } else {
                                            (
                                                Icon::Star,
                                                Color32::from_white_alpha(if sresp.hovered() {
                                                    230
                                                } else {
                                                    150
                                                }),
                                            )
                                        };
                                        if sresp.hovered() {
                                            ui.painter().circle_filled(
                                                star_r.center(),
                                                15.0,
                                                Color32::from_black_alpha(110),
                                            );
                                        }
                                        widgets::paint_icon(
                                            ui.painter(),
                                            Rect::from_center_size(
                                                star_r.center(),
                                                vec2(18.0, 18.0),
                                            ),
                                            icon,
                                            col,
                                        );
                                        if sresp
                                            .on_hover_text(if m.starred {
                                                "Remove Star"
                                            } else {
                                                "Star"
                                            })
                                            .clicked()
                                        {
                                            act_req = Some((Act::ToggleStar, m.id));
                                            clicked = Some(m.id);
                                            continue;
                                        }
                                    }
                                    if resp.clicked() || resp.secondary_clicked() {
                                        clicked = Some(m.id);
                                    }
                                    if resp.double_clicked() {
                                        act_req = Some((primary_act(m), m.id));
                                    }
                                    resp.context_menu(|ui| {
                                        for act in Act::MENU {
                                            let enabled = act.enabled(m, ocr_available);
                                            let b = ui.add_enabled(
                                                enabled,
                                                egui::Button::new(act.label(m)),
                                            );
                                            let b = match act.disabled_reason(m, ocr_available) {
                                                Some(why) => b.on_disabled_hover_text(why),
                                                None => b,
                                            };
                                            if b.clicked() {
                                                act_req = Some((act, m.id));
                                                clicked = Some(m.id);
                                                ui.close();
                                            }
                                        }
                                    });
                                }
                            }
                        });
                });
            }

            // Action bar contents.
            if let Some(m) = sel_meta {
                uc::card(ui, bar_rect, 8, p.card, p.stroke);
                let text_w = (bar_rect.width() - 28.0 - 8.0 * 36.0 - 130.0).max(80.0);
                let mut job = egui::text::LayoutJob::single_section(
                    item_title(m),
                    egui::TextFormat::simple(uc::font(13.0), p.text),
                );
                job.wrap = egui::text::TextWrapping {
                    max_width: text_w,
                    max_rows: 1,
                    break_anywhere: true,
                    overflow_character: Some('\u{2026}'),
                };
                let g = ui.painter().layout_job(job);
                ui.painter().galley(
                    pos2(bar_rect.left() + 14.0, bar_rect.top() + 10.0),
                    g,
                    p.text,
                );
                ui.painter().text(
                    pos2(bar_rect.left() + 14.0, bar_rect.top() + 31.0),
                    Align2::LEFT_TOP,
                    format!(
                        "{} \u{b7} {}",
                        item_meta_line(m, now),
                        uc::fmt_bytes(m.bytes)
                    ),
                    uc::font(12.0),
                    p.text_tertiary,
                );
                let btns = Rect::from_min_max(
                    pos2(bar_rect.left() + 14.0 + text_w + 8.0, bar_rect.top()),
                    pos2(bar_rect.right() - 10.0, bar_rect.bottom()),
                );
                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(btns)
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let mut icon_act =
                            |ui: &mut egui::Ui, act: Act, icon: Icon, selected_look: bool| {
                                let enabled = act.enabled(m, ocr_available);
                                let tip = act
                                    .disabled_reason(m, ocr_available)
                                    .map(|w| format!("{} ({w})", act.label(m)))
                                    .unwrap_or_else(|| act.label(m).to_string());
                                let r = ui
                                    .add_enabled_ui(enabled, |ui| {
                                        widgets::icon_button(ui, icon, &tip, selected_look)
                                    })
                                    .inner;
                                if r.clicked() {
                                    act_req = Some((act, m.id));
                                }
                            };
                        // Right to left: rightmost first.
                        icon_act(ui, Act::Delete, Icon::Trash, false);
                        icon_act(
                            ui,
                            Act::ToggleStar,
                            if m.starred {
                                Icon::StarFilled
                            } else {
                                Icon::Star
                            },
                            m.starred,
                        );
                        icon_act(ui, Act::Reveal, Icon::Folder, false);
                        icon_act(ui, Act::CopyText, Icon::Ocr, false);
                        icon_act(ui, Act::CopyImage, Icon::Copy, false);
                        icon_act(ui, Act::Pin, Icon::Pin, false);
                        let primary = primary_act(m);
                        let label = if primary == Act::Edit { "Edit" } else { "Open" };
                        if widgets::accent_button(ui, label).clicked() {
                            act_req = Some((primary, m.id));
                        }
                    },
                );
            }
        });

        // ---------------- Apply the frame's results ----------------
        self.cols = cols_out;
        if search_changed {
            self.debounce.push(self.search.clone(), Instant::now());
            ctx.request_repaint_after(Duration::from_millis(160));
        }
        if let Some(f) = new_filter
            && f != self.filter
        {
            self.filter = f;
            self.limit = PAGE;
            self.dirty = true;
            self.selected = None;
        }
        if let Some(d) = new_date {
            self.date = d;
            self.limit = PAGE;
            self.dirty = true;
        }
        if clear_filters {
            self.clear_filters();
        }
        if want_more && !self.dirty {
            self.limit += PAGE;
            self.dirty = true;
        }
        if capture_region {
            shared.outbox.push(Command::CaptureRegion);
        }
        if open_folder && let Err(e) = stinkshot_platform::system::open_path(&folder) {
            shared.outbox.push(Command::Notify {
                title: "Could not open folder".into(),
                body: e.to_string(),
                error: true,
            });
        }
        if let Some(id) = clicked {
            self.selected = Some(id);
        }
        if let Some((act, id)) = act_req {
            self.do_act(shared, act, id);
        }
        if !to_request.is_empty()
            && let Some(w) = &self.worker
        {
            w.enqueue(&to_request);
            self.requested.extend(to_request);
        }

        // Delete confirmation.
        if let Some(id) = self.confirm_delete {
            let meta = self.items.iter().find(|m| m.id == id).cloned();
            match meta {
                None => self.confirm_delete = None,
                Some(m) => {
                    let name = item_title(&m);
                    let starred = m.starred;
                    let modal =
                        egui::Modal::new(egui::Id::unique("hist_delete_modal")).show(ctx, |ui| {
                            ui.set_width(360.0);
                            ui.label(
                                RichText::new("Delete this capture?")
                                    .size(15.0)
                                    .color(p.text),
                            );
                            ui.add_space(4.0);
                            ui.label(RichText::new(name).size(12.0).color(p.text_secondary));
                            ui.add_space(2.0);
                            let extra = if starred { "It is starred. " } else { "" };
                            ui.label(
                                RichText::new(format!(
                                    "{extra}The file is removed from disk and cannot be restored."
                                ))
                                .size(12.0)
                                .color(p.text_tertiary),
                            );
                            ui.add_space(14.0);
                            let (mut cancel, mut del) = (false, false);
                            ui.horizontal(|ui| {
                                let (r1, _) =
                                    ui.allocate_exact_size(vec2(170.0, 34.0), Sense::hover());
                                cancel = uc::rect_button(
                                    ui,
                                    "del_cancel",
                                    r1,
                                    "Cancel",
                                    uc::BtnKind::Neutral,
                                    true,
                                    p,
                                    accent,
                                )
                                .clicked();
                                let (r2, _) =
                                    ui.allocate_exact_size(vec2(170.0, 34.0), Sense::hover());
                                del = uc::rect_button(
                                    ui,
                                    "del_ok",
                                    r2,
                                    "Delete",
                                    uc::BtnKind::Danger,
                                    true,
                                    p,
                                    accent,
                                )
                                .clicked();
                            });
                            (cancel, del)
                        });
                    let (cancel, del) = modal.inner;
                    if del {
                        self.confirm_delete = None;
                        self.confirm_delete_now(shared, id);
                    } else if cancel || modal.should_close() {
                        self.confirm_delete = None;
                    }
                }
            }
        }

        if ctx.input(|i| i.viewport().close_requested()) {
            SurfaceStatus::Close
        } else {
            SurfaceStatus::Open
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_card(
    ui: &mut egui::Ui,
    r: Rect,
    m: &ItemMeta,
    tex: Option<(egui::TextureId, [usize; 2])>,
    p: &theme::Palette,
    accent: Color32,
    selected: bool,
    hovered: bool,
    now: DateTime<Local>,
    failed: bool,
) {
    let painter = ui.painter().clone();
    painter.rect_filled(r, CornerRadius::same(8), p.card);
    let thumb = Rect::from_min_size(r.min, vec2(r.width(), THUMB_H));
    let top_round = CornerRadius {
        nw: 8,
        ne: 8,
        sw: 0,
        se: 0,
    };
    painter.rect_filled(thumb, top_round, p.bg);
    match tex {
        Some((id, size)) => {
            let (u0, v0, u1, v1) = uc::cover_uv(
                (size[0] as f32, size[1] as f32),
                thumb.width(),
                thumb.height(),
            );
            painter.add(
                egui::epaint::RectShape::filled(thumb, top_round, Color32::WHITE)
                    .with_texture(id, Rect::from_min_max(pos2(u0, v0), pos2(u1, v1))),
            );
        }
        None => {
            let icon = if m.kind == ItemKind::Image {
                Icon::Search
            } else {
                Icon::Play
            };
            let _ = icon;
            if failed || m.kind == ItemKind::Video {
                // No thumbnail will ever arrive: show the type instead of a spinner.
                let label = match m.kind {
                    ItemKind::Video => "MP4",
                    ItemKind::Gif => "GIF",
                    ItemKind::Image => "Preview unavailable",
                };
                painter.text(
                    thumb.center() + vec2(0.0, 22.0),
                    Align2::CENTER_CENTER,
                    label,
                    uc::font(12.0),
                    p.text_tertiary,
                );
                widgets::paint_icon(
                    &painter,
                    Rect::from_center_size(thumb.center() - vec2(0.0, 8.0), vec2(28.0, 28.0)),
                    if m.kind == ItemKind::Image {
                        Icon::Search
                    } else {
                        Icon::Play
                    },
                    p.text_tertiary,
                );
            } else {
                // Loading shimmer: static soft bars so a screenful of cards does not repaint forever.
                let bar = |y: f32, w: f32, a: u8| {
                    painter.rect_filled(
                        Rect::from_min_size(
                            thumb.min + vec2(18.0, y),
                            vec2(thumb.width() * w, 8.0),
                        ),
                        CornerRadius::same(3),
                        Color32::from_white_alpha(a),
                    );
                };
                bar(22.0, 0.6, 22);
                bar(38.0, 0.4, 14);
            }
        }
    }
    if m.kind != ItemKind::Image {
        // Play badge + kind tag for video-like items.
        painter.circle_filled(thumb.center(), 20.0, Color32::from_black_alpha(150));
        widgets::paint_icon(
            &painter,
            Rect::from_center_size(thumb.center(), vec2(18.0, 18.0)),
            Icon::Play,
            Color32::WHITE,
        );
        let tag = if m.kind == ItemKind::Gif {
            "GIF"
        } else {
            "MP4"
        };
        let g = painter.layout_no_wrap(tag.to_string(), uc::font(11.0), Color32::WHITE);
        let tr = Rect::from_min_size(
            pos2(thumb.left() + 8.0, thumb.bottom() - 8.0 - g.size().y - 6.0),
            g.size() + vec2(10.0, 6.0),
        );
        painter.rect_filled(tr, CornerRadius::same(4), Color32::from_black_alpha(160));
        painter.galley(tr.min + vec2(5.0, 3.0), g, Color32::WHITE);
    }

    // Info block.
    let text_w = r.width() - 24.0;
    let mut job = egui::text::LayoutJob::single_section(
        item_title(m),
        egui::TextFormat::simple(uc::font(13.0), p.text),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: text_w,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('\u{2026}'),
    };
    let g = painter.layout_job(job);
    painter.galley(pos2(r.left() + 12.0, thumb.bottom() + 10.0), g, p.text);
    let mut job = egui::text::LayoutJob::single_section(
        item_meta_line(m, now),
        egui::TextFormat::simple(uc::font(12.0), p.text_tertiary),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: text_w,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('\u{2026}'),
    };
    let g = painter.layout_job(job);
    painter.galley(
        pos2(r.left() + 12.0, thumb.bottom() + 10.0 + 19.0),
        g,
        p.text_tertiary,
    );

    // Border / selection.
    if selected {
        painter.rect_stroke(
            r,
            CornerRadius::same(8),
            Stroke::new(2.0, accent),
            StrokeKind::Inside,
        );
    } else {
        let col = if hovered {
            Color32::from_white_alpha(if p.text == Color32::WHITE { 40 } else { 0 })
        } else {
            p.stroke
        };
        painter.rect_stroke(
            r,
            CornerRadius::same(8),
            Stroke::new(
                1.0,
                if hovered && col.a() == 0 {
                    Color32::from_black_alpha(40)
                } else {
                    col
                },
            ),
            StrokeKind::Inside,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn meta(id: ItemId, kind: ItemKind) -> ItemMeta {
        ItemMeta {
            id,
            path: PathBuf::from(format!("C:\\caps\\shot{id}.png")),
            kind,
            created: Local::now(),
            width: 1280,
            height: 720,
            app: "firefox".into(),
            window_title: "Billing portal".into(),
            starred: false,
            ocr_text: Some("invoice 4471".into()),
            bytes: 1000,
            mode: "region".into(),
        }
    }

    #[test]
    fn grid_columns_follow_width() {
        // 1280 window: sidebar 240 + padding 44 + scrollbar -> ~960 usable.
        let (cols, w) = grid_layout(960.0, MIN_CARD_W, GAP);
        assert_eq!(cols, 3);
        assert!((w - (960.0 - 32.0) / 3.0).abs() < 1e-3);
        assert_eq!(grid_layout(200.0, MIN_CARD_W, GAP).0, 1);
        assert_eq!(grid_layout(250.0, MIN_CARD_W, GAP).0, 1);
        assert_eq!(grid_layout(516.0, MIN_CARD_W, GAP).0, 2);
        assert_eq!(grid_layout(515.0, MIN_CARD_W, GAP).0, 1);
        // Total width always equals available width.
        let (c, cw) = grid_layout(1000.0, MIN_CARD_W, GAP);
        assert!((cw * c as f32 + GAP * (c as f32 - 1.0) - 1000.0).abs() < 1e-3);
    }

    #[test]
    fn virtualization_draws_only_nearby_rows() {
        let pitch = 222.0;
        // 1000 items, 3 cols -> 334 rows. A 700 px window at the top shows rows 0..4 (+1 overscan).
        assert_eq!(visible_rows(0.0, 700.0, pitch, 334, 1), 0..5);
        // Mid-list.
        let r = visible_rows(22200.0, 700.0, pitch, 334, 1);
        assert_eq!(r, 99..105);
        // Never past the end.
        assert_eq!(visible_rows(1e9, 700.0, pitch, 10, 1), 10..10);
        assert_eq!(visible_rows(0.0, 700.0, pitch, 0, 1), 0..0);
        // Window larger than content.
        assert_eq!(visible_rows(0.0, 5000.0, pitch, 3, 1), 0..3);
    }

    #[test]
    fn keyboard_navigation_in_a_grid() {
        // 7 items, 3 cols:   0 1 2 / 3 4 5 / 6
        assert_eq!(move_selection(None, Nav::Down, 3, 7), Some(0));
        assert_eq!(move_selection(Some(0), Nav::Right, 3, 7), Some(1));
        assert_eq!(move_selection(Some(6), Nav::Right, 3, 7), Some(6));
        assert_eq!(move_selection(Some(0), Nav::Left, 3, 7), Some(0));
        assert_eq!(move_selection(Some(4), Nav::Up, 3, 7), Some(1));
        assert_eq!(move_selection(Some(1), Nav::Up, 3, 7), Some(1));
        assert_eq!(move_selection(Some(1), Nav::Down, 3, 7), Some(4));
        // Down from the last full row jumps to the final (short) row's last item.
        assert_eq!(move_selection(Some(5), Nav::Down, 3, 7), Some(6));
        assert_eq!(move_selection(Some(6), Nav::Down, 3, 7), Some(6));
        assert_eq!(move_selection(Some(3), Nav::End, 3, 7), Some(6));
        assert_eq!(move_selection(Some(3), Nav::Home, 3, 7), Some(0));
        assert_eq!(move_selection(Some(2), Nav::Down, 3, 0), None);
        // A stale index (list shrank) restarts at the top.
        assert_eq!(move_selection(Some(50), Nav::Right, 3, 7), Some(0));
    }

    #[test]
    fn titles_and_apps_are_prettified() {
        assert_eq!(pretty_app("firefox"), "Firefox");
        assert_eq!(pretty_app("Code.exe"), "Code");
        assert_eq!(pretty_app(""), "Unknown");
        let mut m = meta(1, ItemKind::Image);
        assert_eq!(item_title(&m), "Firefox - Billing portal");
        m.window_title.clear();
        assert_eq!(item_title(&m), "Firefox");
        m.app.clear();
        assert_eq!(item_title(&m), "shot1.png");
        m.window_title = "Only title".into();
        assert_eq!(item_title(&m), "Only title");
    }

    #[test]
    fn dates_read_naturally() {
        let now = Local.with_ymd_and_hms(2026, 11, 6, 12, 0, 0).unwrap();
        let at = |m, d, h, mi| Local.with_ymd_and_hms(2026, m, d, h, mi, 0).unwrap();
        assert_eq!(friendly_date(at(11, 6, 9, 14), now), "Today 9:14");
        assert_eq!(friendly_date(at(11, 5, 16, 2), now), "Yesterday 16:02");
        assert_eq!(friendly_date(at(11, 3, 8, 0), now), "Nov 3");
        assert_eq!(
            friendly_date(Local.with_ymd_and_hms(2025, 10, 29, 8, 0, 0).unwrap(), now),
            "Oct 29 2025"
        );
    }

    #[test]
    fn meta_line_matches_design() {
        let now = Local::now();
        let mut m = meta(1, ItemKind::Image);
        m.mode = "scrolling".into();
        m.width = 1280;
        m.height = 4210;
        let line = item_meta_line(&m, now);
        assert!(line.starts_with("Today "));
        assert!(
            line.ends_with("scrolling \u{b7} 1,280 \u{d7} 4,210"),
            "{line}"
        );
        m.mode = "region".into();
        assert!(!item_meta_line(&m, now).contains("region"));
    }

    #[test]
    fn date_filters_compute_since() {
        let now = Local.with_ymd_and_hms(2026, 11, 6, 15, 30, 0).unwrap();
        assert_eq!(DateFilter::Any.since(now), None);
        let today = DateFilter::Today.since(now).unwrap();
        assert_eq!(today.date_naive(), now.date_naive());
        assert_eq!(today.format("%H:%M:%S").to_string(), "00:00:00");
        assert_eq!(
            DateFilter::Week.since(now).unwrap(),
            now - chrono::Duration::days(7)
        );
        assert_eq!(
            DateFilter::Month.since(now).unwrap(),
            now - chrono::Duration::days(30)
        );
    }

    #[test]
    fn retention_text() {
        let two_gb = 2_254_857_831;
        assert_eq!(
            retention_summary(90, 5.0, two_gb),
            "Keeping 90 days or 5 GB, starred kept forever. 2.1 GB used."
        );
        assert_eq!(
            retention_summary(1, 0.0, 0),
            "Keeping 1 day, starred kept forever. 0 B used."
        );
        assert_eq!(
            retention_summary(0, 2.5, 0),
            "Keeping up to 2.5 GB, starred kept forever. 0 B used."
        );
        assert_eq!(
            retention_summary(0, 0.0, two_gb),
            "Keeping everything. 2.1 GB used."
        );
    }

    #[test]
    fn results_line_wording() {
        assert_eq!(results_line(1284, false, ""), "1,284 captures");
        assert_eq!(results_line(1, false, ""), "1 capture");
        assert_eq!(
            results_line(6, false, "invoice 4471"),
            "6 captures contain text matching \u{201c}invoice 4471\u{201d}"
        );
        assert_eq!(
            results_line(1, false, "x"),
            "1 capture contains text matching \u{201c}x\u{201d}"
        );
        assert_eq!(results_line(400, true, ""), "400+ captures");
    }

    #[test]
    fn actions_depend_on_kind_and_ocr() {
        let img = meta(1, ItemKind::Image);
        let vid = meta(2, ItemKind::Video);
        assert!(Act::Edit.enabled(&img, true));
        assert!(!Act::Edit.enabled(&vid, true));
        assert!(Act::Open.enabled(&vid, true));
        assert!(Act::CopyText.enabled(&img, true));
        assert!(!Act::CopyText.enabled(&img, false));
        assert_eq!(
            Act::CopyText.disabled_reason(&img, false),
            Some("No OCR language is installed")
        );
        let mut pending = meta(3, ItemKind::Image);
        pending.ocr_text = None;
        assert_eq!(
            Act::CopyText.disabled_reason(&pending, true),
            Some("Text is still being indexed")
        );
        pending.ocr_text = Some("  ".into());
        assert_eq!(
            Act::CopyText.disabled_reason(&pending, true),
            Some("No text found in this capture")
        );
        assert_eq!(primary_act(&img), Act::Edit);
        assert_eq!(primary_act(&vid), Act::Open);
    }

    #[test]
    fn actions_map_to_commands() {
        let mut m = meta(9, ItemKind::Image);
        assert!(matches!(Act::Edit.command(&m), Some(Command::EditItem(9))));
        assert!(matches!(Act::Pin.command(&m), Some(Command::PinItem(9))));
        assert!(matches!(
            Act::CopyImage.command(&m),
            Some(Command::CopyItemImage(9))
        ));
        assert!(matches!(
            Act::CopyText.command(&m),
            Some(Command::CopyItemText(9))
        ));
        assert!(matches!(Act::Open.command(&m), Some(Command::OpenItem(9))));
        assert!(matches!(
            Act::Reveal.command(&m),
            Some(Command::RevealItem(9))
        ));
        assert!(matches!(
            Act::ToggleStar.command(&m),
            Some(Command::StarItem(9, true))
        ));
        m.starred = true;
        assert!(matches!(
            Act::ToggleStar.command(&m),
            Some(Command::StarItem(9, false))
        ));
        // Delete always goes through the confirmation dialog first.
        assert!(Act::Delete.command(&m).is_none());
    }

    #[test]
    fn thumb_queue_is_newest_first() {
        let q = Arc::new((
            Mutex::new(Queue {
                reqs: Vec::new(),
                closed: false,
            }),
            Condvar::new(),
        ));
        let (_tx, rx) = channel();
        let w = ThumbWorker {
            queue: Arc::clone(&q),
            results: rx,
        };
        let k = |id| ThumbKey { id, bytes: 0 };
        w.enqueue(&[k(1), k(2), k(3)]);
        let mut g = q.0.lock().unwrap();
        // The worker pops from the end: the first key of the batch is served first.
        assert_eq!(g.reqs.pop(), Some(k(1)));
        assert_eq!(g.reqs.pop(), Some(k(2)));
        drop(g);
        // A later batch jumps ahead of what is left from the earlier one.
        w.enqueue(&[k(10)]);
        let mut g = q.0.lock().unwrap();
        assert_eq!(g.reqs.pop(), Some(k(10)));
        assert_eq!(g.reqs.pop(), Some(k(3)));
        drop(g);
        drop(w);
        assert!(q.0.lock().unwrap().closed);
    }
}
