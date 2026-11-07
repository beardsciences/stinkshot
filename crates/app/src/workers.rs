//! Background work for the shell. Everything slow (encoding, disk, OCR, recording
//! start/stop, export, scrolling capture, history maintenance, update check) runs here and
//! reports back through `WorkerMsg`; every send wakes the UI thread.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, anyhow};
use stinkshot_core::annotate::{self, Doc};
use stinkshot_core::config::Config;
use stinkshot_core::naming::{self, NameParts};
use stinkshot_core::{PxRect, Rgba};
use stinkshot_history::{History, ItemId, ItemKind, NewItem};
use stinkshot_platform::capture::{self, Frozen};
use stinkshot_platform::ocr::OcrEngine;
use stinkshot_platform::windows_enum::WinInfo;
use stinkshot_platform::{clipboard, input, system};
use stinkshot_record::{GifOptions, MediaInfo, RecordOptions, Recorder, RecordingResult};
use stinkshot_stitch::{PushResult, StitchOptions, Stitcher};

use crate::capture_ops;
use crate::filedialog;
use crate::state::{CaptureOutput, Command, FinishAction, SourceInfo};
use crate::update::{self, Release};

pub type Ocr = Option<Arc<dyn OcrEngine>>;

/// Results posted to the UI thread.
pub enum WorkerMsg {
    /// A command produced by a worker (toasts, follow-up actions).
    Cmd(Command),
    CaptureSaved(Box<CaptureSaved>),
    HistoryChanged,
    /// Window capture finished; the overlay can open in Annotate mode.
    WindowCaptured {
        frozen: Frozen,
        windows: Vec<WinInfo>,
        result: Result<(Rgba, SourceInfo), String>,
    },
    /// The window-capture worker crashed before it could report.
    WindowCaptureFailed(String),
    /// Image + doc loaded for the overlay editor (re-edit).
    OpenAnnotate {
        base: Rgba,
        doc: Doc,
        mode: &'static str,
        editing: Option<ItemId>,
        source: SourceInfo,
    },
    PinReady {
        image: Rgba,
        title: String,
        item: ItemId,
    },
    ScrollProgress {
        frames: usize,
        width: u32,
        height: u32,
    },
    ScrollDone {
        result: Result<Rgba, String>,
        cancelled: bool,
        partial: bool,
        source: SourceInfo,
    },
    RecorderStarted(Result<Recorder, String>),
    RecordStopped(Result<(RecordingResult, MediaInfo), String>),
    ExportProgress(f32),
    Exported(Box<Exported>),
    UpdateChecked(Result<Option<Release>, String>),
}

pub struct CaptureSaved {
    pub item: Option<ItemId>,
    pub path: Option<PathBuf>,
    pub saved_as: Option<PathBuf>,
    pub width: u32,
    pub height: u32,
    pub thumb: Rgba,
    pub copied_image: bool,
    pub copied_text: Option<String>,
    pub mode: &'static str,
    pub source: SourceInfo,
    pub action: FinishAction,
    pub rect: PxRect,
    /// Kept only for `FinishAction::Pin`.
    pub pin_image: Option<Rgba>,
    pub editing: bool,
}

pub struct Exported {
    pub items: Vec<(ItemId, PathBuf, ItemKind)>,
    pub thumb: Option<Rgba>,
    pub duration: Duration,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    /// Set when something failed (partial results may still be in `items`).
    pub error: Option<String>,
}

/// Everything a worker needs, cloned per job.
#[derive(Clone)]
pub struct Env {
    pub tx: Sender<WorkerMsg>,
    pub history: Arc<History>,
    pub ocr: Ocr,
    pub cfg: Config,
    pub bg: Sender<BgKick>,
}

impl Env {
    pub fn send(&self, msg: WorkerMsg) {
        let _ = self.tx.send(msg);
        crate::wake::wake();
    }
    pub fn notify(&self, title: &str, body: impl Into<String>, error: bool) {
        self.send(WorkerMsg::Cmd(Command::Notify {
            title: title.to_string(),
            body: body.into(),
            error,
        }));
    }
    pub fn kick_ocr(&self) {
        let _ = self.bg.send(BgKick::Ocr);
    }
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) {
    if let Err(e) = std::thread::Builder::new().name(name.to_string()).spawn(f) {
        log::error!("cannot spawn worker {name}: {e}");
    }
}

