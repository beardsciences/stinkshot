//! The live recorder: owns the capture/encode/audio worker threads.

use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use stinkshot_platform::input::{InputEvent, InputHook, MouseButton};
use tiny_skia::Pixmap;

use crate::audio::{self, Source};
use crate::capture::Capturer;
use crate::convert::bgra_to_nv12;
use crate::dsp::{OUT_RATE, SourceQueue, mix, to_pcm16};
use crate::encoder::{Encoder, VideoParams};
use crate::mf::{MfGuard, MfSession, TimerResolution, sleep_until};
use crate::overlay::{
    Button, OverlayFonts, OverlayState, WebcamFrame, draw_webcam_bubble, ui_scale, webcam_origin,
};
use crate::timeline::{FrameScheduler, Timeline, pts_hns};
use crate::webcam::Webcam;
use crate::{RecordOptions, RecordingResult, Status};

/// Audio may run this far ahead of the output clock before the oldest data is discarded.
const MAX_AUDIO_LEAD_FRAMES: usize = (OUT_RATE as usize) / 10;
/// What the video thread reports once capture and encoder are up: the shared encoder, its
/// display name and the capture backend name (or an error message).
type InitResult = Result<(Arc<Mutex<Encoder>>, String, &'static str), String>;

const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(5);

struct Shared {
    stop: AtomicBool,
    audio_stop: AtomicBool,
    paused: AtomicBool,
    mic_muted: AtomicBool,
    sys_muted: AtomicBool,
    webcam_visible: AtomicBool,
    clicks: AtomicBool,
    keys: AtomicBool,
    frames: AtomicU64,
    dropped: AtomicU64,
    error: Mutex<Option<String>>,
    timeline: Mutex<Timeline>,
}

impl Shared {
    fn set_error(&self, msg: String) {
        log::error!("recording error: {msg}");
        if let Ok(mut e) = self.error.lock() {
            e.get_or_insert(msg);
        }
    }

    fn elapsed(&self) -> Duration {
        match self.timeline.lock() {
            Ok(t) => t.elapsed_at(Instant::now()),
            Err(_) => Duration::ZERO,
        }
    }
}

pub struct Recorder {
    shared: Arc<Shared>,
    enc: Arc<Mutex<Encoder>>,
    video: Option<JoinHandle<()>>,
    mixer: Option<JoinHandle<()>>,
    captures: Vec<JoinHandle<()>>,
    webcam: Option<Webcam>,
    temp_path: PathBuf,
    final_path: PathBuf,
    width: u32,
    height: u32,
    encoder_name: String,
    capture_backend: &'static str,
    final_elapsed: Option<Duration>,
    finished: bool,
    /// Keeps Media Foundation alive until the encoder has been finalized and dropped.
    _mf: MfGuard,
}

fn temp_sibling(output: &Path) -> PathBuf {
    let stem = output
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "recording".into());
    output.with_file_name(format!("{stem}.recording.tmp.mp4"))
}

