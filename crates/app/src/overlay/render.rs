//! Rendering support: a background worker that flattens committed annotations with
//! `stinkshot_core::annotate::render`, and tiled GPU textures so that images wider than
//! the GPU's maximum texture size (multi-monitor desktops) still draw 1:1.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use egui::{Color32, ColorImage, Context, Pos2, Rect, TextureHandle, TextureOptions, Vec2};
use stinkshot_core::annotate::{self, Doc};
use stinkshot_core::fonts::Fonts;
use stinkshot_core::{PxRect, Rgba};

/// Tile edge in texels. 2048 is supported by every adapter.
pub const TILE: u32 = 2048;

pub struct RenderJob {
    pub gen_id: u64,
    pub base: Arc<Rgba>,
    pub doc: Doc,
    pub fonts: Arc<Fonts>,
}

pub struct RenderResult {
    pub gen_id: u64,
    pub doc: Doc,
    /// None if the renderer failed (the error is logged); the UI keeps its previous frame.
    pub image: Option<Rgba>,
}

/// Single long-lived worker; jobs submitted while it is busy are coalesced so only the
/// newest one is rendered.
pub struct Renderer {
    tx: Sender<RenderJob>,
    rx: Receiver<RenderResult>,
    next_gen: u64,
}

impl Renderer {
    pub fn new(ctx: &Context) -> Self {
        let (tx, job_rx) = channel::<RenderJob>();
        let (res_tx, rx) = channel::<RenderResult>();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("overlay-render".into())
            .spawn(move || {
                while let Ok(mut job) = job_rx.recv() {
                    while let Ok(newer) = job_rx.try_recv() {
                        job = newer;
                    }
                    let image = flatten(&job.base, &job.doc, &job.fonts);
                    if res_tx
                        .send(RenderResult {
                            gen_id: job.gen_id,
                            doc: job.doc,
                            image,
                        })
                        .is_err()
                    {
                        break;
                    }
                    ctx.request_repaint();
                }
            });
        if let Err(e) = spawned {
            log::error!("overlay: could not start render worker: {e}");
        }
        Self {
            tx,
            rx,
            next_gen: 1,
        }
    }

    /// Queue a render; returns its generation id.
    pub fn submit(&mut self, base: Arc<Rgba>, doc: Doc, fonts: Arc<Fonts>) -> u64 {
        let gen_id = self.next_gen;
        self.next_gen += 1;
        if self
            .tx
            .send(RenderJob {
                gen_id,
                base,
                doc,
                fonts,
            })
            .is_err()
        {
            log::error!("overlay: render worker is gone");
        }
        gen_id
    }

    /// Newest finished result, if any.
    pub fn poll(&self) -> Option<RenderResult> {
        let mut last = None;
        while let Ok(r) = self.rx.try_recv() {
            last = Some(r);
        }
        last
    }
}

/// Flatten, converting a panic in the renderer into a logged failure.
pub fn flatten(base: &Rgba, doc: &Doc, fonts: &Fonts) -> Option<Rgba> {
    match std::panic::catch_unwind(AssertUnwindSafe(|| annotate::render(base, doc, fonts))) {
        Ok(img) => Some(img),
        Err(_) => {
            log::error!("overlay: annotate::render panicked");
            None
        }
    }
}

/// An image uploaded as a grid of <= TILE x TILE textures.
pub struct TiledTexture {
    pub width: u32,
    pub height: u32,
    tiles: Vec<(PxRect, TextureHandle)>,
}

fn tile_rects(w: u32, h: u32) -> Vec<PxRect> {
    let mut v = Vec::new();
    let mut y = 0;
    while y < h {
        let th = TILE.min(h - y);
        let mut x = 0;
        while x < w {
            let tw = TILE.min(w - x);
            v.push(PxRect::new(x as i32, y as i32, tw as i32, th as i32));
            x += tw;
        }
        y += th;
    }
    v
}

impl TiledTexture {
    pub fn upload(ctx: &Context, name: &str, img: &Rgba, opaque: bool) -> Self {
        let opts = TextureOptions {
            magnification: egui::TextureFilter::Nearest,
            minification: egui::TextureFilter::Linear,
            ..TextureOptions::NEAREST
        };
        let mut tiles = Vec::new();
        for (i, r) in tile_rects(img.width, img.height).into_iter().enumerate() {
            let tile = if r.w as u32 == img.width && r.h as u32 == img.height {
                None
            } else {
                Some(img.crop(r))
            };
            let src = tile.as_ref().unwrap_or(img);
            let size = [src.width as usize, src.height as usize];
            let ci = if opaque {
                // Opaque frozen pixels: premultiplied == straight, skip the alpha math.
                let pixels = src
                    .data
                    .chunks_exact(4)
                    .map(|p| Color32::from_rgb(p[0], p[1], p[2]))
                    .collect();
                ColorImage {
                    size,
                    source_size: Vec2::new(size[0] as f32, size[1] as f32),
                    pixels,
                }
            } else {
                ColorImage::from_rgba_unmultiplied(size, &src.data)
            };
            tiles.push((r, ctx.load_texture(format!("{name}-{i}"), ci, opts)));
        }
        Self {
            width: img.width,
            height: img.height,
            tiles,
        }
    }

    /// Draw the whole image with its top-left at `origin` (points) and `px_to_pt` points
    /// per image pixel. `tint` modulates; `clip` limits the painted area (points).
    pub fn paint(&self, painter: &egui::Painter, origin: Pos2, px_to_pt: f32, tint: Color32) {
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        for (r, tex) in &self.tiles {
            let min = origin + Vec2::new(r.x as f32, r.y as f32) * px_to_pt;
            let max = min + Vec2::new(r.w as f32, r.h as f32) * px_to_pt;
            painter.image(tex.id(), Rect::from_min_max(min, max), uv, tint);
        }
    }

    /// Draw only image-pixel sub-rect `src` into points rect starting at `dst_min`
    /// scaled by `px_to_pt` (used to paint un-dimmed frozen pixels inside the selection).
    pub fn paint_region(
        &self,
        painter: &egui::Painter,
        src: PxRect,
        dst_min: Pos2,
        px_to_pt: f32,
        tint: Color32,
    ) {
        for (r, tex) in &self.tiles {
            let Some(i) = r.intersect(&src) else { continue };
            let uv = Rect::from_min_max(
                Pos2::new(
                    (i.x - r.x) as f32 / r.w as f32,
                    (i.y - r.y) as f32 / r.h as f32,
                ),
                Pos2::new(
                    (i.right() - r.x) as f32 / r.w as f32,
                    (i.bottom() - r.y) as f32 / r.h as f32,
                ),
            );
            let min = dst_min + Vec2::new((i.x - src.x) as f32, (i.y - src.y) as f32) * px_to_pt;
            let max = min + Vec2::new(i.w as f32, i.h as f32) * px_to_pt;
            painter.image(tex.id(), Rect::from_min_max(min, max), uv, tint);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiles_cover_image_exactly() {
        for (w, h) in [
            (100, 100),
            (2048, 2048),
            (2049, 10),
            (7680, 2160),
            (5000, 4100),
        ] {
            let rects = tile_rects(w, h);
            let area: i64 = rects.iter().map(|r| r.area()).sum();
            assert_eq!(area, w as i64 * h as i64, "{w}x{h}");
            for r in &rects {
                assert!(r.w as u32 <= TILE && r.h as u32 <= TILE);
            }
        }
        assert_eq!(tile_rects(0, 10).len(), 0);
    }
}
