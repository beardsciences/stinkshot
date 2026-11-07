//! Runs the About page logo animation alone in a window and reports its timing:
//! `cargo run -p stinkshot --example about_anim -- [dark|light] [seconds] [screenshot.png]`.
//!
//! Prints how long the still frame and the full loop took to render, then, once the loop
//! plays, the frame interval and the UI-thread CPU time of `logo_anim::show` over the
//! measuring window. Saves a screenshot of the window at the end when a path is given.

#[path = "../src/logo_anim.rs"]
mod logo_anim;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn main() -> eframe::Result {
    let mut args = std::env::args().skip(1);
    let dark = args.next().as_deref() != Some("light");
    let seconds: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(6.0);
    let shot = args.next();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("About logo")
            .with_inner_size([440.0, 240.0]),
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
            Ok(Box::new(App {
                seconds,
                shot,
                opened: Instant::now(),
                playing_since: None,
                last: None,
                dts: Vec::new(),
                show_ms: Vec::new(),
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
    seconds: f64,
    shot: Option<String>,
    opened: Instant,
    playing_since: Option<Instant>,
    last: Option<Instant>,
    dts: Vec<f32>,
    show_ms: Vec<f32>,
    state: Arc<Mutex<Phase>>,
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(8.0);
            logo_anim::show(ui);
        });
        let Some(stats) = logo_anim::stats(&ctx) else {
            return;
        };
        let now = Instant::now();
        if self.playing_since.is_none() && stats.loop_total.is_some() {
            self.playing_since = Some(now);
            println!(
                "frames: {} px, ss {}, still {:?}, loop of {} frames {:?} (window opened {:?} ago)",
                stats.size,
                stats.ss,
                stats.still,
                logo_anim::FRAMES,
                stats.loop_total,
                self.opened.elapsed()
            );
        }
        let Some(since) = self.playing_since else {
            return;
        };
        // Skip the first half second of playback (texture warm-up).
        if since.elapsed() > Duration::from_millis(500) {
            if let Some(last) = self.last {
                self.dts.push((now - last).as_secs_f32() * 1000.0);
            }
            self.show_ms.push(stats.show_ms);
        }
        self.last = Some(now);
        let mut phase = self.state.lock().expect("state");
        if matches!(*phase, Phase::Measuring) && since.elapsed().as_secs_f64() > self.seconds {
            report("frame interval", &mut self.dts);
            report("show() CPU", &mut self.show_ms);
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
