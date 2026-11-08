//! Clipboard. Images are placed as CF_DIBV5 (with alpha) + registered "PNG" format +
//! CF_DIB fallback, so both alpha-aware apps (Office, Discord, browsers) and old apps work.
//! Retries OpenClipboard briefly if another process holds it.

use stinkshot_core::Rgba;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT};
use windows::core::w;

use crate::util::{wide, wide_os};

struct ClipboardGuard;

impl ClipboardGuard {
    /// Open with retries: another process (clipboard managers, remote desktop) may hold it.
    fn open() -> anyhow::Result<ClipboardGuard> {
        let mut last = None;
        for attempt in 0..25 {
            // SAFETY: a null owner window associates the clipboard with the current task.
            match unsafe { OpenClipboard(None) } {
                Ok(()) => return Ok(ClipboardGuard),
                Err(e) => {
                    last = Some(e);
                    std::thread::sleep(std::time::Duration::from_millis(10 + attempt * 4));
                }
            }
        }
        Err(anyhow::anyhow!(
            "OpenClipboard failed: {}",
            last.map(|e| e.to_string()).unwrap_or_default()
        ))
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        // SAFETY: balanced with a successful OpenClipboard on this thread.
        if let Err(e) = unsafe { CloseClipboard() } {
            log::debug!("CloseClipboard: {e}");
        }
    }
}

/// Place `bytes` on the (already open) clipboard under `format`.
fn put(format: u32, bytes: &[u8]) -> anyhow::Result<()> {
    // SAFETY: the HGLOBAL is filled while locked; ownership transfers to the clipboard on
    // success and is freed here on failure.
    unsafe {
        let h: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))?;
        let p = GlobalLock(h);
        if p.is_null() {
            let _ = GlobalFree(Some(h));
            anyhow::bail!("GlobalLock failed");
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p as *mut u8, bytes.len());
        let _ = GlobalUnlock(h);
        if let Err(e) = SetClipboardData(format, Some(HANDLE(h.0))) {
            let _ = GlobalFree(Some(h));
            anyhow::bail!("SetClipboardData({format}): {e}");
        }
    }
    Ok(())
}

fn png_format() -> u32 {
    // SAFETY: static NUL-terminated name.
    unsafe { RegisterClipboardFormatW(w!("PNG")) }
}

fn name_format(name: &str) -> u32 {
    let n = wide(name);
    // SAFETY: NUL-terminated buffer outlives the call.
    unsafe { RegisterClipboardFormatW(windows::core::PCWSTR(n.as_ptr())) }
}

fn le(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn le_i(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn le16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Bottom-up BGRA rows, straight alpha.
fn bgra_bottom_up(img: &Rgba) -> Vec<u8> {
    let (w, h) = (img.width as usize, img.height as usize);
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let src = &img.data[y * w * 4..(y + 1) * w * 4];
        let dst = &mut out[(h - 1 - y) * w * 4..(h - y) * w * 4];
        for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
            d[3] = s[3];
        }
    }
    out
}

/// CF_DIBV5 payload: BITMAPV5HEADER (124 bytes) + bottom-up BGRA with straight alpha.
pub(crate) fn build_dibv5(img: &Rgba) -> Vec<u8> {
    let pixels = bgra_bottom_up(img);
    let mut out = Vec::with_capacity(124 + pixels.len());
    le(&mut out, 124); // bV5Size
    le_i(&mut out, img.width as i32);
    le_i(&mut out, img.height as i32); // positive = bottom-up
    le16(&mut out, 1); // planes
    le16(&mut out, 32); // bit count
    le(&mut out, 3); // BI_BITFIELDS
    le(&mut out, pixels.len() as u32);
    le_i(&mut out, 3780); // 96 dpi in px/m
    le_i(&mut out, 3780);
    le(&mut out, 0); // clr used
    le(&mut out, 0); // clr important
    le(&mut out, 0x00FF_0000); // red mask
    le(&mut out, 0x0000_FF00); // green mask
    le(&mut out, 0x0000_00FF); // blue mask
    le(&mut out, 0xFF00_0000); // alpha mask
    le(&mut out, 0x7352_4742); // LCS_sRGB ('sRGB')
    out.extend_from_slice(&[0u8; 36]); // CIEXYZTRIPLE endpoints
    le(&mut out, 0); // gamma red
    le(&mut out, 0); // gamma green
    le(&mut out, 0); // gamma blue
    le(&mut out, 4); // LCS_GM_IMAGES
    le(&mut out, 0); // profile data
    le(&mut out, 0); // profile size
    le(&mut out, 0); // reserved
    debug_assert_eq!(out.len(), 124);
    out.extend_from_slice(&pixels);
    out
}

