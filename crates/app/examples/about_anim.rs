//! Runs the About page header (the animated logo with the name, version and tagline, in
//! the app's fonts and page colours) alone in a window and reports its timing:
//! `cargo run -p stinkshot --example about_anim -- [dark|light] [seconds] [screenshot.png]
//! [cold] [prewarm]`.
//!
//! The window shows a blank page for 1 s first, so the numbers are those of switching to
//! the About page in a window that is already up (not of the window's own start-up).
//! `cold` deletes the disk cache entry for this display scale first. `prewarm` calls
//! `logo_anim::prewarm` during that second (as the Settings window does), for 2 s instead.
//! `top` keeps the window on top, so it is never covered while measuring.
//! `STINKSHOT_LOGO_CACHE=off` runs without the cache at all.
//!
//! Prints, from the header's first frame: the first orb image, the first movement and every
//! frame at full quality, whether the loop came from the cache, then the frame interval
//! and the UI-thread CPU time of `logo_anim::show` while frames were still arriving and
//! afterwards. Saves a screenshot of the window at the end when a path is given.

#[allow(dead_code)] // the app uses the rest of the strings table
#[path = "../src/l10n/mod.rs"]
mod l10n;
#[allow(dead_code)] // the app also uses prewarm_cache
#[path = "../src/logo_anim.rs"]
mod logo_anim;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn main() -> eframe::Result {
    let mut dark = true;
    let mut seconds = 6.0;
    let mut shot = None;
    let mut cold = false;
    let mut prewarm = false;
    let mut top = false;
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "dark" => dark = true,
            "light" => dark = false,
            "cold" => cold = true,
            "prewarm" => prewarm = true,
            "top" => top = true,
            s => match s.parse::<f64>() {
                Ok(v) => seconds = v,
                Err(_) => shot = Some(s.to_string()),
            },
        }
    }
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("About logo")
        .with_inner_size([520.0, 300.0]);
    if top {
        viewport = viewport.with_always_on_top();
    }
    let intro = Duration::from_millis(if prewarm { 2000 } else { 1000 });
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "About logo",
        options,
        Box::new(move |cc| {
            cc.egui_ctx.set_theme(if dark {
                egui::Theme::Dark
            } else {
                egui::Theme::Light
            });
            install_fonts(&cc.egui_ctx);
            Ok(Box::new(App {
                seconds,
                shot,
                dark,
                cold,
                prewarm,
                intro_until: Instant::now() + intro,
                header_since: None,
                last: None,
                dts: [Vec::new(), Vec::new()],
                show_ms: [Vec::new(), Vec::new()],
                state: Arc::new(Mutex::new(Phase::Measuring)),
            }))
        }),
    )
}

enum Phase {
    Measuring,
    Shooting,
    Done,
}