static JOBS_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

/// Number of worker jobs (and tracked background threads) still running. The shell waits
/// for this to reach zero on quit so recordings, exports and saves are not cut off.
pub fn jobs_in_flight() -> usize {
    JOBS_IN_FLIGHT.load(Ordering::SeqCst)
}

/// Counts one unit of in-flight work until dropped.
pub struct InFlight(());

impl InFlight {
    pub fn begin() -> InFlight {
        JOBS_IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
        InFlight(())
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        JOBS_IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Run `f` on a worker, turning panics into an error toast instead of a dead feature.
fn job(env: &Env, name: &'static str, f: impl FnOnce(&Env) + Send + 'static) {
    job_inner(env, name, f, None);
}

/// Like `job`, but when `f` panics `recover` supplies the terminal message the shell is
/// waiting for, so the state the job owned (scroll HUD, pending capture, recorder
/// Starting/Stopping) is released instead of staying stuck.
fn job_recovering(
    env: &Env,
    name: &'static str,
    f: impl FnOnce(&Env) + Send + 'static,
    recover: impl FnOnce() -> WorkerMsg + Send + 'static,
) {
    job_inner(env, name, f, Some(Box::new(recover)));
}

fn job_inner(
    env: &Env,
    name: &'static str,
    f: impl FnOnce(&Env) + Send + 'static,
    recover: Option<Box<dyn FnOnce() -> WorkerMsg + Send>>,
) {
    let env = env.clone();
    let in_flight = InFlight::begin();
    spawn(name, move || {
        let _in_flight = in_flight;
        let e2 = env.clone();
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&env))).is_err() {
            log::error!("worker {name} panicked");
            if let Some(recover) = recover {
                e2.send(recover());
            }
            e2.notify(
                "Something Went Wrong",
                format!("The {name} task crashed. See stinkshot.log."),
                true,
            );
        }
    });
}

static LAST_SAVE_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

// ------------------------------------------------------------------------------ capture

fn encode_save(img: &Rgba, path: &Path, cfg: &Config) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    img.save(
        path,
        filedialog::format_for_path(path),
        cfg.capture.jpeg_quality,
    )
    .with_context(|| format!("write {}", path.display()))
}

/// Write the flattened image (+ re-edit sidecar) into the history folder and index it.
fn save_to_history(
    env: &Env,
    base: &Rgba,
    doc: &Doc,
    flat: &Rgba,
    source: &SourceInfo,
    mode: &str,
    editing: Option<ItemId>,
) -> anyhow::Result<(ItemId, PathBuf)> {
    let cfg = &env.cfg;
    if let Some(id) = editing
        && let Some(meta) = env.history.get(id)?
    {
        let path = meta.path;
        encode_save(flat, &path, cfg)?;
        write_sidecar(cfg, &path, base, doc, true);
        env.history.touch(id, flat.width, flat.height)?;
        return Ok((id, path));
    }
    let fmt = cfg.capture.format;
    let parts = NameParts {
        now: chrono::Local::now(),
        app: &source.app,
        title: &source.window_title,
        width: flat.width,
        height: flat.height,
    };
    let path = naming::unique_path(
        env.history.folder(),
        &cfg.capture.filename_pattern,
        &parts,
        fmt.extension(),
    );
    encode_save(flat, &path, cfg)?;
    write_sidecar(cfg, &path, base, doc, false);
    let id = env.history.add(NewItem {
        path: path.clone(),
        kind: Some(ItemKind::Image),
        width: flat.width,
        height: flat.height,
        app: source.app.clone(),
        window_title: source.window_title.clone(),
        mode: mode.to_string(),
    })?;
    Ok((id, path))
}

fn write_sidecar(cfg: &Config, path: &Path, base: &Rgba, doc: &Doc, was_editing: bool) {
    if cfg.history.keep_editable && !doc.shapes.is_empty() {
        if let Err(e) = annotate::save_editable(path, base, doc) {
            log::warn!(
                "could not save editable sidecar for {}: {e:#}",
                path.display()
            );
        }
    } else if was_editing && let Err(e) = annotate::delete_editable(path) {
        log::warn!(
            "could not remove stale sidecar for {}: {e:#}",
            path.display()
        );
    }
}

