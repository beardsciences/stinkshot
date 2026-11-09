//! Media Foundation source reader helpers: decoding files to BGRA video frames and PCM audio.

use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::core::{GUID, HSTRING, Interface};

use crate::mf::{MfSession, pack, unpack};

const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
const AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
const ALL: u32 = MF_SOURCE_READER_ALL_STREAMS.0 as u32;
const MEDIASOURCE: u32 = MF_SOURCE_READER_MEDIASOURCE.0 as u32;

const FLAG_EOS: u32 = MF_SOURCE_READERF_ENDOFSTREAM.0 as u32;
const FLAG_TYPE_CHANGED: u32 = MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32;

pub struct VideoFrame {
    pub pts_hns: i64,
    pub dur_hns: i64,
    /// BGRA top-down, alpha forced to 255.
    pub bgra: Vec<u8>,
}

pub struct AudioChunk {
    pub pts_hns: i64,
    pub dur_hns: i64,
    /// 48 kHz stereo s16le
    pub pcm: Vec<u8>,
}

/// Copy a decoded RGB32 sample into a tight BGRA buffer (alpha = 255).
pub fn sample_to_bgra(sample: &IMFSample, w: usize, h: usize) -> Result<Vec<u8>> {
    unsafe {
        let buf = sample
            .ConvertToContiguousBuffer()
            .context("ConvertToContiguousBuffer")?;
        let mut out = vec![0u8; w * h * 4];
        if let Ok(b2d) = buf.cast::<IMF2DBuffer>() {
            let mut p: *mut u8 = std::ptr::null_mut();
            let mut pitch = 0i32;
            b2d.Lock2D(&mut p, &mut pitch).context("Lock2D")?;
            if p.is_null() {
                let _ = b2d.Unlock2D();
                bail!("null frame pointer");
            }
            for y in 0..h {
                let row = p.offset(y as isize * pitch as isize);
                std::ptr::copy_nonoverlapping(row, out.as_mut_ptr().add(y * w * 4), w * 4);
            }
            b2d.Unlock2D().context("Unlock2D")?;
        } else {
            let mut p: *mut u8 = std::ptr::null_mut();
            let mut cur = 0u32;
            buf.Lock(&mut p, None, Some(&mut cur))
                .context("buffer Lock")?;
            if (cur as usize) < w * h * 4 {
                let _ = buf.Unlock();
                bail!("short frame buffer ({cur} bytes for {w}x{h})");
            }
            std::ptr::copy_nonoverlapping(p, out.as_mut_ptr(), w * h * 4);
            buf.Unlock().context("buffer Unlock")?;
        }
        for px in out.chunks_exact_mut(4) {
            px[3] = 255;
        }
        Ok(out)
    }
}

