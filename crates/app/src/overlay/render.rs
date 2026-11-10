//! Rendering support: a background worker that flattens committed annotations with
//! `stinkshot_core::annotate`, and tiled GPU textures so that images wider than the GPU's
//! maximum texture size (multi-monitor desktops) still draw 1:1.
//!
//! The worker keeps the base with the destructive shapes (blur, pixelate) baked in and the
//! last full result. While only vector shapes change (the usual edit: draw, move, recolor,
//! type), it re-renders just the rectangle the edit touched (`annotate::render_region`) and
//! sends that patch, already converted to texture pixels, so the UI thread only uploads the
//! changed texels with `TextureHandle::set_partial`. A new base, a changed blur, pixelate or
//! redact box, a spotlight change or a large edit falls back to a full render.

use std::panic::AssertUnwindSafe;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use egui::{Color32, ColorImage, Context, Pos2, Rect, TextureHandle, TextureOptions, Vec2};
use stinkshot_core::annotate::{self, Doc, Shape, ShapeKind};
use stinkshot_core::fonts::Fonts;
use stinkshot_core::{PxRect, Rgba};

/// Tile edge in texels. 2048 is supported by every adapter.
pub const TILE: u32 = 2048;
/// A patch bigger than this share of the image is rendered in full instead.
const FULL_SHARE: f64 = 0.5;

pub struct RenderJob {
    pub base: Arc<Rgba>,
    pub doc: Doc,
    pub fonts: Arc<Fonts>,
    /// The UI has no texture to patch: send a full image.
    pub need_full: bool,
}

/// Texture-ready pixels for the UI thread.
pub enum RenderOut {
    /// Whole image, per tile.
    Full(Vec<(PxRect, ColorImage)>),
    /// Changed texels: (tile index, position inside the tile, pixels).
    Patch(Vec<(usize, [usize; 2], ColorImage)>),
}

pub struct RenderResult {
    pub doc: Doc,
    /// None if the renderer failed (the error is logged); the UI keeps its previous frame.
    pub out: Option<RenderOut>,
}

/// Last finished full image, kept by the worker, so finishing can reuse it.
type Latest = Arc<Mutex<Option<(Doc, Rgba)>>>;

/// Single long-lived worker; jobs submitted while it is busy are coalesced so only the
/// newest one is rendered.
pub struct Renderer {
    tx: Sender<RenderJob>,
    rx: Receiver<RenderResult>,
    latest: Latest,
}

struct WorkerState {
    base: Option<Arc<Rgba>>,
    /// Destructive shapes `work` was baked with, in z-order.
    destructive: Vec<Shape>,
    work: Option<Rgba>,
    /// Doc of the image in `latest`.
    doc: Option<Doc>,
}

fn destructive_of(doc: &Doc) -> Vec<Shape> {
    doc.shapes
        .iter()
        // Redact boxes too: magnifiers sample `work`, and must never show what a box hides.
        .filter(|s| {
            matches!(
                s.kind,
                ShapeKind::Blur { .. } | ShapeKind::Pixelate { .. } | ShapeKind::Redact { .. }
            )
        })
        .cloned()
        .collect()
}