pub fn finish_capture(env: &Env, out: CaptureOutput) {
    job(env, "capture-save", move |env| {
        let CaptureOutput {
            base,
            doc,
            flattened,
            rect,
            action,
            source,
            mode,
            editing,
        } = out;
        let (width, height) = (flattened.width, flattened.height);

        let (item, path) =
            match save_to_history(env, &base, &doc, &flattened, &source, mode, editing) {
                Ok((id, p)) => (Some(id), Some(p)),
                Err(e) => {
                    log::error!("saving capture to history failed: {e:#}");
                    env.notify("Could Not Save to History", format!("{e:#}"), true);
                    (None, None)
                }
            };
        drop(base);

        let want_copy = match action {
            FinishAction::Copy => true,
            FinishAction::SaveAs | FinishAction::Pin => env.cfg.general.copy_to_clipboard,
            FinishAction::CopyText => false,
        };
        let mut copied_image = false;
        if want_copy {
            match clipboard::set_image(&flattened) {
                Ok(()) => copied_image = true,
                Err(e) => {
                    log::error!("clipboard copy failed: {e:#}");
                    env.notify("Could Not Copy to Clipboard", format!("{e:#}"), true);
                }
            }
        }

        let mut saved_as = None;
        if action == FinishAction::SaveAs {
            let stem = path
                .as_ref()
                .and_then(|p| p.file_stem())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "StinkShot".to_string());
            let start_dir = LAST_SAVE_DIR
                .lock()
                .ok()
                .and_then(|g| g.clone())
                .unwrap_or_else(|| env.history.folder().to_path_buf());
            match filedialog::save_image(&stem, env.cfg.capture.format, Some(&start_dir)) {
                Ok(Some(target)) => match encode_save(&flattened, &target, &env.cfg) {
                    Ok(()) => {
                        if let (Some(parent), Ok(mut g)) = (target.parent(), LAST_SAVE_DIR.lock()) {
                            *g = Some(parent.to_path_buf());
                        }
                        saved_as = Some(target);
                    }
                    Err(e) => env.notify("Could Not Save File", format!("{e:#}"), true),
                },
                Ok(None) => {}
                Err(e) => env.notify("Save As Failed", format!("{e:#}"), true),
            }
        }

        let mut copied_text = None;
        if action == FinishAction::CopyText {
            match &env.ocr {
                None => env.notify(
                    "No OCR Language Installed",
                    "Add a language in Windows Settings > Time & language.",
                    true,
                ),
                Some(engine) => match engine.recognize(&flattened) {
                    Ok(r) => {
                        let text = r.text.trim().to_string();
                        if text.is_empty() {
                            env.notify(
                                "No Text Found",
                                "Nothing readable in the selection.",
                                false,
                            );
                        } else {
                            match clipboard::set_text(&text) {
                                Ok(()) => copied_text = Some(text),
                                Err(e) => env.notify("Could Not Copy Text", format!("{e:#}"), true),
                            }
                        }
                    }
                    Err(e) => env.notify("Text Recognition Failed", format!("{e:#}"), true),
                },
            }
        }

        if env.cfg.general.play_sound {
            system::play_capture_sound();
        }

        let thumb = flattened.thumbnail(360, 220);
        let pin_image = (action == FinishAction::Pin).then_some(flattened);
        env.send(WorkerMsg::CaptureSaved(Box::new(CaptureSaved {
            item,
            path,
            saved_as,
            width,
            height,
            thumb,
            copied_image,
            copied_text,
            mode,
            source,
            action,
            rect,
            pin_image,
            editing: editing.is_some(),
        })));
        if item.is_some() {
            env.send(WorkerMsg::HistoryChanged);
            if env.cfg.history.ocr_index {
                env.kick_ocr();
            }
        }
    });
}

pub fn capture_window(env: &Env, frozen: Frozen, windows: Vec<WinInfo>, win: WinInfo) {
    job_recovering(
        env,
        "window-capture",
        move |env| {
            let source = capture_ops::source_of(&win);
            let result =
                capture_ops::capture_window_image(&env.cfg, &win, env.cfg.capture.include_cursor)
                    .map(|img| (img, source))
                    .map_err(|e| format!("{e:#}"));
            env.send(WorkerMsg::WindowCaptured {
                frozen,
                windows,
                result,
            });
        },
        || WorkerMsg::WindowCaptureFailed("The capture task crashed.".into()),
    );
}

