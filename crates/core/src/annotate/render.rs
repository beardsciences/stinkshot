//! Flattening of annotation shapes onto pixels (tiny-skia for vectors, ab_glyph for text).

use ab_glyph::OutlinedGlyph;
use tiny_skia::{
    BlendMode, Color, FillRule, FilterQuality, LineCap, LineJoin, Mask, Paint, Path, PathBuilder,
    Pattern, Pixmap, Rect, SpreadMode, Stroke, Transform,
};

use super::{
    ArrowGeom, CALLOUT_PAD, HIGHLIGHT_ALPHA, Shape, ShapeKind, Style, arrow_geom, norm, text_rect,
};
use crate::fonts::{Fonts, LINE_HEIGHT};
use crate::geom::{PtF, PxRect, RectF};
use crate::image::Rgba;
use crate::redact;

const KAPPA: f32 = 0.552_284_7;
const SHADOW_RGBA: [u8; 4] = [0, 0, 0, 96];

/// Ring width of a magnifier inset (also used for its bounds).
pub(crate) fn magnifier_ring_width(style: &Style) -> f32 {
    (style.width * 0.75).max(2.5)
}

fn is_destructive(k: &ShapeKind) -> bool {
    matches!(
        k,
        ShapeKind::Blur { .. } | ShapeKind::Pixelate { .. } | ShapeKind::Redact { .. }
    )
}

pub(crate) fn render_shapes(
    base: &Rgba,
    shapes: &[&Shape],
    fonts: &Fonts,
    transparent: bool,
) -> Rgba {
    if base.width == 0 || base.height == 0 {
        return base.clone();
    }
    let (w, h) = (base.width, base.height);

    // Step 2: destructive shapes on a copy of the base pixels.
    let mut work = base.clone();
    for s in shapes {
        match &s.kind {
            ShapeKind::Blur { rect, radius } => {
                redact::blur(&mut work, norm(*rect).to_px(), *radius)
            }
            ShapeKind::Pixelate { rect, block } => {
                redact::pixelate(&mut work, norm(*rect).to_px(), *block)
            }
            ShapeKind::Redact { rect } => {
                let c = [s.style.color[0], s.style.color[1], s.style.color[2], 255];
                redact::fill(&mut work, norm(*rect).to_px(), c);
            }
            _ => {}
        }
    }

    // The magnifier samples this (post-redaction, pre-annotation) image.
    let has_magnifier = shapes
        .iter()
        .any(|s| matches!(s.kind, ShapeKind::Magnifier { .. }));
    let work_pm = if !transparent || has_magnifier {
        rgba_to_pixmap(&work)
    } else {
        None
    };

    let canvas = if transparent {
        let Some(mut pm) = Pixmap::new(w, h) else {
            return Rgba::new(w, h);
        };
        // Destructive shapes show their effect as opaque patches of the processed pixels.
        for s in shapes.iter().filter(|s| is_destructive(&s.kind)) {
            let rect = match &s.kind {
                ShapeKind::Blur { rect, .. }
                | ShapeKind::Pixelate { rect, .. }
                | ShapeKind::Redact { rect } => *rect,
                _ => continue,
            };
            if let Some(r) = norm(rect).to_px().intersect(&work.bounds()) {
                let patch = work.crop(r);
                if let Some(pp) = rgba_to_pixmap(&patch) {
                    pm.draw_pixmap(
                        r.x,
                        r.y,
                        pp.as_ref(),
                        &tiny_skia::PixmapPaint::default(),
                        Transform::identity(),
                        None,
                    );
                }
            }
        }
        pm
    } else {
        match &work_pm {
            Some(p) => p.clone(),
            None => return work,
        }
    };
    let mut pm = canvas;

    // Step 3: spotlight dimming.
    draw_spotlights(&mut pm, shapes);

    // Step 4: remaining vector shapes in z-order.
    for s in shapes {
        if let ShapeKind::Redact { rect } = &s.kind {
            // Redact boxes also cover vector shapes drawn before them (z-order), so text
            // or callouts underneath cannot show through the box.
            fill_pixmap(
                &mut pm,
                norm(*rect).to_px(),
                [s.style.color[0], s.style.color[1], s.style.color[2], 255],
            );
            continue;
        }
        if is_destructive(&s.kind) || matches!(s.kind, ShapeKind::Spotlight { .. }) {
            continue;
        }
        draw_shape(&mut pm, s, fonts, work_pm.as_ref());
    }

    Rgba {
        width: w,
        height: h,
        data: pm.take_demultiplied(),
    }
}