impl Recorder {
    pub fn start(opts: crate::RecordOptions) -> Result<Recorder> {
        let _timer = TimerResolution::new();
        let mf_guard = MfGuard::new()?;
        let width = (opts.region.w.max(0) as u32) & !1;
        let height = (opts.region.h.max(0) as u32) & !1;
        if width < 2 || height < 2 {
            bail!(
                "recording region is too small ({}x{})",
                opts.region.w,
                opts.region.h
            );
        }
        let mut region = opts.region;
        region.w = width as i32;
        region.h = height as i32;
        let fps = opts.fps.clamp(1, 120);
        if let Some(parent) = opts.output.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
        let temp_path = temp_sibling(&opts.output);
        let _ = std::fs::remove_file(&temp_path);

        let want_audio = opts.system_audio || opts.microphone;
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            audio_stop: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            mic_muted: AtomicBool::new(false),
            sys_muted: AtomicBool::new(false),
            webcam_visible: AtomicBool::new(true),
            clicks: AtomicBool::new(opts.show_clicks),
            keys: AtomicBool::new(opts.show_keys),
            frames: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            error: Mutex::new(None),
            timeline: Mutex::new(Timeline::new(Instant::now())),
        });

        // Audio sources first: failing to open a requested device fails the recording.
        let mut guard = StartGuard {
            shared: shared.clone(),
            captures: Vec::new(),
            video: None,
            temp: temp_path.clone(),
            armed: true,
        };
        let mut sys_q = None;
        let mut mic_q = None;
        if opts.system_audio {
            let q = Arc::new(Mutex::new(SourceQueue::default()));
            // capture threads watch audio_stop through a dedicated Arc<AtomicBool>
            match spawn_audio(Source::Loopback, &q, &shared) {
                Ok(h) => {
                    guard.captures.push(h);
                    sys_q = Some(q);
                }
                Err(e) => return Err(e.context("system audio capture")),
            }
        }
        if opts.microphone {
            let q = Arc::new(Mutex::new(SourceQueue::default()));
            match spawn_audio(Source::Microphone(opts.mic_device.clone()), &q, &shared) {
                Ok(h) => {
                    guard.captures.push(h);
                    mic_q = Some(q);
                }
                Err(e) => return Err(e.context("microphone capture")),
            }
        }

        let webcam_latest: Arc<Mutex<Option<WebcamFrame>>> = Arc::new(Mutex::new(None));
        let webcam = match &opts.webcam {
            Some(w) => match Webcam::start(w.device.as_deref(), webcam_latest.clone()) {
                Ok(c) => Some(c),
                Err(e) => return Err(e.context("webcam")),
            },
            None => None,
        };

        // Video thread: owns capture + encoder creation so COM objects live on one MTA thread.
        let (init_tx, init_rx) = mpsc::channel::<InitResult>();
        guard.video = Some({
            let shared = shared.clone();
            let opts = opts.clone();
            let temp = temp_path.clone();
            let webcam_latest = webcam_latest.clone();
            std::thread::Builder::new()
                .name("record-video".into())
                .spawn(move || {
                    video_thread(
                        shared,
                        opts,
                        region,
                        fps,
                        temp,
                        want_audio,
                        webcam_latest,
                        init_tx,
                    );
                })
                .context("spawn video thread")?
        });
        let (enc, encoder_name, capture_backend) =
            match init_rx.recv_timeout(Duration::from_secs(30)) {
                Ok(Ok(v)) => v,
                // The guard stops and joins every worker and removes the temp file.
                Ok(Err(e)) => return Err(anyhow!(e)),
                Err(_) => bail!("recording initialisation timed out"),
            };

        let mixer = if want_audio {
            let shared = shared.clone();
            let enc = enc.clone();
            Some(
                std::thread::Builder::new()
                    .name("record-audio-mix".into())
                    .spawn(move || mixer_thread(shared, enc, sys_q, mic_q))
                    .context("spawn mixer thread")?,
            )
        } else {
            None
        };

        // Everything is running: ownership passes to the recorder.
        guard.armed = false;
        let captures = std::mem::take(&mut guard.captures);
        let video = guard.video.take();
        log::info!(
            "recording started: {width}x{height} @ {fps} fps, encoder {encoder_name}, capture {capture_backend}"
        );
        Ok(Recorder {
            shared,
            enc,
            video,
            mixer,
            captures,
            webcam,
            temp_path,
            final_path: opts.output,
            width,
            height,
            encoder_name,
            capture_backend,
            final_elapsed: None,
            finished: false,
            _mf: mf_guard,
        })
    }

    /// Which screen capture backend is in use ("Windows Graphics Capture" or the
    /// "DXGI Desktop Duplication" fallback).
    pub fn capture_backend(&self) -> &'static str {
        self.capture_backend
    }

    pub fn pause(&self) {
        if let Ok(mut t) = self.shared.timeline.lock() {
            t.pause(Instant::now());
        }
        self.shared.paused.store(true, Ordering::Relaxed);
    }

    pub fn resume(&self) {
        if let Ok(mut t) = self.shared.timeline.lock() {
            t.resume(Instant::now());
        }
        self.shared.paused.store(false, Ordering::Relaxed);
    }

    pub fn status(&self) -> Status {
        Status {
            elapsed: self.final_elapsed.unwrap_or_else(|| self.shared.elapsed()),
            paused: self.shared.paused.load(Ordering::Relaxed),
            frames: self.shared.frames.load(Ordering::Relaxed),
            dropped: self.shared.dropped.load(Ordering::Relaxed),
            error: self.shared.error.lock().ok().and_then(|e| e.clone()),
        }
    }

    pub fn set_mic_muted(&self, muted: bool) {
        self.shared.mic_muted.store(muted, Ordering::Relaxed);
    }

    pub fn set_system_muted(&self, muted: bool) {
        self.shared.sys_muted.store(muted, Ordering::Relaxed);
    }

    pub fn set_webcam_visible(&self, visible: bool) {
        self.shared.webcam_visible.store(visible, Ordering::Relaxed);
    }

    pub fn set_overlays(&self, clicks: bool, keys: bool) {
        self.shared.clicks.store(clicks, Ordering::Relaxed);
        self.shared.keys.store(keys, Ordering::Relaxed);
    }

    fn teardown(&mut self) {
        if self.final_elapsed.is_none() {
            // Freeze the timeline so every worker agrees on the final duration.
            let now = Instant::now();
            if let Ok(mut t) = self.shared.timeline.lock() {
                let e = t.elapsed_at(now);
                t.pause(now);
                self.final_elapsed = Some(e);
            }
        }
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.video.take() {
            let _ = h.join();
        }
        if let Some(h) = self.mixer.take() {
            let _ = h.join();
        }
        self.shared.audio_stop.store(true, Ordering::Relaxed);
        for h in self.captures.drain(..) {
            let _ = h.join();
        }
        self.webcam = None;
    }

    /// Finalize the file. Blocks until the writer flushes (call from a worker thread).
    pub fn stop(mut self) -> Result<RecordingResult> {
        self.teardown();
        self.finished = true;
        let frames = self.shared.frames.load(Ordering::Relaxed);
        let err = self.shared.error.lock().ok().and_then(|e| e.clone());
        let fin = self
            .enc
            .lock()
            .map_err(|_| anyhow!("encoder lock poisoned"))?
            .finalize();
        if frames == 0 {
            let _ = std::fs::remove_file(&self.temp_path);
            return Err(anyhow!(
                err.unwrap_or_else(|| "no video frames were captured".into())
            ));
        }
        if let Err(e) = fin {
            let _ = std::fs::remove_file(&self.temp_path);
            return Err(e.context(format!(
                "could not finalize the recording{}",
                err.map(|m| format!(" ({m})")).unwrap_or_default()
            )));
        }
        if let Some(m) = &err {
            log::warn!("recording finished after an error: {m}");
        }
        if self.final_path.exists() {
            let _ = std::fs::remove_file(&self.final_path);
        }
        std::fs::rename(&self.temp_path, &self.final_path)
            .with_context(|| format!("cannot move recording to {}", self.final_path.display()))?;
        Ok(RecordingResult {
            path: self.final_path.clone(),
            duration: self.final_elapsed.unwrap_or_default(),
            width: self.width,
            height: self.height,
            encoder: self.encoder_name.clone(),
        })
    }

    /// Abort and delete the partial file.
    pub fn cancel(mut self) {
        self.abort();
    }

    fn abort(&mut self) {
        self.teardown();
        self.finished = true;
        if let Ok(e) = self.enc.lock() {
            let _ = e.finalize();
        }
        let _ = std::fs::remove_file(&self.temp_path);
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if !self.finished {
            self.abort();
        }
    }
}

