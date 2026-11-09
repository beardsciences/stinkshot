//! Offscreen renders of the overlay's panels and bars in the pseudo locale, for spotting
//! clipped or overflowing text without opening a real overlay (which is excluded from
//! capture). Ignored by default because it switches the global language:
//! `STINKSHOT_SHOTS_DIR=<dir> cargo test -p stinkshot pseudo_shots -- --ignored`.

use eframe::egui_wgpu::{self, wgpu};
use egui::{Align2, Color32, Context, Pos2, Rect, Vec2};
use stinkshot_core::PxRect;
use stinkshot_core::beautify::Beautify;
use stinkshot_core::codes::Code;
use stinkshot_core::color::ColorFormat;
use stinkshot_core::fonts::Fonts;
use stinkshot_core::sensitive::{Finding, SensitiveKind};
use stinkshot_core::table::Table;

use super::scan::{ScanResult, ScanView};
use super::shapes::Tool;
use super::smart_redact::{PanelView, RedactStyle, Review};
use super::ui::{self, ActionBarState, BarInfo, ContextBar};
use crate::theme;

type Scene = Box<dyn Fn(&Context)>;

fn scenes() -> Vec<(&'static str, Vec2, Scene)> {
    let accent = Color32::from_rgb(0x4c, 0xc2, 0xff);
    let pal = || theme::palette(true);
    let at = Pos2::new(16.0, 16.0);
    vec![
        (
            "bars",
            Vec2::new(1500.0, 260.0),
            Box::new(move |ctx| {
                let info = BarInfo {
                    tool: Tool::Arrow,
                    color: [230, 60, 60, 255],
                    width: 4.0,
                    picking: false,
                };
                ui::tool_bar(ctx, at, &info, pal(), false, &mut Vec::new());
                let st = ActionBarState {
                    ocr: true,
                    add_regions: Some(false),
                    ..Default::default()
                };
                ui::action_bar(
                    ctx,
                    Pos2::new(16.0, 70.0),
                    pal(),
                    st,
                    false,
                    &mut Vec::new(),
                );
                let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1500.0, 260.0));
                let text = ContextBar::Text {
                    bold: false,
                    pill: false,
                    size: 20.0,
                };
                let anchor = Rect::from_min_size(Pos2::new(16.0, 130.0), Vec2::new(10.0, 10.0));
                ui::context_bar(ctx, anchor, screen, text, pal(), &mut Vec::new());
                let mut exact = String::new();
                let mut chips = ui::ChipState {
                    size: (760, 420),
                    ratio: None,
                    exact_open: false,
                    exact_just_opened: false,
                    exact_text: &mut exact,
                    purpose: Some(super::hover::purpose_label(
                        crate::state::RegionPurpose::RecordGif,
                    )),
                    editable: true,
                    zoom: Some(150),
                };
                ui::size_chips(
                    ctx,
                    Pos2::new(16.0, 200.0),
                    &mut chips,
                    pal(),
                    accent,
                    &mut Vec::new(),
                );
            }),
        ),
        (
            "hints",
            Vec2::new(1500.0, 140.0),
            Box::new(move |ctx| {
                let items = [
                    (
                        crate::l10n::tr!("overlay.keys.k.drag"),
                        crate::l10n::tr!("overlay.hint.region"),
                    ),
                    (
                        crate::l10n::tr!("overlay.keys.k.click"),
                        crate::l10n::tr!("overlay.keys.window_or_element"),
                    ),
                    (
                        crate::l10n::tr!("overlay.keys.k.wheel"),
                        crate::l10n::tr!("overlay.hint.parent_child"),
                    ),
                    ("Enter", crate::l10n::tr!("overlay.hint.select_highlight")),
                    ("Esc", crate::l10n::tr!("overlay.hint.cancel")),
                    ("?", crate::l10n::tr!("overlay.hint.shortcuts")),
                ];
                let lead = crate::l10n::tr!("overlay.notice.discard_prompt");
                ui::hint_bar(
                    ctx,
                    Pos2::new(750.0, 16.0),
                    Some(lead),
                    &items,
                    pal(),
                    accent,
                );
                ui::loupe_plate(
                    ctx,
                    Pos2::new(16.0, 70.0),
                    (1234, 567),
                    150,
                    "#CE9178",
                    [0xce, 0x91, 0x78],
                    None,
                    pal(),
                    accent,
                );
                let label = super::snap_label_text("Pane", "Files", 1, 4);
                ui::element_chip(ctx, Pos2::new(300.0, 80.0), &label, pal());
            }),
        ),
        (
            "help",
            Vec2::new(1920.0, 1000.0),
            Box::new(move |ctx| {
                let sheet = super::keys::sheet(false);
                ui::shortcut_sheet(ctx, Pos2::new(960.0, 500.0), 1920.0, &sheet, pal(), accent);
            }),
        ),
        (
            "beautify",
            Vec2::new(420.0, 760.0),
            Box::new(move |ctx| {
                let note = super::beautify_panel::fit_note(
                    stinkshot_core::beautify::Fit::PaddingOnly,
                    stinkshot_core::beautify::Aspect::Wide,
                );
                let b = Beautify {
                    enabled: true,
                    ..Default::default()
                };
                super::beautify_panel::panel(
                    ctx,
                    at,
                    Align2::LEFT_TOP,
                    &b,
                    None,
                    note.as_deref(),
                    pal(),
                    &mut Vec::new(),
                );
            }),
        ),
        (
            "scan",
            Vec2::new(420.0, 560.0),
            Box::new(move |ctx| {
                let result = ScanResult {
                    codes: vec![Code {
                        format: "QR Code",
                        text: "https://example.com/a/long/path".into(),
                        rect: PxRect::new(0, 0, 10, 10),
                    }],
                    text: Some("Name  Qty\nApples  3\nPears  5".into()),
                    table: Some(Table {
                        rows: vec![vec!["a".into(), "b".into()]; 3],
                        columns: 2,
                    }),
                    ocr_failed: false,
                };
                let view = ScanView::Ready {
                    result: &result,
                    copied: None,
                    ocr: true,
                };
                super::scan::panel(
                    ctx,
                    at,
                    Align2::LEFT_TOP,
                    &view,
                    pal(),
                    accent,
                    &mut Vec::new(),
                );
            }),
        ),
        (
            "redact",
            Vec2::new(420.0, 460.0),
            Box::new(move |ctx| {
                let kinds = [
                    SensitiveKind::Ssn,
                    SensitiveKind::Email,
                    SensitiveKind::Card,
                    SensitiveKind::Secret,
                ];
                let review = Review {
                    findings: kinds
                        .iter()
                        .map(|k| Finding {
                            kind: *k,
                            preview: "j\u{2022}\u{2022}\u{2022}@example.com".into(),
                            rect: PxRect::new(0, 0, 10, 10),
                        })
                        .collect(),
                    on: vec![true; 4],
                    hovered: None,
                    style: RedactStyle::Box,
                };
                let view = PanelView::Review(&review);
                super::smart_redact::panel(
                    ctx,
                    at,
                    Align2::LEFT_TOP,
                    &view,
                    pal(),
                    accent,
                    &mut Vec::new(),
                );
            }),
        ),
        (
            "picker",
            Vec2::new(700.0, 120.0),
            Box::new(move |ctx| {
                let recent = vec!["#CE9178".to_string(), "#4CC2FF".to_string()];
                super::picker::panel(
                    ctx,
                    Pos2::new(350.0, 16.0),
                    ColorFormat::Hex,
                    &recent,
                    pal(),
                    &mut Vec::new(),
                );
                let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(700.0, 120.0));
                let anchor = Rect::from_min_size(Pos2::new(16.0, 60.0), Vec2::new(34.0, 4.0));
                let colors = [[230, 60, 60, 255]; 8];
                let recent = [[0xce, 0x91, 0x78, 255]];
                ui::color_popup(
                    ctx,
                    anchor,
                    screen,
                    &colors,
                    &recent,
                    colors[0],
                    pal(),
                    &mut Vec::new(),
                );
            }),
        ),
    ]
}