pub fn ocr_image(env: &Env, img: Rgba) {
    job(env, "ocr-region", move |env| {
        let Some(engine) = &env.ocr else {
            env.notify(
                "No OCR Language Installed",
                "Add a language in Windows Settings > Time & language.",
                true,
            );
            return;
        };
        match engine.recognize(&img) {
            Ok(r) => {
                let text = r.text.trim().to_string();
                if text.is_empty() {
                    env.notify("No Text Found", "Nothing readable in the selection.", false);
                    return;
                }
                match clipboard::set_text(&text) {
                    Ok(()) => env.notify("Text Copied", preview(&text, 140), false),
                    Err(e) => env.notify("Could Not Copy Text", format!("{e:#}"), true),
                }
            }
            Err(e) => env.notify("Text Recognition Failed", format!("{e:#}"), true),
        }
    });
}

/// First `max` characters, whitespace collapsed, with an ellipsis if cut.
pub fn preview(text: &str, max: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        let cut: String = flat.chars().take(max).collect();
        format!("{}...", cut.trim_end())
    }
}

// ------------------------------------------------------------------------- scrolling

pub const SCROLL_RUN: u8 = 0;
pub const SCROLL_STOP: u8 = 1;
pub const SCROLL_CANCEL: u8 = 2;

pub fn scroll_capture(env: &Env, rect: PxRect, source: SourceInfo, control: Arc<AtomicU8>) {
    job_recovering(
        env,
        "scroll-capture",
        move |env| {
            let cfg = &env.cfg.capture;
            let mut stitcher = Stitcher::new(StitchOptions {
                max_height: cfg.scroll_max_height.max(500),
                ..StitchOptions::default()
            });
            let delay = Duration::from_millis(cfg.scroll_delay_ms.clamp(30, 2000) as u64);
            let center = capture_ops::center(rect);
            let scale = capture_ops::scale_at(rect);
            // About half a region per step to keep a healthy overlap for matching.
            let mut notches = ((rect.h as f32 / scale) / 220.0).round().clamp(1.0, 8.0) as i32;
            let mut failed = false;
            let mut error: Option<String> = None;

            // Let the selection overlay and HUD finish disappearing before the first frame.
            std::thread::sleep(Duration::from_millis(250));
            for _ in 0..800 {
                match control.load(Ordering::Relaxed) {
                    SCROLL_CANCEL => {
                        env.send(WorkerMsg::ScrollDone {
                            result: Err("cancelled".into()),
                            cancelled: true,
                            partial: false,
                            source,
                        });
                        return;
                    }
                    SCROLL_STOP => break,
                    _ => {}
                }
                let frame = match capture::capture_rect(rect, false) {
                    Ok(f) => f,
                    Err(e) => {
                        error = Some(format!("{e:#}"));
                        break;
                    }
                };
                let first = stitcher.frames() == 0;
                match stitcher.push(&frame) {
                    PushResult::Appended { added_rows } => {
                        if !first {
                            let ratio = added_rows as f32 / frame.height.max(1) as f32;
                            if ratio > 0.85 {
                                notches = (notches - 1).max(1);
                            } else if ratio < 0.3 && notches < 12 {
                                notches += 1;
                            }
                        }
                    }
                    PushResult::Unchanged => {}
                    PushResult::End => break,
                    PushResult::Failed => {
                        failed = true;
                        break;
                    }
                }
                env.send(WorkerMsg::ScrollProgress {
                    frames: stitcher.frames(),
                    width: frame.width,
                    height: stitcher.height(),
                });
                input::scroll_at(center, -notches);
                std::thread::sleep(delay);
            }
            let frames = stitcher.frames();
            if frames == 0 {
                env.send(WorkerMsg::ScrollDone {
                    result: Err(error.unwrap_or_else(|| "no frames were captured".into())),
                    cancelled: false,
                    partial: false,
                    source,
                });
                return;
            }
            let img = stitcher.finish();
            env.send(WorkerMsg::ScrollDone {
                result: Ok(img),
                cancelled: false,
                partial: failed || error.is_some(),
                source,
            });
        },
        || WorkerMsg::ScrollDone {
            result: Err("The scroll capture task crashed.".into()),
            cancelled: false,
            partial: false,
            source: SourceInfo::default(),
        },
    );
}

// -------------------------------------------------------------------------- recording

