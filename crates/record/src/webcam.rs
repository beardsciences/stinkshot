//! Webcam capture through a Media Foundation source reader. Keeps only the latest frame.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::CoTaskMemFree;

use crate::mf::{MfSession, activate_name, pack, unpack};
use crate::mfread::sample_to_bgra;
use crate::overlay::WebcamFrame;

fn enum_activates() -> Result<Vec<IMFActivate>> {
    unsafe {
        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let attrs = attrs.context("no attributes")?;
        attrs.SetGUID(
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
        )?;
        let mut arr: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0u32;
        MFEnumDeviceSources(&attrs, &mut arr, &mut count).context("MFEnumDeviceSources")?;
        let mut out = Vec::new();
        if !arr.is_null() {
            for i in 0..count as usize {
                if let Some(a) = (*arr.add(i)).take() {
                    out.push(a);
                }
            }
            CoTaskMemFree(Some(arr as *const _));
        }
        Ok(out)
    }
}

/// Friendly names of attached video capture devices.
pub fn list_names() -> Vec<String> {
    let Ok(_s) = MfSession::new() else {
        return Vec::new();
    };
    let Ok(acts) = enum_activates() else {
        return Vec::new();
    };
    acts.iter()
        .filter_map(|a| activate_name(a, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME))
        .collect()
}

pub struct Webcam {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Webcam {
    /// Open the camera (None = first device) and start a reader thread that keeps `latest`
    /// updated.
    pub fn start(device: Option<&str>, latest: Arc<Mutex<Option<WebcamFrame>>>) -> Result<Webcam> {
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<Result<(), String>>();
        let device = device.map(str::to_string);
        let stop2 = stop.clone();
        let handle = std::thread::Builder::new()
            .name("record-webcam".into())
            .spawn(move || {
                if let Err(e) = run(device.as_deref(), &latest, &stop2, &tx) {
                    let msg = format!("{e:#}");
                    if tx.send(Err(msg.clone())).is_err() {
                        log::warn!("webcam stopped: {msg}");
                    }
                }
            })
            .context("spawn webcam thread")?;
        match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(Ok(())) => Ok(Webcam {
                stop,
                handle: Some(handle),
            }),
            Ok(Err(e)) => {
                let _ = handle.join();
                Err(anyhow!(e))
            }
            Err(_) => {
                stop.store(true, Ordering::Relaxed);
                Err(anyhow!("webcam initialisation timed out"))
            }
        }
    }
}

impl Drop for Webcam {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn run(
    device: Option<&str>,
    latest: &Arc<Mutex<Option<WebcamFrame>>>,
    stop: &Arc<AtomicBool>,
    ready: &mpsc::Sender<Result<(), String>>,
) -> Result<()> {
    let _mf = MfSession::new()?;
    let acts = enum_activates()?;
    if acts.is_empty() {
        bail!("no webcam found");
    }
    let act = match device {
        Some(name) => acts
            .iter()
            .find(|a| {
                activate_name(a, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME).as_deref() == Some(name)
            })
            .unwrap_or_else(|| {
                log::warn!("webcam {name:?} not found, using the first device");
                &acts[0]
            }),
        None => &acts[0],
    };
    unsafe {
        let source: IMFMediaSource = act.ActivateObject().context("activate webcam")?;
        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 2)?;
        let attrs = attrs.context("no attributes")?;
        attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?;
        let reader =
            MFCreateSourceReaderFromMediaSource(&source, &attrs).context("webcam source reader")?;
        let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

        // Prefer a native mode that is at least 640 px wide but not huge.
        let mut best: Option<(u32, IMFMediaType)> = None;
        let mut i = 0;
        while let Ok(t) = reader.GetNativeMediaType(stream, i) {
            let (w, _) = unpack(t.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0));
            if w >= 640 && best.as_ref().is_none_or(|(bw, _)| w < *bw) {
                best = Some((w, t));
            }
            i += 1;
        }
        if let Some((_, t)) = &best {
            let _ = reader.SetCurrentMediaType(stream, None, t);
        }
        let out = MFCreateMediaType()?;
        out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        reader
            .SetCurrentMediaType(stream, None, &out)
            .context("webcam RGB32 output")?;
        let cur = reader.GetCurrentMediaType(stream)?;
        let (w, h) = unpack(cur.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(pack(0, 0)));
        if w == 0 || h == 0 {
            bail!("webcam reports no frame size");
        }
        let _ = ready.send(Ok(()));

        while !stop.load(Ordering::Relaxed) {
            let mut flags = 0u32;
            let mut sample: Option<IMFSample> = None;
            reader
                .ReadSample(stream, 0, None, Some(&mut flags), None, Some(&mut sample))
                .context("webcam ReadSample")?;
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
            let Some(sample) = sample else { continue };
            match sample_to_bgra(&sample, w as usize, h as usize) {
                Ok(bgra) => {
                    if let Ok(mut l) = latest.lock() {
                        *l = Some(WebcamFrame {
                            width: w,
                            height: h,
                            bgra,
                        });
                    }
                }
                Err(e) => log::debug!("webcam frame skipped: {e:#}"),
            }
        }
        let _ = source.Shutdown();
    }
    Ok(())
}