impl Renderer {
    pub fn new(ctx: &Context) -> Self {
        let (tx, job_rx) = channel::<RenderJob>();
        let (res_tx, rx) = channel::<RenderResult>();
        let latest: Latest = Arc::new(Mutex::new(None));
        let shared = latest.clone();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("overlay-render".into())
            .spawn(move || {
                let mut st = WorkerState {
                    base: None,
                    destructive: Vec::new(),
                    work: None,
                    doc: None,
                };
                while let Ok(mut job) = job_rx.recv() {
                    let mut need_full = job.need_full;
                    while let Ok(newer) = job_rx.try_recv() {
                        need_full |= newer.need_full;
                        job = newer;
                    }
                    job.need_full = need_full;
                    let t = Instant::now();
                    let out = std::panic::catch_unwind(AssertUnwindSafe(|| st.run(&job, &shared)))
                        .unwrap_or_else(|_| {
                            log::error!("overlay: annotate render panicked");
                            st.doc = None;
                            None
                        });
                    let ms = t.elapsed().as_secs_f64() * 1000.0;
                    log::debug!(
                        "overlay render: {} in {ms:.1} ms",
                        match &out {
                            Some(RenderOut::Full(_)) => "full",
                            Some(RenderOut::Patch(_)) => "patch",
                            None => "failed",
                        }
                    );
                    if res_tx.send(RenderResult { doc: job.doc, out }).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            });
        if let Err(e) = spawned {
            log::error!("overlay: could not start render worker: {e}");
        }
        Self { tx, rx, latest }
    }

    /// Queue a render; only the newest queued job is rendered.
    pub fn submit(&mut self, base: Arc<Rgba>, doc: Doc, fonts: Arc<Fonts>, need_full: bool) {
        if self
            .tx
            .send(RenderJob {
                base,
                doc,
                fonts,
                need_full,
            })
            .is_err()
        {
            log::error!("overlay: render worker is gone");
        }
    }

    /// Finished results in order (patches build on each other: apply every one).
    pub fn poll(&self) -> Vec<RenderResult> {
        let mut out = Vec::new();
        while let Ok(r) = self.rx.try_recv() {
            out.push(r);
        }
        out
    }

    /// The worker's flattened image when it shows exactly `doc`.
    pub fn image_for(&self, doc: &Doc) -> Option<Rgba> {
        let g = self.latest.lock().ok()?;
        match &*g {
            Some((d, img)) if d == doc => Some(img.clone()),
            _ => None,
        }
    }
}

impl WorkerState {
    fn run(&mut self, job: &RenderJob, latest: &Latest) -> Option<RenderOut> {
        let base = &job.base;
        let destructive = destructive_of(&job.doc);
        let same_base = self.base.as_ref().is_some_and(|b| Arc::ptr_eq(b, base));
        if !same_base || self.work.is_none() || destructive != self.destructive {
            let only: Doc = Doc {
                shapes: destructive.clone(),
                ..job.doc.clone()
            };
            self.work = Some(annotate::bake_destructive(base, &only));
            self.destructive = destructive;
            self.base = Some(base.clone());
            self.doc = None;
        }
        let work = self.work.as_ref()?;
        let full_rect = PxRect::new(0, 0, base.width as i32, base.height as i32);
        let dirty = match (&self.doc, job.need_full) {
            (Some(old), false) => dirty_rect(old, &job.doc, &job.fonts),
            _ => None,
        };
        let dirty = dirty.map(|r| grow_over_blurs(r, &job.doc, &job.fonts));
        let dirty = dirty.and_then(|r| r.intersect(&full_rect).or(Some(PxRect::default())));
        match dirty {
            Some(r) if (r.area() as f64) < full_rect.area() as f64 * FULL_SHARE => {
                if r.is_empty() {
                    self.doc = Some(job.doc.clone());
                    if let Ok(mut g) = latest.lock()
                        && let Some((d, _)) = g.as_mut()
                    {
                        *d = job.doc.clone();
                    }
                    return Some(RenderOut::Patch(Vec::new()));
                }
                let patch = annotate::render_region(work, &job.doc, &job.fonts, r);
                let mut g = latest.lock().ok()?;
                let (d, img) = g.as_mut()?;
                blit(img, &patch, r.x, r.y);
                *d = job.doc.clone();
                drop(g);
                self.doc = Some(job.doc.clone());
                Some(RenderOut::Patch(patch_images(
                    &patch,
                    r,
                    base.width,
                    base.height,
                )))
            }
            _ => {
                let img = annotate::render_region(work, &job.doc, &job.fonts, full_rect);
                let tiles = tile_images(&img);
                if let Ok(mut g) = latest.lock() {
                    *g = Some((job.doc.clone(), img));
                }
                self.doc = Some(job.doc.clone());
                Some(RenderOut::Full(tiles))
            }
        }
    }
}

/// A Blur or Pixelate also covers shapes drawn under it, so an edit inside one changes its
/// whole rectangle (and any other one that overlaps).
fn grow_over_blurs(mut r: PxRect, doc: &Doc, fonts: &Fonts) -> PxRect {
    if r.is_empty() {
        return r;
    }
    loop {
        let before = r;
        for s in &doc.shapes {
            if matches!(s.kind, ShapeKind::Blur { .. } | ShapeKind::Pixelate { .. }) {
                let b = annotate::shape_bounds(s, fonts).to_px();
                if b.intersect(&r).is_some() {
                    r = r.union(&b);
                }
            }
        }
        if r == before {
            return r;
        }
    }
}

/// Room around a shape's bounds for its drop shadow, anti-aliasing and glyph overhang.
fn margin(s: &Shape) -> f32 {
    let size = match &s.kind {
        ShapeKind::Text { size, .. } | ShapeKind::Callout { size, .. } => *size,
        _ => 0.0,
    };
    8.0 + s.style.width + size * 0.3
}

/// The image area that differs between renders of `old` and `new` (destructive shapes are
/// the same in both). None when the whole image must be redrawn (a spotlight changed: it
/// dims everything). An empty rect means nothing visible changed.
pub fn dirty_rect(old: &Doc, new: &Doc, fonts: &Fonts) -> Option<PxRect> {
    let spot = |d: &Doc| -> Vec<Shape> {
        d.shapes
            .iter()
            .filter(|s| matches!(s.kind, ShapeKind::Spotlight { .. }))
            .cloned()
            .collect()
    };
    if spot(old) != spot(new) {
        return None;
    }
    let mut r = PxRect::default();
    let mut add = |s: &Shape| {
        let b = annotate::shape_bounds(s, fonts).expand(margin(s)).to_px();
        r = r.union(&b);
    };
    for s in &old.shapes {
        if new.get(s.id) != Some(s) {
            add(s);
        }
    }
    for s in &new.shapes {
        if old.get(s.id) != Some(s) {
            add(s);
        }
    }
    // Shapes present in both whose stacking order changed.
    let common_old: Vec<u64> = old
        .shapes
        .iter()
        .map(|s| s.id)
        .filter(|id| new.get(*id).is_some())
        .collect();
    let common_new: Vec<u64> = new
        .shapes
        .iter()
        .map(|s| s.id)
        .filter(|id| old.get(*id).is_some())
        .collect();
    if common_old != common_new {
        for (a, b) in common_old.iter().zip(&common_new) {
            if a != b {
                if let Some(s) = new.get(*a) {
                    add(s);
                }
                if let Some(s) = new.get(*b) {
                    add(s);
                }
            }
        }
    }
    Some(r)
}

/// Copy `src` into `dst` at (x, y).
fn blit(dst: &mut Rgba, src: &Rgba, x: i32, y: i32) {
    let row = src.width as usize * 4;
    for yy in 0..src.height as usize {
        let dy = y as usize + yy;
        if dy >= dst.height as usize {
            break;
        }
        let d0 = (dy * dst.width as usize + x as usize) * 4;
        let s0 = yy * row;
        dst.data[d0..d0 + row].copy_from_slice(&src.data[s0..s0 + row]);
    }
}

fn color_image(src: &Rgba, r: PxRect) -> ColorImage {
    let size = [r.w as usize, r.h as usize];
    let mut px = Vec::with_capacity(size[0] * size[1]);
    let stride = src.width as usize * 4;
    for y in r.y as usize..(r.y + r.h) as usize {
        let row = &src.data[y * stride + r.x as usize * 4..y * stride + (r.x + r.w) as usize * 4];
        px.extend(
            row.chunks_exact(4)
                .map(|p| Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3])),
        );
    }
    ColorImage {
        size,
        source_size: Vec2::new(size[0] as f32, size[1] as f32),
        pixels: px,
    }
}

