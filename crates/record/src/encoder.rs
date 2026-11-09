//! MP4 H.264 + AAC writer on top of IMFSinkWriter.

use std::path::Path;

use anyhow::{Context, Result};
use windows::Win32::Media::MediaFoundation::*;
use windows::core::{GUID, HSTRING, Interface};

use crate::mf::{make_sample, pack};

#[derive(Clone, Copy, Debug)]
pub struct VideoParams {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
}

pub const AUDIO_RATE: u32 = 48_000;
pub const AUDIO_CHANNELS: u32 = 2;
pub const AUDIO_KBPS: u32 = 192;
/// Highest frame rate recorded without a hardware encoder: the software MFT falls behind
/// above it at full-screen sizes (and 4K at 120 FPS is beyond H.264 level 5.2).
pub const MAX_SOFTWARE_FPS: u32 = 60;

pub struct Encoder {
    writer: IMFSinkWriter,
    video_stream: u32,
    audio_stream: Option<u32>,
    pub encoder_name: String,
    /// The frame rate the stream was opened with (`MAX_SOFTWARE_FPS` when a higher one
    /// needed a hardware encoder that was not there).
    pub fps: u32,
    /// The MFT doing the work is a hardware encoder.
    pub hardware: bool,
}

// The sink writer is free threaded; callers serialise access with a mutex anyway.
unsafe impl Send for Encoder {}
unsafe impl Sync for Encoder {}

impl Encoder {
    /// Open a writer, trying the hardware encoder first and falling back to software. Above
    /// `MAX_SOFTWARE_FPS` only a hardware encoder is used; without one (or when it refuses
    /// the rate) the stream is opened at `MAX_SOFTWARE_FPS` instead (see `Encoder::fps`).
    pub fn create(path: &Path, vp: &VideoParams, audio: bool) -> Result<Encoder> {
        if vp.fps > MAX_SOFTWARE_FPS {
            match Self::create_with(path, vp, audio, true) {
                Ok(e) if e.hardware => return Ok(e),
                Ok(e) => log::warn!(
                    "{} fps needs a hardware encoder, got {}; recording at {MAX_SOFTWARE_FPS} fps",
                    vp.fps,
                    e.encoder_name
                ),
                Err(e) => log::warn!(
                    "hardware H.264 encoder refused {} fps ({e:#}); recording at {MAX_SOFTWARE_FPS} fps",
                    vp.fps
                ),
            }
            let _ = std::fs::remove_file(path);
            let lower = VideoParams {
                fps: MAX_SOFTWARE_FPS,
                ..*vp
            };
            return Self::create_any(path, &lower, audio);
        }
        Self::create_any(path, vp, audio)
    }

    fn create_any(path: &Path, vp: &VideoParams, audio: bool) -> Result<Encoder> {
        match Self::create_with(path, vp, audio, true) {
            Ok(e) => Ok(e),
            Err(hw_err) => {
                log::warn!(
                    "hardware H.264 encoder unavailable ({hw_err:#}); falling back to software"
                );
                let _ = std::fs::remove_file(path);
                Self::create_with(path, vp, audio, false)
                    .with_context(|| format!("hardware attempt failed first: {hw_err:#}"))
            }
        }
    }

    fn create_with(path: &Path, vp: &VideoParams, audio: bool, hardware: bool) -> Result<Encoder> {
        unsafe {
            let mut attrs: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attrs, 4).context("MFCreateAttributes")?;
            let attrs = attrs.context("no attributes")?;
            if hardware {
                attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
            }
            attrs.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)?;
            let url = HSTRING::from(path.as_os_str());
            let writer = MFCreateSinkWriterFromURL(&url, None, &attrs)
                .context("MFCreateSinkWriterFromURL")?;