/// Opaque rectangle fill straight into pixmap memory (opaque, so premultiplied == straight).
fn fill_pixmap(pm: &mut Pixmap, rect: PxRect, color: [u8; 4]) {
    let bounds = PxRect {
        x: 0,
        y: 0,
        w: pm.width() as i32,
        h: pm.height() as i32,
    };
    let Some(r) = rect.intersect(&bounds) else {
        return;
    };
    let stride = pm.width() as usize * 4;
    let data = pm.data_mut();
    for y in r.y..r.bottom() {
        let row = y as usize * stride;
        for x in r.x..r.right() {
            let i = row + x as usize * 4;
            data[i..i + 4].copy_from_slice(&color);
        }
    }
}

fn rgba_to_pixmap(img: &Rgba) -> Option<Pixmap> {
    let mut pm = Pixmap::new(img.width, img.height)?;
    let dst = pm.data_mut();
    for (d, s) in dst.chunks_exact_mut(4).zip(img.data.chunks_exact(4)) {
        let a = s[3] as u32;
        if a == 255 {
            d.copy_from_slice(s);
        } else {
            d[0] = ((s[0] as u32 * a + 127) / 255) as u8;
            d[1] = ((s[1] as u32 * a + 127) / 255) as u8;
            d[2] = ((s[2] as u32 * a + 127) / 255) as u8;
            d[3] = a as u8;
        }
    }
    Some(pm)
}

// ---------------------------------------------------------------------------------------
// Paint helpers
// ---------------------------------------------------------------------------------------

fn paint(c: [u8; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba8(c[0], c[1], c[2], c[3]));
    p.anti_alias = true;
    p
}

fn with_alpha(c: [u8; 4], f: f32) -> [u8; 4] {
    [
        c[0],
        c[1],
        c[2],
        (c[3] as f32 * f.clamp(0.0, 1.0)).round() as u8,
    ]
}

fn stroke_of(width: f32, cap: LineCap, join: LineJoin) -> Stroke {
    Stroke {
        width,
        line_cap: cap,
        line_join: join,
        ..Stroke::default()
    }
}

fn contrast_color(bg: [u8; 4]) -> [u8; 4] {
    let lum = 0.299 * bg[0] as f32 + 0.587 * bg[1] as f32 + 0.114 * bg[2] as f32;
    if lum > 150.0 {
        [20, 20, 22, 255]
    } else {
        [255, 255, 255, 255]
    }
}

fn shadow_offset(width: f32) -> f32 {
    (width * 0.35).clamp(1.0, 3.0)
}

fn rect_of(r: RectF) -> Option<Rect> {
    Rect::from_xywh(r.x, r.y, r.w, r.h)
}

fn push_rounded_rect(pb: &mut PathBuilder, r: RectF, radius: f32) {
    let radius = radius.clamp(0.0, (r.w / 2.0).min(r.h / 2.0));
    let (x, y, w, h) = (r.x, r.y, r.w, r.h);
    if radius <= 0.25 {
        pb.move_to(x, y);
        pb.line_to(x + w, y);
        pb.line_to(x + w, y + h);
        pb.line_to(x, y + h);
        pb.close();
        return;
    }
    let k = radius * KAPPA;
    pb.move_to(x + radius, y);
    pb.line_to(x + w - radius, y);
    pb.cubic_to(
        x + w - radius + k,
        y,
        x + w,
        y + radius - k,
        x + w,
        y + radius,
    );
    pb.line_to(x + w, y + h - radius);
    pb.cubic_to(
        x + w,
        y + h - radius + k,
        x + w - radius + k,
        y + h,
        x + w - radius,
        y + h,
    );
    pb.line_to(x + radius, y + h);
    pb.cubic_to(
        x + radius - k,
        y + h,
        x,
        y + h - radius + k,
        x,
        y + h - radius,
    );
    pb.line_to(x, y + radius);
    pb.cubic_to(x, y + radius - k, x + radius - k, y, x + radius, y);
    pb.close();
}

