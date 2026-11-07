//! HDR (advanced color) displays.
//!
//! When Windows HDR is on for a display, the desktop is composed in linear scRGB FP16 where
//! 1.0 = 80 nits, and SDR content (every normal window) is placed at the user's "SDR content
//! brightness" white level: an sRGB value `v` ends up at `srgb_decode(v) * white_nits / 80`.
//! Capturing such a display as 8-bit (GDI, or Desktop Duplication asking for BGRA8) does not
//! undo that scale, so captures look brighter / washed out compared to the screen.
//!
//! This module finds the per-display white level (`DisplayConfigGetDeviceInfo`) and converts
//! FP16 scRGB back to 8-bit sRGB so SDR content maps 1:1 to its original values. Content above
//! SDR white (HDR video, games) cannot be shown in 8-bit SDR without changing SDR whites too,
//! so it is rolled off toward white along its own hue instead of being clipped per channel
//! (which would shift hues, for example bright orange turning yellow).

use std::sync::{Arc, Mutex, OnceLock};

use windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
    DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SDR_WHITE_LEVEL,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes,
    QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows::Win32::Foundation::ERROR_SUCCESS;

/// scRGB 1.0 in nits.
pub const SCRGB_UNIT_NITS: f32 = 80.0;
/// Windows' default "SDR content brightness" (used when the query fails on an HDR display).
pub const DEFAULT_SDR_WHITE_NITS: f32 = 240.0;

/// Color state of one display.
#[derive(Clone, Debug, PartialEq)]
pub struct DisplayColor {
    /// GDI device name (`\\.\DISPLAY1`), matching `DXGI_OUTPUT_DESC::DeviceName` and
    /// `MONITORINFOEXW::szDevice`.
    pub device: String,
    /// Advanced color (HDR, or auto color management) is active: the desktop is composed in
    /// FP16 scRGB.
    pub advanced_color: bool,
    /// Where SDR white sits on this display, in nits (80 when advanced color is off).
    pub sdr_white_nits: f32,
}

impl DisplayColor {
    /// scRGB value of SDR white on this display.
    pub fn white_scale(&self) -> f32 {
        (self.sdr_white_nits / SCRGB_UNIT_NITS).max(0.01)
    }
}

fn wide_to_string(w: &[u16]) -> String {
    let n = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..n])
}

/// Color state of every active display (DisplayConfig API). Empty when the query fails.
pub fn displays() -> Vec<DisplayColor> {
    // SAFETY: buffers are sized by GetDisplayConfigBufferSizes and every info packet carries
    // its own correct size in the header.
    unsafe {
        let mut np = 0u32;
        let mut nm = 0u32;
        let mut paths: Vec<DISPLAYCONFIG_PATH_INFO> = Vec::new();
        // The topology can change between the two calls; retry a few times.
        let mut ok = false;
        for _ in 0..3 {
            if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) != ERROR_SUCCESS
            {
                return Vec::new();
            }
            paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
            let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
            let r = QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut np,
                paths.as_mut_ptr(),
                &mut nm,
                modes.as_mut_ptr(),
                None,
            );
            if r == ERROR_SUCCESS {
                ok = true;
                break;
            }
        }
        if !ok {
            return Vec::new();
        }
        paths.truncate(np as usize);
        let mut out = Vec::new();
        for p in &paths {
            let mut src = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
                header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                    size: std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                    adapterId: p.sourceInfo.adapterId,
                    id: p.sourceInfo.id,
                },
                ..Default::default()
            };
            if DisplayConfigGetDeviceInfo(&mut src.header) != 0 {
                continue;
            }
            let mut aci = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO {
                header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
                    size: std::mem::size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32,
                    adapterId: p.targetInfo.adapterId,
                    id: p.targetInfo.id,
                },
                ..Default::default()
            };
            // Bit 1 of the flags is advancedColorEnabled.
            let advanced =
                DisplayConfigGetDeviceInfo(&mut aci.header) == 0 && aci.Anonymous.value & 0x2 != 0;
            let mut white = DISPLAYCONFIG_SDR_WHITE_LEVEL {
                header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SDR_WHITE_LEVEL,
                    size: std::mem::size_of::<DISPLAYCONFIG_SDR_WHITE_LEVEL>() as u32,
                    adapterId: p.targetInfo.adapterId,
                    id: p.targetInfo.id,
                },
                ..Default::default()
            };
            let white_nits =
                if DisplayConfigGetDeviceInfo(&mut white.header) == 0 && white.SDRWhiteLevel > 0 {
                    // SDRWhiteLevel is a multiplier of 80 nits, in thousandths.
                    white.SDRWhiteLevel as f32 / 1000.0 * SCRGB_UNIT_NITS
                } else if advanced {
                    DEFAULT_SDR_WHITE_NITS
                } else {
                    SCRGB_UNIT_NITS
                };
            out.push(DisplayColor {
                device: wide_to_string(&src.viewGdiDeviceName),
                advanced_color: advanced,
                sdr_white_nits: if advanced {
                    white_nits
                } else {
                    SCRGB_UNIT_NITS
                },
            });
        }
        out
    }
}

