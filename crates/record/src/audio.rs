//! WASAPI capture (system loopback and microphone) feeding `SourceQueue`s with 48 kHz
//! stereo float.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, STGM_READ};
use windows::core::GUID;

use crate::dsp::{OUT_RATE, Resampler, SampleFormat, SourceQueue, decode_to_stereo};
use crate::mf::ComGuard;

#[derive(Clone, Debug)]
pub enum Source {
    /// Default render device, captured in loopback mode.
    Loopback,
    /// Capture device by friendly name (None = default).
    Microphone(Option<String>),
}

struct Format {
    rate: u32,
    channels: usize,
    sample: SampleFormat,
}

unsafe fn parse_format(p: *const WAVEFORMATEX) -> Result<Format> {
    unsafe {
        let wf = std::ptr::read_unaligned(p);
        let tag = wf.wFormatTag;
        let bits = wf.wBitsPerSample;
        let mut kind = tag as u32; // 1 = PCM, 3 = float
        if tag == 0xFFFE && wf.cbSize >= 22 {
            // WAVEFORMATEXTENSIBLE: SubFormat GUID follows 8 more bytes after the header
            let sub = std::ptr::read_unaligned(
                (p as *const u8).add(std::mem::size_of::<WAVEFORMATEX>() + 6) as *const GUID,
            );
            kind = sub.data1;
        }
        let sample = match (kind, bits) {
            (3, 32) => SampleFormat::F32,
            (1, 16) => SampleFormat::I16,
            (1, 24) => SampleFormat::I24,
            (1, 32) => SampleFormat::I32,
            _ => return Err(anyhow!("unsupported audio format tag {kind} / {bits} bits")),
        };
        Ok(Format {
            rate: wf.nSamplesPerSec,
            channels: wf.nChannels as usize,
            sample,
        })
    }
}

fn device_name(dev: &IMMDevice) -> Option<String> {
    unsafe {
        let store = dev.OpenPropertyStore(STGM_READ).ok()?;
        let v = store.GetValue(&PKEY_Device_FriendlyName).ok()?;
        let s = v.to_string();
        (!s.is_empty()).then_some(s)
    }
}

/// Friendly names of active capture endpoints.
pub fn list_capture_devices() -> Vec<String> {
    let _com = ComGuard::new();
    let mut out = Vec::new();
    unsafe {
        let Ok(en) =
            CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
        else {
            return out;
        };
        let Ok(coll) = en.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE) else {
            return out;
        };
        let n = coll.GetCount().unwrap_or(0);
        for i in 0..n {
            if let Ok(d) = coll.Item(i)
                && let Some(name) = device_name(&d)
            {
                out.push(name);
            }
        }
    }
    out
}

unsafe fn open_device(en: &IMMDeviceEnumerator, source: &Source) -> Result<IMMDevice> {
    unsafe {
        match source {
            Source::Loopback => en
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .context("no default playback device"),
            Source::Microphone(None) => en
                .GetDefaultAudioEndpoint(eCapture, eConsole)
                .context("no default microphone"),
            Source::Microphone(Some(name)) => {
                let coll = en.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)?;
                for i in 0..coll.GetCount()? {
                    let d = coll.Item(i)?;
                    if device_name(&d).as_deref() == Some(name.as_str()) {
                        return Ok(d);
                    }
                }
                log::warn!("microphone {name:?} not found, using the default device");
                en.GetDefaultAudioEndpoint(eCapture, eConsole)
                    .context("no default microphone")
            }
        }
    }
}

/// Start a capture thread. Returns after the device is initialised (or failed to).
pub fn spawn(
    source: Source,
    queue: Arc<Mutex<SourceQueue>>,
    stop: Arc<AtomicBool>,
    fatal: Arc<Mutex<Option<String>>>,
) -> Result<JoinHandle<()>> {
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let stop_on_failure = stop.clone();
    let name = format!("record-audio-{source:?}");
    let handle = std::thread::Builder::new()
        .name(name)
        .spawn(move || {
            let _com = ComGuard::new();
            match run(&source, &queue, &stop, &tx) {
                Ok(()) => {}
                Err(e) => {
                    let msg = format!("{e:#}");
                    if tx.send(Err(msg.clone())).is_err() {
                        // already past init: surface as a fatal recording error
                        log::error!("audio capture failed: {msg}");
                        if let Ok(mut f) = fatal.lock() {
                            f.get_or_insert(format!("audio capture failed: {msg}"));
                        }
                    }
                }
            }
        })
        .context("spawn audio thread")?;
    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => Ok(handle),
        Ok(Err(e)) => {
            let _ = handle.join();
            Err(anyhow!(e))
        }
        Err(_) => {
            // Nobody can reach the thread's stop flag after this returns; set it so the
            // thread exits instead of capturing forever if the device finally opens.
            stop_on_failure.store(true, Ordering::Relaxed);
            Err(anyhow!("audio device initialisation timed out"))
        }
    }
}

