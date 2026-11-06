//! MP4 -> animated GIF with per-frame optimized palettes, dithering and frame-diff
//! transparency.

use std::borrow::Cow;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use imagequant::RGBA;

use crate::GifOptions;
use crate::convert::{fit_within, scale_4ch};
use crate::media::bgra_to_rgba;
use crate::mf::duration_to_hns;
use crate::mfread::{Reader, VideoFrame};

/// Per-channel difference above which a pixel counts as changed.
pub fn change_tolerance(quality: u8) -> u8 {
    (1 + (100 - quality.min(100)) / 12).min(24)
}

/// Bounding box (x, y, w, h) of pixels differing between `reference` and `cur` (both RGBA of
/// the same size), or None when identical within `tol`.
pub fn diff_bbox(
    reference: &[u8],
    cur: &[u8],
    w: usize,
    h: usize,
    tol: u8,
) -> Option<(usize, usize, usize, usize)> {
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for y in 0..h {
        let row = y * w * 4;
        for x in 0..w {
            let i = row + x * 4;
            let changed = (0..3).any(|c| reference[i + c].abs_diff(cur[i + c]) > tol);
            if changed {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    (x0 != usize::MAX).then(|| (x0, y0, x1 - x0 + 1, y1 - y0 + 1))
}

/// Centisecond delay for output frame `k` of `n` at `fps`, rounded cumulatively so the total
/// duration does not drift.
pub fn delay_cs(k: u64, fps: u32) -> u16 {
    let at = |i: u64| (i * 100 + fps as u64 / 2) / fps as u64;
    (at(k + 1) - at(k)).max(1) as u16
}

struct Pending {
    frame: gif::Frame<'static>,
}

fn temp_for(output: &Path) -> PathBuf {
    let stem = output
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "out".into());
    output.with_file_name(format!("{stem}.gif.tmp"))
}

pub fn to_gif(
    input: &Path,
    output: &Path,
    opts: &GifOptions,
    progress: &dyn Fn(f32),
) -> Result<()> {
    let mut rd = Reader::open(input, true, false)?;
    if !rd.has_video {
        bail!("{} has no video stream", input.display());
    }
    let (sw, sh) = (rd.width, rd.height);
    let (gw, gh) = fit_within(sw, sh, opts.max_width, 0);
    if gw > u16::MAX as u32 || gh > u16::MAX as u32 {
        bail!("GIF dimensions too large");
    }
    let fps = opts.fps.clamp(1, 60);
    let start_h = duration_to_hns(opts.start);
    let dur_h = rd.duration_hns;
    let end_h = match opts.end {
        Some(e) => duration_to_hns(e).min(if dur_h > 0 { dur_h } else { i64::MAX }),
        None => {
            if dur_h > 0 {
                dur_h
            } else {
                i64::MAX
            }
        }
    };
    if end_h <= start_h {
        bail!("GIF range is empty");
    }
    let total_frames = if end_h == i64::MAX {
        u64::MAX
    } else {
        (((end_h - start_h) as u128 * fps as u128) / 10_000_000) as u64
    };
    let quality = opts.quality.min(100);
    let tol = change_tolerance(quality);

    let temp = temp_for(output);
    let _ = std::fs::remove_file(&temp);
    let result = (|| -> Result<()> {
        let file =
            File::create(&temp).with_context(|| format!("cannot create {}", temp.display()))?;
        let mut enc = gif::Encoder::new(BufWriter::new(file), gw as u16, gh as u16, &[])
            .context("GIF encoder")?;
        if opts.loop_forever {
            enc.set_repeat(gif::Repeat::Infinite)
                .context("GIF repeat")?;
        }

        if start_h > 0 {
            rd.seek(start_h)?;
        }
        let mut next_src: Option<VideoFrame> = rd.read_video()?;
        let mut cur_src: Option<VideoFrame> = None;
        let mut cur_serial = 0u64; // identifies cur_src
        let mut last_used_serial = u64::MAX;
        let mut reference: Vec<u8> = Vec::new(); // RGBA: source value at each pixel's last update
        let mut pending: Option<Pending> = None;
        let mut out_index = 0u64;
        let mut produced = 0u64;

        let mut k = 0u64;
        while k < total_frames {
            let t = start_h + (k as u128 * 10_000_000 / fps as u128) as i64;
            if t >= end_h {
                break;
            }
            // advance the decoder so cur_src is the latest frame with pts <= t
            loop {
                let advance = match &next_src {
                    Some(n) => n.pts_hns <= t || cur_src.is_none(),
                    None => false,
                };
                if !advance {
                    break;
                }
                cur_src = next_src.take();
                cur_serial += 1;
                next_src = rd.read_video()?;
            }
            let Some(src) = &cur_src else { break };
            if k > 0
                && next_src.is_none()
                && src.pts_hns + src.dur_hns.max(1) <= t
                && total_frames == u64::MAX
            {
                break; // ran past the end of an unbounded range
            }
            let delay = delay_cs(k, fps);
            k += 1;
            produced += 1;
            if cur_serial == last_used_serial {
                if let Some(p) = &mut pending {
                    p.frame.delay = p.frame.delay.saturating_add(delay);
                }
                continue;
            }
            last_used_serial = cur_serial;

            let scaled = if (gw, gh) == (sw, sh) {
                src.bgra.clone()
            } else {
                scale_4ch(
                    &src.bgra,
                    sw as usize,
                    sh as usize,
                    gw as usize,
                    gh as usize,
                )
            };
            let cur = bgra_to_rgba(scaled);
            let (gwu, ghu) = (gw as usize, gh as usize);

            let frame = if reference.is_empty() {
                reference = cur.clone();
                Some(make_frame(
                    &cur,
                    gwu,
                    (0, 0, gwu, ghu),
                    None,
                    quality,
                    delay,
                )?)
            } else {
                match diff_bbox(&reference, &cur, gwu, ghu, tol) {
                    None => None,
                    Some(bb) => Some(make_frame(
                        &cur,
                        gwu,
                        bb,
                        Some((&mut reference, tol)),
                        quality,
                        delay,
                    )?),
                }
            };
            match frame {
                Some(f) => {
                    if let Some(p) = pending.take() {
                        enc.write_frame(&p.frame).context("write GIF frame")?;
                        out_index += 1;
                    }
                    pending = Some(Pending { frame: f });
                }
                None => {
                    if let Some(p) = &mut pending {
                        p.frame.delay = p.frame.delay.saturating_add(delay);
                    }
                }
            }
            if produced.is_multiple_of(4) && total_frames != u64::MAX {
                progress((produced as f32 / total_frames.max(1) as f32).clamp(0.0, 1.0) * 0.99);
            } else if total_frames == u64::MAX {
                progress(((t - start_h) as f32 / 1e7 / 600.0).clamp(0.0, 0.99));
            }
        }
        if let Some(p) = pending.take() {
            enc.write_frame(&p.frame).context("write GIF frame")?;
            out_index += 1;
        }
        if out_index == 0 {
            bail!("no frames in the selected range");
        }
        drop(enc);
        Ok(())
    })();
    match result {
        Ok(()) => {
            if output.exists() {
                let _ = std::fs::remove_file(output);
            }
            std::fs::rename(&temp, output)
                .with_context(|| format!("cannot write {}", output.display()))?;
            progress(1.0);
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            Err(e)
        }
    }
}

/// Quantize `bbox` of `cur` into a GIF frame. With `diff = Some((reference, tol))`, pixels that
/// did not change are made transparent and `reference` is updated for those that did.
fn make_frame(
    cur: &[u8],
    w: usize,
    bbox: (usize, usize, usize, usize),
    mut diff: Option<(&mut Vec<u8>, u8)>,
    quality: u8,
    delay: u16,
) -> Result<gif::Frame<'static>> {
    let (bx, by, bw, bh) = bbox;
    let mut pixels: Vec<RGBA> = Vec::with_capacity(bw * bh);
    let mut any_transparent = false;
    for y in by..by + bh {
        for x in bx..bx + bw {
            let i = (y * w + x) * 4;
            let changed = match &diff {
                Some((reference, tol)) => {
                    (0..3).any(|c| reference[i + c].abs_diff(cur[i + c]) > *tol)
                }
                None => true,
            };
            if changed {
                pixels.push(RGBA::new(cur[i], cur[i + 1], cur[i + 2], 255));
            } else {
                pixels.push(RGBA::new(0, 0, 0, 0));
                any_transparent = true;
            }
        }
    }
    if let Some((reference, _)) = &mut diff {
        for (n, px) in pixels.iter().enumerate() {
            if px.a == 255 {
                let (x, y) = (bx + n % bw, by + n / bw);
                let i = (y * w + x) * 4;
                reference[i..i + 4].copy_from_slice(&[px.r, px.g, px.b, 255]);
            }
        }
    }

    let mut attr = imagequant::Attributes::new();
    attr.set_max_colors(256)
        .map_err(|e| anyhow!("imagequant: {e}"))?;
    let q = quality.min(100);
    attr.set_quality(q / 2, q.max(1))
        .map_err(|e| anyhow!("imagequant: {e}"))?;
    attr.set_speed(4).map_err(|e| anyhow!("imagequant: {e}"))?;
    let mut img = attr
        .new_image(pixels, bw, bh, 0.0)
        .map_err(|e| anyhow!("imagequant: {e}"))?;
    let mut res = attr
        .quantize(&mut img)
        .map_err(|e| anyhow!("imagequant quantize: {e}"))?;
    res.set_dithering_level(1.0)
        .map_err(|e| anyhow!("imagequant: {e}"))?;
    let (palette, indices) = res
        .remapped(&mut img)
        .map_err(|e| anyhow!("imagequant remap: {e}"))?;

    let transparent = if any_transparent {
        palette.iter().position(|c| c.a == 0).map(|i| i as u8)
    } else {
        None
    };
    let mut pal_bytes = Vec::with_capacity(palette.len() * 3);
    for c in &palette {
        pal_bytes.extend_from_slice(&[c.r, c.g, c.b]);
    }
    if pal_bytes.len() < 6 {
        pal_bytes.resize(6, 0);
    }
    Ok(gif::Frame {
        delay,
        dispose: gif::DisposalMethod::Keep,
        transparent,
        left: bx as u16,
        top: by as u16,
        width: bw as u16,
        height: bh as u16,
        palette: Some(pal_bytes),
        buffer: Cow::Owned(indices),
        ..gif::Frame::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tolerance_grows_as_quality_drops() {
        assert_eq!(change_tolerance(100), 1);
        assert!(change_tolerance(60) > change_tolerance(90));
        assert!(change_tolerance(0) <= 24);
    }

    #[test]
    fn delay_rounding_does_not_drift() {
        // 15 fps = 6.67 cs per frame: 15 frames must total exactly 100 cs
        let total: u32 = (0..15).map(|k| delay_cs(k, 15) as u32).sum();
        assert_eq!(total, 100);
        let total: u32 = (0..30).map(|k| delay_cs(k, 30) as u32).sum();
        assert_eq!(total, 100);
        assert!((0..50).all(|k| delay_cs(k, 50) >= 1));
    }

    #[test]
    fn diff_bbox_finds_changed_region() {
        let (w, h) = (10, 8);
        let a = vec![100u8; w * h * 4];
        let mut b = a.clone();
        assert_eq!(diff_bbox(&a, &b, w, h, 2), None);
        // tiny noise below tolerance is ignored
        b[0] = 101;
        assert_eq!(diff_bbox(&a, &b, w, h, 2), None);
        for (x, y) in [(3, 2), (6, 5)] {
            b[(y * w + x) * 4] = 200;
        }
        assert_eq!(diff_bbox(&a, &b, w, h, 2), Some((3, 2, 4, 4)));
    }

    #[test]
    fn make_frame_marks_unchanged_pixels_transparent() {
        let (w, h) = (8usize, 8usize);
        let mut reference = vec![0u8; w * h * 4];
        for p in reference.chunks_exact_mut(4) {
            p.copy_from_slice(&[10, 20, 30, 255]);
        }
        let mut cur = reference.clone();
        // change a 2x2 block to bright red
        for y in 2..4 {
            for x in 3..5 {
                cur[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[250, 0, 0, 255]);
            }
        }
        // and one pixel inside the bbox region stays equal -> transparent
        let bb = diff_bbox(&reference, &cur, w, h, 2).unwrap();
        assert_eq!(bb, (3, 2, 2, 2));
        // widen the bbox by one column to include an unchanged column
        let bb = (3, 2, 3, 2);
        let f = make_frame(&cur, w, bb, Some((&mut reference, 2)), 90, 7).unwrap();
        assert_eq!((f.left, f.top, f.width, f.height), (3, 2, 3, 2));
        assert_eq!(f.delay, 7);
        let t = f.transparent.expect("transparent index");
        // third column (x = 5) is unchanged -> transparent index
        assert_eq!(f.buffer[2], t);
        assert_eq!(f.buffer[5], t);
        assert_ne!(f.buffer[0], t);
        // reference now holds red for the changed block
        assert_eq!(
            &reference[(2 * w + 3) * 4..(2 * w + 3) * 4 + 3],
            &[250, 0, 0]
        );
    }

    #[test]
    fn first_frame_is_opaque_and_full() {
        let (w, h) = (6usize, 4usize);
        let mut cur = vec![0u8; w * h * 4];
        for (i, p) in cur.chunks_exact_mut(4).enumerate() {
            p.copy_from_slice(&[(i * 10) as u8, 50, 200, 255]);
        }
        let f = make_frame(&cur, w, (0, 0, w, h), None, 80, 5).unwrap();
        assert_eq!(f.transparent, None);
        assert_eq!(f.buffer.len(), w * h);
    }
}