/// CF_DIB payload: BITMAPINFOHEADER + 32bpp BI_RGB, composited over white (the plain DIB
/// format has no reliable alpha, and transparent black would otherwise show up as a black box).
pub(crate) fn build_dib(img: &Rgba) -> Vec<u8> {
    let mut pixels = bgra_bottom_up(img);
    for p in pixels.chunks_exact_mut(4) {
        let a = p[3] as u32;
        if a != 255 {
            for c in &mut p[..3] {
                *c = ((*c as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
            }
            p[3] = 255;
        }
    }
    let mut out = Vec::with_capacity(40 + pixels.len());
    le(&mut out, 40);
    le_i(&mut out, img.width as i32);
    le_i(&mut out, img.height as i32);
    le16(&mut out, 1);
    le16(&mut out, 32);
    le(&mut out, 0); // BI_RGB
    le(&mut out, pixels.len() as u32);
    le_i(&mut out, 3780);
    le_i(&mut out, 3780);
    le(&mut out, 0);
    le(&mut out, 0);
    out.extend_from_slice(&pixels);
    out
}

/// The fast PNG path (`Rgba::to_png`): the png crate's default level took about 1.8 s for a
/// 4K screenshot, during which the clipboard did not have the image yet; the fast path takes
/// about 0.1 s (sizes do not matter on the clipboard). Nov 8 2026.
fn encode_png(img: &Rgba) -> anyhow::Result<Vec<u8>> {
    img.to_png()
}

/// The clipboard formats of an image, built before the clipboard is opened so other programs
/// are locked out only while they are copied in.
fn image_payloads(img: &Rgba) -> anyhow::Result<Vec<(u32, Vec<u8>)>> {
    if img.width == 0 || img.height == 0 {
        anyhow::bail!("cannot copy an empty image");
    }
    let mut v = vec![(CF_DIBV5.0 as u32, build_dibv5(img))];
    match encode_png(img) {
        Ok(png) => v.push((png_format(), png)),
        Err(e) => log::warn!("PNG clipboard format skipped: {e}"),
    }
    v.push((CF_DIB.0 as u32, build_dib(img)));
    Ok(v)
}

fn put_all(payloads: &[(u32, Vec<u8>)]) -> anyhow::Result<()> {
    for (format, bytes) in payloads {
        put(*format, bytes)?;
    }
    Ok(())
}

pub fn set_image(img: &Rgba) -> anyhow::Result<()> {
    let payloads = image_payloads(img)?;
    let _g = ClipboardGuard::open()?;
    // SAFETY: clipboard is open on this thread.
    unsafe { EmptyClipboard() }?;
    put_all(&payloads)
}

pub fn set_text(text: &str) -> anyhow::Result<()> {
    let _g = ClipboardGuard::open()?;
    // SAFETY: clipboard is open on this thread.
    unsafe { EmptyClipboard() }?;
    let w = wide(text);
    let bytes: Vec<u8> = w.iter().flat_map(|c| c.to_le_bytes()).collect();
    put(CF_UNICODETEXT.0 as u32, &bytes)
}

/// DROPFILES payload for CF_HDROP (wide paths, double-NUL terminated).
pub(crate) fn build_hdrop(path: &std::path::Path) -> Vec<u8> {
    let mut out = Vec::new();
    le(&mut out, 20); // pFiles: offset of the list
    le_i(&mut out, 0); // pt.x
    le_i(&mut out, 0); // pt.y
    le(&mut out, 0); // fNC
    le(&mut out, 1); // fWide
    for c in wide_os(path.as_os_str()) {
        out.extend_from_slice(&c.to_le_bytes());
    }
    out.extend_from_slice(&0u16.to_le_bytes()); // list terminator
    out
}

/// Also used by "copy file" from history: CF_HDROP with the path, plus the image formats.
pub fn set_file_and_image(path: &std::path::Path, img: Option<&Rgba>) -> anyhow::Result<()> {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let payloads = img.map(image_payloads).transpose()?;
    let _g = ClipboardGuard::open()?;
    // SAFETY: clipboard is open on this thread.
    unsafe { EmptyClipboard() }?;
    put(CF_HDROP.0 as u32, &build_hdrop(&abs))?;
    // DROPEFFECT_COPY, so paste in Explorer copies rather than moves.
    put(name_format("Preferred DropEffect"), &1u32.to_le_bytes())?;
    if let Some(p) = &payloads {
        put_all(p)?;
    }
    Ok(())
}

fn read_global(format: u32) -> Option<Vec<u8>> {
    // SAFETY: clipboard is open; the HGLOBAL belongs to the clipboard and is only read while locked.
    unsafe {
        let h = GetClipboardData(format).ok()?;
        let hg = HGLOBAL(h.0);
        let size = GlobalSize(hg);
        let p = GlobalLock(hg);
        if p.is_null() {
            return None;
        }
        let v = std::slice::from_raw_parts(p as *const u8, size).to_vec();
        let _ = GlobalUnlock(hg);
        Some(v)
    }
}

/// Current clipboard text, if any.
pub fn get_text() -> Option<String> {
    let _g = ClipboardGuard::open().ok()?;
    // SAFETY: trivial query.
    if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32) }.is_err() {
        return None;
    }
    let bytes = read_global(CF_UNICODETEXT.0 as u32)?;
    let w: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect();
    Some(crate::util::from_wide(&w))
}