            // Video: output H.264, input NV12.
            let out = MFCreateMediaType()?;
            out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            out.SetUINT32(&MF_MT_AVG_BITRATE, vp.bitrate_kbps.max(100) * 1000)?;
            out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            out.SetUINT64(&MF_MT_FRAME_SIZE, pack(vp.width, vp.height))?;
            out.SetUINT64(&MF_MT_FRAME_RATE, pack(vp.fps, 1))?;
            out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            let _ = out.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32);
            let _ = out.SetUINT32(&MF_MT_MAX_KEYFRAME_SPACING, vp.fps * 2);
            let video_stream = writer.AddStream(&out).context("AddStream(video)")?;

            let inp = MFCreateMediaType()?;
            inp.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            inp.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
            inp.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            inp.SetUINT64(&MF_MT_FRAME_SIZE, pack(vp.width, vp.height))?;
            inp.SetUINT64(&MF_MT_FRAME_RATE, pack(vp.fps, 1))?;
            inp.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            inp.SetUINT32(&MF_MT_DEFAULT_STRIDE, vp.width)?;
            inp.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32)?;
            inp.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32)?;
            writer
                .SetInputMediaType(video_stream, &inp, None)
                .context("SetInputMediaType(video NV12)")?;

            let audio_stream = if audio {
                let aout = MFCreateMediaType()?;
                aout.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                aout.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
                aout.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
                aout.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, AUDIO_RATE)?;
                aout.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, AUDIO_CHANNELS)?;
                aout.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, AUDIO_KBPS * 1000 / 8)?;
                aout.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)?;
                aout.SetUINT32(&MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, 0x29)?;
                let idx = writer.AddStream(&aout).context("AddStream(audio)")?;
                let ain = MFCreateMediaType()?;
                ain.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
                ain.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
                ain.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
                ain.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, AUDIO_RATE)?;
                ain.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, AUDIO_CHANNELS)?;
                ain.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, AUDIO_CHANNELS * 2)?;
                ain.SetUINT32(
                    &MF_MT_AUDIO_AVG_BYTES_PER_SECOND,
                    AUDIO_RATE * AUDIO_CHANNELS * 2,
                )?;
                writer
                    .SetInputMediaType(idx, &ain, None)
                    .context("SetInputMediaType(audio PCM)")?;
                Some(idx)
            } else {
                None
            };

            writer.BeginWriting().context("BeginWriting")?;
            let (encoder_name, is_hw, level) = describe_encoder(&writer, video_stream, hardware);
            log::info!(
                "H.264 encoder {encoder_name}: {}x{} @ {} fps, level {}",
                vp.width,
                vp.height,
                vp.fps,
                level.map_or_else(|| "not reported".to_string(), |l| l.to_string())
            );
            Ok(Encoder {
                writer,
                video_stream,
                audio_stream,
                encoder_name,
                fps: vp.fps,
                hardware: is_hw,
            })
        }
    }

    pub fn write_video(&self, nv12: &[u8], pts_hns: i64, dur_hns: i64) -> Result<()> {
        let sample = make_sample(nv12, pts_hns, dur_hns)?;
        unsafe { self.writer.WriteSample(self.video_stream, &sample) }.context("WriteSample(video)")
    }

    /// `pcm` is 48 kHz stereo signed 16-bit little endian.
    pub fn write_audio(&self, pcm: &[u8], pts_hns: i64, dur_hns: i64) -> Result<()> {
        let Some(stream) = self.audio_stream else {
            return Ok(());
        };
        if pcm.is_empty() {
            return Ok(());
        }
        let sample = make_sample(pcm, pts_hns, dur_hns)?;
        unsafe { self.writer.WriteSample(stream, &sample) }.context("WriteSample(audio)")
    }

    pub fn finalize(&self) -> Result<()> {
        let r = unsafe { self.writer.Finalize() }.context("sink writer Finalize");
        self.shutdown_parts();
        r
    }

    /// Shut down the encoder MFT and media sink explicitly: releasing the writer alone left
    /// the hardware encoder's session objects (events, sections, mutants) behind per recording.
    fn shutdown_parts(&self) {
        unsafe {
            let mut raw: *mut core::ffi::c_void = std::ptr::null_mut();
            let got = self.writer.GetServiceForStream(
                self.video_stream,
                &GUID::zeroed(),
                &IMFTransform::IID,
                &mut raw,
            );
            if got.is_ok() && !raw.is_null() {
                let mft = IMFTransform::from_raw(raw);
                if let Ok(s) = mft.cast::<IMFShutdown>() {
                    let _ = s.Shutdown();
                }
            }
            let mut raw: *mut core::ffi::c_void = std::ptr::null_mut();
            let got = self.writer.GetServiceForStream(
                MF_SINK_WRITER_MEDIASINK.0,
                &GUID::zeroed(),
                &IMFMediaSink::IID,
                &mut raw,
            );
            if got.is_ok() && !raw.is_null() {
                let sink = IMFMediaSink::from_raw(raw);
                let _ = sink.Shutdown();
            }
        }
    }
}

/// Display name of the stream's encoder MFT, whether it is a hardware encoder, and the
/// H.264 level of its output type (eAVEncH264VLevel value, e.g. 51 for 5.1) when reported.
fn describe_encoder(
    writer: &IMFSinkWriter,
    stream: u32,
    requested_hw: bool,
) -> (String, bool, Option<u32>) {
    unsafe {
        let mut raw: *mut core::ffi::c_void = std::ptr::null_mut();
        let got = writer.GetServiceForStream(stream, &GUID::zeroed(), &IMFTransform::IID, &mut raw);
        if got.is_ok() && !raw.is_null() {
            let mft = IMFTransform::from_raw(raw);
            let level = mft
                .GetOutputCurrentType(0)
                .ok()
                .and_then(|t| t.GetUINT32(&MF_MT_MPEG2_LEVEL).ok());
            if let Ok(attrs) = mft.GetAttributes() {
                let name = attr_string(&attrs, &MFT_FRIENDLY_NAME_Attribute);
                let hw_url = attr_string(&attrs, &MFT_ENUM_HARDWARE_URL_Attribute).is_some();
                let is_async = attrs.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) == 1;
                let hw = requested_hw && (hw_url || is_async);
                let base = name.unwrap_or_else(|| "H.264 Encoder MFT".to_string());
                let label = format!("{base} ({})", if hw { "hardware" } else { "software" });
                return (label, hw, level);
            }
        }
    }
    let label = format!(
        "H.264 Encoder MFT ({})",
        if requested_hw {
            "hardware requested"
        } else {
            "software"
        }
    );
    // Unknown: only a confirmed hardware MFT may run above MAX_SOFTWARE_FPS.
    (label, false, None)
}

fn attr_string(attrs: &IMFAttributes, key: &GUID) -> Option<String> {
    unsafe {
        let mut p = windows::core::PWSTR::null();
        let mut len = 0u32;
        attrs.GetAllocatedString(key, &mut p, &mut len).ok()?;
        if p.is_null() {
            return None;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p.0, len as usize));
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
        Some(s)
    }
}
