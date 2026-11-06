//! Audio DSP helpers (pure): sample format decoding, channel downmix, streaming sinc
//! resampler, mixer, float -> PCM16.

pub const OUT_RATE: u32 = 48_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleFormat {
    F32,
    I16,
    I24,
    I32,
}

impl SampleFormat {
    pub fn bytes(self) -> usize {
        match self {
            SampleFormat::F32 | SampleFormat::I32 => 4,
            SampleFormat::I24 => 3,
            SampleFormat::I16 => 2,
        }
    }
}

/// Decode interleaved packed audio of `channels` channels into stereo f32 appended to `out`.
pub fn decode_to_stereo(data: &[u8], fmt: SampleFormat, channels: usize, out: &mut Vec<f32>) {
    let channels = channels.max(1);
    let bps = fmt.bytes();
    let frame_bytes = bps * channels;
    let frames = data.len() / frame_bytes;
    out.reserve(frames * 2);
    let mut tmp = [0f32; 16];
    let n = channels.min(16);
    for f in 0..frames {
        let base = f * frame_bytes;
        for (c, slot) in tmp.iter_mut().enumerate().take(n) {
            let o = base + c * bps;
            *slot = match fmt {
                SampleFormat::F32 => {
                    f32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]])
                }
                SampleFormat::I16 => i16::from_le_bytes([data[o], data[o + 1]]) as f32 / 32768.0,
                SampleFormat::I24 => {
                    let v = ((data[o] as i32) << 8)
                        | ((data[o + 1] as i32) << 16)
                        | ((data[o + 2] as i32) << 24);
                    (v >> 8) as f32 / 8_388_608.0
                }
                SampleFormat::I32 => {
                    i32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]) as f32
                        / 2_147_483_648.0
                }
            };
        }
        let (l, r) = downmix(&tmp[..n]);
        out.push(l);
        out.push(r);
    }
}

/// Downmix one frame (WASAPI channel order: FL FR FC LFE BL BR SL SR) to stereo.
pub fn downmix(f: &[f32]) -> (f32, f32) {
    match f.len() {
        0 => (0.0, 0.0),
        1 => (f[0], f[0]),
        2 => (f[0], f[1]),
        _ => {
            let c = f.get(2).copied().unwrap_or(0.0) * 0.707;
            let (bl, br) = (
                f.get(4).copied().unwrap_or(0.0) * 0.707,
                f.get(5).copied().unwrap_or(0.0) * 0.707,
            );
            let (sl, sr) = (
                f.get(6).copied().unwrap_or(0.0) * 0.707,
                f.get(7).copied().unwrap_or(0.0) * 0.707,
            );
            let norm = 1.0
                / (1.0
                    + 0.707
                        * if f.len() > 6 {
                            3.0
                        } else if f.len() > 4 {
                            2.0
                        } else {
                            1.0
                        });
            (
                ((f[0] + c + bl + sl) * norm).clamp(-1.0, 1.0),
                ((f[1] + c + br + sr) * norm).clamp(-1.0, 1.0),
            )
        }
    }
}

const HALF_TAPS: f64 = 12.0;
const TABLE_STEPS: usize = 2048;

/// Streaming stereo resampler (windowed sinc, anti-aliased when downsampling). Input and
/// output are interleaved stereo f32.
pub struct Resampler {
    ratio: f64, // input samples per output sample
    scale: f64, // max(1, ratio): low-pass stretch when downsampling
    table: Vec<f32>,
    buf: Vec<f32>, // pending input, interleaved stereo
    base: usize,   // absolute input frame index of buf[0]
    pos: f64,      // absolute input position of the next output sample
    passthrough: bool,
}

