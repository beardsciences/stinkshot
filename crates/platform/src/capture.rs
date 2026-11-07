//! Screen and window capture.
//!
//! - `freeze_all`: instant snapshot of every monitor composited into one image covering
//!   `monitors::virtual_bounds()` (gaps between monitors are opaque black). Must be fast
//!   (< 60 ms on 2x 4K): use DXGI Desktop Duplication per output (one-shot AcquireNextFrame
//!   with a short timeout, falling back to GDI BitBlt from the screen DC on failure, e.g.
//!   when duplication is unavailable). HDR outputs: convert to SDR sRGB.
//! - `capture_window`: Windows.Graphics.Capture on the HWND so covered/occluded windows
//!   capture correctly; crop to DWM extended frame bounds; returns straight alpha with
//!   transparent pixels outside the window's visible area where WGC provides them.
//!   Cursor and yellow capture border disabled when supported (IsCursorCaptureEnabled,
//!   IsBorderRequired = false).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use stinkshot_core::{PxPoint, PxRect, Rgba};
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HMODULE, HWND, RECT};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_FLAG, D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_RESOURCE_MISC_FLAG, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, D3D11CreateDevice,
    ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
    DXGI_FORMAT_B8G8R8X8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
    DXGI_FORMAT_R10G10B10A2_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_MODE_ROTATION_IDENTITY,
    DXGI_MODE_ROTATION_UNSPECIFIED, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_NOT_FOUND, DXGI_ERROR_WAIT_TIMEOUT,
    DXGI_OUTDUPL_FRAME_INFO, IDXGIAdapter, IDXGIDevice, IDXGIFactory1, IDXGIOutput1, IDXGIOutput5,
    IDXGIOutputDuplication, IDXGIResource,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC,
    CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetObjectW, HBITMAP, HGDIOBJ,
    ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CURSOR_SHOWING, CURSORINFO, DI_NORMAL, DrawIconEx, GetCursorInfo, GetIconInfo, GetWindowRect,
    HICON, ICONINFO, IsZoomed,
};
use windows::core::{Interface, factory};

use crate::Hwnd;
use crate::hdr::{self, DisplayColor};
use crate::monitors::{self, rect_from_win};

#[derive(Clone, Debug)]
pub struct Frozen {
    /// Virtual-desktop rect that `image` covers (== virtual_bounds at freeze time).
    pub bounds: PxRect,
    pub image: Rgba,
    /// Cursor position at freeze time (virtual coords).
    pub cursor: PxPoint,
}

pub fn freeze_all(include_cursor: bool) -> anyhow::Result<Frozen> {
    let bounds = monitors::virtual_bounds();
    if bounds.is_empty() {
        anyhow::bail!("no monitors found");
    }
    let cursor = crate::input::cursor_pos();
    let mut image = capture_region(bounds)?;
    if include_cursor {
        draw_cursor(
            &mut image,
            PxPoint {
                x: bounds.x,
                y: bounds.y,
            },
        );
    }
    Ok(Frozen {
        bounds,
        image,
        cursor,
    })
}

/// Live capture of a virtual-desktop rect (used by scrolling capture each step).
/// Fast path: reuse a cached duplication session between calls.
pub fn capture_rect(rect: PxRect, include_cursor: bool) -> anyhow::Result<Rgba> {
    if rect.is_empty() {
        anyhow::bail!("capture rect is empty");
    }
    let mut image = capture_region(rect)?;
    if include_cursor {
        draw_cursor(
            &mut image,
            PxPoint {
                x: rect.x,
                y: rect.y,
            },
        );
    }
    Ok(image)
}

// ---------------------------------------------------------------------------------------
// Pixel format conversion (pure, unit tested)
// ---------------------------------------------------------------------------------------

fn bytes_per_pixel(fmt: DXGI_FORMAT) -> Option<usize> {
    match fmt {
        DXGI_FORMAT_B8G8R8A8_UNORM
        | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB
        | DXGI_FORMAT_B8G8R8X8_UNORM
        | DXGI_FORMAT_R8G8B8A8_UNORM
        | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB
        | DXGI_FORMAT_R10G10B10A2_UNORM => Some(4),
        DXGI_FORMAT_R16G16B16A16_FLOAT => Some(8),
        _ => None,
    }
}

/// Convert `rows` rows of source pixels into RGBA8 (`dst` holds exactly those rows).
fn convert_rows(
    fmt: DXGI_FORMAT,
    src: &[u8],
    pitch: usize,
    w: usize,
    dst: &mut [u8],
    keep_alpha: bool,
) {
    for (y, drow) in dst.chunks_exact_mut(w * 4).enumerate() {
        let srow = &src[y * pitch..];
        match fmt {
            DXGI_FORMAT_B8G8R8A8_UNORM
            | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB
            | DXGI_FORMAT_B8G8R8X8_UNORM => {
                let alpha = keep_alpha && fmt != DXGI_FORMAT_B8G8R8X8_UNORM;
                for (d, s) in drow.chunks_exact_mut(4).zip(srow.chunks_exact(4)) {
                    d[0] = s[2];
                    d[1] = s[1];
                    d[2] = s[0];
                    d[3] = if alpha { s[3] } else { 255 };
                }
            }
            DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => {
                for (d, s) in drow.chunks_exact_mut(4).zip(srow.chunks_exact(4)) {
                    d[..3].copy_from_slice(&s[..3]);
                    d[3] = if keep_alpha { s[3] } else { 255 };
                }
            }
            DXGI_FORMAT_R10G10B10A2_UNORM => {
                for (d, s) in drow.chunks_exact_mut(4).zip(srow.chunks_exact(4)) {
                    let v = u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
                    d[0] = ((v & 0x3ff) >> 2) as u8;
                    d[1] = (((v >> 10) & 0x3ff) >> 2) as u8;
                    d[2] = (((v >> 20) & 0x3ff) >> 2) as u8;
                    d[3] = 255;
                }
            }
            // FP16 is handled by `hdr::ScrgbToSrgb` in `convert_to_rgba_hdr`.
            _ => {}
        }
    }
}

/// Convert a mapped `w` x `h` surface to straight-alpha RGBA8, splitting rows across threads.
/// Returns None for formats this module does not understand.
/// FP16 surfaces are assumed to be scRGB with SDR white at 80 nits; use
/// `convert_to_rgba_hdr` for a display with another white level.
pub(crate) fn convert_to_rgba(
    fmt: DXGI_FORMAT,
    src: &[u8],
    pitch: usize,
    w: usize,
    h: usize,
    keep_alpha: bool,
) -> Option<Rgba> {
    convert_to_rgba_hdr(fmt, src, pitch, w, h, keep_alpha, hdr::SCRGB_UNIT_NITS)
}

