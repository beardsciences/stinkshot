//! Lossless PNG re-compression, oxipng style, in pure Rust.
//!
//! The capture path writes PNGs with the fast encoder (fdeflate, adaptive filter, RGBA): about
//! 0.1 s for a 4K screenshot, but 1.3-1.9x the size per pixel of other tools' files. The app
//! re-compresses each saved PNG afterwards on a low-priority thread (`app/src/recompress.rs`)
//! with [`optimize`]:
//!
//! 1. Decode (8-bit only) and normalize to RGBA8.
//! 2. Lossless color reductions: palette (up to 256 colors, 1/2/4/8 bit), grayscale, and no
//!    alpha channel when every pixel is opaque (screenshots almost always are).
//! 3. Filter trials: the five fixed filters plus two per-row heuristics (minimum sum of
//!    absolute values, minimum entropy), each filtered buffer ranked by a quick deflate.
//! 4. The winner is deflated again with zopfli (or miniz level 10).
//! 5. The result is decoded and compared with the original pixels; any difference discards
//!    it. Ancillary chunks (color space, text, physical size, ...) are copied unchanged.
//!
//! Nothing here touches files: the caller decides what to do with the bytes.

use std::collections::HashMap;
use std::io::Cursor;

use anyhow::{Context, bail};

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// How hard the final deflate tries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effort {
    /// miniz_oxide at its best level (about zlib -9). Seconds for a 4K screenshot.
    Miniz,
    /// zopfli with this many iterations (1-15). Several times slower than `Miniz`, about
    /// 4-8 % smaller.
    Zopfli(u8),
}

/// Result of [`optimize`].
#[derive(Debug)]
pub enum Outcome {
    /// A smaller file with exactly the same pixels.
    Smaller(Vec<u8>),
    /// Nothing smaller was found; keep the original.
    NotSmaller,
    /// The file was left alone (why: 16-bit, animated, unknown critical chunk, ...).
    Skipped(&'static str),
}

/// What the optimizer chose, for logs and benchmarks.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub color: &'static str,
    pub filter: &'static str,
    pub idat_estimate: usize,
    /// Time spent on reductions and filter trials, and on the final deflate (ms).
    pub trial_ms: f64,
    pub final_ms: f64,
}

struct Chunk<'a> {
    ty: [u8; 4],
    data: &'a [u8],
}

fn parse_chunks(png: &[u8]) -> anyhow::Result<Vec<Chunk<'_>>> {
    if png.len() < 8 || png[..8] != SIGNATURE {
        bail!("not a PNG file");
    }
    let mut out = Vec::new();
    let mut at = 8;
    while at + 12 <= png.len() {
        let len = u32::from_be_bytes(png[at..at + 4].try_into()?) as usize;
        let ty: [u8; 4] = png[at + 4..at + 8].try_into()?;
        let end = at
            .checked_add(12)
            .and_then(|v| v.checked_add(len))
            .context("chunk length overflow")?;
        if end > png.len() {
            bail!("truncated chunk");
        }
        out.push(Chunk {
            ty,
            data: &png[at + 8..at + 8 + len],
        });
        at = end;
        if &ty == b"IEND" {
            return Ok(out);
        }
    }
    bail!("no IEND chunk")
}

