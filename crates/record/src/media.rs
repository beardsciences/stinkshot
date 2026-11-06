//! Reading and editing finished recordings: probe, thumbnails, frame_at, trim.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use stinkshot_core::Rgba;

use crate::convert::{bgra_to_nv12, fit_within, scale_4ch};
use crate::encoder::{Encoder, VideoParams};
use crate::mf::{MfSession, duration_to_hns, hns_to_duration};
use crate::mfread::{AudioChunk, Reader, VideoFrame};
use crate::{MediaInfo, timeline::pts_hns};

pub fn probe(input: &Path) -> Result<MediaInfo> {
    let rd = Reader::open(input, false, false)?;
    if !rd.has_video {
        bail!("{} has no video stream", input.display());
    }
    Ok(MediaInfo {
        duration: hns_to_duration(rd.duration_hns),
        width: rd.width,
        height: rd.height,
        fps: rd.fps,
        has_audio: rd.has_audio,
    })
}

pub fn bgra_to_rgba(mut v: Vec<u8>) -> Vec<u8> {
    for p in v.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    v
}

/// Decode the frame that is on screen at `hns` (the last frame whose span covers it).
pub(crate) fn decode_frame_at(rd: &mut Reader, hns: i64) -> Result<(VideoFrame, u32, u32)> {
    rd.seek(hns)?;
    let mut last: Option<VideoFrame> = None;
    let mut size = (rd.width, rd.height);
    for _ in 0..2000 {
        match rd.read_video()? {
            Some(f) => {
                size = (rd.width, rd.height);
                let covers = f.pts_hns + f.dur_hns.max(1) > hns;
                last = Some(f);
                if covers {
                    break;
                }
            }
            None => break,
        }
    }
    match last {
        Some(f) => Ok((f, size.0, size.1)),
        None => bail!("no video frame at {}", hns_to_duration(hns).as_secs_f32()),
    }
}

fn to_rgba_scaled(frame: VideoFrame, w: u32, h: u32, target_h: u32) -> Rgba {
    let (dw, dh) = fit_within(w, h, 0, target_h);
    let data = if (dw, dh) == (w, h) {
        frame.bgra
    } else {
        scale_4ch(
            &frame.bgra,
            w as usize,
            h as usize,
            dw as usize,
            dh as usize,
        )
    };
    Rgba::from_raw(dw, dh, bgra_to_rgba(data))
}

/// `count` evenly spaced frames scaled to `height` px tall.
pub fn thumbnails(input: &Path, count: usize, height: u32) -> Result<Vec<Rgba>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let mut rd = Reader::open(input, true, false)?;
    if !rd.has_video {
        bail!("{} has no video stream", input.display());
    }
    let dur = rd.duration_hns.max(1);
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let t = dur * (2 * i as i64 + 1) / (2 * count as i64);
        let (frame, w, h) = decode_frame_at(&mut rd, t)?;
        out.push(to_rgba_scaled(frame, w, h, height));
    }
    Ok(out)
}

/// Single frame at `at`.
pub fn frame_at(input: &Path, at: Duration, max_height: u32) -> Result<Rgba> {
    let mut rd = Reader::open(input, true, false)?;
    if !rd.has_video {
        bail!("{} has no video stream", input.display());
    }
    let mut t = duration_to_hns(at);
    if rd.duration_hns > 0 {
        // never ask for the very end: step back one frame so a picture exists
        let one_frame = if rd.fps > 0.0 {
            (1e7 / rd.fps as f64) as i64
        } else {
            333_333
        };
        t = t.min((rd.duration_hns - one_frame).max(0));
    }
    let (frame, w, h) = decode_frame_at(&mut rd, t)?;
    Ok(to_rgba_scaled(frame, w, h, max_height))
}

fn temp_for(output: &Path) -> PathBuf {
    let stem = output
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "trim".into());
    output.with_file_name(format!("{stem}.trim.tmp.mp4"))
}