fn rounded_rect_path(r: RectF, radius: f32) -> Option<Path> {
    if r.w < 0.5 || r.h < 0.5 {
        return None;
    }
    let mut pb = PathBuilder::new();
    push_rounded_rect(&mut pb, r, radius);
    pb.finish()
}

fn fill_path(pm: &mut Pixmap, path: &Path, color: [u8; 4], tf: Transform) {
    pm.fill_path(path, &paint(color), FillRule::Winding, tf, None);
}

fn stroke_path(pm: &mut Pixmap, path: &Path, color: [u8; 4], stroke: &Stroke, tf: Transform) {
    if stroke.width > 0.0 {
        pm.stroke_path(path, &paint(color), stroke, tf, None);
    }
}

/// Midpoint-quadratic smoothing through the sampled points.
fn smooth_path(points: &[PtF]) -> Option<Path> {
    let pts: Vec<PtF> = points
        .iter()
        .copied()
        .filter(|p| p.x.is_finite() && p.y.is_finite())
        .collect();
    if pts.len() < 2 {
        return None;
    }
    let mut pb = PathBuilder::new();
    pb.move_to(pts[0].x, pts[0].y);
    if pts.len() == 2 {
        pb.line_to(pts[1].x, pts[1].y);
    } else {
        for i in 1..pts.len() - 1 {
            let mid = PtF::new(
                (pts[i].x + pts[i + 1].x) / 2.0,
                (pts[i].y + pts[i + 1].y) / 2.0,
            );
            pb.quad_to(pts[i].x, pts[i].y, mid.x, mid.y);
        }
        let last = pts[pts.len() - 1];
        pb.line_to(last.x, last.y);
    }
    pb.finish()
}

fn dot_path(points: &[PtF], radius: f32) -> Option<Path> {
    let p = points.iter().find(|p| p.x.is_finite() && p.y.is_finite())?;
    PathBuilder::from_circle(p.x, p.y, radius.max(0.5))
}

// ---------------------------------------------------------------------------------------
// Spotlight
// ---------------------------------------------------------------------------------------

fn draw_spotlights(pm: &mut Pixmap, shapes: &[&Shape]) {
    let spots: Vec<(RectF, f32)> = shapes
        .iter()
        .filter_map(|s| match &s.kind {
            ShapeKind::Spotlight { rect, dim } => {
                let r = norm(*rect);
                (r.w >= 1.0 && r.h >= 1.0).then_some((r, *dim))
            }
            _ => None,
        })
        .collect();
    if spots.is_empty() {
        return;
    }
    let Some(mut mask) = Mask::new(pm.width(), pm.height()) else {
        return;
    };
    for (r, _) in &spots {
        if let Some(p) = rounded_rect_path(*r, 4.0) {
            mask.fill_path(&p, FillRule::Winding, true, Transform::identity());
        }
    }
    mask.invert();
    let dim = spots
        .iter()
        .map(|(_, d)| *d)
        .fold(0.0f32, f32::max)
        .clamp(0.0, 1.0);
    let alpha = (dim * 255.0).round() as u8;
    if alpha == 0 {
        return;
    }
    let Some(full) = Rect::from_xywh(0.0, 0.0, pm.width() as f32, pm.height() as f32) else {
        return;
    };
    let mut p = paint([0, 0, 0, alpha]);
    p.anti_alias = false;
    pm.fill_rect(full, &p, Transform::identity(), Some(&mask));
}

// ---------------------------------------------------------------------------------------
// Shapes
// ---------------------------------------------------------------------------------------