/// Color state of the display with GDI device name `device` (`\\.\DISPLAY1`). Unknown
/// devices are reported as plain SDR.
pub fn display_for_device(device: &str) -> DisplayColor {
    displays()
        .into_iter()
        .find(|d| d.device.eq_ignore_ascii_case(device))
        .unwrap_or_else(|| DisplayColor {
            device: device.to_string(),
            advanced_color: false,
            sdr_white_nits: SCRGB_UNIT_NITS,
        })
}

/// Color state of the display showing monitor `hmonitor` (raw `HMONITOR`).
pub fn display_for_monitor(hmonitor: isize) -> DisplayColor {
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, HMONITOR, MONITORINFO, MONITORINFOEXW};
    let mut mi = MONITORINFOEXW {
        monitorInfo: MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    // SAFETY: cbSize announces the EX struct, which is what the pointer refers to.
    let ok = unsafe {
        GetMonitorInfoW(
            HMONITOR(hmonitor as *mut _),
            &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO,
        )
    }
    .as_bool();
    if !ok {
        return DisplayColor {
            device: String::new(),
            advanced_color: false,
            sdr_white_nits: SCRGB_UNIT_NITS,
        };
    }
    display_for_device(&wide_to_string(&mi.szDevice))
}

/// GDI device name from a fixed `[u16; 32]` buffer (`DXGI_OUTPUT_DESC::DeviceName`).
pub fn device_name(w: &[u16]) -> String {
    wide_to_string(w)
}

// ---------------------------------------------------------------------------------------
// Conversion math (pure, unit tested)
// ---------------------------------------------------------------------------------------

/// IEEE half to f32.
pub fn half_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0f32 } else { 1.0 };
    let exp = ((h >> 10) & 0x1f) as i32;
    let man = (h & 0x3ff) as f32;
    if exp == 0 {
        sign * man * 2f32.powi(-24)
    } else if exp == 31 {
        if man == 0.0 {
            sign * f32::INFINITY
        } else {
            f32::NAN
        }
    } else {
        sign * (1.0 + man / 1024.0) * 2f32.powi(exp - 15)
    }
}