/// `convert_to_rgba` for a surface from a display whose SDR white sits at `sdr_white_nits`
/// (only matters for FP16 scRGB surfaces: SDR content maps back to its original sRGB values).
pub(crate) fn convert_to_rgba_hdr(
    fmt: DXGI_FORMAT,
    src: &[u8],
    pitch: usize,
    w: usize,
    h: usize,
    keep_alpha: bool,
    sdr_white_nits: f32,
) -> Option<Rgba> {
    bytes_per_pixel(fmt)?;
    let mut out = Rgba::new(w as u32, h as u32);
    if w == 0 || h == 0 {
        return Some(out);
    }
    if fmt == DXGI_FORMAT_R16G16B16A16_FLOAT {
        hdr::ScrgbToSrgb::shared(sdr_white_nits).convert_parallel(
            src,
            pitch,
            w,
            h,
            &mut out.data,
            w * 4,
            false,
        );
        return Some(out);
    }
    let threads = if w * h < 300_000 {
        1
    } else {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
            .clamp(1, 8)
    };
    if threads == 1 {
        convert_rows(fmt, src, pitch, w, &mut out.data, keep_alpha);
    } else {
        let rows_per = h.div_ceil(threads);
        std::thread::scope(|s| {
            for (i, chunk) in out.data.chunks_mut(rows_per * w * 4).enumerate() {
                let src = &src[i * rows_per * pitch..];
                s.spawn(move || convert_rows(fmt, src, pitch, w, chunk, keep_alpha));
            }
        });
    }
    Some(out)
}

/// Premultiplied -> straight alpha. If nothing is opaque the source did not carry alpha at all
/// (some windows report 0), so treat the image as fully opaque instead of invisible.
fn fix_alpha(img: &mut Rgba) {
    if img.data.chunks_exact(4).all(|p| p[3] == 0) {
        for p in img.data.chunks_exact_mut(4) {
            p[3] = 255;
        }
        return;
    }
    for p in img.data.chunks_exact_mut(4) {
        let a = p[3] as u32;
        if a != 0 && a != 255 {
            for c in &mut p[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// GDI path
// ---------------------------------------------------------------------------------------

fn dib_info(w: i32, h: i32) -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// BitBlt from the screen DC (works for every output at once, including rotated ones).
fn gdi_capture(rect: PxRect) -> anyhow::Result<Rgba> {
    let (w, h) = (rect.w, rect.h);
    if w <= 0 || h <= 0 {
        anyhow::bail!("empty GDI capture rect");
    }
    // SAFETY: every GDI object created below is released before returning; `bits` is only read
    // while the DIB section is alive.
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            anyhow::bail!("GetDC(NULL) failed");
        }
        let mem = CreateCompatibleDC(Some(screen));
        let result = (|| -> anyhow::Result<Rgba> {
            let bmi = dib_info(w, h);
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let bmp = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)?;
            let old = SelectObject(mem, HGDIOBJ(bmp.0));
            let blt = BitBlt(
                mem,
                0,
                0,
                w,
                h,
                Some(screen),
                rect.x,
                rect.y,
                SRCCOPY | CAPTUREBLT,
            );
            let res = match blt {
                Ok(()) if !bits.is_null() => {
                    let src =
                        std::slice::from_raw_parts(bits as *const u8, w as usize * h as usize * 4);
                    convert_to_rgba(
                        DXGI_FORMAT_B8G8R8X8_UNORM,
                        src,
                        w as usize * 4,
                        w as usize,
                        h as usize,
                        false,
                    )
                    .ok_or_else(|| anyhow::anyhow!("GDI conversion failed"))
                }
                Ok(()) => Err(anyhow::anyhow!("CreateDIBSection returned no bits")),
                Err(e) => Err(anyhow::anyhow!("BitBlt failed: {e}")),
            };
            SelectObject(mem, old);
            let _ = DeleteObject(HGDIOBJ(bmp.0));
            res
        })();
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        result
    }
}

// ---------------------------------------------------------------------------------------
// DXGI Desktop Duplication
// ---------------------------------------------------------------------------------------

#[derive(Debug)]
struct AccessLost;

impl std::fmt::Display for AccessLost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "desktop duplication access lost")
    }
}

impl std::error::Error for AccessLost {}

struct OutputDup {
    /// Output rect in virtual-desktop coordinates.
    rect: PxRect,
    /// None when duplication is unavailable for this output (GDI is used instead).
    dup: Option<IDXGIOutputDuplication>,
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    desc: Option<D3D11_TEXTURE2D_DESC>,
    /// Persistent copy of the latest desktop image (GPU side).
    shadow: Option<ID3D11Texture2D>,
    staging: Option<ID3D11Texture2D>,
    has_image: bool,
    /// Whether the first (untrusted) duplication frame has been consumed.
    seen_first: bool,
    /// HDR state and SDR white level. With advanced color on, the duplication delivers FP16
    /// scRGB and the shadow texture is FP16 too.
    color: DisplayColor,
}

impl OutputDup {
    fn hdr(&self) -> bool {
        self.color.advanced_color
    }

    /// Texture format the shadow holds for this output.
    fn shadow_format(&self) -> DXGI_FORMAT {
        if self.hdr() {
            DXGI_FORMAT_R16G16B16A16_FLOAT
        } else {
            DXGI_FORMAT_B8G8R8A8_UNORM
        }
    }
}

struct Session {
    layout: Vec<PxRect>,
    outputs: Vec<OutputDup>,
    /// When the displays' color state was last read (SDR white can change at any time).
    color_checked: std::time::Instant,
}

/// How often a cached session re-reads HDR state and SDR white levels.
const COLOR_RECHECK: std::time::Duration = std::time::Duration::from_millis(1000);

// SAFETY: D3D11/DXGI objects are free-threaded COM objects; all access to a Session goes
// through the SESSION mutex, so the immediate context is never used concurrently.
unsafe impl Send for Session {}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

/// How long the first AcquireNextFrame on a fresh duplication may wait.
const FIRST_FRAME_TIMEOUT_MS: u32 = 300;
/// Cached sessions already hold the full image; only pick up a pending update.
const CACHED_FRAME_TIMEOUT_MS: u32 = 2;

fn current_layout() -> Vec<PxRect> {
    monitors::list().iter().map(|m| m.rect).collect()
}