fn draw_shape(pm: &mut Pixmap, shape: &Shape, fonts: &Fonts, magnifier_src: Option<&Pixmap>) {
    let st = &shape.style;
    match &shape.kind {
        ShapeKind::Arrow { from, to } => {
            if let Some(g) = arrow_geom(*from, *to, st.width) {
                if st.shadow {
                    let off = Transform::from_translate(0.0, shadow_offset(st.width));
                    draw_arrow(pm, &g, *from, SHADOW_RGBA, st.width, off);
                }
                draw_arrow(pm, &g, *from, st.color, st.width, Transform::identity());
            }
        }
        ShapeKind::Line { from, to } => {
            let mut pb = PathBuilder::new();
            pb.move_to(from.x, from.y);
            pb.line_to(to.x, to.y);
            let Some(path) = pb.finish() else { return };
            let stroke = stroke_of(st.width.max(0.5), LineCap::Round, LineJoin::Round);
            if st.shadow {
                let off = Transform::from_translate(0.0, shadow_offset(st.width));
                stroke_path(pm, &path, SHADOW_RGBA, &stroke, off);
            }
            stroke_path(pm, &path, st.color, &stroke, Transform::identity());
        }
        ShapeKind::Rect {
            rect,
            corner_radius,
        } => {
            let r = norm(*rect);
            let Some(path) = rounded_rect_path(r, *corner_radius) else {
                return;
            };
            let join = if *corner_radius > 0.5 {
                LineJoin::Round
            } else {
                LineJoin::Miter
            };
            draw_outlined(pm, &path, st, join);
        }
        ShapeKind::Ellipse { rect } => {
            let r = norm(*rect);
            let Some(rr) = rect_of(r) else { return };
            let Some(path) = PathBuilder::from_oval(rr) else {
                return;
            };
            draw_outlined(pm, &path, st, LineJoin::Round);
        }
        ShapeKind::Pen { points } => {
            let stroke = stroke_of(st.width.max(0.5), LineCap::Round, LineJoin::Round);
            match smooth_path(points) {
                Some(path) => {
                    if st.shadow {
                        let off = Transform::from_translate(0.0, shadow_offset(st.width));
                        stroke_path(pm, &path, SHADOW_RGBA, &stroke, off);
                    }
                    stroke_path(pm, &path, st.color, &stroke, Transform::identity());
                }
                None => {
                    if let Some(dot) = dot_path(points, st.width / 2.0) {
                        fill_path(pm, &dot, st.color, Transform::identity());
                    }
                }
            }
        }
        ShapeKind::Highlighter { points } => {
            let color = with_alpha(st.color, HIGHLIGHT_ALPHA);
            match smooth_path(points) {
                Some(path) => {
                    let stroke = stroke_of(st.width.max(1.0), LineCap::Round, LineJoin::Round);
                    stroke_path(pm, &path, color, &stroke, Transform::identity());
                }
                None => {
                    if let Some(dot) = dot_path(points, st.width / 2.0) {
                        fill_path(pm, &dot, color, Transform::identity());
                    }
                }
            }
        }
        ShapeKind::Text {
            pos,
            text,
            size,
            background,
        } => {
            let size = size.max(1.0);
            match background {
                Some(bg) => {
                    let pill = text_rect(fonts, *pos, text, size, true);
                    if let Some(path) = rounded_rect_path(pill, (pill.h / 2.0).min(size * 0.7)) {
                        if st.shadow {
                            let off = Transform::from_translate(0.0, shadow_offset(size / 4.0));
                            fill_path(pm, &path, SHADOW_RGBA, off);
                        }
                        fill_path(pm, &path, *bg, Transform::identity());
                    }
                    draw_text_block(pm, fonts, text, *pos, size, false, st.color, false);
                }
                None => draw_text_block(pm, fonts, text, *pos, size, false, st.color, st.shadow),
            }
        }
        ShapeKind::Step { center, radius, n } => draw_step(pm, fonts, st, *center, *radius, *n),
        ShapeKind::Magnifier {
            source,
            dest,
            circular,
        } => {
            if let Some(src) = magnifier_src {
                draw_magnifier(pm, st, norm(*source), norm(*dest), *circular, src);
            }
        }
        ShapeKind::Callout {
            rect,
            tail,
            text,
            size,
        } => {
            draw_callout(pm, fonts, st, norm(*rect), *tail, text, *size);
        }
        // Handled in earlier passes.
        ShapeKind::Blur { .. }
        | ShapeKind::Pixelate { .. }
        | ShapeKind::Redact { .. }
        | ShapeKind::Spotlight { .. } => {}
    }
}