impl Resampler {
    pub fn new(in_rate: u32, out_rate: u32) -> Self {
        let ratio = in_rate as f64 / out_rate as f64;
        let mut table = Vec::new();
        if in_rate != out_rate {
            table = (0..=TABLE_STEPS)
                .map(|i| {
                    let x = i as f64 / TABLE_STEPS as f64 * HALF_TAPS;
                    let sinc = if x == 0.0 {
                        1.0
                    } else {
                        (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
                    };
                    let win = 0.5 * (1.0 + (std::f64::consts::PI * x / HALF_TAPS).cos());
                    (sinc * win) as f32
                })
                .collect();
        }
        Self {
            ratio,
            scale: ratio.max(1.0),
            table,
            buf: Vec::new(),
            base: 0,
            pos: 0.0,
            passthrough: in_rate == out_rate,
        }
    }

    fn kernel(&self, d: f64) -> f32 {
        let x = (d.abs() / self.scale) / HALF_TAPS * TABLE_STEPS as f64;
        if x >= TABLE_STEPS as f64 {
            return 0.0;
        }
        let i = x as usize;
        let t = (x - i as f64) as f32;
        self.table[i] * (1.0 - t) + self.table[i + 1] * t
    }

    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.passthrough {
            out.extend_from_slice(input);
            return;
        }
        self.buf.extend_from_slice(input);
        let avail = self.buf.len() / 2;
        let support = HALF_TAPS * self.scale;
        loop {
            // need input up to pos + support
            let last_needed = self.pos + support;
            if last_needed >= (self.base + avail) as f64 {
                break;
            }
            let lo = ((self.pos - support).ceil().max(self.base as f64)) as usize;
            let hi = (self.pos + support).floor() as usize;
            let (mut l, mut r, mut wsum) = (0f32, 0f32, 0f32);
            for k in lo..=hi {
                let w = self.kernel(k as f64 - self.pos);
                let i = (k - self.base) * 2;
                l += self.buf[i] * w;
                r += self.buf[i + 1] * w;
                wsum += w;
            }
            if wsum.abs() > 1e-6 {
                l /= wsum;
                r /= wsum;
            }
            out.push(l);
            out.push(r);
            self.pos += self.ratio;
        }
        // Drop input that can no longer be referenced.
        let keep_from = ((self.pos - support).floor().max(0.0) as usize).max(self.base);
        let drop_frames = keep_from - self.base;
        if drop_frames > 0 {
            self.buf.drain(..drop_frames * 2);
            self.base = keep_from;
        }
    }
}

/// Sum `sources` (each interleaved stereo, same length) into `dst` with clipping.
pub fn mix(dst: &mut [f32], sources: &[&[f32]]) {
    for (i, d) in dst.iter_mut().enumerate() {
        let mut s = 0.0;
        for src in sources {
            s += src.get(i).copied().unwrap_or(0.0);
        }
        *d = s.clamp(-1.0, 1.0);
    }
}

/// Float -> little-endian signed 16-bit PCM bytes.
pub fn to_pcm16(samples: &[f32], out: &mut Vec<u8>) {
    out.reserve(samples.len() * 2);
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
}

/// Per-source FIFO of stereo frames that is pulled at the output clock. Underruns pad with
/// silence; a queue that runs further ahead than `max_lead_frames` (the device clock is
/// faster than ours) is trimmed from the oldest end to keep sync.
#[derive(Default)]
pub struct SourceQueue {
    q: std::collections::VecDeque<f32>,
}

impl SourceQueue {
    pub fn push(&mut self, stereo: &[f32]) {
        self.q.extend(stereo.iter().copied());
    }

    pub fn len_frames(&self) -> usize {
        self.q.len() / 2
    }

    pub fn clear(&mut self) {
        self.q.clear();
    }