impl Session {
    fn create() -> anyhow::Result<Session> {
        // SAFETY: standard DXGI enumeration; every returned interface is owned by RAII wrappers.
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let colors = hdr::displays();
            let mut outputs = Vec::new();
            let mut ai = 0;
            loop {
                let adapter = match factory.EnumAdapters1(ai) {
                    Ok(a) => a,
                    Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                    Err(e) => return Err(e.into()),
                };
                ai += 1;
                let adapter: IDXGIAdapter = adapter.cast()?;
                // Adapters with no attached output (WARP, idle iGPU) are skipped before the
                // comparatively expensive device creation.
                let mut attached = Vec::new();
                let mut oi = 0;
                while let Ok(output) = adapter.EnumOutputs(oi) {
                    oi += 1;
                    if let Ok(desc) = output.GetDesc()
                        && desc.AttachedToDesktop.as_bool()
                    {
                        let name = hdr::device_name(&desc.DeviceName);
                        let color = colors
                            .iter()
                            .find(|c| c.device.eq_ignore_ascii_case(&name))
                            .cloned()
                            .unwrap_or_else(|| hdr::display_for_device(&name));
                        attached.push((output, rect_from_win(&desc.DesktopCoordinates), color));
                    }
                }
                if attached.is_empty() {
                    continue;
                }
                let mut device: Option<ID3D11Device> = None;
                let mut ctx: Option<ID3D11DeviceContext> = None;
                if let Err(e) = D3D11CreateDevice(
                    &adapter,
                    D3D_DRIVER_TYPE_UNKNOWN,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    Some(&mut ctx),
                ) {
                    log::debug!("D3D11CreateDevice for adapter {} failed: {e}", ai - 1);
                    continue;
                }
                let (Some(device), Some(ctx)) = (device, ctx) else {
                    continue;
                };
                for (output, rect, color) in attached {
                    if color.advanced_color {
                        log::debug!(
                            "output {} is HDR (SDR white {} nits); duplicating as FP16",
                            color.device,
                            color.sdr_white_nits
                        );
                    }
                    let dup = duplicate(&output, &device, color.advanced_color);
                    outputs.push(OutputDup {
                        rect,
                        dup,
                        device: device.clone(),
                        ctx: ctx.clone(),
                        desc: None,
                        shadow: None,
                        staging: None,
                        has_image: false,
                        seen_first: false,
                        color,
                    });
                }
            }
            Ok(Session {
                layout: current_layout(),
                outputs,
                color_checked: std::time::Instant::now(),
            })
        }
    }

    /// Pick up SDR white level changes. Returns false when an output's HDR state changed
    /// (the session must be rebuilt: the duplication format depends on it).
    fn recheck_colors(&mut self) -> bool {
        if self.color_checked.elapsed() < COLOR_RECHECK {
            return true;
        }
        self.color_checked = std::time::Instant::now();
        let colors = hdr::displays();
        if colors.is_empty() {
            return true;
        }
        for out in &mut self.outputs {
            if let Some(c) = colors.iter().find(|c| c.device == out.color.device) {
                if c.advanced_color != out.color.advanced_color {
                    return false;
                }
                out.color.sdr_white_nits = c.sdr_white_nits;
            }
        }
        true
    }

    fn capture(&mut self, rect: PxRect, first: bool) -> anyhow::Result<Rgba> {
        let mut img = Rgba::filled(rect.w as u32, rect.h as u32, [0, 0, 0, 255]);
        for out in &mut self.outputs {
            let Some(inter) = out.rect.intersect(&rect) else {
                continue;
            };
            let local = inter.translate(-out.rect.x, -out.rect.y);
            let tile = out.grab(local, inter, first)?;
            if inter == rect {
                return Ok(tile);
            }
            img.blit(&tile, inter.x - rect.x, inter.y - rect.y);
        }
        Ok(img)
    }
}

/// Create a duplication for `output`, or None if it cannot be used reliably.
unsafe fn duplicate(
    output: &windows::Win32::Graphics::Dxgi::IDXGIOutput,
    device: &ID3D11Device,
    hdr: bool,
) -> Option<IDXGIOutputDuplication> {
    if duplication_lent() {
        // The recorder owns this process's duplications right now (a second duplication of
        // an output in one process fails with E_INVALIDARG); GDI serves the capture.
        log::debug!("duplication lent to the recorder; using GDI for this output");
        return None;
    }
    // DuplicateOutput1 refuses DPI-unaware callers on scaled displays.
    let _dpi = crate::dpi::ThreadPerMonitorAware::new();
    // SAFETY: caller guarantees `output` and `device` are live.
    unsafe {
        let attempt = (|| -> windows::core::Result<IDXGIOutputDuplication> {
            if hdr {
                // An 8-bit duplication of an HDR output is the FP16 desktop clipped to 0..1
                // without the SDR white scaling, so everything above 80 nits blows out
                // (measured: #808080 at 480-nit SDR white came back as #FFFFFF). Take the FP16
                // surface and convert it ourselves; without it, GDI (which is correct) is used.
                return output.cast::<IDXGIOutput5>()?.DuplicateOutput1(
                    device,
                    0,
                    &[DXGI_FORMAT_R16G16B16A16_FLOAT],
                );
            }
            if let Ok(o5) = output.cast::<IDXGIOutput5>()
                && let Ok(d) = o5.DuplicateOutput1(device, 0, &[DXGI_FORMAT_B8G8R8A8_UNORM])
            {
                return Ok(d);
            }
            output.cast::<IDXGIOutput1>()?.DuplicateOutput(device)
        })();
        match attempt {
            Ok(d) => {
                let desc = d.GetDesc();
                let rot = desc.Rotation;
                if desc.DesktopImageInSystemMemory.as_bool()
                    || !(rot == DXGI_MODE_ROTATION_IDENTITY
                        || rot == DXGI_MODE_ROTATION_UNSPECIFIED)
                {
                    // Rotated or software-composed outputs: let GDI (which sees the composed,
                    // correctly oriented desktop) handle them.
                    log::debug!("output duplication unsuitable (rotation {rot:?}); using GDI");
                    None
                } else {
                    Some(d)
                }
            }
            Err(e) => {
                log::debug!("DuplicateOutput failed ({e}); using GDI for this output");
                None
            }
        }
    }
}

impl OutputDup {
    /// Bring `shadow` up to date. Ok(false) when no image is available yet.
    fn refresh(&mut self, timeout_ms: u32) -> anyhow::Result<bool> {
        let Some(dup) = self.dup.clone() else {
            return Ok(false);
        };
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut res: Option<IDXGIResource> = None;
        // SAFETY: out pointers reference live locals; a frame acquired here is released below.
        let acquired = unsafe { dup.AcquireNextFrame(timeout_ms, &mut info, &mut res) };
        match acquired {
            Ok(()) => {
                let copy = (|| -> anyhow::Result<()> {
                    let res = res.ok_or_else(|| anyhow::anyhow!("no desktop resource"))?;
                    let tex: ID3D11Texture2D = res.cast()?;
                    // The very first frame of a new duplication is a placeholder (observed as an
                    // uninitialized FP16 surface even on SDR content), so it is never trusted;
                    // `grab` seeds the shadow texture from GDI instead. Mouse-only updates carry
                    // no new pixels either.
                    let first_ever = !self.seen_first;
                    self.seen_first = true;
                    if first_ever || info.LastPresentTime == 0 {
                        return Ok(());
                    }
                    let mut fd = D3D11_TEXTURE2D_DESC::default();
                    // SAFETY: out pointer is a live local.
                    unsafe { tex.GetDesc(&mut fd) };
                    if self.hdr() && fd.Format != DXGI_FORMAT_R16G16B16A16_FLOAT {
                        // An 8-bit frame of an HDR output would be blown out (see `duplicate`).
                        anyhow::bail!("HDR output delivered {:?} frames", fd.Format);
                    }
                    self.ensure_textures(fd.Width, fd.Height, fd.Format)?;
                    if let Some(shadow) = &self.shadow {
                        // SAFETY: both textures belong to this device and now have identical
                        // size and format.
                        unsafe { self.ctx.CopyResource(shadow, &tex) };
                        self.has_image = true;
                    }
                    Ok(())
                })();
                // Make sure the copy is submitted before DXGI recycles the frame.
                // SAFETY: plain flush of the immediate context.
                unsafe { self.ctx.Flush() };
                // SAFETY: balanced with the successful AcquireNextFrame.
                let _ = unsafe { dup.ReleaseFrame() };
                copy?;
                Ok(self.has_image)
            }
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => Ok(self.has_image),
            Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => Err(AccessLost.into()),
            Err(e) => Err(e.into()),
        }
    }