/// f32 to IEEE half, round to nearest even (overflow to infinity, NaN kept).
pub fn f32_to_half(v: f32) -> u16 {
    let x = v.to_bits();
    let sign = ((x >> 16) & 0x8000) as u16;
    let exp = ((x >> 23) & 0xff) as i32;
    let man = x & 0x7f_ffff;
    if exp == 0xff {
        return sign | 0x7c00 | if man != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 31 {
        return sign | 0x7c00;
    }
    if e <= 0 {
        // Subnormal half (or zero).
        if e < -10 {
            return sign;
        }
        let m = man | 0x80_0000;
        let shift = (14 - e) as u32;
        let half = m >> shift;
        let rem = m & ((1 << shift) - 1);
        let mid = 1 << (shift - 1);
        let round = rem > mid || (rem == mid && half & 1 == 1);
        return sign | (half + round as u32) as u16;
    }
    let half = ((e as u32) << 10) | (man >> 13);
    let rem = man & 0x1fff;
    let round = rem > 0x1000 || (rem == 0x1000 && half & 1 == 1);
    // A carry out of the mantissa correctly bumps the exponent.
    sign | (half + round as u32) as u16
}

/// sRGB byte -> FP16 scRGB bit pattern as Windows composes SDR content on a display whose
/// SDR white is `sdr_white_nits`.
pub fn srgb8_to_scrgb_half(sdr_white_nits: f32) -> [u16; 256] {
    let scale = (sdr_white_nits / SCRGB_UNIT_NITS).max(0.01);
    std::array::from_fn(|i| f32_to_half(srgb_decode(i as f32 / 255.0) * scale))
}

/// sRGB OETF (linear 0..1 to encoded 0..1).
pub fn srgb_encode(l: f32) -> f32 {
    let l = if l.is_nan() { 0.0 } else { l.clamp(0.0, 1.0) };
    if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    }
}

/// sRGB EOTF (encoded 0..1 to linear 0..1).
pub fn srgb_decode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// How fast over-white colors approach white: at `1 + ROLLOFF_K` times SDR white a
/// saturated color is half way to white.
const ROLLOFF_K: f32 = 3.0;

/// Map linear RGB relative to SDR white (1.0 = SDR white) into 0..=1.
///
/// Colors whose largest channel is at most 1.0 (all SDR content) pass through unchanged.
/// Brighter colors keep their hue: they are scaled so the largest channel is 1.0 and then
/// blended toward white by `t = (m - 1) / (m - 1 + K)`, so brightness keeps rising smoothly
/// with the input instead of clipping per channel.
pub fn rolloff(rgb: [f32; 3]) -> [f32; 3] {
    let c = rgb.map(|v| if v.is_nan() { 0.0 } else { v.max(0.0) });
    let m = c[0].max(c[1]).max(c[2]);
    if m <= 1.0 {
        return c;
    }
    if !m.is_finite() {
        return [1.0; 3];
    }
    let t = (m - 1.0) / (m - 1.0 + ROLLOFF_K);
    c.map(|v| (v / m) * (1.0 - t) + t)
}

/// Rec.709 relative luminance of linear RGB.
pub fn luminance(rgb: [f32; 3]) -> f32 {
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

fn half_f32_lut() -> &'static [f32] {
    static LUT: OnceLock<Vec<f32>> = OnceLock::new();
    LUT.get_or_init(|| (0..=u16::MAX).map(half_to_f32).collect())
}

/// Fine sRGB encode table over linear 0..=1 (for the roll-off path).
const FINE: usize = 16384;
fn srgb_fine_lut() -> &'static [u8] {
    static LUT: OnceLock<Vec<u8>> = OnceLock::new();
    LUT.get_or_init(|| {
        (0..=FINE)
            .map(|i| (srgb_encode(i as f32 / FINE as f32) * 255.0 + 0.5) as u8)
            .collect()
    })
}

fn encode8(l: f32) -> u8 {
    srgb_fine_lut()[(l.clamp(0.0, 1.0) * FINE as f32 + 0.5) as usize]
}

/// FP16 scRGB to 8-bit sRGB converter for one SDR white level.
pub struct ScrgbToSrgb {
    /// 1 / (scRGB value of SDR white).
    inv_white: f32,
    /// Half bit pattern -> sRGB byte, valid for values at or below SDR white.
    lut: Vec<u8>,
    /// Largest positive half bit pattern that is at or below SDR white.
    max_sdr_bits: i16,
}