    /// Pull exactly `frames` frames (interleaved) into `out`, padding with silence.
    pub fn pull(&mut self, frames: usize, max_lead_frames: usize, out: &mut Vec<f32>) {
        out.clear();
        let have = self.len_frames();
        let take = have.min(frames);
        out.extend(self.q.drain(..take * 2));
        out.resize(frames * 2, 0.0);
        let left = self.len_frames();
        if left > max_lead_frames {
            let drop = left - max_lead_frames;
            self.q.drain(..drop * 2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, freq: f64, frames: usize) -> Vec<f32> {
        let mut v = Vec::with_capacity(frames * 2);
        for i in 0..frames {
            let s = (2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64).sin() as f32 * 0.5;
            v.push(s);
            v.push(s);
        }
        v
    }

    fn dominant_freq(samples: &[f32], rate: u32) -> f64 {
        // zero crossings on the left channel
        let mut crossings = 0;
        let mut prev = samples[0];
        for i in 1..samples.len() / 2 {
            let s = samples[i * 2];
            if prev <= 0.0 && s > 0.0 {
                crossings += 1;
            }
            prev = s;
        }
        crossings as f64 / (samples.len() as f64 / 2.0 / rate as f64)
    }

    #[test]
    fn passthrough_is_identity() {
        let mut r = Resampler::new(48_000, 48_000);
        let input = sine(48_000, 440.0, 1000);
        let mut out = Vec::new();
        r.process(&input, &mut out);
        assert_eq!(out, input);
    }

    #[test]
    fn upsample_16k_to_48k_keeps_frequency_and_length() {
        let mut r = Resampler::new(16_000, 48_000);
        let input = sine(16_000, 500.0, 16_000);
        let mut out = Vec::new();
        // feed in odd-sized chunks to exercise streaming state
        for chunk in input.chunks(2 * 333) {
            r.process(chunk, &mut out);
        }
        let frames = out.len() / 2;
        assert!((frames as i64 - 48_000).abs() < 40, "frames={frames}");
        let f = dominant_freq(&out[2000..out.len() - 2000], 48_000);
        assert!((f - 500.0).abs() < 15.0, "f={f}");
        let peak = out[2000..].iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!((0.45..0.55).contains(&peak), "peak={peak}");
    }

    #[test]
    fn downsample_44k1_to_48k_ratio() {
        let mut r = Resampler::new(44_100, 48_000);
        let input = sine(44_100, 1000.0, 44_100);
        let mut out = Vec::new();
        r.process(&input, &mut out);
        let frames = out.len() / 2;
        assert!((frames as i64 - 48_000).abs() < 40, "frames={frames}");
        let f = dominant_freq(&out[2000..out.len() - 2000], 48_000);
        assert!((f - 1000.0).abs() < 20.0, "f={f}");
    }

    #[test]
    fn downsample_96k_attenuates_above_nyquist() {
        let mut r = Resampler::new(96_000, 48_000);
        // 30 kHz is above the 24 kHz output Nyquist: must be strongly attenuated
        let input = sine(96_000, 30_000.0, 20_000);
        let mut out = Vec::new();
        r.process(&input, &mut out);
        let peak = out[1000..].iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(peak < 0.1, "peak={peak}");
        // 5 kHz passes
        let mut r = Resampler::new(96_000, 48_000);
        let input = sine(96_000, 5_000.0, 20_000);
        let mut out = Vec::new();
        r.process(&input, &mut out);
        let peak = out[1000..].iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.45, "peak={peak}");
    }

    #[test]
    fn mixer_sums_and_clips() {
        let a = [0.5f32, -0.5, 0.9, 0.9];
        let b = [0.25f32, 0.25, 0.9, -0.2];
        let mut dst = [0f32; 4];
        mix(&mut dst, &[&a, &b]);
        assert_eq!(dst, [0.75, -0.25, 1.0, 0.7]);
        // shorter source treated as silence
        let c = [0.1f32];
        mix(&mut dst, &[&a, &c]);
        assert!((dst[0] - 0.6).abs() < 1e-6 && dst[1] == -0.5);
    }

    #[test]
    fn pcm16_conversion() {
        let mut out = Vec::new();
        to_pcm16(&[0.0, 1.0, -1.0, 2.0], &mut out);
        let v: Vec<i16> = out
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        assert_eq!(v, vec![0, 32767, -32767, 32767]);
    }

    #[test]
    fn decode_formats() {
        let mut out = Vec::new();
        let mut data = Vec::new();
        for s in [16384i16, -16384] {
            data.extend_from_slice(&s.to_le_bytes());
        }
        decode_to_stereo(&data, SampleFormat::I16, 2, &mut out);
        assert_eq!(out, vec![0.5, -0.5]);

        out.clear();
        let data = 0.25f32.to_le_bytes().to_vec();
        decode_to_stereo(&data, SampleFormat::F32, 1, &mut out);
        assert_eq!(out, vec![0.25, 0.25]);

        out.clear();
        // 24-bit: 0x400000 = 0.5
        decode_to_stereo(
            &[0x00, 0x00, 0x40, 0x00, 0x00, 0xC0],
            SampleFormat::I24,
            2,
            &mut out,
        );
        assert_eq!(out, vec![0.5, -0.5]);
    }

    #[test]
    fn downmix_surround_stays_bounded() {
        let (l, r) = downmix(&[1.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert!(l > 0.3 && l <= 1.0 && r == 0.0);
        let (l, r) = downmix(&[1.0; 8]);
        assert!(l <= 1.0 && r <= 1.0 && l > 0.9);
    }

    #[test]
    fn source_queue_pads_and_trims() {
        let mut q = SourceQueue::default();
        q.push(&[1.0, 1.0, 2.0, 2.0]);
        let mut out = Vec::new();
        q.pull(3, 10, &mut out);
        assert_eq!(out, vec![1.0, 1.0, 2.0, 2.0, 0.0, 0.0]);
        assert_eq!(q.len_frames(), 0);
        // lead trimming keeps the newest frames
        q.push(&(0..20).map(|i| i as f32).collect::<Vec<_>>()); // 10 frames
        q.pull(1, 4, &mut out);
        assert_eq!(out, vec![0.0, 1.0]);
        assert_eq!(q.len_frames(), 4);
        q.pull(1, 4, &mut out);
        assert_eq!(out, vec![12.0, 13.0]);
    }
}