fn draw_arrow(
    pm: &mut Pixmap,
    g: &ArrowGeom,
    from: PtF,
    color: [u8; 4],
    width: f32,
    tf: Transform,
) {
    // Shaft stops at the notch so the head's concave back stays clean.
    let shaft_len = ((g.notch.x - from.x).powi(2) + (g.notch.y - from.y).powi(2)).sqrt();
    if shaft_len > 0.5 {
        let mut pb = PathBuilder::new();
        pb.move_to(from.x, from.y);
        pb.line_to(g.notch.x, g.notch.y);
        if let Some(path) = pb.finish() {
            let stroke = stroke_of(width.max(1.0), LineCap::Round, LineJoin::Round);
            stroke_path(pm, &path, color, &stroke, tf);
        }
    }
    let mut pb = PathBuilder::new();
    pb.move_to(g.tip.x, g.tip.y);
    pb.line_to(g.left.x, g.left.y);
    pb.line_to(g.notch.x, g.notch.y);
    pb.line_to(g.right.x, g.right.y);
    pb.close();
    if let Some(path) = pb.finish() {
        fill_path(pm, &path, color, tf);
        // A hairline of stroke rounds the head's points slightly and avoids a thin look.
        let stroke = stroke_of((width * 0.25).max(0.8), LineCap::Round, LineJoin::Round);
        stroke_path(pm, &path, color, &stroke, tf);
    }
}

/// Fill (if any) then stroke, with an optional shadow underneath both.
fn draw_outlined(pm: &mut Pixmap, path: &Path, st: &Style, join: LineJoin) {
    let stroke = stroke_of(st.width, LineCap::Butt, join);
    if st.shadow {
        let off = Transform::from_translate(0.0, shadow_offset(st.width));
        if st.fill.is_some() {
            fill_path(pm, path, SHADOW_RGBA, off);
        } else {
            stroke_path(pm, path, SHADOW_RGBA, &stroke, off);
        }
    }
    if let Some(f) = st.fill {
        fill_path(pm, path, f, Transform::identity());
    }
    stroke_path(pm, path, st.color, &stroke, Transform::identity());
}

// ---------------------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------------------