    /// (Re)create the shadow + staging textures when the surface size or format changes.
    fn ensure_textures(
        &mut self,
        width: u32,
        height: u32,
        format: DXGI_FORMAT,
    ) -> anyhow::Result<()> {
        if self.shadow.is_some()
            && self
                .desc
                .is_some_and(|d| d.Width == width && d.Height == height && d.Format == format)
        {
            return Ok(());
        }
        self.has_image = false;
        log::debug!(
            "duplication shadow for {}: {width}x{height} {format:?}",
            self.color.device
        );
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            ..Default::default()
        };
        // SAFETY: texture creation uses a fully initialized desc.
        unsafe {
            let mut shadow_desc = desc;
            shadow_desc.Usage = D3D11_USAGE_DEFAULT;
            shadow_desc.BindFlags = D3D11_BIND_FLAG(0).0 as u32;
            shadow_desc.CPUAccessFlags = 0;
            shadow_desc.MiscFlags = D3D11_RESOURCE_MISC_FLAG(0).0 as u32;
            let mut shadow = None;
            self.device
                .CreateTexture2D(&shadow_desc, None, Some(&mut shadow))?;
            let mut staging_desc = shadow_desc;
            staging_desc.Usage = D3D11_USAGE_STAGING;
            staging_desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut staging = None;
            self.device
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))?;
            self.shadow = shadow;
            self.staging = staging;
        }
        self.desc = Some(desc);
        Ok(())
    }

    /// Pixels of `local` (output-local px). `virt` is the same rect in virtual coords, used
    /// by the GDI fallback.
    fn grab(&mut self, local: PxRect, virt: PxRect, first: bool) -> anyhow::Result<Rgba> {
        if self.dup.is_some() {
            let timeout = if first {
                FIRST_FRAME_TIMEOUT_MS
            } else {
                CACHED_FRAME_TIMEOUT_MS
            };
            let mut state = self.refresh(timeout);
            if matches!(state, Ok(false)) {
                match self.seed_from_gdi() {
                    Ok(true) => state = Ok(true),
                    Ok(false) => {}
                    Err(e) => log::debug!("seeding duplication shadow failed: {e}"),
                }
            }
            match state {
                Ok(true) => match self.read_back(local) {
                    Ok(Some(img)) => return Ok(img),
                    Ok(None) => {}
                    Err(e) => log::debug!("duplication read-back failed: {e}"),
                },
                Ok(false) => {}
                Err(e) if e.is::<AccessLost>() => return Err(e),
                Err(e) => log::debug!("duplication refresh failed: {e}"),
            }
        }
        gdi_capture(virt)
    }

    /// A static desktop never delivers a trustworthy first duplication frame, so fill the
    /// shadow texture once from a GDI capture of the whole output. Later frames overwrite it.
    /// Ok(false) when the format does not allow a direct upload.
    fn seed_from_gdi(&mut self) -> anyhow::Result<bool> {
        if self.shadow.is_none() {
            self.ensure_textures(self.rect.w as u32, self.rect.h as u32, self.shadow_format())?;
        }
        let (Some(shadow), Some(desc)) = (&self.shadow, &self.desc) else {
            return Ok(false);
        };
        let full = PxRect::new(
            self.rect.x,
            self.rect.y,
            desc.Width as i32,
            desc.Height as i32,
        );
        if desc.Format == DXGI_FORMAT_R16G16B16A16_FLOAT {
            // GDI returns correct SDR values on an HDR output (verified); lift them into scRGB
            // at the display's SDR white so they match what later FP16 frames contain.
            let img = gdi_capture(full)?;
            let table = hdr::srgb8_to_scrgb_half(self.color.sdr_white_nits);
            let one = hdr::f32_to_half(1.0);
            let mut halves = vec![0u16; img.data.len()];
            for (d, s) in halves.chunks_exact_mut(4).zip(img.data.chunks_exact(4)) {
                d[0] = table[s[0] as usize];
                d[1] = table[s[1] as usize];
                d[2] = table[s[2] as usize];
                d[3] = one;
            }
            // SAFETY: `halves` holds Width*Height tightly packed RGBA16F pixels.
            unsafe {
                self.ctx.UpdateSubresource(
                    shadow,
                    0,
                    None,
                    halves.as_ptr() as *const _,
                    desc.Width * 8,
                    0,
                );
            }
            self.has_image = true;
            return Ok(true);
        }
        if desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM
            && desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM_SRGB
        {
            return Ok(false);
        }
        let mut img = gdi_capture(full)?;
        for p in img.data.chunks_exact_mut(4) {
            p.swap(0, 2);
        }
        // SAFETY: `img` holds Width*Height tightly packed BGRA pixels matching the texture.
        unsafe {
            self.ctx.UpdateSubresource(
                shadow,
                0,
                None,
                img.data.as_ptr() as *const _,
                desc.Width * 4,
                0,
            );
        }
        self.has_image = true;
        Ok(true)
    }

    fn read_back(&self, local: PxRect) -> anyhow::Result<Option<Rgba>> {
        let (Some(shadow), Some(staging), Some(desc)) = (&self.shadow, &self.staging, &self.desc)
        else {
            return Ok(None);
        };
        let local = local
            .intersect(&PxRect::new(0, 0, desc.Width as i32, desc.Height as i32))
            .ok_or_else(|| anyhow::anyhow!("region outside output"))?;
        let b = D3D11_BOX {
            left: local.x as u32,
            top: local.y as u32,
            front: 0,
            right: local.right() as u32,
            bottom: local.bottom() as u32,
            back: 1,
        };
        let bpp = bytes_per_pixel(desc.Format)
            .ok_or_else(|| anyhow::anyhow!("unsupported format {:?}", desc.Format))?;
        // SAFETY: the staging texture is mapped only for the conversion and unmapped right after;
        // the slice covers rows [0, h) of the mapped region written by the copy above.
        unsafe {
            let t0 = std::time::Instant::now();
            self.ctx
                .CopySubresourceRegion(staging, 0, 0, 0, 0, shadow, 0, Some(&b));
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.ctx
                .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let t_map = t0.elapsed();
            let (w, h) = (local.w as usize, local.h as usize);
            let pitch = mapped.RowPitch as usize;
            let len = pitch * (h - 1) + w * bpp;
            let src = std::slice::from_raw_parts(mapped.pData as *const u8, len);
            let img = convert_to_rgba_hdr(
                desc.Format,
                src,
                pitch,
                w,
                h,
                false,
                self.color.sdr_white_nits,
            );
            self.ctx.Unmap(staging, 0);
            log::trace!(
                "read_back {}x{} {:?}: copy+map {t_map:?}, convert {:?}",
                w,
                h,
                desc.Format,
                t0.elapsed() - t_map
            );
            Ok(img)
        }
    }
}

fn capture_region(rect: PxRect) -> anyhow::Result<Rgba> {
    let mut guard = SESSION.lock().unwrap_or_else(|p| p.into_inner());
    for attempt in 0..2 {
        let stale = guard
            .as_mut()
            .is_none_or(|s| s.layout != current_layout() || !s.recheck_colors());
        let first = stale;
        if stale {
            *guard = match Session::create() {
                Ok(s) if !s.outputs.is_empty() => Some(s),
                Ok(_) => None,
                Err(e) => {
                    log::debug!("duplication session unavailable: {e}");
                    None
                }
            };
        }
        let Some(session) = guard.as_mut() else {
            return gdi_capture(rect);
        };
        match session.capture(rect, first) {
            Ok(img) => return Ok(img),
            Err(e) if e.is::<AccessLost>() && attempt == 0 => {
                // Mode change, secure desktop, or driver reset: rebuild the session once.
                *guard = None;
            }
            Err(e) => {
                log::debug!("duplication capture failed ({e}); GDI fallback");
                *guard = None;
                return gdi_capture(rect);
            }
        }
    }
    gdi_capture(rect)
}