/// Current clipboard image decoded from the "PNG" format, if present.
pub fn get_image() -> Option<Rgba> {
    let _g = ClipboardGuard::open().ok()?;
    let png = read_global(png_format())?;
    let img = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
        .ok()?
        .to_rgba8();
    Some(Rgba::from_raw(img.width(), img.height(), img.into_raw()))
}

/// True when the clipboard currently offers `CF_DIBV5`.
pub fn has_dibv5() -> bool {
    let Ok(_g) = ClipboardGuard::open() else {
        return false;
    };
    // SAFETY: trivial query.
    unsafe { IsClipboardFormatAvailable(CF_DIBV5.0 as u32) }.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dibv5_layout() {
        let img = Rgba::filled(3, 2, [10, 20, 30, 128]);
        let d = build_dibv5(&img);
        assert_eq!(d.len(), 124 + 3 * 2 * 4);
        assert_eq!(&d[0..4], &124u32.to_le_bytes());
        // BGRA straight alpha
        assert_eq!(&d[124..128], &[30, 20, 10, 128]);
    }

    #[test]
    fn dibv5_is_bottom_up() {
        let mut img = Rgba::new(1, 2);
        img.set_pixel(0, 0, [1, 2, 3, 255]);
        img.set_pixel(0, 1, [4, 5, 6, 255]);
        let d = build_dibv5(&img);
        assert_eq!(&d[124..128], &[6, 5, 4, 255]);
        assert_eq!(&d[128..132], &[3, 2, 1, 255]);
    }

    #[test]
    fn dib_composites_over_white() {
        let img = Rgba::filled(1, 1, [0, 0, 0, 0]);
        let d = build_dib(&img);
        assert_eq!(&d[40..44], &[255, 255, 255, 255]);
    }

    #[test]
    fn hdrop_layout() {
        let d = build_hdrop(std::path::Path::new("C:\\a.png"));
        assert_eq!(&d[0..4], &20u32.to_le_bytes());
        assert_eq!(&d[16..20], &1u32.to_le_bytes());
        assert_eq!(d.len(), 20 + ("C:\\a.png".len() + 2) * 2);
        assert_eq!(&d[d.len() - 4..], &[0, 0, 0, 0]);
    }
}