/// Decode any 8-bit (or lower) PNG to straight RGBA8. None for 16-bit images.
fn decode_rgba8(png: &[u8]) -> anyhow::Result<Option<(u32, u32, Vec<u8>)>> {
    let mut dec = png::Decoder::new(Cursor::new(png));
    dec.set_transformations(png::Transformations::EXPAND);
    let mut reader = dec.read_info().context("PNG header")?;
    if reader.info().bit_depth == png::BitDepth::Sixteen {
        return Ok(None);
    }
    let (w, h) = (reader.info().width, reader.info().height);
    let mut buf = vec![
        0u8;
        reader
            .output_buffer_size()
            .context("PNG output size overflow")?
    ];
    let frame = reader.next_frame(&mut buf).context("PNG data")?;
    let (color, depth) = (frame.color_type, frame.bit_depth);
    if depth != png::BitDepth::Eight {
        return Ok(None);
    }
    buf.truncate(frame.buffer_size());
    let px = w as usize * h as usize;
    let rgba = match color {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => {
            let mut o = Vec::with_capacity(px * 4);
            for p in buf.chunks_exact(3) {
                o.extend_from_slice(&[p[0], p[1], p[2], 255]);
            }
            o
        }
        png::ColorType::Grayscale => {
            let mut o = Vec::with_capacity(px * 4);
            for &g in &buf {
                o.extend_from_slice(&[g, g, g, 255]);
            }
            o
        }
        png::ColorType::GrayscaleAlpha => {
            let mut o = Vec::with_capacity(px * 4);
            for p in buf.chunks_exact(2) {
                o.extend_from_slice(&[p[0], p[0], p[0], p[1]]);
            }
            o
        }
        // EXPAND turns palettes into RGB/RGBA.
        png::ColorType::Indexed => bail!("palette not expanded"),
    };
    if rgba.len() != px * 4 {
        bail!("decoded size mismatch");
    }
    Ok(Some((w, h, rgba)))
}

/// Chunks that are rewritten (or dropped because they depend on the old color type).
fn regenerated(ty: &[u8; 4]) -> bool {
    matches!(
        ty,
        b"IHDR" | b"PLTE" | b"tRNS" | b"IDAT" | b"IEND" | b"bKGD" | b"sBIT" | b"hIST"
    )
}

/// One lossless pixel layout to try.
struct Layout {
    name: &'static str,
    /// PNG color type byte.
    color_type: u8,
    bit_depth: u8,
    /// Unfiltered scanlines, without filter bytes.
    rows: Vec<u8>,
    row_bytes: usize,
    /// Bytes per complete pixel for filtering (at least 1).
    bpp: usize,
    palette: Vec<[u8; 4]>,
}

fn layouts(w: usize, h: usize, rgba: &[u8], allow_gray: bool) -> Vec<Layout> {
    let opaque = rgba.chunks_exact(4).all(|p| p[3] == 255);
    let gray = allow_gray && rgba.chunks_exact(4).all(|p| p[0] == p[1] && p[1] == p[2]);
    let mut out = Vec::new();

    // Palette, when there are at most 256 distinct colors.
    let mut index: HashMap<u32, u8> = HashMap::new();
    let mut palette: Vec<[u8; 4]> = Vec::new();
    let mut fits = true;
    let mut last: Option<(u32, u8)> = None;
    let mut idx = Vec::with_capacity(w * h);
    for p in rgba.chunks_exact(4) {
        let key = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
        let i = match last {
            Some((k, i)) if k == key => i,
            _ => match index.get(&key) {
                Some(&i) => i,
                None => {
                    if palette.len() == 256 {
                        fits = false;
                        break;
                    }
                    let i = palette.len() as u8;
                    palette.push([p[0], p[1], p[2], p[3]]);
                    index.insert(key, i);
                    i
                }
            },
        };
        last = Some((key, i));
        idx.push(i);
    }
    if fits {
        // Entries with alpha < 255 first, so tRNS can stop after them.
        let mut order: Vec<usize> = (0..palette.len()).collect();
        order.sort_by_key(|&i| (palette[i][3] == 255, i));
        let mut remap = vec![0u8; palette.len()];
        for (new, &old) in order.iter().enumerate() {
            remap[old] = new as u8;
        }
        let sorted: Vec<[u8; 4]> = order.iter().map(|&i| palette[i]).collect();
        for v in &mut idx {
            *v = remap[*v as usize];
        }
        let depth = match sorted.len() {
            0..=2 => 1,
            3..=4 => 2,
            5..=16 => 4,
            _ => 8,
        };
        let row_bytes = (w * depth as usize).div_ceil(8);
        let mut rows = vec![0u8; row_bytes * h];
        let per_byte = 8 / depth as usize;
        for y in 0..h {
            let src = &idx[y * w..(y + 1) * w];
            let dst = &mut rows[y * row_bytes..(y + 1) * row_bytes];
            if depth == 8 {
                dst.copy_from_slice(src);
            } else {
                for (x, &v) in src.iter().enumerate() {
                    let shift = 8 - depth as usize * (x % per_byte + 1);
                    dst[x / per_byte] |= v << shift;
                }
            }
        }
        out.push(Layout {
            name: "palette",
            color_type: 3,
            bit_depth: depth,
            rows,
            row_bytes,
            bpp: 1,
            palette: sorted,
        });
    }
    drop(idx);

    let (name, color_type, channels): (&'static str, u8, usize) = match (gray, opaque) {
        (true, true) => ("gray", 0, 1),
        (true, false) => ("gray+alpha", 4, 2),
        (false, true) => ("rgb", 2, 3),
        (false, false) => ("rgba", 6, 4),
    };
    let mut rows = Vec::with_capacity(w * h * channels);
    for p in rgba.chunks_exact(4) {
        match channels {
            1 => rows.push(p[0]),
            2 => rows.extend_from_slice(&[p[0], p[3]]),
            3 => rows.extend_from_slice(&p[..3]),
            _ => rows.extend_from_slice(p),
        }
    }
    out.push(Layout {
        name,
        color_type,
        bit_depth: 8,
        rows,
        row_bytes: w * channels,
        bpp: channels,
        palette: Vec::new(),
    });
    out
}