/// Cut [start, end) into `output`, re-encoding video (frame accurate) and audio.
pub fn trim(
    input: &Path,
    output: &Path,
    start: Duration,
    end: Duration,
    progress: &dyn Fn(f32),
) -> Result<()> {
    if end <= start {
        bail!("trim range is empty");
    }
    if let (Ok(a), Ok(b)) = (std::fs::canonicalize(input), std::fs::canonicalize(output))
        && a == b
    {
        bail!("trim output must differ from the input file");
    }
    let _s = MfSession::new()?;
    let mut rd = Reader::open(input, true, true)?;
    if !rd.has_video {
        bail!("{} has no video stream", input.display());
    }
    let w = rd.width & !1;
    let h = rd.height & !1;
    if w < 2 || h < 2 {
        bail!("video is too small");
    }
    let fps = if rd.fps >= 1.0 {
        rd.fps.round() as u32
    } else {
        30
    }
    .clamp(1, 120);
    let size = std::fs::metadata(input).map(|m| m.len()).unwrap_or(0);
    let dur_s = (rd.duration_hns as f64 / 1e7).max(1.0);
    let src_kbps = (size as f64 * 8.0 / dur_s / 1000.0) as u32;
    let bitrate_kbps = src_kbps.clamp(2000, 60_000);

    if let Some(p) = output.parent()
        && !p.as_os_str().is_empty()
    {
        std::fs::create_dir_all(p).ok();
    }
    let temp = temp_for(output);
    let _ = std::fs::remove_file(&temp);
    let result = (|| -> Result<()> {
        let enc = Encoder::create(
            &temp,
            &VideoParams {
                width: w,
                height: h,
                fps,
                bitrate_kbps,
            },
            rd.has_audio,
        )?;
        let start_h = duration_to_hns(start);
        let end_h = duration_to_hns(end);
        rd.seek(start_h)?;
        let mut nv12 = vec![0u8; (w * h * 3 / 2) as usize];
        let mut pending_audio: Option<AudioChunk> = None;
        let mut audio_done = !rd.has_audio;
        let stride = rd.width as usize * 4;
        let one_frame = pts_hns(1, fps);
        let mut written_video = 0u64;
        let span = (end_h - start_h) as f32;

        while let Some(frame) = rd.read_video()? {
            let dur = if frame.dur_hns > 0 {
                frame.dur_hns
            } else {
                one_frame
            };
            if frame.pts_hns + dur <= start_h {
                continue;
            }
            if frame.pts_hns >= end_h {
                break;
            }
            let t = (frame.pts_hns - start_h).max(0);
            let d = dur.min(end_h - frame.pts_hns.max(start_h));
            if frame.bgra.len() < stride * h as usize {
                bail!("decoded frame has unexpected size");
            }
            bgra_to_nv12(&frame.bgra, stride, w as usize, h as usize, &mut nv12);
            enc.write_video(&nv12, t, d.max(1))?;
            written_video += 1;
            pump_audio(
                &mut rd,
                &enc,
                &mut pending_audio,
                &mut audio_done,
                start_h,
                end_h,
                Some(frame.pts_hns + dur),
            )?;
            progress(((frame.pts_hns - start_h) as f32 / span).clamp(0.0, 1.0) * 0.98);
        }
        if written_video == 0 {
            bail!("the selected range contains no video frames");
        }
        pump_audio(
            &mut rd,
            &enc,
            &mut pending_audio,
            &mut audio_done,
            start_h,
            end_h,
            None,
        )?;
        enc.finalize()?;
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

/// Write audio chunks whose start is before `until_abs` (None = to the end of the range).
fn pump_audio(
    rd: &mut Reader,
    enc: &Encoder,
    pending: &mut Option<AudioChunk>,
    done: &mut bool,
    start_h: i64,
    end_h: i64,
    until_abs: Option<i64>,
) -> Result<()> {
    while !*done {
        let chunk = match pending.take() {
            Some(c) => c,
            None => match rd.read_audio()? {
                Some(c) => c,
                None => {
                    *done = true;
                    break;
                }
            },
        };
        if let Some(limit) = until_abs
            && chunk.pts_hns >= limit
        {
            *pending = Some(chunk);
            break;
        }
        if chunk.pts_hns >= end_h {
            *done = true;
            break;
        }
        let chunk_end = chunk.pts_hns + chunk.dur_hns;
        if chunk_end <= start_h {
            continue;
        }
        // Trim head/tail to the range, on whole 4-byte frames.
        let frames_total = chunk.pcm.len() / 4;
        let to_frames = |hns: i64| (hns.max(0) as u128 * 48_000 / 10_000_000) as usize;
        let skip = if chunk.pts_hns < start_h {
            to_frames(start_h - chunk.pts_hns).min(frames_total)
        } else {
            0
        };
        let keep_end = if chunk_end > end_h {
            frames_total.saturating_sub(to_frames(chunk_end - end_h))
        } else {
            frames_total
        };
        if keep_end <= skip {
            continue;
        }
        let pcm = &chunk.pcm[skip * 4..keep_end * 4];
        let pts = chunk.pts_hns.max(start_h) - start_h;
        let dur = ((keep_end - skip) as u128 * 10_000_000 / 48_000) as i64;
        enc.write_audio(pcm, pts, dur)?;
    }
    Ok(())
}