pub fn temp_recording_path(history: &History) -> anyhow::Result<PathBuf> {
    let dir = history.folder().join(".stinkshot").join("tmp");
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    Ok(dir.join(format!(
        "Recording_{}.mp4",
        chrono::Local::now().format("%Y%m%d_%H%M%S_%3f")
    )))
}

pub fn start_recorder(env: &Env, opts: RecordOptions) {
    job_recovering(
        env,
        "recorder-start",
        move |env| {
            let r = Recorder::start(opts).map_err(|e| format!("{e:#}"));
            env.send(WorkerMsg::RecorderStarted(r));
        },
        || WorkerMsg::RecorderStarted(Err("The recorder task crashed.".into())),
    );
}

pub fn stop_recorder(env: &Env, rec: Recorder) {
    job_recovering(
        env,
        "recorder-stop",
        move |env| {
            let r = rec.stop().map_err(|e| format!("{e:#}")).and_then(|res| {
                stinkshot_record::probe(&res.path)
                    .map(|info| (res, info))
                    .map_err(|e| format!("{e:#}"))
            });
            env.send(WorkerMsg::RecordStopped(r));
        },
        || WorkerMsg::RecordStopped(Err("The recorder task crashed.".into())),
    );
}

pub fn cancel_recorder(env: &Env, rec: Recorder) {
    job(env, "recorder-cancel", move |_| rec.cancel());
}

fn media_parts(info: &MediaInfo) -> (u32, u32) {
    (info.width, info.height)
}

/// Export the recording at `source` (trimmed to [start, end)) as MP4 and/or GIF into the
/// history folder, index the results and delete the temporary source.
pub fn export_recording(
    env: &Env,
    source: PathBuf,
    start: Duration,
    end: Duration,
    as_gif: bool,
    as_mp4: bool,
) {
    job(env, "recording-export", move |env| {
        let result = export_inner(env, &source, start, end, as_gif, as_mp4);
        let exported = match result {
            Ok(x) => x,
            Err(e) => {
                log::error!("export failed: {e:#}");
                Exported {
                    items: Vec::new(),
                    thumb: None,
                    duration: Duration::ZERO,
                    width: 0,
                    height: 0,
                    bytes: 0,
                    error: Some(format!("{e:#}")),
                }
            }
        };
        env.send(WorkerMsg::Exported(Box::new(exported)));
        env.send(WorkerMsg::HistoryChanged);
    });
}

