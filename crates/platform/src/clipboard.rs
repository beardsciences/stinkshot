//! Clipboard. Images are placed as CF_DIBV5 (with alpha) + registered "PNG" format +
//! CF_DIB fallback, so both alpha-aware apps (Office, Discord, browsers) and old apps work.
//! Retries OpenClipboard briefly if another process holds it.

use stinkshot_core::Rgba;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber,
    IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
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

/// A filled global memory block, freed on drop unless the clipboard took it.
struct Block(HGLOBAL);

impl Block {
    fn new(bytes: &[u8]) -> anyhow::Result<Block> {
        // SAFETY: the HGLOBAL is filled while locked and freed here on failure.
        unsafe {
            let h: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))?;
            let p = GlobalLock(h);
            if p.is_null() {
                let _ = GlobalFree(Some(h));
                anyhow::bail!("GlobalLock failed");
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), p as *mut u8, bytes.len());
            let _ = GlobalUnlock(h);
            Ok(Block(h))
        }
    }

    /// Hand the block to the (already open) clipboard under `format`.
    fn set(self, format: u32) -> anyhow::Result<()> {
        // SAFETY: ownership transfers to the clipboard on success; on failure Drop frees it.
        unsafe { SetClipboardData(format, Some(HANDLE(self.0.0))) }
            .map_err(|e| anyhow::anyhow!("SetClipboardData({format}): {e}"))?;
        std::mem::forget(self);
        Ok(())
    }
}

impl Drop for Block {
    fn drop(&mut self) {
        // SAFETY: the block was never given to the clipboard.
        let _ = unsafe { GlobalFree(Some(self.0)) };
    }
}