#[inline]
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let pa = (p - a as i16).abs();
    let pb = (p - b as i16).abs();
    let pc = (p - c as i16).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Filter one row with PNG filter `ty` (0-4) into `out` (same length as `cur`).
fn filter_row(ty: u8, bpp: usize, prev: &[u8], cur: &[u8], out: &mut [u8]) {
    let n = cur.len();
    match ty {
        0 => out.copy_from_slice(cur),
        1 => {
            out[..bpp.min(n)].copy_from_slice(&cur[..bpp.min(n)]);
            for i in bpp..n {
                out[i] = cur[i].wrapping_sub(cur[i - bpp]);
            }
        }
        2 => {
            for i in 0..n {
                out[i] = cur[i].wrapping_sub(prev[i]);
            }
        }
        3 => {
            for i in 0..n {
                let a = if i >= bpp { cur[i - bpp] as u16 } else { 0 };
                out[i] = cur[i].wrapping_sub(((a + prev[i] as u16) / 2) as u8);
            }
        }
        _ => {
            for i in 0..n {
                let (a, c) = if i >= bpp {
                    (cur[i - bpp], prev[i - bpp])
                } else {
                    (0, 0)
                };
                out[i] = cur[i].wrapping_sub(paeth(a, prev[i], c));
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Strategy {
    Fixed(u8),
    MinSum,
    Entropy,
}

impl Strategy {
    fn name(self) -> &'static str {
        match self {
            Strategy::Fixed(0) => "none",
            Strategy::Fixed(1) => "sub",
            Strategy::Fixed(2) => "up",
            Strategy::Fixed(3) => "average",
            Strategy::Fixed(_) => "paeth",
            Strategy::MinSum => "minsum",
            Strategy::Entropy => "entropy",
        }
    }
}

fn row_cost(s: Strategy, data: &[u8]) -> u64 {
    match s {
        Strategy::Entropy => {
            let mut hist = [0u32; 256];
            for &b in data {
                hist[b as usize] += 1;
            }
            let n = data.len() as f64;
            let mut e = 0.0;
            for &c in &hist {
                if c > 0 {
                    let p = c as f64 / n;
                    e -= p * p.log2();
                }
            }
            (e * 1_000_000.0) as u64
        }
        _ => data.iter().map(|&b| (b as i8).unsigned_abs() as u64).sum(),
    }
}

/// The whole filtered image (filter byte + row per scanline).
fn filter_image(l: &Layout, h: usize, s: Strategy) -> Vec<u8> {
    let rb = l.row_bytes;
    let mut out = Vec::with_capacity((rb + 1) * h);
    let zero = vec![0u8; rb];
    let mut tmp = vec![0u8; rb];
    let mut best = vec![0u8; rb];
    for y in 0..h {
        let cur = &l.rows[y * rb..(y + 1) * rb];
        let prev = if y == 0 {
            &zero[..]
        } else {
            &l.rows[(y - 1) * rb..y * rb]
        };
        match s {
            Strategy::Fixed(t) => {
                out.push(t);
                let at = out.len();
                out.resize(at + rb, 0);
                filter_row(t, l.bpp, prev, cur, &mut out[at..]);
            }
            Strategy::MinSum | Strategy::Entropy => {
                let mut best_t = 0u8;
                let mut best_cost = u64::MAX;
                for t in 0..5u8 {
                    filter_row(t, l.bpp, prev, cur, &mut tmp);
                    let c = row_cost(s, &tmp);
                    if c < best_cost {
                        best_cost = c;
                        best_t = t;
                        std::mem::swap(&mut best, &mut tmp);
                    }
                }
                out.push(best_t);
                out.extend_from_slice(&best);
            }
        }
    }
    out
}

fn chunk(out: &mut Vec<u8>, ty: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(ty);
    out.extend_from_slice(data);
    let crc = crc32fast::hash(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

fn zlib(data: &[u8], effort: Effort) -> anyhow::Result<Vec<u8>> {
    match effort {
        Effort::Miniz => Ok(miniz_oxide::deflate::compress_to_vec_zlib(data, 10)),
        Effort::Zopfli(iterations) => {
            let opts = zopfli::Options {
                iteration_count: std::num::NonZeroU64::new(iterations.clamp(1, 15) as u64)
                    .unwrap_or(std::num::NonZeroU64::MIN),
                ..Default::default()
            };
            let mut out = Vec::with_capacity(data.len() / 4);
            zopfli::compress(opts, zopfli::Format::Zlib, data, &mut out)
                .context("zopfli failed")?;
            Ok(out)
        }
    }
}

/// Re-compress `original` losslessly. See the module docs.
pub fn optimize(original: &[u8], effort: Effort) -> anyhow::Result<(Outcome, Report)> {
    let chunks = parse_chunks(original)?;
    let mut report = Report::default();
    let mut has_iccp = false;
    for c in &chunks {
        if &c.ty == b"acTL" {
            return Ok((Outcome::Skipped("animated PNG"), report));
        }
        if c.ty[0].is_ascii_uppercase() && !regenerated(&c.ty) {
            return Ok((Outcome::Skipped("unknown critical chunk"), report));
        }
        has_iccp |= &c.ty == b"iCCP";
    }
    let Some((w, h, rgba)) = decode_rgba8(original)? else {
        return Ok((Outcome::Skipped("16-bit PNG"), report));
    };
    let (wu, hu) = (w as usize, h as usize);
    // A grayscale image must not carry an RGB ICC profile.
    let started = std::time::Instant::now();
    let candidates = layouts(wu, hu, &rgba, !has_iccp);

    // Rank layout x filter by a quick deflate of the filtered data; keep only the best buffer.
    let mut best: Option<(usize, usize, Strategy, Vec<u8>)> = None;
    for (li, l) in candidates.iter().enumerate() {
        let strategies: &[Strategy] = if l.color_type == 3 && l.bit_depth < 8 {
            &[Strategy::Fixed(0), Strategy::MinSum]
        } else {
            &[
                Strategy::Fixed(0),
                Strategy::Fixed(1),
                Strategy::Fixed(2),
                Strategy::Fixed(3),
                Strategy::Fixed(4),
                Strategy::MinSum,
                Strategy::Entropy,
            ]
        };
        for &s in strategies {
            let filtered = filter_image(l, hu, s);
            let est = miniz_oxide::deflate::compress_to_vec_zlib(&filtered, 3).len()
                + l.palette.len() * 4;
            if best.as_ref().is_none_or(|b| est < b.1) {
                best = Some((li, est, s, filtered));
            }
        }
    }
    let Some((li, est, strategy, filtered)) = best else {
        return Ok((Outcome::NotSmaller, report));
    };
    let layout = &candidates[li];
    let trial_ms = started.elapsed().as_secs_f64() * 1000.0;
    let final_start = std::time::Instant::now();
    let idat = zlib(&filtered, effort)?;
    drop(filtered);
    report = Report {
        color: layout.name,
        filter: strategy.name(),
        idat_estimate: est,
        trial_ms,
        final_ms: final_start.elapsed().as_secs_f64() * 1000.0,
    };

    let mut out = Vec::with_capacity(idat.len() + 4096);
    out.extend_from_slice(&SIGNATURE);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[layout.bit_depth, layout.color_type, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    let first_idat = chunks
        .iter()
        .position(|c| &c.ty == b"IDAT")
        .unwrap_or(chunks.len());
    for c in &chunks[..first_idat] {
        if !regenerated(&c.ty) {
            chunk(&mut out, &c.ty, c.data);
        }
    }
    if layout.color_type == 3 {
        let plte: Vec<u8> = layout
            .palette
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        chunk(&mut out, b"PLTE", &plte);
        let trns: Vec<u8> = layout
            .palette
            .iter()
            .take_while(|p| p[3] != 255)
            .map(|p| p[3])
            .collect();
        if !trns.is_empty() {
            chunk(&mut out, b"tRNS", &trns);
        }
    }
    chunk(&mut out, b"IDAT", &idat);
    for c in &chunks[first_idat..] {
        if !regenerated(&c.ty) {
            chunk(&mut out, &c.ty, c.data);
        }
    }
    chunk(&mut out, b"IEND", &[]);

    if out.len() >= original.len() {
        return Ok((Outcome::NotSmaller, report));
    }
    // Never trust the writer: the new file must decode to exactly the same pixels.
    match decode_rgba8(&out) {
        Ok(Some((w2, h2, px))) if w2 == w && h2 == h && px == rgba => {
            Ok((Outcome::Smaller(out), report))
        }
        Ok(_) => bail!("re-encoded PNG does not match the original pixels"),
        Err(e) => Err(e.context("re-encoded PNG does not decode")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rgba;

    fn fast_png(img: &Rgba) -> Vec<u8> {
        img.to_png().unwrap()
    }

    fn check(img: &Rgba, expect_color: &str) {
        let png = fast_png(img);
        let (outcome, report) = optimize(&png, Effort::Miniz).unwrap();
        if !expect_color.is_empty() {
            assert_eq!(report.color, expect_color);
        }
        if let Outcome::Smaller(bytes) = outcome {
            assert!(bytes.len() < png.len());
            let back = Rgba::decode(&bytes).unwrap();
            assert_eq!(back.width, img.width);
            assert_eq!(back.data, img.data);
        }
    }

    fn noisy(w: u32, h: u32, alpha: bool, gray: bool) -> Rgba {
        let mut img = Rgba::new(w, h);
        let mut s = 12345u32;
        for p in img.data.chunks_exact_mut(4) {
            s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let v = (s >> 16) as u8;
            let u = (s >> 8) as u8;
            let (r, g, b) = if gray { (v, v, v) } else { (v, u, 255 - v) };
            p.copy_from_slice(&[r, g, b, if alpha { v | 1 } else { 255 }]);
        }
        img
    }

    #[test]
    fn opaque_true_color_drops_alpha() {
        let mut img = noisy(97, 41, false, false);
        // A smooth gradient part so filters matter.
        for y in 0..20u32 {
            for x in 0..97u32 {
                let i = ((y * 97 + x) * 4) as usize;
                img.data[i..i + 4].copy_from_slice(&[x as u8, y as u8, 7, 255]);
            }
        }
        check(&img, "rgb");
    }

    #[test]
    fn alpha_is_kept() {
        check(&noisy(33, 17, true, false), "rgba");
    }

    #[test]
    fn gray_images() {
        check(&noisy(64, 9, false, true), "gray");
        check(&noisy(64, 9, true, true), "gray+alpha");
    }

    #[test]
    fn few_colors_become_a_palette_at_every_depth() {
        for colors in [2u32, 3, 9, 200] {
            let mut img = Rgba::new(203, 59);
            for (i, p) in img.data.chunks_exact_mut(4).enumerate() {
                let c = (i as u32 / 5) % colors;
                let a = if c == 1 { 128 } else { 255 };
                p.copy_from_slice(&[(c * 37) as u8, (c * 11) as u8, (c * 5) as u8, a]);
            }
            // With many colors a true-color layout may compress better; it must still be exact.
            check(&img, if colors <= 16 { "palette" } else { "" });
        }
    }

    #[test]
    fn ancillary_chunks_survive() {
        let img = noisy(20, 20, false, false);
        let png = fast_png(&img);
        // Insert a tEXt chunk right after IHDR (8 + 25 bytes).
        let mut with_text = png[..33].to_vec();
        chunk(&mut with_text, b"tEXt", b"Comment\0hello");
        with_text.extend_from_slice(&png[33..]);
        if let (Outcome::Smaller(out), _) = optimize(&with_text, Effort::Miniz).unwrap() {
            let names: Vec<[u8; 4]> = parse_chunks(&out).unwrap().iter().map(|c| c.ty).collect();
            assert!(names.contains(b"tEXt"));
        }
    }

    #[test]
    fn rejects_garbage_and_skips_animation() {
        assert!(optimize(b"not a png", Effort::Miniz).is_err());
        let png = fast_png(&noisy(8, 8, false, false));
        let mut anim = png[..33].to_vec();
        chunk(&mut anim, b"acTL", &[0, 0, 0, 1, 0, 0, 0, 0]);
        anim.extend_from_slice(&png[33..]);
        assert!(matches!(
            optimize(&anim, Effort::Miniz).unwrap().0,
            Outcome::Skipped(_)
        ));
    }

    #[test]
    fn zopfli_round_trips() {
        let img = noisy(40, 30, false, false);
        let png = fast_png(&img);
        if let (Outcome::Smaller(out), _) = optimize(&png, Effort::Zopfli(1)).unwrap() {
            assert_eq!(Rgba::decode(&out).unwrap().data, img.data);
        }
    }

    #[test]
    fn filters_invert() {
        // Unfilter each fixed filter by hand and compare with the source row.
        let prev: Vec<u8> = (0..24).map(|i| (i * 13) as u8).collect();
        let cur: Vec<u8> = (0..24).map(|i| (i * 29 + 5) as u8).collect();
        for t in 0..5u8 {
            let mut f = vec![0u8; 24];
            filter_row(t, 3, &prev, &cur, &mut f);
            let mut r = vec![0u8; 24];
            for i in 0..24 {
                let a = if i >= 3 { r[i - 3] } else { 0 };
                let c = if i >= 3 { prev[i - 3] } else { 0 };
                let pred = match t {
                    0 => 0,
                    1 => a,
                    2 => prev[i],
                    3 => ((a as u16 + prev[i] as u16) / 2) as u8,
                    _ => paeth(a, prev[i], c),
                };
                r[i] = f[i].wrapping_add(pred);
            }
            assert_eq!(r, cur, "filter {t}");
        }
    }
}