struct App {
    dark: bool,
    seconds: f64,
    shot: Option<String>,
    cold: bool,
    prewarm: bool,
    intro_until: Instant,
    header_since: Option<Instant>,
    last: Option<Instant>,
    /// Samples while frames were still arriving [0] and once all were full quality [1].
    dts: [Vec<f32>; 2],
    show_ms: [Vec<f32>; 2],
    state: Arc<Mutex<Phase>>,
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.cold {
            self.cold = false;
            let (size, ss) = frame_size(ctx.pixels_per_point());
            if let Some(p) = logo_anim::cache_path(size, ss) {
                let gone = std::fs::remove_file(&p).is_ok();
                println!(
                    "cold: {} ({})",
                    p.display(),
                    if gone { "deleted" } else { "absent" }
                );
            }
        }
        // The settings content pane colour and text colours (theme.rs palette).
        let (layer, text, secondary) = if self.dark {
            (
                egui::Color32::from_rgb(0x27, 0x27, 0x27),
                egui::Color32::WHITE,
                egui::Color32::from_rgb(0xc8, 0xc8, 0xc8),
            )
        } else {
            (
                egui::Color32::from_rgb(0xf9, 0xf9, 0xf9),
                egui::Color32::from_rgb(0x1a, 0x1a, 0x1a),
                egui::Color32::from_rgb(0x5c, 0x5c, 0x5c),
            )
        };
        let intro = Instant::now() < self.intro_until;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(layer).inner_margin(12.0))
            .show(ui, |ui| {
                if intro {
                    if self.prewarm {
                        logo_anim::prewarm(&ctx);
                    }
                    ui.label("Another settings page");
                    ctx.request_repaint();
                } else {
                    logo_anim::header(
                        ui,
                        egui::FontId::new(22.0, egui::FontFamily::Name(SEMIBOLD.into())),
                        egui::FontId::proportional(13.0),
                        text,
                        secondary,
                    );
                }
            });
        if intro {
            return;
        }
        let now = Instant::now();
        let since = *self.header_since.get_or_insert(now);
        let Some(stats) = logo_anim::stats(&ctx) else {
            return;
        };
        let bucket = usize::from(stats.all_full.is_some());
        if let Some(last) = self.last {
            self.dts[bucket].push((now - last).as_secs_f32() * 1000.0);
        }
        self.show_ms[bucket].push(stats.show_ms);
        self.last = Some(now);
        let mut phase = self.state.lock().expect("state");
        if matches!(*phase, Phase::Measuring) && since.elapsed().as_secs_f64() > self.seconds {
            println!(
                "frames: {} px, ss {}, {} frames {} in {:?}",
                stats.size,
                stats.ss,
                logo_anim::FRAMES,
                if stats.from_cache {
                    "loaded from the cache"
                } else {
                    "rendered"
                },
                stats.loop_total,
            );
            println!(
                "from the header's first frame: first image {:?}, first motion {:?}, all full quality {:?}",
                stats.first_frame, stats.first_motion, stats.all_full
            );
            println!("playback held waiting for frames: {:?}", stats.held);
            report("frame interval while frames arrive", &mut self.dts[0]);
            report("show() CPU while frames arrive", &mut self.show_ms[0]);
            report("frame interval after", &mut self.dts[1]);
            report("show() CPU after", &mut self.show_ms[1]);
            match &self.shot {
                Some(path) => {
                    let path = path.clone();
                    let state = self.state.clone();
                    let c = ctx.clone();
                    ctx.request_screenshot(move |img| {
                        let px: Vec<u8> = img.pixels.iter().flat_map(|p| p.to_array()).collect();
                        let (w, h) = (img.size[0] as u32, img.size[1] as u32);
                        if let Err(e) =
                            image::save_buffer(&path, &px, w, h, image::ExtendedColorType::Rgba8)
                        {
                            eprintln!("screenshot: {e}");
                        }
                        *state.lock().expect("state") = Phase::Done;
                        c.request_repaint();
                    });
                    *phase = Phase::Shooting;
                }
                None => *phase = Phase::Done,
            }
        }
        if matches!(*phase, Phase::Done) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

/// Same as `logo_anim`'s private helper.
fn frame_size(ppp: f32) -> (u32, u32) {
    let size = (logo_anim::VIEW * ppp).round().max(16.0) as u32;
    (size, if ppp < 1.5 { 2 } else { 1 })
}

const SEMIBOLD: &str = "StinkShot.Semibold";

/// The app's faces (theme.rs): Segoe UI Variable, and Segoe UI Semibold for headings.
fn install_fonts(ctx: &egui::Context) {
    let dir =
        std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap_or_default()).join("Fonts");
    let mut defs = egui::FontDefinitions::default();
    for (name, file) in [("Regular", "SegUIVar.ttf"), ("Semibold", "seguisb.ttf")] {
        if let Ok(bytes) = std::fs::read(dir.join(file)) {
            defs.font_data
                .insert(name.into(), Arc::new(egui::FontData::from_owned(bytes)));
        }
    }
    let mut fallbacks = defs
        .families
        .get(&egui::FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    if defs.font_data.contains_key("Regular") {
        defs.families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "Regular".into());
    }
    if defs.font_data.contains_key("Semibold") {
        fallbacks.insert(0, "Semibold".into());
    }
    defs.families
        .insert(egui::FontFamily::Name(SEMIBOLD.into()), fallbacks);
    ctx.set_fonts(defs);
}

fn report(name: &str, v: &mut [f32]) {
    if v.is_empty() {
        println!("{name}: no samples");
        return;
    }
    v.sort_by(f32::total_cmp);
    let mean = v.iter().sum::<f32>() / v.len() as f32;
    let pct = |p: f32| v[((v.len() - 1) as f32 * p) as usize];
    println!(
        "{name}: n={} mean {mean:.2} ms, p50 {:.2}, p95 {:.2}, p99 {:.2}, max {:.2}",
        v.len(),
        pct(0.5),
        pct(0.95),
        pct(0.99),
        v[v.len() - 1]
    );
}