fn tile_images(img: &Rgba) -> Vec<(PxRect, ColorImage)> {
    tile_rects(img.width, img.height)
        .into_iter()
        .map(|r| (r, color_image(img, r)))
        .collect()
}

/// Split a patch at `at` (image px) into per-tile pieces.
fn patch_images(patch: &Rgba, at: PxRect, w: u32, h: u32) -> Vec<(usize, [usize; 2], ColorImage)> {
    let mut out = Vec::new();
    for (i, t) in tile_rects(w, h).into_iter().enumerate() {
        if let Some(part) = at.intersect(&t) {
            let local = PxRect::new(part.x - at.x, part.y - at.y, part.w, part.h);
            out.push((
                i,
                [(part.x - t.x) as usize, (part.y - t.y) as usize],
                color_image(patch, local),
            ));
        }
    }
    out
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

fn tex_options() -> TextureOptions {
    TextureOptions {
        magnification: egui::TextureFilter::Nearest,
        minification: egui::TextureFilter::Linear,
        ..TextureOptions::NEAREST
    }
}

impl TiledTexture {
    pub fn upload(ctx: &Context, name: &str, img: &Rgba, opaque: bool) -> Self {
        let mut tiles = Vec::new();
        for (i, r) in tile_rects(img.width, img.height).into_iter().enumerate() {
            let ci = if opaque {
                // Opaque frozen pixels: premultiplied == straight, skip the alpha math.
                let stride = img.width as usize * 4;
                let mut pixels = Vec::with_capacity((r.w * r.h) as usize);
                for y in r.y as usize..(r.y + r.h) as usize {
                    let row = &img.data
                        [y * stride + r.x as usize * 4..y * stride + (r.x + r.w) as usize * 4];
                    pixels.extend(
                        row.chunks_exact(4)
                            .map(|p| Color32::from_rgb(p[0], p[1], p[2])),
                    );
                }
                ColorImage {
                    size: [r.w as usize, r.h as usize],
                    source_size: Vec2::new(r.w as f32, r.h as f32),
                    pixels,
                }
            } else {
                color_image(img, r)
            };
            tiles.push((
                r,
                ctx.load_texture(format!("{name}-{i}"), ci, tex_options()),
            ));
        }
        Self { tiles }
    }

    /// Upload tiles the worker already converted.
    pub fn from_tiles(ctx: &Context, name: &str, tiles: Vec<(PxRect, ColorImage)>) -> Self {
        Self {
            tiles: tiles
                .into_iter()
                .enumerate()
                .map(|(i, (r, ci))| {
                    (
                        r,
                        ctx.load_texture(format!("{name}-{i}"), ci, tex_options()),
                    )
                })
                .collect(),
        }
    }

    /// Overwrite part of the texture (pieces from `RenderOut::Patch`).
    pub fn patch(&mut self, pieces: Vec<(usize, [usize; 2], ColorImage)>) -> bool {
        for (i, pos, ci) in pieces {
            let Some((r, tex)) = self.tiles.get_mut(i) else {
                return false;
            };
            if pos[0] + ci.size[0] > r.w as usize || pos[1] + ci.size[1] > r.h as usize {
                return false;
            }
            tex.set_partial(pos, ci, tex_options());
        }
        true
    }

    /// Draw the whole image with its top-left at `origin` (points) and `px_to_pt` points
    /// per image pixel. `tint` modulates.
    pub fn paint(&self, painter: &egui::Painter, origin: Pos2, px_to_pt: f32, tint: Color32) {
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        for (r, tex) in &self.tiles {
            let min = origin + Vec2::new(r.x as f32, r.y as f32) * px_to_pt;
            let max = min + Vec2::new(r.w as f32, r.h as f32) * px_to_pt;
            painter.image(tex.id(), Rect::from_min_max(min, max), uv, tint);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stinkshot_core::annotate::{ArrowHead, Style};
    use stinkshot_core::{PtF, RectF};

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

    #[test]
    fn patches_split_across_tiles() {
        let patch = Rgba::filled(100, 50, [1, 2, 3, 255]);
        let at = PxRect::new(2000, 10, 100, 50);
        let pieces = patch_images(&patch, at, 4000, 100);
        assert_eq!(pieces.len(), 2);
        assert_eq!(pieces[0].0, 0);
        assert_eq!(pieces[0].1, [2000, 10]);
        assert_eq!(pieces[0].2.size, [48, 50]);
        assert_eq!(pieces[1].1, [0, 10]);
        assert_eq!(pieces[1].2.size, [52, 50]);
    }

    fn arrow(from: (f32, f32), to: (f32, f32)) -> ShapeKind {
        ShapeKind::Arrow {
            from: PtF::new(from.0, from.1),
            to: PtF::new(to.0, to.1),
            ctrl: None,
            double: false,
            head: ArrowHead::Notched,
        }
    }

    #[test]
    fn dirty_rect_covers_old_and_new_spots_only() {
        let fonts = Fonts::load_system().unwrap();
        let mut a = Doc::new(4000, 3000);
        a.add(arrow((10.0, 10.0), (100.0, 100.0)), Style::default());
        let far = a.add(arrow((3000.0, 2000.0), (3500.0, 2500.0)), Style::default());
        let mut b = a.clone();
        assert_eq!(dirty_rect(&a, &b, &fonts), Some(PxRect::default()));
        stinkshot_core::annotate::translate_shape(&mut b.get_mut(far).unwrap().kind, 50.0, 0.0);
        let r = dirty_rect(&a, &b, &fonts).unwrap();
        assert!(r.x < 3000 && r.right() > 3550 && r.y < 2000 && r.bottom() > 2500);
        assert!(r.x > 200, "the untouched arrow is not dirty: {r:?}");
        // A spotlight change dims everything: full render.
        let mut c = b.clone();
        c.add(
            ShapeKind::Spotlight {
                rect: RectF::new(0.0, 0.0, 10.0, 10.0),
                dim: 0.5,
            },
            Style::default(),
        );
        assert_eq!(dirty_rect(&b, &c, &fonts), None);
        // Reordering marks both shapes.
        let mut d = b.clone();
        d.shapes.swap(0, 1);
        let r = dirty_rect(&b, &d, &fonts).unwrap();
        assert!(r.x < 20 && r.right() > 3500);
    }

    /// Before / after numbers for the editor's committed-shape render on a 4K capture
    /// (run: `cargo test -p stinkshot perf_4k -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn perf_4k() {
        let fonts = Fonts::load_system().unwrap();
        let (w, h) = (3840u32, 2160u32);
        let mut base = Rgba::new(w, h);
        for (i, p) in base.data.chunks_exact_mut(4).enumerate() {
            let (x, y) = ((i % w as usize) as u8, (i / w as usize) as u8);
            p.copy_from_slice(&[x, y, x ^ y, 255]);
        }
        let mut doc = Doc::new(w, h);
        for i in 0..24 {
            let o = i as f32 * 140.0;
            doc.add(
                arrow((100.0 + o, 200.0), (300.0 + o, 900.0 + o * 0.2)),
                Style::default(),
            );
            doc.add(
                ShapeKind::Rect {
                    rect: RectF::new(80.0 + o, 1200.0, 120.0, 300.0),
                    corner_radius: 3.0,
                },
                Style::default(),
            );
        }
        doc.add(
            ShapeKind::Blur {
                rect: RectF::new(2000.0, 1600.0, 900.0, 300.0),
                radius: 12.0,
            },
            Style::default(),
        );
        doc.add(
            ShapeKind::Text {
                pos: PtF::new(400.0, 1800.0),
                text: "Committed text annotation".into(),
                size: 40.0,
                background: None,
                bold: false,
                width: None,
            },
            Style::default(),
        );
        let base = Arc::new(base);
        let time = |f: &mut dyn FnMut()| {
            let n = 5;
            let t = Instant::now();
            for _ in 0..n {
                f();
            }
            t.elapsed().as_secs_f64() * 1000.0 / n as f64
        };
        // Before: every committed change flattened the whole image, then the UI thread
        // converted all of it to texture pixels.
        let full = time(&mut || {
            let img = annotate::render(&base, &doc, &fonts);
            std::hint::black_box(&img);
        });
        let img = annotate::render(&base, &doc, &fonts);
        let convert = time(&mut || {
            let ci = ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &img.data);
            std::hint::black_box(&ci);
        });
        // After: one arrow moved; the worker re-renders and converts only its area.
        let st_doc = doc.clone();
        let mut moved = doc.clone();
        let id = moved.shapes[4].id;
        annotate::translate_shape(&mut moved.get_mut(id).unwrap().kind, 40.0, 25.0);
        let work = annotate::bake_destructive(&base, &doc);
        let r = dirty_rect(&st_doc, &moved, &fonts).unwrap();
        let patch_ms = time(&mut || {
            let p = annotate::render_region(&work, &moved, &fonts, r);
            let pieces = patch_images(&p, r, w, h);
            std::hint::black_box(&pieces);
        });
        let p = annotate::render_region(&work, &moved, &fonts, r);
        let pieces = patch_images(&p, r, w, h);
        let texels: usize = pieces.iter().map(|(_, _, c)| c.pixels.len()).sum();
        println!(
            "4K, {} shapes: full render {full:.1} ms + UI-thread conversion {convert:.1} ms; \
             patch for one moved arrow ({}x{} px, {texels} texels) {patch_ms:.1} ms on the worker",
            doc.shapes.len(),
            r.w,
            r.h
        );
    }
}