/// Place `bytes` on the (already open) clipboard under `format`.
fn put(format: u32, bytes: &[u8]) -> anyhow::Result<()> {
    Block::new(bytes)?.set(format)
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

/// Copy every payload into global memory. Done before the clipboard is emptied, so running
/// out of memory on a huge image leaves what the user had there.
fn blocks(payloads: Vec<(u32, Vec<u8>)>) -> anyhow::Result<Vec<(u32, Block)>> {
    payloads
        .into_iter()
        .map(|(format, bytes)| Ok((format, Block::new(&bytes)?)))
        .collect()
}

fn set_all(blocks: Vec<(u32, Block)>) -> anyhow::Result<()> {
    for (format, block) in blocks {
        block.set(format)?;
    }
    Ok(())
}

pub fn set_image(img: &Rgba) -> anyhow::Result<()> {
    let blocks = blocks(image_payloads(img)?)?;
    let _g = ClipboardGuard::open()?;
    // SAFETY: clipboard is open on this thread.
    unsafe { EmptyClipboard() }?;
    set_all(blocks)
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
    let mut all = vec![
        (CF_HDROP.0 as u32, build_hdrop(&abs)),
        // DROPEFFECT_COPY, so paste in Explorer copies rather than moves.
        (
            name_format("Preferred DropEffect"),
            1u32.to_le_bytes().to_vec(),
        ),
    ];
    if let Some(img) = img {
        all.extend(image_payloads(img)?);
    }
    let blocks = blocks(all)?;
    let _g = ClipboardGuard::open()?;
    // SAFETY: clipboard is open on this thread.
    unsafe { EmptyClipboard() }?;
    set_all(blocks)
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

/// Changes whenever any app puts something on the clipboard.
pub fn sequence() -> u32 {
    // SAFETY: trivial query.
    unsafe { GetClipboardSequenceNumber() }
}

/// Current clipboard image: the "PNG" format if present, else a 24 or 32 bit DIB.
pub fn get_image() -> Option<Rgba> {
    let _g = ClipboardGuard::open().ok()?;
    if let Some(png) = read_global(png_format())
        && let Ok(img) = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
    {
        let img = img.to_rgba8();
        return Some(Rgba::from_raw(img.width(), img.height(), img.into_raw()));
    }
    [CF_DIBV5.0, CF_DIB.0]
        .into_iter()
        .find_map(|f| read_global(u32::from(f)).and_then(|d| dib_to_rgba(&d)))
}

/// Decode a packed DIB (BITMAPINFOHEADER or later, then pixels) of 24 or 32 bits per pixel,
/// uncompressed or with the usual BGR(A) bit fields. A 32 bit DIB whose alpha is zero
/// everywhere is opaque (most apps leave that byte empty).
fn dib_to_rgba(dib: &[u8]) -> Option<Rgba> {
    let u16_at = |o: usize| Some(u16::from_le_bytes(dib.get(o..o + 2)?.try_into().ok()?));
    let u32_at = |o: usize| Some(u32::from_le_bytes(dib.get(o..o + 4)?.try_into().ok()?));
    let hsize = u32_at(0)? as usize;
    if hsize < 40 {
        return None;
    }
    let w = i32::from_le_bytes(dib.get(4..8)?.try_into().ok()?);
    let h = i32::from_le_bytes(dib.get(8..12)?.try_into().ok()?);
    let bits = u16_at(14)?;
    let compression = u32_at(16)?;
    let colors = u32_at(32)? as usize;
    const BI_RGB: u32 = 0;
    const BI_BITFIELDS: u32 = 3;
    if w <= 0 || h == 0 || !(bits == 24 || bits == 32) {
        return None;
    }
    if compression != BI_RGB && !(compression == BI_BITFIELDS && bits == 32) {
        return None;
    }
    // A 40 byte header with bit fields is followed by the three masks.
    let masks = if hsize == 40 && compression == BI_BITFIELDS {
        12
    } else {
        0
    };
    let start = hsize + masks + colors * 4;
    let (w, top_down, h) = (w as usize, h < 0, h.unsigned_abs() as usize);
    if w > 32768 || h > 32768 {
        return None;
    }
    let bpp = usize::from(bits / 8);
    let stride = (w * bpp).div_ceil(4) * 4;
    let px = dib.get(start..start + stride * h)?;
    let mut out = vec![0u8; w * h * 4];
    let mut any_alpha = false;
    for y in 0..h {
        let src_row = if top_down { y } else { h - 1 - y };
        let row = &px[src_row * stride..src_row * stride + w * bpp];
        for x in 0..w {
            let s = &row[x * bpp..x * bpp + bpp];
            let a = if bpp == 4 { s[3] } else { 255 };
            any_alpha |= a != 0;
            out[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[s[2], s[1], s[0], a]);
        }
    }
    if !any_alpha {
        for p in out.chunks_exact_mut(4) {
            p[3] = 255;
        }
    }
    Some(Rgba::from_raw(w as u32, h as u32, out))
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
    fn dibv5_round_trips_through_the_reader() {
        let mut img = Rgba::new(3, 2);
        img.set_pixel(0, 0, [1, 2, 3, 255]);
        img.set_pixel(2, 1, [200, 100, 50, 128]);
        let back = dib_to_rgba(&build_dibv5(&img)).unwrap();
        assert_eq!((back.width, back.height), (3, 2));
        assert_eq!(back.data, img.data);
    }

    #[test]
    fn dib_24_bit_bottom_up_and_empty_alpha() {
        // 2x2, 24 bit: rows padded to 8 bytes, bottom row first.
        let mut d = vec![0u8; 40];
        d[0..4].copy_from_slice(&40u32.to_le_bytes());
        d[4..8].copy_from_slice(&2i32.to_le_bytes());
        d[8..12].copy_from_slice(&2i32.to_le_bytes());
        d[14..16].copy_from_slice(&24u16.to_le_bytes());
        d.extend([10, 20, 30, 40, 50, 60, 0, 0]); // bottom row (BGR)
        d.extend([1, 2, 3, 4, 5, 6, 0, 0]); // top row
        let img = dib_to_rgba(&d).unwrap();
        assert_eq!(&img.data[0..8], &[3, 2, 1, 255, 6, 5, 4, 255]);
        assert_eq!(&img.data[8..12], &[30, 20, 10, 255]);
        // 32 bit with every alpha byte zero reads as opaque.
        d[14..16].copy_from_slice(&32u16.to_le_bytes());
        d.truncate(40);
        d.extend([0u8; 16]);
        assert!(dib_to_rgba(&d).unwrap().data.chunks(4).all(|p| p[3] == 255));
    }

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