impl ScrgbToSrgb {
    pub fn new(sdr_white_nits: f32) -> ScrgbToSrgb {
        let white = (sdr_white_nits / SCRGB_UNIT_NITS).max(0.01);
        let inv_white = 1.0 / white;
        let halves = half_f32_lut();
        let lut: Vec<u8> = halves
            .iter()
            .map(|&v| {
                let l = v * inv_white;
                if l.is_nan() || l <= 0.0 {
                    0
                } else {
                    (srgb_encode(l.min(1.0)) * 255.0 + 0.5) as u8
                }
            })
            .collect();
        // Positive finite halves are ordered like their bit patterns (0x0000..0x7BFF).
        let mut max_sdr_bits = 0i16;
        for bits in 0..0x7c00u16 {
            if halves[bits as usize] * inv_white <= 1.0 {
                max_sdr_bits = bits as i16;
            } else {
                break;
            }
        }
        ScrgbToSrgb {
            inv_white,
            lut,
            max_sdr_bits,
        }
    }

    /// Shared converter for `sdr_white_nits` (tables are built once per white level).
    pub fn shared(sdr_white_nits: f32) -> Arc<ScrgbToSrgb> {
        static CACHE: Mutex<Vec<(u32, Arc<ScrgbToSrgb>)>> = Mutex::new(Vec::new());
        let key = sdr_white_nits.to_bits();
        let mut cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((_, c)) = cache.iter().find(|(k, _)| *k == key) {
            return Arc::clone(c);
        }
        let c = Arc::new(ScrgbToSrgb::new(sdr_white_nits));
        if cache.len() >= 8 {
            cache.remove(0);
        }
        cache.push((key, Arc::clone(&c)));
        c
    }

    /// Convert one pixel (half bit patterns R, G, B) to sRGB bytes.
    #[inline(always)]
    pub fn pixel(&self, r: u16, g: u16, b: u16) -> [u8; 3] {
        let m = self.max_sdr_bits;
        // Negative halves are negative as i16 and take the fast path (clamped to 0).
        if (r as i16) <= m && (g as i16) <= m && (b as i16) <= m {
            [
                self.lut[r as usize],
                self.lut[g as usize],
                self.lut[b as usize],
            ]
        } else {
            self.pixel_slow(r, g, b)
        }
    }

    #[cold]
    fn pixel_slow(&self, r: u16, g: u16, b: u16) -> [u8; 3] {
        let h = half_f32_lut();
        let c = rolloff([
            h[r as usize] * self.inv_white,
            h[g as usize] * self.inv_white,
            h[b as usize] * self.inv_white,
        ]);
        c.map(encode8)
    }

    /// Convert rows of `R16G16B16A16_FLOAT` pixels into tightly packed 4-byte pixels.
    /// `bgra` selects the output byte order (BGRA when true, RGBA otherwise); alpha is 255.
    pub fn convert_rows(&self, src: &[u8], pitch: usize, w: usize, dst: &mut [u8], bgra: bool) {
        for (y, drow) in dst.chunks_exact_mut(w * 4).enumerate() {
            let srow = &src[y * pitch..][..w * 8];
            for (d, s) in drow.chunks_exact_mut(4).zip(srow.chunks_exact(8)) {
                let r = u16::from_le_bytes([s[0], s[1]]);
                let g = u16::from_le_bytes([s[2], s[3]]);
                let b = u16::from_le_bytes([s[4], s[5]]);
                let p = self.pixel(r, g, b);
                if bgra {
                    d[0] = p[2];
                    d[1] = p[1];
                    d[2] = p[0];
                } else {
                    d[..3].copy_from_slice(&p);
                }
                d[3] = 255;
            }
        }
    }

