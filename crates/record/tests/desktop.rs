//! Desktop integration tests: they need an interactive session with a display and (for the
//! audio assertions) a playback device. Run with
//! `cargo test -p stinkshot-record -- --ignored --test-threads=1`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use stinkshot_record::*;

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "stinkshot-record-test-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn opts(out: PathBuf, system_audio: bool) -> RecordOptions {
    let region = primary_monitor_rect().expect("primary monitor");
    RecordOptions {
        region,
        output: out,
        fps: 30,
        bitrate_kbps: 8000,
        system_audio,
        microphone: false,
        mic_device: None,
        webcam: None,
        show_clicks: false,
        show_keys: false,
        include_cursor: true,
        accent: [0x4C, 0xA3, 0xFF, 255],
    }
}

fn secs(d: Duration) -> f32 {
    d.as_secs_f32()
}

#[test]
#[ignore = "needs a desktop session"]
fn record_probe_thumbnails_trim_gif() {
    let dir = temp_dir("full");
    let out = dir.join("rec.mp4");
    let rec = Recorder::start(opts(out.clone(), true)).expect("start recording");
    std::thread::sleep(Duration::from_millis(2500));
    let st = rec.status();
    println!("status after 2.5 s: {st:?}");
    assert!(st.error.is_none(), "{:?}", st.error);
    assert!(st.frames > 20, "frames={}", st.frames);
    let backend = rec.capture_backend();
    let res = rec.stop().expect("stop");
    println!("result: {res:?}");
    println!("capture backend: {backend}");
    assert_eq!(res.path, out);
    assert!(out.exists());
    assert!(
        !dir.join("rec.recording.tmp.mp4").exists(),
        "temp file must be renamed away"
    );
    assert_eq!(res.width % 2, 0);
    assert_eq!(res.height % 2, 0);
    assert!(
        secs(res.duration) > 2.2 && secs(res.duration) < 3.5,
        "duration {:?}",
        res.duration
    );
    println!("encoder: {}", res.encoder);

    let info = probe(&out).expect("probe");
    println!("probe: {info:?}");
    assert_eq!((info.width, info.height), (res.width, res.height));
    assert!(info.has_audio, "system audio track expected");
    assert!(
        (secs(info.duration) - secs(res.duration)).abs() < 0.4,
        "{:?} vs {:?}",
        info.duration,
        res.duration
    );
    assert!((info.fps - 30.0).abs() < 1.0, "fps {}", info.fps);

    let thumbs = thumbnails(&out, 5, 90).expect("thumbnails");
    assert_eq!(thumbs.len(), 5);
    for t in &thumbs {
        assert_eq!(t.height, 90);
        assert!(t.width > 90);
        assert_eq!(t.data.len(), (t.width * t.height * 4) as usize);
        assert!(t.data.chunks_exact(4).all(|p| p[3] == 255));
    }
    let f = frame_at(&out, Duration::from_millis(1000), 240).expect("frame_at");
    assert_eq!(f.height, 240);
    let first = f.pixel(0, 0);
    assert!(
        f.data.chunks_exact(4).any(|p| p[..3] != first[..3]),
        "frame is a single flat colour"
    );

    let trimmed = dir.join("trimmed.mp4");
    let last = std::cell::Cell::new(0f32);
    trim(
        &out,
        &trimmed,
        Duration::from_millis(500),
        Duration::from_millis(1500),
        &|p| last.set(p),
    )
    .expect("trim");
    assert!((last.get() - 1.0).abs() < 1e-6);
    let tinfo = probe(&trimmed).expect("probe trimmed");
    println!("trimmed: {tinfo:?}");
    assert!(
        (secs(tinfo.duration) - 1.0).abs() < 0.25,
        "trimmed duration {:?}",
        tinfo.duration
    );
    assert!(tinfo.has_audio);
    assert_eq!((tinfo.width, tinfo.height), (res.width, res.height));

    let gif_path = dir.join("out.gif");
    let gopts = GifOptions {
        fps: 10,
        max_width: 480,
        start: Duration::ZERO,
        end: None,
        quality: 80,
        loop_forever: true,
    };
    let prog = std::cell::Cell::new(0f32);
    to_gif(&out, &gif_path, &gopts, &|p| prog.set(p)).expect("to_gif");
    assert!((prog.get() - 1.0).abs() < 1e-6);
    let mut dec_opts = gif::DecodeOptions::new();
    dec_opts.set_color_output(gif::ColorOutput::RGBA);
    let mut dec = dec_opts
        .read_info(std::fs::File::open(&gif_path).unwrap())
        .expect("gif header");
    assert_eq!(dec.width(), 480);
    let mut frames = 0;
    let mut total_cs = 0u32;
    while let Some(fr) = dec.read_next_frame().expect("gif frame") {
        frames += 1;
        total_cs += fr.delay as u32;
    }
    println!(
        "gif: {} bytes, {frames} frames, {total_cs} cs",
        std::fs::metadata(&gif_path).unwrap().len()
    );
    assert!(frames >= 1);
    assert!(
        (total_cs as f32 / 100.0 - secs(info.duration)).abs() < 0.5,
        "gif length {total_cs} cs vs {:?}",
        info.duration
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "needs a desktop session"]
fn pause_resume_gives_one_continuous_file_and_cancel_cleans_up() {
    let dir = temp_dir("pause");
    let out = dir.join("paused.mp4");
    let mut o = opts(out.clone(), false);
    o.show_clicks = true;
    o.show_keys = true;
    let rec = Recorder::start(o).expect("start");
    let t0 = Instant::now();
    std::thread::sleep(Duration::from_millis(1000));
    rec.pause();
    assert!(rec.status().paused);
    // let any frames already in flight drain, then nothing more may be written
    std::thread::sleep(Duration::from_millis(400));
    let frames_at_pause = rec.status().frames;
    std::thread::sleep(Duration::from_millis(1100));
    let frames_after = rec.status().frames;
    assert_eq!(
        frames_after, frames_at_pause,
        "frames written while paused: {frames_at_pause} -> {frames_after}"
    );
    rec.resume();
    std::thread::sleep(Duration::from_millis(1000));
    let res = rec.stop().expect("stop");
    let wall = t0.elapsed();
    println!("wall {wall:?}, recorded {:?}", res.duration);
    assert!(
        secs(res.duration) > 1.7 && secs(res.duration) < 2.6,
        "{:?}",
        res.duration
    );
    assert!(
        secs(wall) - secs(res.duration) > 1.2,
        "pause time must be removed from the output"
    );
    let info = probe(&out).expect("probe");
    assert!(!info.has_audio);
    assert!((secs(info.duration) - secs(res.duration)).abs() < 0.4);

    // cancel deletes everything
    let out2 = dir.join("cancelled.mp4");
    let rec = Recorder::start(opts(out2.clone(), false)).expect("start 2");
    std::thread::sleep(Duration::from_millis(600));
    rec.cancel();
    assert!(!out2.exists());
    assert!(!dir.join("cancelled.recording.tmp.mp4").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "needs a desktop session"]
fn mute_and_mic_enumeration() {
    let mics = list_microphones();
    let cams = list_webcams();
    println!("microphones: {mics:?}");
    println!("webcams: {cams:?}");
    let dir = temp_dir("mute");
    let out = dir.join("mute.mp4");
    let mut o = opts(out.clone(), true);
    if !mics.is_empty() {
        o.microphone = true;
        o.mic_device = Some(mics[0].clone());
    }
    let rec = Recorder::start(o).expect("start");
    rec.set_system_muted(true);
    rec.set_mic_muted(true);
    std::thread::sleep(Duration::from_millis(1200));
    rec.set_system_muted(false);
    rec.set_mic_muted(false);
    std::thread::sleep(Duration::from_millis(600));
    let res = rec.stop().expect("stop");
    assert!(probe(&res.path).unwrap().has_audio);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "needs a webcam"]
fn webcam_overlay_records_when_a_camera_exists() {
    let cams = list_webcams();
    if cams.is_empty() {
        println!("no webcam attached; skipping");
        return;
    }
    let dir = temp_dir("cam");
    let out = dir.join("cam.mp4");
    let mut o = opts(out.clone(), false);
    o.webcam = Some(WebcamOptions {
        device: Some(cams[0].clone()),
        diameter: 220,
        corner: Corner::BottomRight,
    });
    let rec = Recorder::start(o).expect("start with webcam");
    std::thread::sleep(Duration::from_millis(2000));
    let res = rec.stop().expect("stop");
    assert!(res.path.exists());
    // the white ring of the bubble sits at the top-centre of the bubble (24 px margin, scaled
    // by the 2.0 clamp for a tall frame)
    let frame = frame_at(&res.path, Duration::from_millis(1200), res.height).expect("frame");
    let s = (res.height as f32 / 1080.0).clamp(0.8, 2.0);
    let margin = (24.0 * s) as u32;
    let (x, y) = (res.width - 220 - margin, res.height - 220 - margin);
    let ring = frame.pixel(x + 110, y + 1);
    println!("ring pixel {ring:?}");
    assert!(
        ring[0] > 200 && ring[1] > 200 && ring[2] > 200,
        "bubble ring not found: {ring:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The app keeps a warm Desktop Duplication session (platform `capture::warm_up`) for fast
/// overlays. A process may duplicate each output only once, so the recorder must take the
/// duplication over (and give it back) instead of failing with E_INVALIDARG. Captures made
/// while recording fall back to GDI and still work.
#[test]
#[ignore = "needs a desktop session"]
fn records_while_the_capture_session_is_warm() {
    use stinkshot_platform::capture;
    capture::warm_up();
    let region = primary_monitor_rect().expect("primary monitor");
    let small = stinkshot_core::PxRect::new(region.x, region.y, 64, 64);
    capture::capture_rect(small, false).expect("capture before recording");
    let dir = temp_dir("warm");
    let out = dir.join("warm.mp4");
    let rec = Recorder::start(opts(out.clone(), false)).expect("start with a warm session");
    std::thread::sleep(Duration::from_millis(800));
    let during = capture::capture_rect(small, false).expect("capture while recording");
    assert_eq!((during.width, during.height), (64, 64));
    capture::warm_up(); // must not take the duplication back
    std::thread::sleep(Duration::from_millis(700));
    let st = rec.status();
    assert!(st.error.is_none(), "{:?}", st.error);
    println!("backend {}", rec.capture_backend());
    let res = rec.stop().expect("stop");
    assert!(secs(res.duration) > 1.0, "{:?}", res.duration);
    capture::capture_rect(small, false).expect("capture after recording");
    // and a second recording right after works too
    let rec = Recorder::start(opts(dir.join("again.mp4"), false)).expect("second start");
    std::thread::sleep(Duration::from_millis(500));
    rec.stop().expect("second stop");
    let _ = std::fs::remove_dir_all(&dir);
}