/// Renders `scene` for a few frames and returns RGBA8 pixels of the last one.
fn render(
    dev: &wgpu::Device,
    queue: &wgpu::Queue,
    fonts: &Fonts,
    size: Vec2,
    scene: &dyn Fn(&Context),
) -> image::RgbaImage {
    let ctx = Context::default();
    theme::install(&ctx, fonts, true, Color32::from_rgb(0x4c, 0xc2, 0xff));
    let (w, h) = (size.x as u32, size.y as u32);
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut renderer = egui_wgpu::Renderer::new(dev, format, Default::default());
    let mut jobs = Vec::new();
    let mut ppp = 1.0;
    for _ in 0..4 {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| scene(ui.ctx()));
        for (id, deltas) in &out.textures_delta.set {
            for delta in deltas {
                renderer.update_texture(dev, queue, *id, delta);
            }
        }
        for id in &out.textures_delta.free {
            renderer.free_texture(id);
        }
        out.textures_delta.clear();
        ppp = out.pixels_per_point;
        jobs = ctx.tessellate(std::mem::take(&mut out.shapes), ppp);
    }
    let tex = dev.create_texture(&wgpu::TextureDescriptor {
        label: Some("pseudo shot"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [w, h],
        pixels_per_point: ppp,
    };
    let mut enc = dev.create_command_encoder(&Default::default());
    let mut cmds = renderer.update_buffers(dev, queue, &mut enc, &jobs, &screen);
    {
        let mut pass = enc
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pseudo shot"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.18,
                            g: 0.2,
                            b: 0.24,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            })
            .forget_lifetime();
        renderer.render(&mut pass, &jobs, &screen);
    }
    let row = (w * 4).div_ceil(256) * 256;
    let buf = dev.create_buffer(&wgpu::BufferDescriptor {
        label: Some("pseudo shot readback"),
        size: u64::from(row * h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    cmds.push(enc.finish());
    queue.submit(cmds);
    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    dev.poll(wgpu::PollType::wait_indefinitely()).expect("poll");
    let data = slice.get_mapped_range().expect("mapped");
    let mut img = image::RgbaImage::new(w, h);
    for y in 0..h {
        let src = &data[(y * row) as usize..(y * row + w * 4) as usize];
        img.as_mut()[(y * w * 4) as usize..((y + 1) * w * 4) as usize].copy_from_slice(src);
    }
    img
}

#[test]
#[ignore = "switches the global language; run alone with STINKSHOT_SHOTS_DIR set"]
fn pseudo_shots() {
    let dir = std::path::PathBuf::from(
        std::env::var("STINKSHOT_SHOTS_DIR").unwrap_or_else(|_| "pseudo-shots".into()),
    );
    std::fs::create_dir_all(&dir).unwrap();
    crate::l10n::apply("pseudo");
    let setup = crate::gpu::start()
        .and_then(|h| h.join().ok().flatten())
        .expect("no wgpu device");
    let fonts = Fonts::load_system().unwrap();
    for (name, size, scene) in scenes() {
        let img = render(&setup.device, &setup.queue, &fonts, size, scene.as_ref());
        img.save(dir.join(format!("{name}.png"))).unwrap();
    }
    crate::l10n::apply("en");
}