    /// Like `convert_rows`, but writes each row at `dst_pitch` (for mapped textures) and
    /// splits the work across threads for large frames.
    #[allow(clippy::too_many_arguments)]
    pub fn convert_parallel(
        &self,
        src: &[u8],
        src_pitch: usize,
        w: usize,
        h: usize,
        dst: &mut [u8],
        dst_pitch: usize,
        bgra: bool,
    ) {
        if w == 0 || h == 0 {
            return;
        }
        let threads = if w * h < 300_000 {
            1
        } else {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(2)
                .clamp(1, 8)
        };
        let rows_per = h.div_ceil(threads);
        let work = |src: &[u8], dst: &mut [u8], rows: usize| {
            for y in 0..rows {
                let s = &src[y * src_pitch..];
                let d = &mut dst[y * dst_pitch..][..w * 4];
                self.convert_rows(s, src_pitch, w, d, bgra);
            }
        };
        if threads == 1 {
            work(src, dst, h);
            return;
        }
        std::thread::scope(|s| {
            let mut rest = dst;
            let mut y0 = 0;
            while y0 < h {
                let rows = rows_per.min(h - y0);
                // The last band may be shorter than a full pitch at its end.
                let take = if y0 + rows == h {
                    rest.len()
                } else {
                    rows * dst_pitch
                };
                let (band, tail) = rest.split_at_mut(take);
                rest = tail;
                let src = &src[y0 * src_pitch..];
                s.spawn(move || work(src, band, rows));
                y0 += rows;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_conversions_round_trip() {
        assert_eq!(half_to_f32(0x3C00), 1.0);
        assert_eq!(half_to_f32(0xC000), -2.0);
        assert!((half_to_f32(0x3800) - 0.5).abs() < 1e-6);
        // every finite half survives f32 and back
        for b in (0..0x7c00u16).chain(0x8000..0xfc00) {
            assert_eq!(f32_to_half(half_to_f32(b)), b, "{b:#x}");
        }
        assert_eq!(f32_to_half(1e6), 0x7c00);
        assert_eq!(f32_to_half(f32::NEG_INFINITY), 0xfc00);
        assert!(half_to_f32(f32_to_half(f32::NAN)).is_nan());
        // nearest rounding: 1 + 1/2048 is a tie between 1.0 and 1 + 1/1024 -> even (1.0)
        assert_eq!(f32_to_half(1.0 + 1.0 / 2048.0), 0x3C00);
        assert_eq!(f32_to_half(1.0 + 1.5 / 2048.0), 0x3C01);
    }

    #[test]
    fn gdi_seed_table_round_trips_through_converter() {
        for white in [80.0f32, 240.0, 480.0] {
            let t = srgb8_to_scrgb_half(white);
            let c = ScrgbToSrgb::new(white);
            for (v, &h) in t.iter().enumerate() {
                assert_eq!(c.pixel(h, h, h)[0] as usize, v, "white {white}");
            }
        }
    }

    #[test]
    fn srgb_round_trip() {
        for v in 0..=255u32 {
            let l = srgb_decode(v as f32 / 255.0);
            let back = (srgb_encode(l) * 255.0 + 0.5) as u32;
            assert_eq!(back, v);
        }
        assert_eq!(srgb_encode(0.0), 0.0);
        assert!((srgb_encode(1.0) - 1.0).abs() < 1e-6);
        assert_eq!(srgb_encode(f32::NAN), 0.0);
        assert!((srgb_encode(0.5) * 255.0 - 187.5).abs() < 1.0);
    }

    #[test]
    fn white_level_mapping_is_identity_for_sdr_content() {
        // SDR content as Windows composes it: decode, scale by white/80, store as FP16.
        for white in [80.0f32, 200.0, 240.0, 280.0, 480.0] {
            let conv = ScrgbToSrgb::new(white);
            for v in [0u8, 1, 10, 0x40, 0x6A, 0x7F, 0x80, 0xD3, 0xFE, 0xFF] {
                let lin = srgb_decode(v as f32 / 255.0) * white / 80.0;
                let h = f32_to_half(lin);
                let out = conv.pixel(h, h, h);
                assert!(
                    (out[0] as i32 - v as i32).abs() <= 1,
                    "white {white}: {v} -> {}",
                    out[0]
                );
            }
        }
    }

    #[test]
    fn sdr_white_is_255_and_doubling_white_halves_light() {
        let conv = ScrgbToSrgb::new(240.0);
        let w = f32_to_half(3.0);
        assert_eq!(conv.pixel(w, w, w), [255, 255, 255]);
        // half of SDR white in linear light is sRGB ~188
        let half = f32_to_half(1.5);
        assert!((conv.pixel(half, half, half)[0] as i32 - 188).abs() <= 1);
        // negative (out of gamut) and NaN clamp to 0
        assert_eq!(conv.pixel(0xBC00, 0x7E00, 0)[0], 0);
        assert_eq!(conv.pixel(0xBC00, 0x7E00, 0)[1], 0);
    }

    #[test]
    fn rolloff_passes_sdr_and_is_monotonic_above_white() {
        assert_eq!(rolloff([0.2, 0.5, 1.0]), [0.2, 0.5, 1.0]);
        assert_eq!(rolloff([-0.1, 0.0, 0.3]), [0.0, 0.0, 0.3]);
        for hue in [
            [1.0, 0.0, 0.0],
            [1.0, 0.5, 0.1],
            [0.3, 0.9, 0.4],
            [1.0, 1.0, 1.0],
        ] {
            let mut prev_l = 0.0f32;
            let mut prev_sat = f32::INFINITY;
            for i in 0..400 {
                let k = 0.05 + i as f32 * 0.05; // 0.05x .. 20x SDR white
                let o = rolloff(hue.map(|c: f32| c * k));
                assert!(o.iter().all(|&c| (0.0..=1.0).contains(&c)));
                let l = luminance(o);
                assert!(l + 1e-6 >= prev_l, "luminance fell at {k} for {hue:?}");
                prev_l = l;
                let mx = o[0].max(o[1]).max(o[2]);
                let mn = o[0].min(o[1]).min(o[2]);
                let sat = if mx > 0.0 { (mx - mn) / mx } else { 0.0 };
                if k > 1.0 {
                    assert!(sat <= prev_sat + 1e-6, "saturation rose at {k}");
                    prev_sat = sat;
                }
            }
            // continuous at white
            let a = rolloff(hue.map(|c: f32| c * 1.0));
            let b = rolloff(hue.map(|c: f32| c * 1.0001));
            for i in 0..3 {
                assert!((a[i] - b[i]).abs() < 1e-3);
            }
        }
        // the dominant channel stays the dominant one (hue kept, no per-channel clip)
        let o = rolloff([3.0, 1.5, 0.3]);
        assert!(o[0] >= o[1] && o[1] >= o[2]);
        assert_eq!(rolloff([f32::INFINITY, 0.0, 0.0]), [1.0; 3]);
    }

    #[test]
    fn hdr_highlights_are_not_hard_clipped() {
        let conv = ScrgbToSrgb::new(240.0);
        // Bright orange at 2x and 6x SDR white: still orange-ish, blue rises gradually.
        let px = |k: f32| {
            let [r, g, b] = [1.0f32, 0.4, 0.05].map(|c| f32_to_half(c * 3.0 * k));
            conv.pixel(r, g, b)
        };
        let (a, b, c) = (px(1.0), px(2.0), px(6.0));
        assert_eq!(a[0], 255);
        assert!(a[2] < b[2] && b[2] < c[2]);
        assert!(c[0] >= c[1] && c[1] >= c[2]);
    }

    #[test]
    fn row_conversion_and_parallel_bands() {
        let conv = ScrgbToSrgb::new(80.0);
        let (w, h) = (640usize, 500usize);
        let pitch = w * 8 + 16;
        let mut src = vec![0u8; pitch * h];
        for y in 0..h {
            for x in 0..w {
                let o = y * pitch + x * 8;
                let v = f32_to_half(((x + y) % 256) as f32 / 255.0);
                for c in 0..3 {
                    src[o + c * 2..o + c * 2 + 2].copy_from_slice(&v.to_le_bytes());
                }
            }
        }
        let mut serial = vec![0u8; w * h * 4];
        conv.convert_rows(&src, pitch, w, &mut serial, true);
        let dpitch = w * 4 + 12;
        let mut par = vec![0u8; dpitch * (h - 1) + w * 4];
        conv.convert_parallel(&src, pitch, w, h, &mut par, dpitch, true);
        for y in 0..h {
            assert_eq!(&par[y * dpitch..][..w * 4], &serial[y * w * 4..][..w * 4]);
        }
        assert_eq!(serial[3], 255);
    }
}