fn draw_glyphs(pm: &mut Pixmap, glyphs: &[OutlinedGlyph], ox: i32, oy: i32, color: [u8; 4]) {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let data = pm.data_mut();
    let col_a = color[3] as f32 / 255.0;
    if col_a <= 0.0 {
        return;
    }
    for g in glyphs {
        let b = g.px_bounds();
        let (bx, by) = (b.min.x as i32 + ox, b.min.y as i32 + oy);
        g.draw(|x, y, cov| {
            let (px, py) = (bx + x as i32, by + y as i32);
            if px < 0 || py < 0 || px >= w || py >= h {
                return;
            }
            let a = cov.clamp(0.0, 1.0) * col_a;
            if a <= 0.0 {
                return;
            }
            let i = ((py * w + px) * 4) as usize;
            let inv = 1.0 - a;
            for c in 0..3 {
                let v = color[c] as f32 * a + data[i + c] as f32 * inv;
                data[i + c] = v.round().min(255.0) as u8;
            }
            let va = 255.0 * a + data[i + 3] as f32 * inv;
            data[i + 3] = va.round().min(255.0) as u8;
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_text_block(
    pm: &mut Pixmap,
    fonts: &Fonts,
    text: &str,
    pos: PtF,
    size: f32,
    bold: bool,
    color: [u8; 4],
    shadow: bool,
) {
    let lh = size * LINE_HEIGHT;
    let ascent = fonts.ascent(size, bold);
    let lines: Vec<Vec<OutlinedGlyph>> = text
        .split('\n')
        .enumerate()
        .map(|(i, line)| {
            let baseline = pos.y + i as f32 * lh + (lh - size) / 2.0 + ascent;
            fonts.outline_line(line, size, bold, pos.x, baseline)
        })
        .collect();
    if shadow {
        let off = (size * 0.06).round().clamp(1.0, 3.0) as i32;
        for g in &lines {
            draw_glyphs(pm, g, 0, off, [0, 0, 0, 120]);
        }
    }
    for g in &lines {
        draw_glyphs(pm, g, 0, 0, color);
    }
}

/// Center the *ink* of a short string (digits) on `center`.
fn draw_centered_ink(
    pm: &mut Pixmap,
    fonts: &Fonts,
    text: &str,
    size: f32,
    bold: bool,
    center: PtF,
    color: [u8; 4],
) {
    let probe = fonts.outline_line(text, size, bold, 0.0, 0.0);
    if probe.is_empty() {
        return;
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for g in &probe {
        let b = g.px_bounds();
        x0 = x0.min(b.min.x);
        y0 = y0.min(b.min.y);
        x1 = x1.max(b.max.x);
        y1 = y1.max(b.max.y);
    }
    let ox = center.x - (x0 + x1) / 2.0;
    let oy = center.y - (y0 + y1) / 2.0;
    let glyphs = fonts.outline_line(text, size, bold, ox, oy);
    draw_glyphs(pm, &glyphs, 0, 0, color);
}

fn draw_step(pm: &mut Pixmap, fonts: &Fonts, st: &Style, center: PtF, radius: f32, n: u32) {
    let radius = radius.max(2.0);
    let Some(circle) = PathBuilder::from_circle(center.x, center.y, radius) else {
        return;
    };
    if st.shadow {
        let off = Transform::from_translate(0.0, shadow_offset(radius / 3.0));
        fill_path(pm, &circle, SHADOW_RGBA, off);
    }
    let body = [st.color[0], st.color[1], st.color[2], 255];
    fill_path(pm, &circle, body, Transform::identity());
    let ring_w = (radius * 0.12).max(1.2);
    if let Some(ring) = PathBuilder::from_circle(center.x, center.y, radius - ring_w / 2.0) {
        let stroke = stroke_of(ring_w, LineCap::Butt, LineJoin::Round);
        stroke_path(
            pm,
            &ring,
            [255, 255, 255, 235],
            &stroke,
            Transform::identity(),
        );
    }
    let digits = n.to_string();
    let size = radius
        * match digits.len() {
            1 => 1.15,
            2 => 0.95,
            _ => 0.75,
        };
    draw_centered_ink(pm, fonts, &digits, size, true, center, contrast_color(body));
}

// ---------------------------------------------------------------------------------------
// Magnifier
// ---------------------------------------------------------------------------------------

pub(crate) fn draw_magnifier(
    pm: &mut Pixmap,
    st: &Style,
    source: RectF,
    dest: RectF,
    circular: bool,
    src: &Pixmap,
) {
    if dest.w < 1.0 || dest.h < 1.0 || source.w < 1.0 || source.h < 1.0 {
        return;
    }
    // Circular insets use the inscribed square so the zoom stays uniform.
    let target = if circular {
        let side = dest.w.min(dest.h);
        let c = dest.center();
        RectF::new(c.x - side / 2.0, c.y - side / 2.0, side, side)
    } else {
        dest
    };
    let path = if circular {
        PathBuilder::from_circle(
            target.x + target.w / 2.0,
            target.y + target.h / 2.0,
            target.w / 2.0,
        )
    } else {
        rounded_rect_path(target, (target.w.min(target.h) * 0.12).clamp(4.0, 24.0))
    };
    let Some(path) = path else { return };
    let ring = magnifier_ring_width(st);

    // Soft shadow under the whole inset.
    if st.shadow {
        let off = Transform::from_translate(0.0, 3.0);
        fill_path(pm, &path, [0, 0, 0, 70], off);
    }

    let sx = target.w / source.w;
    let sy = target.h / source.h;
    let tf = Transform::from_row(
        sx,
        0.0,
        0.0,
        sy,
        target.x - source.x * sx,
        target.y - source.y * sy,
    );
    let p = Paint {
        shader: Pattern::new(
            src.as_ref(),
            SpreadMode::Pad,
            FilterQuality::Bilinear,
            1.0,
            tf,
        ),
        anti_alias: true,
        blend_mode: BlendMode::SourceOver,
        ..Paint::default()
    };
    pm.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);

    let stroke = stroke_of(ring, LineCap::Butt, LineJoin::Round);
    stroke_path(
        pm,
        &path,
        [255, 255, 255, 255],
        &stroke,
        Transform::identity(),
    );
}

// ---------------------------------------------------------------------------------------
// Callout
// ---------------------------------------------------------------------------------------

fn wrap_lines(fonts: &Fonts, text: &str, size: f32, max_w: f32) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let candidate = if line.is_empty() {
                word.to_string()
            } else {
                format!("{line} {word}")
            };
            if !line.is_empty() && fonts.line_width(&candidate, size, false) > max_w {
                out.push(std::mem::take(&mut line));
                line = word.to_string();
            } else {
                line = candidate;
            }
        }
        out.push(line);
    }
    out
}

fn callout_path(r: RectF, tail: PtF, radius: f32) -> Option<Path> {
    if r.w < 1.0 || r.h < 1.0 {
        return None;
    }
    let mut pb = PathBuilder::new();
    push_rounded_rect(&mut pb, r, radius);
    let c = r.center();
    let (dx, dy) = (tail.x - c.x, tail.y - c.y);
    let outside = !r.contains(tail);
    if outside && tail.x.is_finite() && tail.y.is_finite() {
        let (hw, hh) = (r.w / 2.0, r.h / 2.0);
        let horizontal = (dx / hw).abs() >= (dy / hh).abs();
        let inset = 1.0;
        let (a, b) = if horizontal {
            let side_x = if dx > 0.0 { r.x + r.w } else { r.x };
            let t = (side_x - c.x) / dx;
            let bw = (r.h * 0.45).clamp(6.0, 28.0);
            let lo = r.y + radius + bw / 2.0;
            let hi = r.y + r.h - radius - bw / 2.0;
            let ay = if lo < hi {
                (c.y + dy * t).clamp(lo, hi)
            } else {
                c.y
            };
            let ix = if dx > 0.0 { -inset } else { inset };
            (
                PtF::new(side_x + ix, ay - bw / 2.0),
                PtF::new(side_x + ix, ay + bw / 2.0),
            )
        } else {
            let side_y = if dy > 0.0 { r.y + r.h } else { r.y };
            let t = (side_y - c.y) / dy;
            let bw = (r.w * 0.2).clamp(6.0, 28.0);
            let lo = r.x + radius + bw / 2.0;
            let hi = r.x + r.w - radius - bw / 2.0;
            let ax = if lo < hi {
                (c.x + dx * t).clamp(lo, hi)
            } else {
                c.x
            };
            let iy = if dy > 0.0 { -inset } else { inset };
            (
                PtF::new(ax - bw / 2.0, side_y + iy),
                PtF::new(ax + bw / 2.0, side_y + iy),
            )
        };
        // Match the winding of the rounded rect (positive shoelace area in y-down coords)
        // so the union fills cleanly under the non-zero rule.
        let area =
            a.x * b.y - b.x * a.y + b.x * tail.y - tail.x * b.y + tail.x * a.y - a.x * tail.y;
        let (p1, p2) = if area >= 0.0 { (a, b) } else { (b, a) };
        pb.move_to(p1.x, p1.y);
        pb.line_to(p2.x, p2.y);
        pb.line_to(tail.x, tail.y);
        pb.close();
    }
    pb.finish()
}

fn draw_callout(
    pm: &mut Pixmap,
    fonts: &Fonts,
    st: &Style,
    r: RectF,
    tail: PtF,
    text: &str,
    size: f32,
) {
    let size = size.max(1.0);
    let radius = (r.w.min(r.h) * 0.25).clamp(4.0, 16.0);
    let Some(path) = callout_path(r, tail, radius) else {
        return;
    };
    let body = st
        .fill
        .unwrap_or([st.color[0], st.color[1], st.color[2], 255]);
    if st.shadow {
        let off = Transform::from_translate(0.0, shadow_offset(st.width.max(4.0)));
        fill_path(pm, &path, SHADOW_RGBA, off);
    }
    fill_path(pm, &path, body, Transform::identity());
    if st.fill.is_some() && st.width > 0.0 {
        let stroke = stroke_of(st.width, LineCap::Round, LineJoin::Round);
        stroke_path(pm, &path, st.color, &stroke, Transform::identity());
    }
    let pad = size * CALLOUT_PAD;
    let lines = wrap_lines(fonts, text, size, (r.w - 2.0 * pad).max(size));
    let block_h = lines.len() as f32 * size * LINE_HEIGHT;
    let top = r.y + (r.h - block_h) / 2.0;
    let color = contrast_color(body);
    draw_text_block(
        pm,
        fonts,
        &lines.join("\n"),
        PtF::new(r.x + pad, top),
        size,
        false,
        color,
        false,
    );
}