fn spawn_audio(
    source: Source,
    queue: &Arc<Mutex<SourceQueue>>,
    shared: &Arc<Shared>,
) -> Result<JoinHandle<()>> {
    // `audio::spawn` takes an Arc<AtomicBool>; bridge from Shared's flag with a tiny watcher.
    let stop = Arc::new(AtomicBool::new(false));
    let fatal = Arc::new(Mutex::new(None::<String>));
    let handle = audio::spawn(source, queue.clone(), stop.clone(), fatal.clone())?;
    let shared = shared.clone();
    let stop_on_failure = stop.clone();
    // Watcher thread: forwards the shared stop flag and surfaces runtime errors. It is the
    // handle the recorder joins, and it joins the capture thread itself.
    std::thread::Builder::new()
        .name("record-audio-watch".into())
        .spawn(move || {
            while !shared.audio_stop.load(Ordering::Relaxed) {
                if let Ok(f) = fatal.lock()
                    && let Some(msg) = f.clone()
                {
                    shared.set_error(msg);
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            stop.store(true, Ordering::Relaxed);
            let _ = handle.join();
        })
        .map_err(|e| {
            // The capture thread is already running; with no watcher to forward the stop
            // flag it would run until the process exits.
            stop_on_failure.store(true, Ordering::Relaxed);
            anyhow::Error::from(e).context("spawn audio watcher")
        })
}

/// Joins a thread that may be stuck in an OS call; false when it did not finish in time.
fn join_bounded(h: JoinHandle<()>, limit: Duration) -> bool {
    let end = Instant::now() + limit;
    while !h.is_finished() {
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = h.join();
    true
}

/// Owns everything `Recorder::start` has spawned until the recorder exists, so every early
/// return stops and joins the workers (releasing devices) and removes the temp file.
struct StartGuard {
    shared: Arc<Shared>,
    captures: Vec<JoinHandle<()>>,
    video: Option<JoinHandle<()>>,
    temp: PathBuf,
    armed: bool,
}

impl Drop for StartGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.shared.stop.store(true, Ordering::Relaxed);
        self.shared.audio_stop.store(true, Ordering::Relaxed);
        let video_done = match self.video.take() {
            Some(v) => {
                let done = join_bounded(v, Duration::from_secs(10));
                if !done {
                    log::warn!("recording start failed while the video thread was still busy");
                }
                done
            }
            None => true,
        };
        for h in self.captures.drain(..) {
            let _ = h.join();
        }
        if video_done {
            let _ = std::fs::remove_file(&self.temp);
        }
        // Otherwise the video thread removes the temp file itself once it notices nobody
        // is waiting for it (see `video_thread`).
    }
}

#[allow(clippy::too_many_arguments)]
fn video_thread(
    shared: Arc<Shared>,
    opts: RecordOptions,
    region: stinkshot_core::PxRect,
    fps: u32,
    temp: PathBuf,
    want_audio: bool,
    webcam_latest: Arc<Mutex<Option<WebcamFrame>>>,
    init_tx: mpsc::Sender<InitResult>,
) {
    let _timer = TimerResolution::new();
    // Every capture call (duplication, frame acquisition, cursor position) runs on this
    // thread; Desktop Duplication refuses DPI-unaware callers on scaled displays, and a
    // duplication made from an aware thread delivers no frames to an unaware one. The app is
    // aware through its manifest; tools and test binaries are not.
    let _dpi = stinkshot_platform::dpi::ThreadPerMonitorAware::new();
    let session = match MfSession::new() {
        Ok(s) => s,
        Err(e) => {
            let _ = init_tx.send(Err(format!("{e:#}")));
            return;
        }
    };
    let init = (|| -> Result<(Capturer, Arc<Mutex<Encoder>>)> {
        let cap = Capturer::new(region, opts.include_cursor, fps)?;
        let vp = VideoParams {
            width: region.w as u32,
            height: region.h as u32,
            fps,
            bitrate_kbps: opts.bitrate_kbps,
        };
        let enc = Encoder::create(&temp, &vp, want_audio)?;
        Ok((cap, Arc::new(Mutex::new(enc))))
    })();
    let (mut cap, enc) = match init {
        Ok(v) => v,
        Err(e) => {
            let _ = init_tx.send(Err(format!("{e:#}")));
            return;
        }
    };
    log::info!("capture backend: {}", cap.backend_name);
    let name = enc
        .lock()
        .map(|e| e.encoder_name.clone())
        .unwrap_or_default();
    // The recording clock starts now.
    if let Ok(mut t) = shared.timeline.lock() {
        *t = Timeline::new(Instant::now());
    }
    if init_tx
        .send(Ok((enc.clone(), name, cap.backend_name)))
        .is_err()
    {
        // `Recorder::start` gave up (timed out) while we were still initialising: nobody
        // will finalize this file.
        drop(cap);
        if let Ok(e) = enc.lock() {
            let _ = e.finalize();
        }
        drop(enc);
        let _ = std::fs::remove_file(&temp);
        return;
    }

    if let Err(e) = video_loop(&shared, &opts, region, fps, &mut cap, &enc, &webcam_latest) {
        shared.set_error(format!("{e:#}"));
    }
    drop(cap);
    drop(session);
}

fn video_loop(
    shared: &Arc<Shared>,
    opts: &RecordOptions,
    region: stinkshot_core::PxRect,
    fps: u32,
    cap: &mut Capturer,
    enc: &Arc<Mutex<Encoder>>,
    webcam_latest: &Arc<Mutex<Option<WebcamFrame>>>,
) -> Result<()> {
    let (w, h) = (region.w as usize, region.h as usize);
    let mut sched = FrameScheduler::new(fps);
    let mut overlay = OverlayState::new();
    let fonts = OverlayFonts::load();
    if fonts.is_none() {
        log::warn!("no UI font found; key chips will not be drawn");
    }
    let mut pixmap = Pixmap::new(w as u32, h as u32).context("overlay pixmap")?;
    let mut nv12 = vec![0u8; w * h * 3 / 2];
    let mut have_picture = false;
    let (ev_tx, ev_rx) = mpsc::channel();
    let mut hook: Option<InputHook> = None;
    let mut hook_failed = false;
    let accent = opts.accent;
    let webcam_enabled = opts.webcam.clone();
    let start_wall = Instant::now();
    let mut was_paused = false;

    while !shared.stop.load(Ordering::Relaxed) {
        let now = Instant::now();
        let clicks = shared.clicks.load(Ordering::Relaxed);
        let keys = shared.keys.load(Ordering::Relaxed);

        // Input hook only exists while an overlay that needs it is enabled.
        if (clicks || keys) && hook.is_none() && !hook_failed {
            match std::panic::catch_unwind(AssertUnwindSafe(|| InputHook::install(ev_tx.clone()))) {
                Ok(Ok(h)) => hook = Some(h),
                Ok(Err(e)) => {
                    log::warn!("input hook unavailable: {e:#}");
                    hook_failed = true;
                }
                Err(_) => {
                    log::warn!("input hook unavailable (not implemented)");
                    hook_failed = true;
                }
            }
        } else if !clicks && !keys && hook.is_some() {
            hook = None;
            overlay.clear();
        }

        let (elapsed, paused) = match shared.timeline.lock() {
            Ok(t) => (t.elapsed_at(now), t.is_paused()),
            Err(_) => bail!("timeline lock poisoned"),
        };
        if paused != was_paused {
            if paused {
                overlay.release_all(elapsed);
            }
            was_paused = paused;
        }
        while let Ok((t, ev)) = ev_rx.try_recv() {
            if paused {
                continue;
            }
            let at = shared
                .timeline
                .lock()
                .map(|tl| tl.elapsed_at(t))
                .unwrap_or(elapsed);
            match ev {
                InputEvent::MouseDown { pos, button } if clicks => overlay.mouse_down(
                    (pos.x - region.x) as f32,
                    (pos.y - region.y) as f32,
                    map_button(button),
                    at,
                ),
                InputEvent::MouseUp { pos, button } if clicks => overlay.mouse_up(
                    (pos.x - region.x) as f32,
                    (pos.y - region.y) as f32,
                    map_button(button),
                    at,
                ),
                InputEvent::KeyDown { label, vk } if keys => overlay.key_down(vk, &label, at),
                InputEvent::KeyUp { vk } if keys => overlay.key_up(vk, at),
                _ => {}
            }
        }
        overlay.prune(elapsed);

        let stats = cap.poll()?;
        shared
            .dropped
            .fetch_add(stats.dropped as u64, Ordering::Relaxed);

        let ready = cap.has_all_frames()
            || (cap.has_any_frame() && now.duration_since(start_wall) > Duration::from_secs(1));
        if !ready && now.duration_since(start_wall) > FIRST_FRAME_TIMEOUT {
            bail!("the screen capture delivered no frames");
        }

        let show_webcam = webcam_enabled.is_some() && shared.webcam_visible.load(Ordering::Relaxed);
        let mut deadline_wall = now + Duration::from_millis(10);
        if ready && !paused {
            let due = sched.due(elapsed);
            if !due.is_empty() {
                let overlay_live = overlay.is_active(elapsed, clicks, keys);
                let webcam_live =
                    show_webcam && webcam_latest.lock().map(|g| g.is_some()).unwrap_or(false);
                let cursor_live = cap.needs_cursor_overlay();
                if stats.new_frames > 0
                    || overlay_live
                    || webcam_live
                    || cursor_live
                    || !have_picture
                {
                    let mapped = cap.map()?;
                    if overlay_live || webcam_live || cursor_live {
                        let dst = pixmap.data_mut();
                        let src = mapped.bytes(h);
                        for y in 0..h {
                            dst[y * w * 4..(y + 1) * w * 4].copy_from_slice(
                                &src[y * mapped.stride..y * mapped.stride + w * 4],
                            );
                        }
                        drop(mapped);
                        cap.composite_cursor(pixmap.data_mut(), w, h);
                        overlay.render(&mut pixmap, elapsed, accent, clicks, keys, fonts.as_ref());
                        if webcam_live
                            && let Some(wc) = &webcam_enabled
                            && let Ok(g) = webcam_latest.lock()
                            && let Some(frame) = g.as_ref()
                        {
                            let s = ui_scale(h as u32);
                            let d = wc.diameter.clamp(32, (w.min(h)) as u32);
                            let (x, y) = webcam_origin(
                                wc.corner,
                                d as i32,
                                w as i32,
                                h as i32,
                                (24.0 * s) as i32,
                            );
                            draw_webcam_bubble(&mut pixmap, frame, x, y, d, s);
                        }
                        bgra_to_nv12(pixmap.data(), w * 4, w, h, &mut nv12);
                    } else {
                        bgra_to_nv12(mapped.bytes(h), mapped.stride, w, h, &mut nv12);
                    }
                    have_picture = true;
                }
                let guard = enc.lock().map_err(|_| anyhow!("encoder lock poisoned"))?;
                for idx in due {
                    let pts = pts_hns(idx, fps);
                    let dur = pts_hns(idx + 1, fps) - pts;
                    guard.write_video(&nv12, pts, dur)?;
                    shared.frames.fetch_add(1, Ordering::Relaxed);
                }
            }
            let ahead = sched.next_deadline().saturating_sub(elapsed);
            deadline_wall = now + ahead;
        }
        sleep_until(deadline_wall.min(Instant::now() + Duration::from_millis(10)));
    }

    // Final pad so the video is as long as the recording (static tail).
    if have_picture {
        let elapsed = shared
            .timeline
            .lock()
            .map(|t| t.elapsed_at(Instant::now()))
            .unwrap_or_default();
        let due = sched.due(elapsed);
        if !due.is_empty() {
            let guard = enc.lock().map_err(|_| anyhow!("encoder lock poisoned"))?;
            for idx in due {
                let pts = pts_hns(idx, fps);
                guard.write_video(&nv12, pts, pts_hns(idx + 1, fps) - pts)?;
                shared.frames.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    Ok(())
}

fn map_button(b: MouseButton) -> Button {
    match b {
        MouseButton::Left => Button::Left,
        MouseButton::Right => Button::Right,
        MouseButton::Middle => Button::Middle,
    }
}

fn mixer_thread(
    shared: Arc<Shared>,
    enc: Arc<Mutex<Encoder>>,
    sys_q: Option<Arc<Mutex<SourceQueue>>>,
    mic_q: Option<Arc<Mutex<SourceQueue>>>,
) {
    let _timer = TimerResolution::new();
    for q in [&sys_q, &mic_q].into_iter().flatten() {
        if let Ok(mut q) = q.lock() {
            q.clear();
        }
    }
    let mut emitted: u64 = 0; // output frames written so far
    let mut buf_sys = Vec::new();
    let mut buf_mic = Vec::new();
    let mut mixed = Vec::new();
    let mut pcm = Vec::new();
    loop {
        let stopping = shared.stop.load(Ordering::Relaxed);
        let (elapsed, paused) = match shared.timeline.lock() {
            Ok(t) => (t.elapsed_at(Instant::now()), t.is_paused()),
            Err(_) => return,
        };
        if paused && !stopping {
            // drop audio captured while paused so resume starts clean
            for q in [&sys_q, &mic_q].into_iter().flatten() {
                if let Ok(mut q) = q.lock() {
                    q.clear();
                }
            }
        } else {
            let target = (elapsed.as_secs_f64() * OUT_RATE as f64) as u64;
            let required = target.saturating_sub(emitted) as usize;
            if required >= 960 || (stopping && required > 0) {
                let pull =
                    |q: &Option<Arc<Mutex<SourceQueue>>>, muted: bool, out: &mut Vec<f32>| {
                        out.clear();
                        if let Some(q) = q
                            && let Ok(mut q) = q.lock()
                        {
                            q.pull(required, MAX_AUDIO_LEAD_FRAMES, out);
                            if muted {
                                out.iter_mut().for_each(|s| *s = 0.0);
                            }
                        }
                    };
                pull(
                    &sys_q,
                    shared.sys_muted.load(Ordering::Relaxed),
                    &mut buf_sys,
                );
                pull(
                    &mic_q,
                    shared.mic_muted.load(Ordering::Relaxed),
                    &mut buf_mic,
                );
                mixed.clear();
                mixed.resize(required * 2, 0.0);
                mix(&mut mixed, &[&buf_sys, &buf_mic]);
                pcm.clear();
                to_pcm16(&mixed, &mut pcm);
                let pts = (emitted as u128 * 10_000_000 / OUT_RATE as u128) as i64;
                let dur = (required as u128 * 10_000_000 / OUT_RATE as u128) as i64;
                let res = match enc.lock() {
                    Ok(e) => e.write_audio(&pcm, pts, dur),
                    Err(_) => return,
                };
                if let Err(e) = res {
                    shared.set_error(format!("audio write failed: {e:#}"));
                    return;
                }
                emitted += required as u64;
            }
        }
        if stopping {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