/// Crop (or pad with black) a tight BGRA buffer from `sw` x `sh` to `dw` x `dh`, top-left
/// aligned. Returns the input unchanged when the sizes match.
fn crop_bgra(src: Vec<u8>, sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<u8> {
    if (sw, sh) == (dw, dh) {
        return src;
    }
    let mut out = vec![0u8; dw * dh * 4];
    let (cw, ch) = (sw.min(dw), sh.min(dh));
    for y in 0..ch {
        out[y * dw * 4..y * dw * 4 + cw * 4].copy_from_slice(&src[y * sw * 4..y * sw * 4 + cw * 4]);
    }
    for px in out.chunks_exact_mut(4) {
        px[3] = 255;
    }
    out
}

pub struct Reader {
    reader: IMFSourceReader,
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    pub duration_hns: i64,
    pub has_audio: bool,
    pub has_video: bool,
    /// Size of the decoded buffers. Decoders pad to whole macroblocks (566 wide becomes 576)
    /// and announce it mid-stream; `width`/`height` stay the picture size and frames are
    /// cropped back to it.
    dec_w: u32,
    dec_h: u32,
    _session: MfSession,
}

impl Reader {
    /// Open `path`. `video` decodes the first video stream to RGB32; `audio` decodes the
    /// first audio stream to 48 kHz stereo 16-bit PCM.
    pub fn open(path: &Path, video: bool, audio: bool) -> Result<Reader> {
        let session = MfSession::new()?;
        unsafe {
            let mut attrs: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attrs, 2)?;
            let attrs = attrs.context("no attributes")?;
            attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?;
            attrs.SetUINT32(&MF_SOURCE_READER_DISABLE_DXVA, 1)?;
            let url = HSTRING::from(path.as_os_str());
            let reader = MFCreateSourceReaderFromURL(&url, &attrs)
                .with_context(|| format!("cannot open {}", path.display()))?;

            let duration_hns = reader
                .GetPresentationAttribute(MEDIASOURCE, &MF_PD_DURATION)
                .ok()
                .and_then(|v: PROPVARIANT| u64::try_from(&v).ok())
                .unwrap_or(0) as i64;

            let native_video = reader.GetNativeMediaType(VIDEO, 0).ok();
            let has_video = native_video.is_some();
            let has_audio = reader.GetNativeMediaType(AUDIO, 0).is_ok();
            let (mut width, mut height, mut fps) = (0, 0, 0.0f32);
            if let Some(nt) = &native_video {
                let (w, h) = unpack(nt.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0));
                width = w;
                height = h;
                let (n, d) = unpack(nt.GetUINT64(&MF_MT_FRAME_RATE).unwrap_or(0));
                if d != 0 {
                    fps = n as f32 / d as f32;
                }
            }

            reader.SetStreamSelection(ALL, false)?;
            if video && has_video {
                reader.SetStreamSelection(VIDEO, true)?;
                let t = MFCreateMediaType()?;
                t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
                reader
                    .SetCurrentMediaType(VIDEO, None, &t)
                    .context("cannot decode video to RGB32")?;
                if let Ok(cur) = reader.GetCurrentMediaType(VIDEO) {
                    let (w, h) = unpack(
                        cur.GetUINT64(&MF_MT_FRAME_SIZE)
                            .unwrap_or(pack(width, height)),
                    );
                    width = w;
                    height = h;
                }
            }
            if audio && has_audio {
                reader.SetStreamSelection(AUDIO, true)?;
                let t = MFCreateMediaType()?;
                t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                t.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
                t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
                t.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, 48_000)?;
                t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
                t.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)?;
                t.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 48_000 * 4)?;
                reader
                    .SetCurrentMediaType(AUDIO, None, &t)
                    .context("cannot decode audio to PCM")?;
            }
            Ok(Reader {
                reader,
                width,
                height,
                dec_w: width,
                dec_h: height,
                fps,
                duration_hns,
                has_audio,
                has_video,
                _session: session,
            })
        }
    }

    pub fn seek(&self, hns: i64) -> Result<()> {
        unsafe {
            let v = PROPVARIANT::from(hns.max(0));
            self.reader
                .SetCurrentPosition(&GUID::zeroed(), &v)
                .context("seek")
        }
    }

    /// Next decoded video frame, or None at the end of the stream.
    pub fn read_video(&mut self) -> Result<Option<VideoFrame>> {
        loop {
            let (flags, ts, sample) = self.read(VIDEO)?;
            if flags & FLAG_TYPE_CHANGED != 0 {
                unsafe {
                    if let Ok(cur) = self.reader.GetCurrentMediaType(VIDEO) {
                        let (w, h) = unpack(cur.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0));
                        if w > 0 && h > 0 {
                            self.dec_w = w;
                            self.dec_h = h;
                            if self.width == 0 || self.height == 0 {
                                self.width = w;
                                self.height = h;
                            }
                        }
                    }
                }
            }
            if flags & FLAG_EOS != 0 && sample.is_none() {
                return Ok(None);
            }
            let Some(sample) = sample else {
                if flags & FLAG_EOS != 0 {
                    return Ok(None);
                }
                continue;
            };
            let dur = unsafe { sample.GetSampleDuration() }.unwrap_or(0);
            let decoded = sample_to_bgra(&sample, self.dec_w as usize, self.dec_h as usize)?;
            let bgra = crop_bgra(
                decoded,
                self.dec_w as usize,
                self.dec_h as usize,
                self.width as usize,
                self.height as usize,
            );
            return Ok(Some(VideoFrame {
                pts_hns: ts,
                dur_hns: dur,
                bgra,
            }));
        }
    }

    /// Next decoded audio chunk, or None at the end of the stream.
    pub fn read_audio(&mut self) -> Result<Option<AudioChunk>> {
        loop {
            let (flags, ts, sample) = self.read(AUDIO)?;
            let Some(sample) = sample else {
                if flags & FLAG_EOS != 0 {
                    return Ok(None);
                }
                continue;
            };
            unsafe {
                let buf = sample.ConvertToContiguousBuffer().context("audio buffer")?;
                let mut p: *mut u8 = std::ptr::null_mut();
                let mut cur = 0u32;
                buf.Lock(&mut p, None, Some(&mut cur))
                    .context("audio lock")?;
                let pcm = std::slice::from_raw_parts(p, cur as usize).to_vec();
                buf.Unlock().ok();
                let dur = sample
                    .GetSampleDuration()
                    .unwrap_or((pcm.len() as i64 / 4) * 10_000_000 / 48_000);
                return Ok(Some(AudioChunk {
                    pts_hns: ts,
                    dur_hns: dur,
                    pcm,
                }));
            }
        }
    }

    fn read(&self, stream: u32) -> Result<(u32, i64, Option<IMFSample>)> {
        let mut flags = 0u32;
        let mut ts = 0i64;
        let mut sample: Option<IMFSample> = None;
        unsafe {
            self.reader
                .ReadSample(
                    stream,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut ts),
                    Some(&mut sample),
                )
                .map_err(|e| anyhow!("ReadSample failed: {e}"))?;
        }
        Ok((flags, ts, sample))
    }
}

#[cfg(test)]
mod crop_tests {
    use super::crop_bgra;

    #[test]
    fn padded_decode_is_cropped_to_the_picture() {
        // 3x2 picture decoded into 4x2 buffers (one padding column per row).
        let mut src = Vec::new();
        for y in 0..2u8 {
            for x in 0..4u8 {
                src.extend_from_slice(&[x, y, 9, 255]);
            }
        }
        let out = crop_bgra(src, 4, 2, 3, 2);
        assert_eq!(out.len(), 3 * 2 * 4);
        for y in 0..2usize {
            for x in 0..3usize {
                assert_eq!(
                    &out[(y * 3 + x) * 4..(y * 3 + x) * 4 + 2],
                    &[x as u8, y as u8]
                );
            }
        }
    }

    #[test]
    fn same_size_is_untouched() {
        let src = vec![1, 2, 3, 255, 4, 5, 6, 255];
        assert_eq!(crop_bgra(src.clone(), 2, 1, 2, 1), src);
    }
}