/// Create (or re-create) the duplication session ahead of time, from a worker thread: at
/// start-up and again after events that invalidate it (unlock, display change, resume; see
/// `system::start_session_watcher`). Session creation plus each output's first frame (a
/// placeholder that is replaced by a GDI seed) costs several hundred ms; afterwards
/// `freeze_all` stays within a few tens of ms. Every output is primed (past its first frame,
/// shadow texture seeded) without the read-back and pixel conversion a real capture does.
/// A session that reports access lost (desktop switch, mode change) is rebuilt once. Failure
/// is harmless: captures create the session themselves or fall back to GDI.
pub fn warm_up() {
    if duplication_lent() {
        log::debug!("capture warm-up skipped: duplication lent to the recorder");
        return;
    }
    let t0 = std::time::Instant::now();
    let mut guard = SESSION.lock().unwrap_or_else(|p| p.into_inner());
    for attempt in 0..2 {
        let stale = guard
            .as_mut()
            .is_none_or(|s| s.layout != current_layout() || !s.recheck_colors());
        if stale {
            *guard = match Session::create() {
                Ok(s) if !s.outputs.is_empty() => Some(s),
                Ok(_) => None,
                Err(e) => {
                    log::debug!("capture warm-up: duplication session unavailable: {e}");
                    None
                }
            };
        }
        let Some(session) = guard.as_mut() else {
            return;
        };
        let mut lost = false;
        for out in &mut session.outputs {
            if out.dup.is_none() {
                continue;
            }
            match out.refresh(FIRST_FRAME_TIMEOUT_MS) {
                Ok(true) => {}
                Ok(false) => {
                    if let Err(e) = out.seed_from_gdi() {
                        log::debug!("capture warm-up: seeding {} failed: {e}", out.color.device);
                    }
                }
                Err(e) if e.is::<AccessLost>() => lost = true,
                Err(e) => log::debug!("capture warm-up: {} refresh failed: {e}", out.color.device),
            }
        }
        if !lost {
            log::debug!(
                "capture warm-up: duplication session ready in {:.1} ms",
                t0.elapsed().as_secs_f64() * 1000.0
            );
            return;
        }
        // Mode change or desktop switch: drop it and build a new one (once). If that is lost
        // too (still on the secure desktop), the next capture rebuilds it.
        *guard = None;
        if attempt == 1 {
            log::debug!("capture warm-up: session lost again, left for the next capture");
        }
    }
}

/// Drop the cached duplication session (frees GPU resources). Safe to call at any time; the
/// next capture creates a new session.
pub fn release_cached_capture() {
    *SESSION.lock().unwrap_or_else(|p| p.into_inner()) = None;
}

/// Outstanding `DuplicationLease`s.
static LENT: AtomicUsize = AtomicUsize::new(0);

fn duplication_lent() -> bool {
    LENT.load(Ordering::SeqCst) > 0
}

/// Hands this process's Desktop Duplication over to another user (the recorder) until dropped.
///
/// Windows allows one duplication per output per process: a second `DuplicateOutput(1)` of
/// an output this process already duplicates fails with `E_INVALIDARG`. The cached capture
/// session (kept warm from start-up so the overlay opens fast) holds every output, so the
/// recorder's own duplication failed while it existed. Taking a lease drops the cached
/// session; while any lease is held, captures (`freeze_all`, `capture_rect`) use GDI for
/// every output and `warm_up` does nothing. Dropping the last lease discards the GDI-only
/// session made meanwhile and, if the session was warm before, warms it up again on a
/// worker thread.
#[must_use = "the duplication is returned as soon as the lease is dropped"]
pub struct DuplicationLease(());

/// Whether a warm duplication session was dropped by a lease (restored after the last one).
static REWARM_AFTER_LEASE: AtomicBool = AtomicBool::new(false);

pub fn lend_duplication() -> DuplicationLease {
    LENT.fetch_add(1, Ordering::SeqCst);
    let was_warm = SESSION
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take()
        .is_some_and(|s| s.outputs.iter().any(|o| o.dup.is_some()));
    if was_warm {
        REWARM_AFTER_LEASE.store(true, Ordering::SeqCst);
    }
    DuplicationLease(())
}

impl Drop for DuplicationLease {
    fn drop(&mut self) {
        if LENT.fetch_sub(1, Ordering::SeqCst) != 1 {
            return;
        }
        release_cached_capture();
        if REWARM_AFTER_LEASE.swap(false, Ordering::SeqCst) {
            let _ = std::thread::Builder::new()
                .name("capture-rewarm-after-lease".into())
                .spawn(warm_up);
        }
    }
}

// ---------------------------------------------------------------------------------------
// Window capture (Windows.Graphics.Capture)
// ---------------------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct WindowCaptureOpts {
    pub include_cursor: bool,
}

/// Upper bound for one window capture.
const WGC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

pub fn capture_window(hwnd: Hwnd, opts: &WindowCaptureOpts) -> anyhow::Result<Rgba> {
    if hwnd == 0 {
        anyhow::bail!("null window handle");
    }
    let win = HWND(hwnd as *mut _);
    let mut img = match wgc_with_timeout(hwnd, opts.include_cursor) {
        Ok(img) => img,
        Err(e) => {
            log::warn!("Windows.Graphics.Capture failed ({e}); falling back to PrintWindow");
            let mut img = printwindow_capture(win)?;
            if opts.include_cursor
                && let Some((_, outer)) = window_bounds(win)
            {
                draw_cursor(
                    &mut img,
                    PxPoint {
                        x: outer.x,
                        y: outer.y,
                    },
                );
            }
            img
        }
    };
    if let Some((frame, win_rect)) = window_bounds(win) {
        // WGC returns the window's outer rect (including the invisible resize border on
        // some builds). Keep only the visible DWM frame.
        let off_x = frame.x - win_rect.x;
        let off_y = frame.y - win_rect.y;
        let (iw, ih) = (img.width as i32, img.height as i32);
        if (iw, ih) != (frame.w, frame.h) && (iw, ih) == (win_rect.w, win_rect.h) {
            img = img.crop(PxRect::new(off_x.max(0), off_y.max(0), frame.w, frame.h));
        } else if (iw, ih) != (frame.w, frame.h) && iw > frame.w && ih > frame.h {
            log::debug!(
                "WGC frame {iw}x{ih} differs from frame bounds {}x{}; keeping full frame",
                frame.w,
                frame.h
            );
        }
    }
    Ok(img)
}

/// When a WGC attempt hangs (observed: `CreateCaptureSession` never returning on some
/// systems), skip WGC for a while instead of making every capture wait for the timeout.
static WGC_HUNG_AT: Mutex<Option<std::time::Instant>> = Mutex::new(None);
const WGC_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(300);