fn export_inner(
    env: &Env,
    source: &Path,
    start: Duration,
    end: Duration,
    as_gif: bool,
    as_mp4: bool,
) -> anyhow::Result<Exported> {
    let info = stinkshot_record::probe(source).context("read the recording")?;
    let end = if end.is_zero() || end > info.duration {
        info.duration
    } else {
        end
    };
    let start = start.min(end);
    let full =
        start <= Duration::from_millis(40) && end + Duration::from_millis(80) >= info.duration;
    let (w, h) = media_parts(&info);
    let mid = start + (end - start) / 2;
    let thumb = stinkshot_record::frame_at(source, mid, 220).ok();
    let steps = (as_gif as u32 + as_mp4 as u32).max(1) as f32;
    let progress_for = |index: u32| {
        let tx = env.tx.clone();
        move |p: f32| {
            let _ = tx.send(WorkerMsg::ExportProgress(
                ((index as f32 + p.clamp(0.0, 1.0)) / steps).clamp(0.0, 1.0),
            ));
            crate::wake::wake();
        }
    };
    let name_for = |ext: &str| {
        let parts = NameParts {
            now: chrono::Local::now(),
            app: "",
            title: "",
            width: w,
            height: h,
        };
        naming::unique_path(
            env.history.folder(),
            &env.cfg.capture.filename_pattern,
            &parts,
            ext,
        )
    };

    let mut items = Vec::new();
    let mut error: Option<String> = None;
    let mut step = 0u32;
    let mut source_consumed = false;

    if as_gif {
        let dest = name_for("gif");
        let rec = &env.cfg.recording;
        let opts = GifOptions {
            fps: rec.gif_fps.clamp(1, 50),
            max_width: rec.gif_max_width,
            start,
            end: Some(end),
            quality: 90,
            loop_forever: true,
        };
        let p = progress_for(step);
        match stinkshot_record::to_gif(source, &dest, &opts, &p) {
            Ok(()) => items.push((dest, ItemKind::Gif)),
            Err(e) => {
                let _ = std::fs::remove_file(&dest);
                error = Some(format!("GIF export failed: {e:#}"));
            }
        }
        step += 1;
    }
    if as_mp4 {
        let dest = name_for("mp4");
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let outcome = if full {
            let moved = std::fs::rename(source, &dest).or_else(|_| {
                // Different volume: copy, then drop the original.
                std::fs::copy(source, &dest)?;
                std::fs::remove_file(source)
            });
            if moved.is_ok() {
                source_consumed = true;
            }
            moved.context("move recording into history")
        } else {
            let p = progress_for(step);
            stinkshot_record::trim(source, &dest, start, end, &p).context("trim recording")
        };
        match outcome {
            Ok(()) => items.push((dest, ItemKind::Video)),
            Err(e) => {
                let _ = std::fs::remove_file(&dest);
                error = Some(format!("MP4 export failed: {e:#}"));
            }
        }
    }

    // Keep the raw recording if nothing could be written.
    if !items.is_empty() && !source_consumed {
        let _ = std::fs::remove_file(source);
    }

    let mut indexed = Vec::new();
    let mut bytes = 0u64;
    for (path, kind) in items {
        bytes += std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let id = env
            .history
            .add(NewItem {
                path: path.clone(),
                kind: Some(kind),
                width: w,
                height: h,
                app: String::new(),
                window_title: String::new(),
                mode: "recording".into(),
            })
            .context("add recording to history")?;
        if let Some(t) = &thumb
            && let Err(e) = env.history.store_thumbnail(id, &t.thumbnail(320, 200))
        {
            log::debug!("store thumbnail: {e:#}");
        }
        indexed.push((id, path, kind));
    }
    if indexed.is_empty() && error.is_none() {
        return Err(anyhow!("nothing was exported"));
    }
    Ok(Exported {
        items: indexed,
        thumb,
        duration: end - start,
        width: w,
        height: h,
        bytes,
        error,
    })
}

// ------------------------------------------------------------------------ history items

pub fn copy_item_image(env: &Env, id: ItemId) {
    job(env, "copy-image", move |env| {
        let run = || -> anyhow::Result<&'static str> {
            let meta = env
                .history
                .get(id)?
                .ok_or_else(|| anyhow!("item no longer exists"))?;
            match meta.kind {
                ItemKind::Image => {
                    let img = Rgba::load(&meta.path)?;
                    clipboard::set_file_and_image(&meta.path, Some(&img))?;
                    Ok("Image copied")
                }
                _ => {
                    clipboard::set_file_and_image(&meta.path, None)?;
                    Ok("File copied")
                }
            }
        };
        match run() {
            Ok(title) => env.notify(title, "Ready to paste.", false),
            Err(e) => env.notify("Could Not Copy", format!("{e:#}"), true),
        }
    });
}

pub fn copy_item_text(env: &Env, id: ItemId) {
    job(env, "copy-text", move |env| {
        let run = || -> anyhow::Result<Option<String>> {
            let meta = env
                .history
                .get(id)?
                .ok_or_else(|| anyhow!("item no longer exists"))?;
            let text = match meta.ocr_text {
                Some(t) if !t.trim().is_empty() => t,
                _ => {
                    if meta.kind != ItemKind::Image {
                        return Ok(None);
                    }
                    let engine = env
                        .ocr
                        .as_ref()
                        .ok_or_else(|| anyhow!("no OCR language is installed"))?;
                    let img = Rgba::load(&meta.path)?;
                    let text = engine.recognize(&img)?.text.trim().to_string();
                    env.history.set_ocr_text(id, &text)?;
                    text
                }
            };
            if text.trim().is_empty() {
                return Ok(None);
            }
            clipboard::set_text(text.trim())?;
            Ok(Some(text))
        };
        match run() {
            Ok(Some(t)) => env.notify("Text Copied", preview(&t, 140), false),
            Ok(None) => env.notify(
                "No Text Found",
                "This item has no recognizable text.",
                false,
            ),
            Err(e) => env.notify("Could Not Copy Text", format!("{e:#}"), true),
        }
    });
}