/// How often a lost audio device is looked for again.
const REOPEN_EVERY: Duration = Duration::from_millis(500);

/// A started capture stream.
struct Stream {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    fmt: Format,
}

fn open(source: &Source) -> Result<Stream> {
    unsafe {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
            .context("MMDeviceEnumerator")?;
        let dev = open_device(&en, source)?;
        let client: IAudioClient = dev.Activate(CLSCTX_ALL, None).context("IAudioClient")?;
        let mix = client.GetMixFormat().context("GetMixFormat")?;
        let fmt = parse_format(mix);
        let flags = if matches!(source, Source::Loopback) {
            AUDCLNT_STREAMFLAGS_LOOPBACK
        } else {
            0
        };
        let init = client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 2_000_000, 0, mix, None);
        CoTaskMemFree(Some(mix as *const _));
        let fmt = fmt?;
        init.context("IAudioClient::Initialize")?;
        let capture: IAudioCaptureClient = client.GetService().context("IAudioCaptureClient")?;
        client.Start().context("IAudioClient::Start")?;
        log::info!(
            "audio capture {source:?}: {} Hz, {} ch, {:?}",
            fmt.rate,
            fmt.channels,
            fmt.sample
        );
        Ok(Stream {
            client,
            capture,
            fmt,
        })
    }
}

/// Capture until `stop`. Failing to open the device at the start is an error; once capture
/// has begun, losing the device (unplugged, disabled, default changed under a loopback) is
/// not: that source goes silent and the device is looked for again, so the recording goes on.
fn run(
    source: &Source,
    queue: &Arc<Mutex<SourceQueue>>,
    stop: &Arc<AtomicBool>,
    ready: &mpsc::Sender<Result<(), String>>,
) -> Result<()> {
    let mut stream = open(source)?;
    let _ = ready.send(Ok(()));
    loop {
        match pump(&stream, queue, stop) {
            Ok(()) => return Ok(()),
            Err(e) => log::warn!("audio capture {source:?} lost its device, retrying: {e:#}"),
        }
        drop(stream);
        stream = loop {
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            std::thread::sleep(REOPEN_EVERY);
            match open(source) {
                Ok(s) => break s,
                Err(e) => log::debug!("audio capture {source:?} still unavailable: {e:#}"),
            }
        };
    }
}

fn pump(stream: &Stream, queue: &Arc<Mutex<SourceQueue>>, stop: &Arc<AtomicBool>) -> Result<()> {
    let Stream {
        client,
        capture,
        fmt,
    } = stream;
    unsafe {
        let mut resampler = Resampler::new(fmt.rate, OUT_RATE);
        let frame_bytes = fmt.sample.bytes() * fmt.channels;
        let mut stereo = Vec::<f32>::new();
        let mut resampled = Vec::<f32>::new();
        let silent_flag = AUDCLNT_BUFFERFLAGS_SILENT.0 as u32;
        while !stop.load(Ordering::Relaxed) {
            let mut got_any = false;
            loop {
                let pending = capture.GetNextPacketSize().context("GetNextPacketSize")?;
                if pending == 0 {
                    break;
                }
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                    .context("GetBuffer")?;
                stereo.clear();
                if frames > 0 {
                    if flags & silent_flag != 0 || data.is_null() {
                        stereo.resize(frames as usize * 2, 0.0);
                    } else {
                        let bytes = std::slice::from_raw_parts(data, frames as usize * frame_bytes);
                        decode_to_stereo(bytes, fmt.sample, fmt.channels, &mut stereo);
                    }
                }
                capture.ReleaseBuffer(frames).context("ReleaseBuffer")?;
                if !stereo.is_empty() {
                    resampled.clear();
                    resampler.process(&stereo, &mut resampled);
                    if let Ok(mut q) = queue.lock() {
                        q.push(&resampled);
                    }
                    got_any = true;
                }
            }
            if !got_any {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        let _ = client.Stop();
    }
    Ok(())
}