/// Run WGC on a private MTA thread: the caller's apartment (UI thread = STA) is irrelevant
/// and a window that never delivers a frame costs this call at most `WGC_TIMEOUT`.
fn wgc_with_timeout(hwnd: Hwnd, include_cursor: bool) -> anyhow::Result<Rgba> {
    {
        let hung = WGC_HUNG_AT.lock().unwrap_or_else(|p| p.into_inner());
        if hung.is_some_and(|t| t.elapsed() < WGC_COOLDOWN) {
            anyhow::bail!("Windows.Graphics.Capture disabled after an earlier hang");
        }
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("stinkshot-wgc".into())
        .spawn(move || {
            crate::util::ensure_com();
            let _ = tx.send(wgc_capture(HWND(hwnd as *mut _), include_cursor));
        })
        .map_err(|e| anyhow::anyhow!("spawn capture thread: {e}"))?;
    match rx.recv_timeout(WGC_TIMEOUT) {
        Ok(r) => r,
        Err(_) => {
            *WGC_HUNG_AT.lock().unwrap_or_else(|p| p.into_inner()) =
                Some(std::time::Instant::now());
            anyhow::bail!("Windows.Graphics.Capture timed out")
        }
    }
}

/// PrintWindow(PW_RENDERFULLCONTENT) fallback: asks the window to render itself (through DWM
/// for modern apps), so occluded windows still work. The result covers the full window rect,
/// is opaque, and gets Windows 11 rounded-corner alpha applied for non-maximized windows.
fn printwindow_capture(hwnd: HWND) -> anyhow::Result<Rgba> {
    let (frame, outer) =
        window_bounds(hwnd).ok_or_else(|| anyhow::anyhow!("window has no bounds"))?;
    let (w, h) = (outer.w, outer.h);
    if w <= 0 || h <= 0 {
        anyhow::bail!("window has no visible area");
    }
    // SAFETY: GDI objects are released before returning; DIB bits are read while alive.
    let mut img = unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let result = (|| -> anyhow::Result<Rgba> {
            let bmi = dib_info(w, h);
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let bmp = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)?;
            if bits.is_null() {
                let _ = DeleteObject(HGDIOBJ(bmp.0));
                anyhow::bail!("no DIB bits");
            }
            let old = SelectObject(mem, HGDIOBJ(bmp.0));
            let ok = PrintWindow(hwnd, mem, PRINT_WINDOW_FLAGS(2)); // PW_RENDERFULLCONTENT
            let src = std::slice::from_raw_parts(bits as *const u8, w as usize * h as usize * 4);
            let img = convert_to_rgba(
                DXGI_FORMAT_B8G8R8X8_UNORM,
                src,
                w as usize * 4,
                w as usize,
                h as usize,
                false,
            );
            SelectObject(mem, old);
            let _ = DeleteObject(HGDIOBJ(bmp.0));
            if !ok.as_bool() {
                anyhow::bail!("PrintWindow failed");
            }
            img.ok_or_else(|| anyhow::anyhow!("conversion failed"))
        })();
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        result?
    };
    // SAFETY: trivial queries.
    let (maximized, dpi) = unsafe { (IsZoomed(hwnd).as_bool(), GetDpiForWindow(hwnd)) };
    if !maximized {
        let radius = 8.0 * dpi.max(96) as f32 / 96.0;
        let rel = PxRect::new(frame.x - outer.x, frame.y - outer.y, frame.w, frame.h);
        apply_corner_alpha(&mut img, rel, radius);
    }
    Ok(img)
}

/// Make the four corners of `rect` (inside `img`) transparent with a rounded, anti-aliased edge.
pub(crate) fn apply_corner_alpha(img: &mut Rgba, rect: PxRect, radius: f32) {
    let r = radius.min(rect.w as f32 / 2.0).min(rect.h as f32 / 2.0);
    if r < 1.0 {
        return;
    }
    let n = r.ceil() as i32 + 1;
    for dy in 0..n {
        for dx in 0..n {
            // Distance of this pixel centre from the corner circle's centre.
            let (px, py) = (dx as f32 + 0.5, dy as f32 + 0.5);
            let dist = ((r - px).powi(2) + (r - py).powi(2)).sqrt();
            let cover = if px >= r || py >= r {
                1.0
            } else {
                (r - dist + 0.5).clamp(0.0, 1.0)
            };
            if cover >= 1.0 {
                continue;
            }
            for (x, y) in [
                (rect.x + dx, rect.y + dy),
                (rect.right() - 1 - dx, rect.y + dy),
                (rect.x + dx, rect.bottom() - 1 - dy),
                (rect.right() - 1 - dx, rect.bottom() - 1 - dy),
            ] {
                if x >= 0 && y >= 0 && (x as u32) < img.width && (y as u32) < img.height {
                    let mut p = img.pixel(x as u32, y as u32);
                    p[3] = (p[3] as f32 * cover).round() as u8;
                    img.set_pixel(x as u32, y as u32, p);
                }
            }
        }
    }
}

fn window_bounds(hwnd: HWND) -> Option<(PxRect, PxRect)> {
    let mut frame = RECT::default();
    let mut outer = RECT::default();
    // SAFETY: out pointers are live RECT locals of the declared size.
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut frame as *mut RECT as *mut _,
            std::mem::size_of::<RECT>() as u32,
        )
        .ok()?;
        GetWindowRect(hwnd, &mut outer).ok()?;
    }
    Some((rect_from_win(&frame), rect_from_win(&outer)))
}

fn create_hw_device() -> anyhow::Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device: Option<ID3D11Device> = None;
    let mut ctx: Option<ID3D11DeviceContext> = None;
    // SAFETY: out pointers reference live locals.
    unsafe {
        D3D11CreateDevice(
            None::<&IDXGIAdapter>,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut ctx),
        )?;
    }
    match (device, ctx) {
        (Some(d), Some(c)) => Ok((d, c)),
        _ => anyhow::bail!("D3D11CreateDevice returned no device"),
    }
}

fn wgc_capture(hwnd: HWND, include_cursor: bool) -> anyhow::Result<Rgba> {
    let (device, ctx) = create_hw_device()?;
    // SAFETY: WinRT/D3D calls on live objects; all resources are RAII wrappers closed on drop.
    unsafe {
        let dxgi: IDXGIDevice = device.cast()?;
        let winrt_device: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?.cast()?;

        let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
        let item: GraphicsCaptureItem = interop.CreateForWindow(hwnd).map_err(|e| {
            anyhow::anyhow!("window cannot be captured (minimized, protected or closed): {e}")
        })?;
        let size = item.Size()?;
        if size.Width <= 0 || size.Height <= 0 {
            anyhow::bail!("window has no visible area");
        }
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &winrt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            1,
            size,
        )?;
        let session = pool.CreateCaptureSession(&item)?;
        // Both properties need a recent Windows 10/11 build; older ones simply keep the default.
        if let Err(e) = session.SetIsCursorCaptureEnabled(include_cursor) {
            log::debug!("SetIsCursorCaptureEnabled: {e}");
        }
        if let Err(e) = session.SetIsBorderRequired(false) {
            log::debug!("SetIsBorderRequired(false): {e}");
        }
        session.StartCapture()?;

        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(2000);
        let frame = loop {
            match pool.TryGetNextFrame() {
                Ok(f) => break f,
                Err(_) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(4));
                }
                Err(_) => {
                    let _ = session.Close();
                    let _ = pool.Close();
                    anyhow::bail!(
                        "timed out waiting for a window frame (is the window minimized?)"
                    );
                }
            }
        };
        let content = frame.ContentSize()?;
        let surface = frame.Surface()?;
        let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
        let tex: ID3D11Texture2D = access.GetInterface()?;
        let result = read_texture(&device, &ctx, &tex, content.Width, content.Height);
        let _ = frame.Close();
        let _ = session.Close();
        let _ = pool.Close();
        let mut img = result?;
        fix_alpha(&mut img);
        Ok(img)
    }
}