pub fn pin_item(env: &Env, id: ItemId) {
    job(env, "pin-item", move |env| {
        let run = || -> anyhow::Result<(Rgba, String)> {
            let meta = env
                .history
                .get(id)?
                .ok_or_else(|| anyhow!("item no longer exists"))?;
            if meta.kind != ItemKind::Image {
                return Err(anyhow!("only images can be pinned"));
            }
            let title = meta
                .path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            Ok((Rgba::load(&meta.path)?, title))
        };
        match run() {
            Ok((image, title)) => env.send(WorkerMsg::PinReady {
                image,
                title,
                item: id,
            }),
            Err(e) => env.notify("Could Not Pin", format!("{e:#}"), true),
        }
    });
}

pub fn edit_item(env: &Env, id: ItemId) {
    job(env, "edit-item", move |env| {
        let run = || -> anyhow::Result<(Rgba, Doc, SourceInfo)> {
            let meta = env
                .history
                .get(id)?
                .ok_or_else(|| anyhow!("item no longer exists"))?;
            if meta.kind != ItemKind::Image {
                return Err(anyhow!("only images can be edited"));
            }
            let source = SourceInfo {
                app: meta.app.clone(),
                window_title: meta.window_title.clone(),
            };
            if let Some((base, doc)) = annotate::load_editable(&meta.path)? {
                return Ok((base, doc, source));
            }
            let img = Rgba::load(&meta.path)?;
            let doc = Doc::new(img.width, img.height);
            Ok((img, doc, source))
        };
        match run() {
            Ok((base, doc, source)) => {
                env.send(WorkerMsg::OpenAnnotate {
                    base,
                    doc,
                    mode: "region",
                    editing: Some(id),
                    source,
                });
            }
            Err(e) => env.notify("Could Not Open for Editing", format!("{e:#}"), true),
        }
    });
}

pub fn delete_item(env: &Env, id: ItemId) {
    job(env, "delete-item", move |env| {
        if let Err(e) = env.history.delete(id) {
            env.notify("Could Not Delete", format!("{e:#}"), true);
        }
        env.send(WorkerMsg::HistoryChanged);
    });
}

/// Run a shell action on one history item off the UI thread (Explorer can take seconds).
fn with_item_job(
    env: &Env,
    name: &'static str,
    id: ItemId,
    f: impl FnOnce(&Path) -> anyhow::Result<()> + Send + 'static,
) {
    job(env, name, move |env| match env.history.get(id) {
        Ok(Some(meta)) => {
            if let Err(e) = f(&meta.path) {
                env.notify("Action Failed", format!("{e:#}"), true);
            }
        }
        Ok(None) => env.notify("Item Not Found", "It may have been deleted.", true),
        Err(e) => env.notify("History Error", format!("{e:#}"), true),
    });
}

pub fn reveal_item(env: &Env, id: ItemId) {
    with_item_job(env, "reveal-item", id, system::reveal_in_explorer);
}

pub fn open_item(env: &Env, id: ItemId) {
    with_item_job(env, "open-item", id, system::open_path);
}

pub fn open_url(env: &Env, url: String) {
    job(env, "open-url", move |env| {
        if let Err(e) = system::open_path(Path::new(&url)) {
            env.notify("Could Not Open the Page", format!("{e:#}"), true);
        }
    });
}

// ---------------------------------------------------------------- background maintenance

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BgKick {
    /// Re-run the OCR indexer.
    Ocr,
    /// Rescan + retention + OCR.
    Full,
}

/// Settings the background thread reads on every pass.
#[derive(Clone)]
pub struct BgConfig {
    pub history: Arc<History>,
    pub ocr: Ocr,
    pub ocr_index: bool,
    pub keep_days: u32,
    pub max_bytes: u64,
}

impl BgConfig {
    pub fn from(cfg: &Config, history: Arc<History>, ocr: Ocr) -> Self {
        Self {
            history,
            ocr,
            ocr_index: cfg.history.ocr_index,
            keep_days: cfg.history.keep_days,
            max_bytes: (cfg.history.max_gb.max(0.0) as f64 * 1024.0 * 1024.0 * 1024.0) as u64,
        }
    }
}