/// Copy a GPU texture to the CPU and convert to RGBA (keeping alpha), cropped to
/// `content_w` x `content_h`.
unsafe fn read_texture(
    device: &ID3D11Device,
    ctx: &ID3D11DeviceContext,
    tex: &ID3D11Texture2D,
    content_w: i32,
    content_h: i32,
) -> anyhow::Result<Rgba> {
    // SAFETY: caller guarantees live objects; the staging texture is unmapped before return.
    unsafe {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        tex.GetDesc(&mut desc);
        let mut sd = desc;
        sd.Usage = D3D11_USAGE_STAGING;
        sd.BindFlags = 0;
        sd.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        sd.MiscFlags = 0;
        let mut staging = None;
        device.CreateTexture2D(&sd, None, Some(&mut staging))?;
        let staging = staging.ok_or_else(|| anyhow::anyhow!("no staging texture"))?;
        ctx.CopyResource(&staging, tex);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        let bpp = bytes_per_pixel(desc.Format).unwrap_or(4);
        let (w, h) = (
            (content_w.max(1) as usize).min(desc.Width as usize),
            (content_h.max(1) as usize).min(desc.Height as usize),
        );
        let pitch = mapped.RowPitch as usize;
        let len = pitch * (h - 1) + w * bpp;
        let src = std::slice::from_raw_parts(mapped.pData as *const u8, len);
        let img = convert_to_rgba(desc.Format, src, pitch, w, h, true);
        ctx.Unmap(&staging, 0);
        img.ok_or_else(|| anyhow::anyhow!("unsupported window frame format {:?}", desc.Format))
    }
}

// ---------------------------------------------------------------------------------------
// Cursor
// ---------------------------------------------------------------------------------------

/// Render `hicon` into a w x h BGRA buffer over an opaque grey level `bg`.
unsafe fn render_icon(hicon: HICON, w: i32, h: i32, bg: u8) -> anyhow::Result<Vec<u8>> {
    // SAFETY: GDI objects are released before returning; the DIB bits are read while alive.
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let out = (|| -> anyhow::Result<Vec<u8>> {
            let bmi = dib_info(w, h);
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let bmp: HBITMAP =
                CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)?;
            if bits.is_null() {
                let _ = DeleteObject(HGDIOBJ(bmp.0));
                anyhow::bail!("no DIB bits");
            }
            let len = w as usize * h as usize * 4;
            let buf = std::slice::from_raw_parts_mut(bits as *mut u8, len);
            for px in buf.chunks_exact_mut(4) {
                px.copy_from_slice(&[bg, bg, bg, 255]);
            }
            let old = SelectObject(mem, HGDIOBJ(bmp.0));
            let drawn = DrawIconEx(mem, 0, 0, hicon, w, h, 0, None, DI_NORMAL);
            let copy = std::slice::from_raw_parts(bits as *const u8, len).to_vec();
            SelectObject(mem, old);
            let _ = DeleteObject(HGDIOBJ(bmp.0));
            drawn?;
            Ok(copy)
        })();
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        out
    }
}

/// Recover straight-alpha RGBA from the same glyph rendered over black and over white.
pub(crate) fn combine_black_white(black: &[u8], white: &[u8], w: u32, h: u32) -> Rgba {
    let mut out = Rgba::new(w, h);
    for ((o, b), wh) in out
        .data
        .chunks_exact_mut(4)
        .zip(black.chunks_exact(4))
        .zip(white.chunks_exact(4))
    {
        // BGRA inputs. Over black the pixel is C*a, over white C*a + 255*(1-a).
        let diff: i32 = (0..3).map(|i| wh[i] as i32 - b[i] as i32).sum::<i32>() / 3;
        let a = (255 - diff).clamp(0, 255) as u32;
        if a == 0 {
            continue;
        }
        o[0] = ((b[2] as u32 * 255 + a / 2) / a).min(255) as u8;
        o[1] = ((b[1] as u32 * 255 + a / 2) / a).min(255) as u8;
        o[2] = ((b[0] as u32 * 255 + a / 2) / a).min(255) as u8;
        o[3] = a as u8;
    }
    out
}

fn bitmap_size(h: HBITMAP) -> Option<(i32, i32)> {
    let mut bm = BITMAP::default();
    // SAFETY: out buffer is a BITMAP of the declared size.
    let n = unsafe {
        GetObjectW(
            HGDIOBJ(h.0),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut bm as *mut BITMAP as *mut _),
        )
    };
    (n > 0).then_some((bm.bmWidth, bm.bmHeight))
}

fn try_draw_cursor(img: &mut Rgba, origin: PxPoint) -> anyhow::Result<()> {
    let mut ci = CURSORINFO {
        cbSize: std::mem::size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: cbSize is set; GDI bitmaps from GetIconInfo are deleted below.
    unsafe {
        GetCursorInfo(&mut ci)?;
        if ci.flags.0 & CURSOR_SHOWING.0 == 0 || ci.hCursor.is_invalid() {
            return Ok(());
        }
        let hicon = HICON(ci.hCursor.0);
        let mut ii = ICONINFO::default();
        GetIconInfo(hicon, &mut ii)?;
        let size = if !ii.hbmColor.is_invalid() {
            bitmap_size(ii.hbmColor)
        } else {
            // Monochrome cursor: AND mask stacked over XOR mask.
            bitmap_size(ii.hbmMask).map(|(w, h)| (w, h / 2))
        };
        if !ii.hbmColor.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(ii.hbmColor.0));
        }
        if !ii.hbmMask.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(ii.hbmMask.0));
        }
        let Some((cw, ch)) = size.filter(|(w, h)| *w > 0 && *h > 0 && *w <= 512 && *h <= 512)
        else {
            return Ok(());
        };
        let px = ci.ptScreenPos.x - ii.xHotspot as i32 - origin.x;
        let py = ci.ptScreenPos.y - ii.yHotspot as i32 - origin.y;
        if px >= img.width as i32 || py >= img.height as i32 || px + cw <= 0 || py + ch <= 0 {
            return Ok(());
        }
        let black = render_icon(hicon, cw, ch, 0)?;
        let white = render_icon(hicon, cw, ch, 255)?;
        let cursor = combine_black_white(&black, &white, cw as u32, ch as u32);
        blend_over(img, &cursor, px, py);
    }
    Ok(())
}

/// Straight-alpha source-over of `src` onto `dst` at (x, y).
pub(crate) fn blend_over(dst: &mut Rgba, src: &Rgba, x: i32, y: i32) {
    let Some(r) = PxRect::new(x, y, src.width as i32, src.height as i32).intersect(&dst.bounds())
    else {
        return;
    };
    for yy in r.y..r.bottom() {
        for xx in r.x..r.right() {
            let s = src.pixel((xx - x) as u32, (yy - y) as u32);
            let sa = s[3] as u32;
            if sa == 0 {
                continue;
            }
            let d = dst.pixel(xx as u32, yy as u32);
            let da = d[3] as u32;
            let out_a = sa + da * (255 - sa) / 255;
            let mut o = [0u8; 4];
            for i in 0..3 {
                let num = s[i] as u32 * sa * 255 + d[i] as u32 * da * (255 - sa);
                o[i] = if out_a == 0 {
                    0
                } else {
                    (num / (out_a * 255)).min(255) as u8
                };
            }
            o[3] = out_a.min(255) as u8;
            dst.set_pixel(xx as u32, yy as u32, o);
        }
    }
}