/// History maintenance thread: rescan + retention on start and hourly, OCR indexing on
/// demand. Returns the kick channel; dropping it stops the thread.
pub fn start_background(cfg: Arc<Mutex<BgConfig>>, tx: Sender<WorkerMsg>) -> Sender<BgKick> {
    let (kick_tx, kick_rx): (Sender<BgKick>, Receiver<BgKick>) = channel();
    spawn("history-maintenance", move || {
        let mut kind = BgKick::Full;
        loop {
            let snapshot = cfg.lock().map(|g| g.clone()).ok();
            if let Some(c) = snapshot {
                if kind == BgKick::Full {
                    maintain(&c, &tx);
                }
                index_ocr(&c, &tx);
            }
            kind = match kick_rx.recv_timeout(Duration::from_secs(3600)) {
                Ok(BgKick::Full) => BgKick::Full,
                Ok(BgKick::Ocr) => {
                    // Coalesce a burst of kicks.
                    let mut k = BgKick::Ocr;
                    while let Ok(next) = kick_rx.try_recv() {
                        if next == BgKick::Full {
                            k = BgKick::Full;
                        }
                    }
                    k
                }
                Err(RecvTimeoutError::Timeout) => BgKick::Full,
                Err(RecvTimeoutError::Disconnected) => return,
            };
        }
    });
    kick_tx
}

fn post(tx: &Sender<WorkerMsg>, msg: WorkerMsg) {
    let _ = tx.send(msg);
    crate::wake::wake();
}

fn maintain(c: &BgConfig, tx: &Sender<WorkerMsg>) {
    let mut changed = false;
    match c.history.rescan() {
        Ok((added, removed)) => {
            if added + removed > 0 {
                log::info!("history rescan: {added} added, {removed} removed");
                changed = true;
            }
        }
        Err(e) => log::warn!("history rescan failed: {e:#}"),
    }
    if c.keep_days > 0 || c.max_bytes > 0 {
        match c.history.apply_retention(c.keep_days, c.max_bytes) {
            Ok(n) if n > 0 => {
                log::info!("retention removed {n} items");
                changed = true;
            }
            Ok(_) => {}
            Err(e) => log::warn!("retention failed: {e:#}"),
        }
    }
    clean_temp_recordings(&c.history);
    if changed {
        post(tx, WorkerMsg::HistoryChanged);
    }
}

/// Raw recordings abandoned by a crash are kept a week, then removed.
fn clean_temp_recordings(history: &History) {
    let dir = history.folder().join(".stinkshot").join("tmp");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let cutoff = std::time::SystemTime::now() - Duration::from_secs(7 * 24 * 3600);
    for e in entries.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .map(|t| t < cutoff)
            .unwrap_or(false);
        if old && let Err(err) = std::fs::remove_file(e.path()) {
            log::debug!("temp cleanup {}: {err}", e.path().display());
        }
    }
}

fn index_ocr(c: &BgConfig, tx: &Sender<WorkerMsg>) {
    let Some(engine) = &c.ocr else { return };
    if !c.ocr_index {
        return;
    }
    let mut done_since_post = 0;
    loop {
        let batch = match c.history.pending_ocr(25) {
            Ok(b) => b,
            Err(e) => {
                log::warn!("pending_ocr failed: {e:#}");
                return;
            }
        };
        if batch.is_empty() {
            break;
        }
        let mut stored = 0;
        for item in batch {
            let text = match Rgba::load(&item.path).and_then(|img| engine.recognize(&img)) {
                Ok(r) => r.text,
                Err(e) => {
                    log::debug!("ocr index {}: {e:#}", item.path.display());
                    String::new()
                }
            };
            if c.history.set_ocr_text(item.id, text.trim()).is_ok() {
                stored += 1;
                done_since_post += 1;
            }
            if done_since_post >= 10 {
                done_since_post = 0;
                post(tx, WorkerMsg::HistoryChanged);
            }
            // Stay out of the way of the UI and capture workers.
            std::thread::sleep(Duration::from_millis(25));
        }
        if stored == 0 {
            log::warn!("ocr indexer made no progress; stopping this pass");
            break;
        }
    }
    if done_since_post > 0 {
        post(tx, WorkerMsg::HistoryChanged);
    }
}

pub fn check_updates(tx: Sender<WorkerMsg>) {
    spawn("update-check", move || {
        let r = update::check().map_err(|e| format!("{e:#}"));
        post(&tx, WorkerMsg::UpdateChecked(r));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_collapses_and_truncates() {
        assert_eq!(preview("a  b\n c", 20), "a b c");
        assert_eq!(preview("abcdefghij", 5), "abcde...");
        assert_eq!(preview("short", 5), "short");
    }
}