/// Draw the current cursor image into `img` (whose top-left is at `origin` in virtual
/// coords). Used when include_cursor is on and the capture path didn't include it.
pub fn draw_cursor(img: &mut Rgba, origin: PxPoint) {
    if let Err(e) = try_draw_cursor(img, origin) {
        log::debug!("draw_cursor: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fp16_uses_white_level() {
        // #808080 as composed on a 480-nit SDR white display: linear 0.2158 * 6.
        let h = hdr::f32_to_half(hdr::srgb_decode(128.0 / 255.0) * 6.0);
        let mut src = Vec::new();
        for v in [h, h, h, hdr::f32_to_half(1.0)] {
            src.extend_from_slice(&v.to_le_bytes());
        }
        let img = convert_to_rgba_hdr(DXGI_FORMAT_R16G16B16A16_FLOAT, &src, 8, 1, 1, false, 480.0)
            .unwrap();
        assert_eq!(img.pixel(0, 0), [128, 128, 128, 255]);
        // the same pixel read as 80-nit scRGB is blown out to white
        let img = convert_to_rgba(DXGI_FORMAT_R16G16B16A16_FLOAT, &src, 8, 1, 1, false).unwrap();
        assert_eq!(img.pixel(0, 0), [255, 255, 255, 255]);
    }

    #[test]
    fn bgra_swizzle_and_pitch() {
        // 2x2 BGRA with 12-byte pitch (4 bytes padding per row)
        let src = [
            1, 2, 3, 9, 4, 5, 6, 9, 0, 0, 0, 0, //
            7, 8, 9, 9, 10, 11, 12, 9, 0, 0, 0, 0,
        ];
        let img = convert_to_rgba(DXGI_FORMAT_B8G8R8A8_UNORM, &src, 12, 2, 2, false).unwrap();
        assert_eq!(img.pixel(0, 0), [3, 2, 1, 255]);
        assert_eq!(img.pixel(1, 1), [12, 11, 10, 255]);
        let keep = convert_to_rgba(DXGI_FORMAT_B8G8R8A8_UNORM, &src, 12, 2, 2, true).unwrap();
        assert_eq!(keep.pixel(0, 0), [3, 2, 1, 9]);
    }

    #[test]
    fn float_and_10bit_formats() {
        // one pixel of FP16 (1.0, 0.0, 1.0, 1.0)
        let mut src = Vec::new();
        for h in [0x3C00u16, 0, 0x3C00, 0x3C00] {
            src.extend_from_slice(&h.to_le_bytes());
        }
        let img = convert_to_rgba(DXGI_FORMAT_R16G16B16A16_FLOAT, &src, 8, 1, 1, false).unwrap();
        assert_eq!(img.pixel(0, 0), [255, 0, 255, 255]);
        // 10-bit: r=1023, g=0, b=512
        let v: u32 = 1023 | (512 << 20);
        let img = convert_to_rgba(
            DXGI_FORMAT_R10G10B10A2_UNORM,
            &v.to_le_bytes(),
            4,
            1,
            1,
            false,
        )
        .unwrap();
        assert_eq!(img.pixel(0, 0), [255, 0, 128, 255]);
    }

    #[test]
    fn parallel_conversion_matches_serial() {
        let (w, h) = (700usize, 600usize);
        let pitch = w * 4 + 8;
        let mut src = vec![0u8; pitch * h];
        for (i, b) in src.iter_mut().enumerate() {
            *b = (i * 31 % 251) as u8;
        }
        let par = convert_to_rgba(DXGI_FORMAT_B8G8R8A8_UNORM, &src, pitch, w, h, false).unwrap();
        let mut ser = vec![0u8; w * h * 4];
        convert_rows(DXGI_FORMAT_B8G8R8A8_UNORM, &src, pitch, w, &mut ser, false);
        assert_eq!(par.data, ser);
    }

    #[test]
    fn unsupported_format_is_none() {
        assert!(convert_to_rgba(DXGI_FORMAT(0), &[0; 4], 4, 1, 1, false).is_none());
    }

    #[test]
    fn alpha_fix_unpremultiplies_and_rescues_blank() {
        let mut img = Rgba::new(2, 1);
        img.set_pixel(0, 0, [50, 25, 0, 100]);
        img.set_pixel(1, 0, [10, 10, 10, 255]);
        fix_alpha(&mut img);
        assert_eq!(img.pixel(0, 0), [128, 64, 0, 100]);
        assert_eq!(img.pixel(1, 0), [10, 10, 10, 255]);
        let mut blank = Rgba::filled(2, 1, [5, 6, 7, 0]);
        fix_alpha(&mut blank);
        assert_eq!(blank.pixel(0, 0), [5, 6, 7, 255]);
    }

    #[test]
    fn black_white_alpha_recovery() {
        // Opaque red: black pass = (0,0,255) BGRA, white pass identical.
        let black = [0u8, 0, 255, 255];
        let white = [0u8, 0, 255, 255];
        let c = combine_black_white(&black, &white, 1, 1);
        assert_eq!(c.pixel(0, 0), [255, 0, 0, 255]);
        // Fully transparent: black pass 0, white pass 255.
        let c = combine_black_white(&[0, 0, 0, 255], &[255, 255, 255, 255], 1, 1);
        assert_eq!(c.pixel(0, 0)[3], 0);
        // 50% white: black pass 128, white pass 255 (C*a = 128, +127)
        let c = combine_black_white(&[128, 128, 128, 255], &[255, 255, 255, 255], 1, 1);
        let p = c.pixel(0, 0);
        assert!((p[3] as i32 - 128).abs() <= 1 && p[0] >= 250);
    }

    #[test]
    fn corner_alpha_rounds_only_corners() {
        let mut img = Rgba::filled(40, 30, [9, 9, 9, 255]);
        apply_corner_alpha(&mut img, PxRect::new(0, 0, 40, 30), 8.0);
        assert_eq!(img.pixel(0, 0)[3], 0);
        assert_eq!(img.pixel(39, 0)[3], 0);
        assert_eq!(img.pixel(0, 29)[3], 0);
        assert_eq!(img.pixel(39, 29)[3], 0);
        // edge midpoints and the interior stay opaque
        assert_eq!(img.pixel(20, 0)[3], 255);
        assert_eq!(img.pixel(0, 15)[3], 255);
        assert_eq!(img.pixel(20, 15)[3], 255);
        // the arc is anti-aliased
        let partial = (0..8).any(|d| {
            let a = img.pixel(d, 1)[3];
            a > 0 && a < 255
        });
        assert!(partial);
    }

    #[test]
    fn blend_over_basics() {
        let mut dst = Rgba::filled(2, 2, [0, 0, 0, 255]);
        let src = Rgba::filled(1, 1, [255, 255, 255, 128]);
        blend_over(&mut dst, &src, 1, 1);
        let p = dst.pixel(1, 1);
        assert!((p[0] as i32 - 128).abs() <= 1 && p[3] == 255);
        assert_eq!(dst.pixel(0, 0), [0, 0, 0, 255]);
        // fully outside is a no-op
        blend_over(&mut dst, &src, 5, 5);
    }
}
