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
    DXGI_OUTDUPL_FRAME_INFO, IDXGIAdapter, IDXGIDevice, IDXGIDevice3, IDXGIFactory1, IDXGIOutput1,
    IDXGIOutput5, IDXGIOutputDuplication, IDXGIResource,
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
use windows::Win32::UI::HiDpi::GetSystemMetricsForDpi;
use windows::Win32::UI::WindowsAndMessaging::{
    CURSOR_SHOWING, CURSORINFO, DI_NORMAL, DrawIconEx, GetCursorInfo, GetIconInfo, GetWindowRect,
    HICON, ICONINFO, IsZoomed, SM_CXCURSOR,
};
use windows::core::{Interface, factory};

use crate::Hwnd;
use crate::display_power;
use crate::hdr::{self, DisplayColor};
use crate::monitors::{self, rect_from_win};

#[derive(Clone, Debug)]
pub struct Frozen {
    /// Virtual-desktop rect that `image` covers (== virtual_bounds at freeze time).
    pub bounds: PxRect,
    pub image: Rgba,
    /// Cursor position at freeze time (virtual coords).
    pub cursor: PxPoint,
    /// The pointer image at freeze time, kept apart from `image` (which has it drawn in
    /// only when `include_cursor` was set). None when the pointer was hidden.
    pub cursor_shot: Option<CursorShot>,
    /// `cursor_shot` is drawn into `image`.
    pub cursor_drawn: bool,
}

pub fn freeze_all(include_cursor: bool) -> anyhow::Result<Frozen> {
    let bounds = monitors::virtual_bounds();
    if bounds.is_empty() {
        anyhow::bail!("no monitors found");
    }
    let cursor = crate::input::cursor_pos();
    let mut image = capture_region(bounds, false)?;
    let cursor_shot = cursor_shot();
    if include_cursor && let Some(c) = &cursor_shot {
        draw_cursor_shot(
            &mut image,
            PxPoint {
                x: bounds.x,
                y: bounds.y,
            },
            c,
        );
    }
    Ok(Frozen {
        bounds,
        image,
        cursor,
        cursor_drawn: include_cursor && cursor_shot.is_some(),
        cursor_shot,
    })
}

/// `freeze_all` limited to `rect` (virtual coords): the same source and freshness rules, but
/// only the outputs under `rect` are read back and converted. For a capture of one monitor
/// that needs no overlay (full-screen capture with the editor off).
pub fn freeze_rect(rect: PxRect, include_cursor: bool) -> anyhow::Result<Rgba> {
    if rect.is_empty() {
        anyhow::bail!("capture rect is empty");
    }
    let mut image = capture_region(rect, false)?;
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

/// Live capture of a virtual-desktop rect (used by scrolling capture each step).
/// Fast path: reuse a cached duplication session between calls.
pub fn capture_rect(rect: PxRect, include_cursor: bool) -> anyhow::Result<Rgba> {
    if rect.is_empty() {
        anyhow::bail!("capture rect is empty");
    }
    let mut image = capture_region(rect, true)?;
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

/// One-shot capture of a virtual-desktop rect with GDI only: no duplication session is
/// created or used. For short-lived processes (the headless CLI), where building a session
/// (device, duplication, first frame, and a GDI seed anyway) costs 0.2 to 1 s against about
/// 60 ms for one GDI copy of a 4K monitor. GDI returns correct SDR values on HDR displays.
pub fn capture_rect_once(rect: PxRect, include_cursor: bool) -> anyhow::Result<Rgba> {
    if rect.is_empty() {
        anyhow::bail!("capture rect is empty");
    }
    let _dpi = crate::dpi::ThreadPerMonitorAware::new();
    let mut image = gdi_capture(rect)?;
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

#[allow(clippy::too_many_arguments)]
/// Convert `rows` rows of source pixels into RGBA8 rows `dst_stride` bytes apart (`dst`
/// starts at the first destination row; its last row may end right after `w * 4` bytes).
fn convert_rows(
    fmt: DXGI_FORMAT,
    src: &[u8],
    pitch: usize,
    w: usize,
    rows: usize,
    dst: &mut [u8],
    dst_stride: usize,
    keep_alpha: bool,
) {
    for y in 0..rows {
        let srow = &src[y * pitch..];
        let drow = &mut dst[y * dst_stride..][..w * 4];
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
            // FP16 is handled by `hdr::ScrgbToSrgb` in `convert_into`.
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
    convert_into(
        fmt,
        src,
        pitch,
        w,
        h,
        keep_alpha,
        sdr_white_nits,
        &mut out.data,
        w * 4,
    )
    .then_some(out)
}

/// Convert a mapped `w` x `h` surface into `dst` (RGBA8 rows `dst_stride` bytes apart, e.g. a
/// sub-rectangle of a larger image), splitting rows across threads. False for formats this
/// module does not understand.
#[allow(clippy::too_many_arguments)]
fn convert_into(
    fmt: DXGI_FORMAT,
    src: &[u8],
    pitch: usize,
    w: usize,
    h: usize,
    keep_alpha: bool,
    sdr_white_nits: f32,
    dst: &mut [u8],
    dst_stride: usize,
) -> bool {
    if bytes_per_pixel(fmt).is_none() {
        return false;
    }
    if w == 0 || h == 0 {
        return true;
    }
    if fmt == DXGI_FORMAT_R16G16B16A16_FLOAT {
        hdr::ScrgbToSrgb::shared(sdr_white_nits)
            .convert_parallel(src, pitch, w, h, dst, dst_stride, false);
        return true;
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
        convert_rows(fmt, src, pitch, w, h, dst, dst_stride, keep_alpha);
        return true;
    }
    let rows_per = h.div_ceil(threads);
    std::thread::scope(|s| {
        let mut rest = dst;
        let mut y0 = 0;
        while y0 < h {
            let rows = rows_per.min(h - y0);
            // The last band may end right after its last row's pixels.
            let take = if y0 + rows == h {
                rest.len()
            } else {
                rows * dst_stride
            };
            let (band, tail) = rest.split_at_mut(take);
            rest = tail;
            let src = &src[y0 * pitch..];
            s.spawn(move || convert_rows(fmt, src, pitch, w, rows, band, dst_stride, keep_alpha));
            y0 += rows;
        }
    });
    true
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
    /// Freshness of `shadow` (see `prime_output`).
    state: ShadowState,
    /// When the shadow last received a new picture (duplication frame or GDI seed).
    image_at: Option<std::time::Instant>,
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
    /// Last capture or warm-up (`release_idle_shadows`).
    last_used: std::time::Instant,
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
/// Live captures (`capture_rect`) use GDI unless the shadow was updated this recently,
/// i.e. the duplication delivered a frame for this very call. Scrolling waits at least
/// 30 ms between steps, so an older picture may predate the last scroll.
const LIVE_MAX_AGE: std::time::Duration = std::time::Duration::from_millis(20);
/// Rows per upload when an HDR shadow is seeded from GDI (3840 px wide: 8 MB per band).
const SEED_BAND_ROWS: usize = 270;

/// Private bytes (commit charge) of this process in MB, for trace logs.
pub(crate) fn private_mb() -> f64 {
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        page_fault_count: u32,
        sizes: [usize; 8],
        private_usage: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, cb: u32) -> i32;
        fn GetCurrentProcess() -> isize;
    }
    let mut c = Counters {
        cb: std::mem::size_of::<Counters>() as u32,
        ..Default::default()
    };
    // SAFETY: live out pointer laid out like PROCESS_MEMORY_COUNTERS_EX.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    if ok != 0 {
        c.private_usage as f64 / 1048576.0
    } else {
        0.0
    }
}

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
                log::trace!("mem: d3d11 device {:.1} MB", private_mb());
                for (output, rect, color) in attached {
                    if color.advanced_color {
                        log::debug!(
                            "output {} is HDR (SDR white {} nits); duplicating as FP16",
                            color.device,
                            color.sdr_white_nits
                        );
                    }
                    let dup = duplicate(&output, &device, color.advanced_color);
                    log::trace!("mem: duplication {} {:.1} MB", color.device, private_mb());
                    outputs.push(OutputDup {
                        rect,
                        dup,
                        device: device.clone(),
                        ctx: ctx.clone(),
                        desc: None,
                        shadow: None,
                        state: ShadowState::new(display_power::epoch()),
                        image_at: None,
                        color,
                    });
                }
            }
            Ok(Session {
                layout: current_layout(),
                outputs,
                color_checked: std::time::Instant::now(),
                last_used: std::time::Instant::now(),
            })
        }
    }

    /// Hand the driver's temporary allocations back (`IDXGIDevice3::Trim`): the GDI seed's
    /// upload buffers alone kept about 80 MB committed after every warm-up (measured
    /// Nov 7 2026: a warm session 286 -> 209 MB private). Cheap; called after each warm-up and
    /// capture. Each output shares its adapter's device, so a device is trimmed once.
    fn trim(&self) {
        let mut done: Vec<*mut core::ffi::c_void> = Vec::new();
        for out in &self.outputs {
            let raw = out.device.as_raw();
            if done.contains(&raw) {
                continue;
            }
            done.push(raw);
            // SAFETY: plain calls on a live device and its immediate context; Trim requires
            // the context to be flushed first.
            unsafe {
                out.ctx.Flush();
                if let Ok(d) = out.device.cast::<IDXGIDevice3>() {
                    d.Trim();
                }
            }
        }
        log::trace!("mem: trimmed, {:.1} MB", private_mb());
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

    /// `live`: the caller needs the screen as it is now (scrolling capture), not the last
    /// picture the duplication delivered; see `OutputDup::grab`.
    ///
    /// Each output converts straight into its part of the result (no per-output tile and no
    /// copy into a pre-filled canvas: for the 6400x2160 desktop that was a 55 MB fill plus
    /// 48 MB of copies per freeze). The result starts zeroed (cheap: fresh pages); only when
    /// the outputs leave gaps are those made opaque black.
    fn capture(&mut self, rect: PxRect, first: bool, live: bool) -> anyhow::Result<Rgba> {
        let any_dup = self.outputs.iter().any(|o| o.dup.is_some());
        if session_source(&SystemDisplays, any_dup) == SessionSource::GdiWholeRect {
            // No duplication frames while the displays are off (see `prime_output`): one GDI
            // copy of the whole rect, and every shadow is re-seeded once they are back.
            for out in &mut self.outputs {
                out.state.needs_seed = true;
            }
            return gdi_capture(rect);
        }
        let mut img = Rgba::new(rect.w as u32, rect.h as u32);
        let stride = rect.w as usize * 4;
        let mut parts = Vec::with_capacity(self.outputs.len());
        for out in &mut self.outputs {
            let Some(inter) = out.rect.intersect(&rect) else {
                continue;
            };
            let local = inter.translate(-out.rect.x, -out.rect.y);
            let off =
                ((inter.y - rect.y) as usize * rect.w as usize + (inter.x - rect.x) as usize) * 4;
            out.grab_into(local, inter, first, live, &mut img.data[off..], stride)?;
            parts.push(inter);
        }
        if !union_covers(rect, &parts) {
            // Every captured pixel is opaque, so alpha 0 marks a gap between monitors.
            for p in img.data.chunks_exact_mut(4) {
                if p[3] == 0 {
                    p[3] = 255;
                }
            }
        }
        Ok(img)
    }
}

/// Whether the union of `parts` covers all of `rect`. Overlapping outputs (a duplicated or
/// mirrored display) can add up to the rect's area while leaving a gap, so areas are not
/// summed: the rect is cut at every part edge and each cell is checked.
fn union_covers(rect: PxRect, parts: &[PxRect]) -> bool {
    if rect.is_empty() {
        return true;
    }
    let clipped: Vec<PxRect> = parts.iter().filter_map(|p| p.intersect(&rect)).collect();
    let mut xs = vec![rect.x, rect.right()];
    let mut ys = vec![rect.y, rect.bottom()];
    for p in &clipped {
        xs.extend([p.x, p.right()]);
        ys.extend([p.y, p.bottom()]);
    }
    xs.sort_unstable();
    xs.dedup();
    ys.sort_unstable();
    ys.dedup();
    ys.windows(2).all(|y| {
        xs.windows(2).all(|x| {
            clipped
                .iter()
                .any(|p| p.x <= x[0] && x[1] <= p.right() && p.y <= y[0] && y[1] <= p.bottom())
        })
    })
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

// ---------------------------------------------------------------------------------------
// Capture source decision (pure, unit tested with fake displays and frame sources)
// ---------------------------------------------------------------------------------------

/// Freshness bookkeeping for one output's shadow texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ShadowState {
    /// The shadow holds a picture (duplication frame or GDI seed).
    has_image: bool,
    /// Whether the first (untrusted) duplication frame has been consumed.
    seen_first: bool,
    /// The shadow must be (re)filled from GDI before it is used: the first duplication frame
    /// arrived after an earlier seed, or the displays were off during a capture.
    needs_seed: bool,
    /// Display epoch when the shadow was last seeded or got a real frame. A different epoch
    /// means the displays went off (and maybe on again) since: frames may have been missed.
    seed_epoch: u64,
}

impl ShadowState {
    fn new(epoch: u64) -> Self {
        ShadowState {
            has_image: false,
            seen_first: false,
            needs_seed: true,
            seed_epoch: epoch,
        }
    }

    /// Whether the shadow cannot be trusted to show the screen as it is now.
    fn must_seed(&self, epoch: u64) -> bool {
        !self.has_image || self.needs_seed || self.seed_epoch != epoch
    }

    /// The shadow just received the whole desktop as it is now (real frame or GDI seed).
    fn fresh(&mut self, epoch: u64) {
        self.has_image = true;
        self.needs_seed = false;
        self.seed_epoch = epoch;
    }
}

/// The display power state as the capture decision sees it (`display_power` in the app,
/// scripted in tests).
trait Displays {
    fn is_off(&self) -> bool;
    fn epoch(&self) -> u64;
}

struct SystemDisplays;

impl Displays for SystemDisplays {
    fn is_off(&self) -> bool {
        display_power::is_off()
    }
    fn epoch(&self) -> u64 {
        display_power::epoch()
    }
}

/// What one `AcquireNextFrame` produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Frame {
    /// Nothing new within the timeout: the screen did not change since the last frame (or,
    /// while the displays are off, DWM is not feeding the duplication).
    None,
    /// The first frame of a new duplication. A placeholder (observed as an uninitialized FP16
    /// surface even on SDR content), never copied, but it stands for every change since the
    /// duplication was made.
    Placeholder,
    /// An update without new pixels (mouse only).
    PointerOnly,
    /// A real frame, copied into the shadow.
    Copied,
}

/// The duplication side of one output (`OutputDup` in the app, scripted in tests).
trait FrameSource {
    /// Acquire the next frame within `timeout_ms`. With `placeholder` set (the first frame of
    /// this duplication) any frame is reported as `Placeholder` and not copied.
    fn acquire(&mut self, timeout_ms: u32, placeholder: bool) -> anyhow::Result<Frame>;
    /// Fill the shadow from a GDI capture of the whole output. Ok(false) when it cannot.
    fn seed(&mut self) -> anyhow::Result<bool>;
}

/// Where a capture of a session's rect comes from, before any output is looked at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionSource {
    /// One GDI copy of the whole rect: the displays are off, so no duplication delivers
    /// frames, and every shadow is marked for a re-seed.
    GdiWholeRect,
    /// Each output decides for itself (`prime_output`).
    PerOutput,
}

fn session_source(displays: &impl Displays, any_duplication: bool) -> SessionSource {
    if any_duplication && displays.is_off() {
        SessionSource::GdiWholeRect
    } else {
        SessionSource::PerOutput
    }
}

/// Bring one output's shadow up to date and decide whether it can serve a capture: pick up a
/// pending frame, then re-seed from GDI when the shadow may predate the screen. Ok(true) when
/// the shadow holds the screen as it is now; Ok(false) when GDI has to serve this capture.
///
/// The two ways a shadow went stale before Nov 7 2026, both covered by tests:
/// - **Displays off.** DWM feeds no frames then, so "no frame" no longer means "unchanged"
///   (measured: every freeze of an idle-off session returned the warm-up picture). GDI serves
///   those captures, and the shadow is re-seeded once the displays are back (the epoch moved).
/// - **Late first frame.** When the warm-up's wait for the first frame timed out, the shadow
///   was seeded first; the placeholder frame that arrives later stands for every change since
///   and forces another seed instead of being dropped silently.
fn prime_output(
    state: &mut ShadowState,
    displays: &impl Displays,
    src: &mut impl FrameSource,
    timeout_ms: u32,
) -> anyhow::Result<bool> {
    if displays.is_off() {
        state.needs_seed = true;
        return Ok(false);
    }
    let epoch = displays.epoch();
    match src.acquire(timeout_ms, !state.seen_first)? {
        Frame::Placeholder => {
            state.seen_first = true;
            state.needs_seed = true;
        }
        Frame::Copied => {
            state.seen_first = true;
            state.fresh(epoch);
        }
        Frame::PointerOnly | Frame::None => {}
    }
    if state.must_seed(epoch) {
        match src.seed() {
            Ok(true) => state.fresh(epoch),
            Ok(false) => {}
            Err(e) => log::debug!("seeding failed: {e}"),
        }
    }
    Ok(state.has_image && !state.must_seed(epoch))
}

impl FrameSource for OutputDup {
    fn acquire(&mut self, timeout_ms: u32, placeholder: bool) -> anyhow::Result<Frame> {
        let Some(dup) = self.dup.clone() else {
            return Ok(Frame::None);
        };
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut res: Option<IDXGIResource> = None;
        // SAFETY: out pointers reference live locals; a frame acquired here is released below.
        let acquired = unsafe { dup.AcquireNextFrame(timeout_ms, &mut info, &mut res) };
        match acquired {
            Ok(()) => {
                let copy = (|| -> anyhow::Result<Frame> {
                    if placeholder {
                        return Ok(Frame::Placeholder);
                    }
                    if info.LastPresentTime == 0 {
                        return Ok(Frame::PointerOnly);
                    }
                    let res = res.ok_or_else(|| anyhow::anyhow!("no desktop resource"))?;
                    let tex: ID3D11Texture2D = res.cast()?;
                    let mut fd = D3D11_TEXTURE2D_DESC::default();
                    // SAFETY: out pointer is a live local.
                    unsafe { tex.GetDesc(&mut fd) };
                    if self.hdr() && fd.Format != DXGI_FORMAT_R16G16B16A16_FLOAT {
                        // An 8-bit frame of an HDR output would be blown out (see `duplicate`).
                        anyhow::bail!("HDR output delivered {:?} frames", fd.Format);
                    }
                    self.ensure_shadow(fd.Width, fd.Height, fd.Format)?;
                    let Some(shadow) = &self.shadow else {
                        return Ok(Frame::None);
                    };
                    // SAFETY: both textures belong to this device and now have identical size
                    // and format.
                    unsafe { self.ctx.CopyResource(shadow, &tex) };
                    self.image_at = Some(std::time::Instant::now());
                    Ok(Frame::Copied)
                })();
                // Make sure the copy is submitted before DXGI recycles the frame.
                // SAFETY: plain flush of the immediate context.
                unsafe { self.ctx.Flush() };
                // SAFETY: balanced with the successful AcquireNextFrame.
                let _ = unsafe { dup.ReleaseFrame() };
                copy
            }
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => Ok(Frame::None),
            Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => Err(AccessLost.into()),
            Err(e) => Err(e.into()),
        }
    }

    fn seed(&mut self) -> anyhow::Result<bool> {
        let ok = self.seed_from_gdi()?;
        if ok {
            self.image_at = Some(std::time::Instant::now());
        }
        Ok(ok)
    }
}

impl OutputDup {
    /// (Re)create the shadow texture when the surface size or format changes. It lives on the
    /// GPU; read-backs go through a staging texture made for each read (`read_back_into`).
    fn ensure_shadow(
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
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_FLAG(0).0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: D3D11_RESOURCE_MISC_FLAG(0).0 as u32,
        };
        // The old picture is gone with the old texture (`prime` notices).
        self.shadow = None;
        let mut shadow = None;
        // SAFETY: texture creation uses a fully initialized desc.
        unsafe {
            self.device
                .CreateTexture2D(&desc, None, Some(&mut shadow))?
        };
        log::trace!("mem: shadow {:.1} MB", private_mb());
        self.shadow = shadow;
        self.desc = Some(desc);
        Ok(())
    }

    /// `prime_output` for this output with the real display state. No duplication: Ok(false).
    fn prime(&mut self, timeout_ms: u32) -> anyhow::Result<bool> {
        if self.dup.is_none() {
            return Ok(false);
        }
        let mut state = self.state;
        let r = prime_output(&mut state, &SystemDisplays, self, timeout_ms);
        // A failed `ensure_shadow` leaves no texture, and no picture with it.
        if self.shadow.is_none() {
            state.has_image = false;
        }
        self.state = state;
        r
    }

    /// Write the pixels of `local` (output-local px) into `dst` (rows `dst_stride` bytes
    /// apart). `virt` is the same rect in virtual coords, used by the GDI fallback.
    ///
    /// `live`: when the shadow has not been updated for `LIVE_MAX_AGE` the region is taken
    /// with GDI instead. A duplication delivers nothing while the screen is static, which
    /// is fine for a freeze, but a scrolling capture must see the page as it is after each
    /// scroll even if no frame has arrived yet.
    fn grab_into(
        &mut self,
        local: PxRect,
        virt: PxRect,
        first: bool,
        live: bool,
        dst: &mut [u8],
        dst_stride: usize,
    ) -> anyhow::Result<()> {
        if self.dup.is_some() {
            let timeout = if first {
                FIRST_FRAME_TIMEOUT_MS
            } else {
                CACHED_FRAME_TIMEOUT_MS
            };
            let state = self.prime(timeout);
            let stale = self.image_at.is_none_or(|t| t.elapsed() > LIVE_MAX_AGE);
            match state {
                Ok(true) if live && stale => {}
                Ok(true) => match self.read_back_into(local, dst, dst_stride) {
                    Ok(true) => return Ok(()),
                    Ok(false) => {}
                    Err(e) => log::debug!("duplication read-back failed: {e}"),
                },
                Ok(false) => {}
                Err(e) if e.is::<AccessLost>() => return Err(e),
                Err(e) => log::debug!("duplication refresh failed: {e}"),
            }
        }
        let img = gdi_capture(virt)?;
        let row = img.width as usize * 4;
        for (y, src) in img.data.chunks_exact(row).enumerate() {
            dst[y * dst_stride..][..row].copy_from_slice(src);
        }
        Ok(())
    }

    /// A static desktop never delivers a trustworthy first duplication frame, so fill the
    /// shadow texture from a GDI capture of the whole output (again whenever the shadow may
    /// predate the screen). Later frames overwrite it. Ok(false) when the format does not
    /// allow a direct upload.
    fn seed_from_gdi(&mut self) -> anyhow::Result<bool> {
        if self.shadow.is_none() {
            self.ensure_shadow(self.rect.w as u32, self.rect.h as u32, self.shadow_format())?;
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
        let w = desc.Width as usize;
        if desc.Format == DXGI_FORMAT_R16G16B16A16_FLOAT {
            // GDI returns correct SDR values on an HDR output (verified); lift them into scRGB
            // at the display's SDR white so they match what later FP16 frames contain. Uploaded
            // in bands: a whole 4K FP16 picture would be a 66 MB buffer on top of the GDI copy.
            let img = gdi_capture(full)?;
            let table = hdr::srgb8_to_scrgb_half(self.color.sdr_white_nits);
            let one = hdr::f32_to_half(1.0);
            let rows = SEED_BAND_ROWS.min(desc.Height as usize).max(1);
            let mut halves = vec![0u16; w * 4 * rows];
            for (band, src) in img.data.chunks(w * 4 * rows).enumerate() {
                let n = src.len() / (w * 4);
                for (d, s) in halves.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                    d[0] = table[s[0] as usize];
                    d[1] = table[s[1] as usize];
                    d[2] = table[s[2] as usize];
                    d[3] = one;
                }
                let top = (band * rows) as u32;
                let b = D3D11_BOX {
                    left: 0,
                    top,
                    front: 0,
                    right: desc.Width,
                    bottom: top + n as u32,
                    back: 1,
                };
                // SAFETY: `halves` holds at least `n` rows of Width tightly packed RGBA16F
                // pixels, which is what the box covers.
                unsafe {
                    self.ctx.UpdateSubresource(
                        shadow,
                        0,
                        Some(&b),
                        halves.as_ptr() as *const _,
                        desc.Width * 8,
                        0,
                    );
                }
            }
            log::trace!("mem: hdr seed uploaded {:.1} MB", private_mb());
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
        log::trace!("mem: sdr seed uploaded {:.1} MB", private_mb());
        Ok(true)
    }

    /// Copy `local` of the shadow to the CPU and convert it into `dst` (rows `dst_stride`
    /// bytes apart). Ok(false) when the shadow cannot serve that rect.
    ///
    /// The staging texture is made for this read at the size of `local` and dropped right
    /// after: kept between captures, the two full-output staging textures were 81 MB of
    /// committed memory (4K FP16 + 1440p BGRA) for a few ms of use per capture.
    fn read_back_into(
        &self,
        local: PxRect,
        dst: &mut [u8],
        dst_stride: usize,
    ) -> anyhow::Result<bool> {
        let (Some(shadow), Some(desc)) = (&self.shadow, &self.desc) else {
            return Ok(false);
        };
        if local.is_empty()
            || local.intersect(&PxRect::new(0, 0, desc.Width as i32, desc.Height as i32))
                != Some(local)
        {
            // The surface does not cover the whole output rect (size change in flight).
            return Ok(false);
        }
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
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Width: local.w as u32,
            Height: local.h as u32,
            Usage: D3D11_USAGE_STAGING,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            BindFlags: 0,
            MiscFlags: 0,
            ..*desc
        };
        // SAFETY: the staging texture is mapped only for the conversion and unmapped right after;
        // the slice covers rows [0, h) of the mapped region written by the copy above.
        unsafe {
            let t0 = std::time::Instant::now();
            let mut staging = None;
            self.device
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))?;
            let staging = staging.ok_or_else(|| anyhow::anyhow!("no staging texture"))?;
            let t_create = t0.elapsed();
            self.ctx
                .CopySubresourceRegion(&staging, 0, 0, 0, 0, shadow, 0, Some(&b));
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.ctx
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let t_map = t0.elapsed();
            let (w, h) = (local.w as usize, local.h as usize);
            let pitch = mapped.RowPitch as usize;
            let len = pitch * (h - 1) + w * bpp;
            let src = std::slice::from_raw_parts(mapped.pData as *const u8, len);
            let ok = convert_into(
                desc.Format,
                src,
                pitch,
                w,
                h,
                false,
                self.color.sdr_white_nits,
                dst,
                dst_stride,
            );
            self.ctx.Unmap(&staging, 0);
            log::trace!(
                "read_back {}x{} {:?}: create {t_create:?}, copy+map {:?}, convert {:?}",
                w,
                h,
                desc.Format,
                t_map - t_create,
                t0.elapsed() - t_map
            );
            Ok(ok)
        }
    }
}

/// Why a cached session cannot be used as it is, if it cannot.
fn session_invalid(session: &mut Option<Session>) -> Option<&'static str> {
    let Some(s) = session.as_mut() else {
        return Some("no session");
    };
    if s.layout != current_layout() {
        Some("monitor layout changed")
    } else if !s.recheck_colors() {
        Some("HDR state changed")
    } else {
        None
    }
}

fn new_session(context: &str) -> Option<Session> {
    let t0 = std::time::Instant::now();
    let s = match Session::create() {
        Ok(s) if !s.outputs.is_empty() => Some(s),
        Ok(_) => None,
        Err(e) => {
            log::debug!("{context}: duplication session unavailable: {e}");
            None
        }
    };
    log::debug!(
        "{context}: duplication session created in {:.1} ms",
        t0.elapsed().as_secs_f64() * 1000.0
    );
    s
}

/// Set while a background warm-up (start-up, re-warm-up, display wake) holds `SESSION`.
static WARMING: AtomicBool = AtomicBool::new(false);

/// What a capture does when it finds the session lock taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LockBusy {
    /// Wait for the holder (another capture: it is quick, and the session stays warm).
    Wait,
    /// Do not wait: a warm-up can hold the lock for several hundred ms (session creation,
    /// a 300 ms wait for each first frame, GDI seeds); GDI serves this capture at once.
    Gdi,
}

fn on_lock_busy(warming: bool) -> LockBusy {
    if warming {
        LockBusy::Gdi
    } else {
        LockBusy::Wait
    }
}

/// The session lock for a capture, or None when a warm-up holds it (use GDI).
fn lock_for_capture() -> Option<std::sync::MutexGuard<'static, Option<Session>>> {
    match SESSION.try_lock() {
        Ok(g) => Some(g),
        Err(std::sync::TryLockError::Poisoned(p)) => Some(p.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => {
            match on_lock_busy(WARMING.load(Ordering::SeqCst)) {
                LockBusy::Gdi => None,
                LockBusy::Wait => Some(SESSION.lock().unwrap_or_else(|p| p.into_inner())),
            }
        }
    }
}

/// The session lock for a background warm-up; captures do not wait while it is held.
struct WarmGuard(std::sync::MutexGuard<'static, Option<Session>>);

impl WarmGuard {
    fn lock() -> Self {
        let g = SESSION.lock().unwrap_or_else(|p| p.into_inner());
        WARMING.store(true, Ordering::SeqCst);
        WarmGuard(g)
    }
}

impl Drop for WarmGuard {
    fn drop(&mut self) {
        WARMING.store(false, Ordering::SeqCst);
    }
}

impl std::ops::Deref for WarmGuard {
    type Target = Option<Session>;
    fn deref(&self) -> &Option<Session> {
        &self.0
    }
}

impl std::ops::DerefMut for WarmGuard {
    fn deref_mut(&mut self) -> &mut Option<Session> {
        &mut self.0
    }
}

fn capture_region(rect: PxRect, live: bool) -> anyhow::Result<Rgba> {
    display_power::watch(revalidate_after_wake);
    // Aware for the whole capture, not just DuplicateOutput1: a duplication made from an
    // aware thread delivers no frames to an unaware one, and GDI coordinates on a scaled
    // monitor are rescaled for unaware callers (tools and tests have no manifest).
    let _dpi = crate::dpi::ThreadPerMonitorAware::new();
    let Some(mut guard) = lock_for_capture() else {
        log::debug!("capture: warm-up in progress; GDI serves this capture");
        return gdi_capture(rect);
    };
    let r = capture_with(&mut guard, rect, live);
    if let Some(s) = guard.as_mut() {
        s.last_used = std::time::Instant::now();
        s.trim();
    }
    r
}

fn capture_with(guard: &mut Option<Session>, rect: PxRect, live: bool) -> anyhow::Result<Rgba> {
    for attempt in 0..2 {
        let invalid = session_invalid(guard);
        let first = invalid.is_some();
        if let Some(why) = invalid {
            log::debug!("capture: building a duplication session ({why})");
            *guard = new_session("capture");
        }
        let Some(session) = guard.as_mut() else {
            return gdi_capture(rect);
        };
        match session.capture(rect, first, live) {
            Ok(img) => return Ok(img),
            Err(e) if e.is::<AccessLost>() && attempt == 0 => {
                // Mode change, secure desktop, or driver reset: rebuild the session once.
                log::debug!("capture: duplication access lost; rebuilding the session");
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

/// The displays came back on after being off: frames shown meanwhile never reached the
/// duplication, so re-seed the warm session (if there is one) now instead of on the next
/// capture. Runs on its own thread (`display_power` spawns it).
///
/// Captures never wait for this: while it holds the session lock they use GDI, and it only
/// picks up pending frames (2 ms wait each, the duplications already exist) before seeding.
fn revalidate_after_wake() {
    // Let the desktop finish repainting after the wake.
    std::thread::sleep(std::time::Duration::from_millis(500));
    let warm = match SESSION.try_lock() {
        Ok(g) => g
            .as_ref()
            .is_some_and(|s| s.outputs.iter().any(|o| o.dup.is_some())),
        // A capture or warm-up is running right now; it validates the session itself.
        Err(_) => false,
    };
    if warm {
        log::debug!("displays back on: re-seeding the capture session");
        warm_up_with(CACHED_FRAME_TIMEOUT_MS);
    }
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
    warm_up_with(FIRST_FRAME_TIMEOUT_MS);
}

/// `warm_up` with another wait for each output's first duplication frame. Diagnostics and
/// tests only: a 0 ms wait simulates a start-up where the first frame arrives late.
#[doc(hidden)]
pub fn warm_up_with(first_frame_timeout_ms: u32) {
    if duplication_lent() {
        log::debug!("capture warm-up skipped: duplication lent to the recorder");
        return;
    }
    display_power::watch(revalidate_after_wake);
    let _dpi = crate::dpi::ThreadPerMonitorAware::new();
    let t0 = std::time::Instant::now();
    let mut guard = WarmGuard::lock();
    for attempt in 0..2 {
        if let Some(why) = session_invalid(&mut guard) {
            log::debug!("capture warm-up: building a duplication session ({why})");
            *guard = new_session("capture warm-up");
        }
        let Some(session) = guard.as_mut() else {
            return;
        };
        let mut lost = false;
        for out in &mut session.outputs {
            match out.prime(first_frame_timeout_ms) {
                Ok(_) => {}
                Err(e) if e.is::<AccessLost>() => lost = true,
                Err(e) => log::debug!("capture warm-up: {} refresh failed: {e}", out.color.device),
            }
        }
        if !lost {
            session.last_used = std::time::Instant::now();
            session.trim();
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

/// Drop the shadow textures of a session unused for `max_idle` (the device and duplications
/// stay). Returns how long until it is worth calling again (an hour when there is nothing to
/// release).
///
/// The shadows (78 MB committed here: 4K FP16 + 1440p BGRA) only matter when nothing on an
/// output changed since the previous frame was taken: otherwise the next capture's frame is
/// the whole desktop as it is now and refills the shadow (measured: a capture after the drop
/// costs the same as a warm one). After a quiet period something has almost always changed
/// (the taskbar clock, a caret, the overlay going away). When nothing did, that output is
/// seeded from GDI, the same as the first capture of a session: slower, never stale.
pub fn release_idle_shadows(max_idle: std::time::Duration) -> std::time::Duration {
    let Ok(mut guard) = SESSION.try_lock() else {
        return max_idle;
    };
    let nothing = std::time::Duration::from_secs(3600).max(max_idle);
    let Some(session) = guard.as_mut() else {
        return nothing;
    };
    if session.outputs.iter().all(|o| o.shadow.is_none()) {
        // Nothing to release until a capture refills them.
        return nothing;
    }
    let idle = session.last_used.elapsed();
    if idle < max_idle {
        return max_idle - idle;
    }
    for out in &mut session.outputs {
        out.shadow = None;
        out.desc = None;
        out.state.has_image = false;
        out.image_at = None;
    }
    session.trim();
    log::debug!(
        "capture: shadows released after {:.0} s idle ({:.1} MB private)",
        idle.as_secs_f64(),
        private_mb()
    );
    nothing
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
/// systems), skip WGC while that helper thread is still blocked and for a while after, instead
/// of making every capture wait for the timeout and leave one more blocked thread behind.
static WGC_GATE: crate::hang::HangGate =
    crate::hang::HangGate::new(std::time::Duration::from_secs(300));

/// Run WGC on a private MTA thread: the caller's apartment (UI thread = STA) is irrelevant
/// and a window that never delivers a frame costs this call at most `WGC_TIMEOUT`.
fn wgc_with_timeout(hwnd: Hwnd, include_cursor: bool) -> anyhow::Result<Rgba> {
    if crate::wgc_memo::known_hung() {
        anyhow::bail!("Windows.Graphics.Capture skipped (it hung on this PC recently)");
    }
    match WGC_GATE.run("stinkshot-wgc", WGC_TIMEOUT, move || {
        crate::util::ensure_com();
        wgc_capture(HWND(hwnd as *mut _), include_cursor)
    }) {
        Ok(r) => r,
        Err(crate::hang::Hang::Skipped(why)) => {
            anyhow::bail!("Windows.Graphics.Capture skipped ({why})")
        }
        Err(crate::hang::Hang::TimedOut) => {
            crate::wgc_memo::remember_hang();
            anyhow::bail!("Windows.Graphics.Capture timed out")
        }
        Err(e) => anyhow::bail!("Windows.Graphics.Capture: {e}"),
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

/// Recover straight-alpha RGBA from the same glyph rendered over black and over white
/// (inverting pixels become black with a white outline, see `cursor_from_renders`).
pub(crate) fn combine_black_white(black: &[u8], white: &[u8], w: u32, h: u32) -> Rgba {
    stinkshot_core::annotate::cursor_from_renders(black, white, w, h)
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

/// The pointer at the moment of a capture, apart from the screen pixels.
#[derive(Clone, Debug)]
pub struct CursorShot {
    /// Straight-alpha pointer image at the size Windows draws it.
    pub image: Rgba,
    /// Where the image's top-left sits, virtual coords (position minus hotspot).
    pub top_left: PxPoint,
    /// Hotspot inside the image.
    pub hotspot: PxPoint,
}

impl CursorShot {
    /// The image's rect, virtual coords.
    pub fn rect(&self) -> PxRect {
        PxRect::new(
            self.top_left.x,
            self.top_left.y,
            self.image.width as i32,
            self.image.height as i32,
        )
    }
}

fn try_cursor_shot() -> anyhow::Result<Option<CursorShot>> {
    let mut ci = CURSORINFO {
        cbSize: std::mem::size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: cbSize is set; GDI bitmaps from GetIconInfo are deleted below.
    unsafe {
        GetCursorInfo(&mut ci)?;
        if ci.flags.0 & CURSOR_SHOWING.0 == 0 || ci.hCursor.is_invalid() {
            return Ok(None);
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
        let max = stinkshot_core::annotate::MAX_CURSOR_SIDE as i32;
        let Some((cw, ch)) = size.filter(|(w, h)| *w > 0 && *h > 0 && *w <= max && *h <= max)
        else {
            return Ok(None);
        };
        // The bitmap is the cursor at 96 dpi; Windows draws it scaled for the monitor's
        // DPI, so render it at that size (bigger accessibility sizes are already big).
        let pos = PxPoint {
            x: ci.ptScreenPos.x,
            y: ci.ptScreenPos.y,
        };
        let mon_scale = crate::monitors::list()
            .iter()
            .find(|m| m.rect.contains(pos))
            .map_or(1.0, |m| m.scale);
        let base = GetSystemMetricsForDpi(SM_CXCURSOR, 96).max(1);
        let k = cursor_draw_scale(cw, base, mon_scale);
        let (dw, dh) = (
            ((cw as f32 * k).round() as i32).min(max),
            ((ch as f32 * k).round() as i32).min(max),
        );
        let hot = PxPoint {
            x: (ii.xHotspot as f32 * dw as f32 / cw as f32).round() as i32,
            y: (ii.yHotspot as f32 * dh as f32 / ch as f32).round() as i32,
        };
        // DrawIconEx applies the AND/XOR masks itself; two passes recover alpha.
        let black = render_icon(hicon, dw, dh, 0)?;
        let white = render_icon(hicon, dw, dh, 255)?;
        let image = combine_black_white(&black, &white, dw as u32, dh as u32);
        Ok(Some(CursorShot {
            image,
            top_left: PxPoint {
                x: pos.x - hot.x,
                y: pos.y - hot.y,
            },
            hotspot: hot,
        }))
    }
}

/// How much to enlarge a `bitmap_w` px cursor bitmap on a monitor at `mon_scale`: bitmaps at
/// the 96 dpi size (`base_w`, SM_CXCURSOR at 96 dpi) get the monitor's scale, larger ones
/// (accessibility pointer sizes) are drawn as they are.
pub(crate) fn cursor_draw_scale(bitmap_w: i32, base_w: i32, mon_scale: f32) -> f32 {
    if bitmap_w <= base_w && mon_scale > 1.01 {
        mon_scale
    } else {
        1.0
    }
}

/// The current pointer image and where it is, or None when it is hidden (or on failure).
pub fn cursor_shot() -> Option<CursorShot> {
    let _dpi = crate::dpi::ThreadPerMonitorAware::new();
    try_cursor_shot()
        .map_err(|e| log::debug!("cursor_shot: {e}"))
        .ok()
        .flatten()
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
    if let Some(c) = cursor_shot() {
        draw_cursor_shot(img, origin, &c);
    }
}

/// Draw a pointer taken with `cursor_shot` into `img` (top-left at `origin`, virtual coords).
pub fn draw_cursor_shot(img: &mut Rgba, origin: PxPoint, c: &CursorShot) {
    blend_over(
        img,
        &c.image,
        c.top_left.x - origin.x,
        c.top_left.y - origin.y,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::VecDeque;

    /// Scripted display power: off or on, and the epoch (bumped on each off/on transition).
    struct FakeDisplays {
        off: Cell<bool>,
        epoch: Cell<u64>,
    }

    impl FakeDisplays {
        fn on() -> Self {
            FakeDisplays {
                off: Cell::new(false),
                epoch: Cell::new(0),
            }
        }
        fn set_off(&self, off: bool) {
            if self.off.replace(off) != off {
                self.epoch.set(self.epoch.get() + 1);
            }
        }
    }

    impl Displays for FakeDisplays {
        fn is_off(&self) -> bool {
            self.off.get()
        }
        fn epoch(&self) -> u64 {
            self.epoch.get()
        }
    }

    /// Scripted duplication: what each acquire sees (frames DWM delivered meanwhile), and a
    /// count of GDI seeds. A placeholder acquire returns `Placeholder` for any pending frame,
    /// like the real source.
    #[derive(Default)]
    struct FakeSource {
        pending: VecDeque<Frame>,
        seeds: usize,
        copies: usize,
    }

    impl FakeSource {
        fn deliver(&mut self, f: Frame) {
            self.pending.push_back(f);
        }
    }

    impl FrameSource for FakeSource {
        fn acquire(&mut self, _timeout_ms: u32, placeholder: bool) -> anyhow::Result<Frame> {
            let f = self.pending.pop_front().unwrap_or(Frame::None);
            Ok(match f {
                Frame::None => Frame::None,
                _ if placeholder => Frame::Placeholder,
                Frame::Copied => {
                    self.copies += 1;
                    Frame::Copied
                }
                other => other,
            })
        }
        fn seed(&mut self) -> anyhow::Result<bool> {
            self.seeds += 1;
            Ok(true)
        }
    }

    /// The warm-up: a new duplication whose first frame arrives within the wait.
    fn warmed(d: &FakeDisplays, src: &mut FakeSource) -> ShadowState {
        let mut st = ShadowState::new(d.epoch());
        src.deliver(Frame::Copied);
        assert!(prime_output(&mut st, d, src, FIRST_FRAME_TIMEOUT_MS).unwrap());
        st
    }

    #[test]
    fn warm_up_skips_the_placeholder_and_seeds() {
        let d = FakeDisplays::on();
        let mut src = FakeSource::default();
        let st = warmed(&d, &mut src);
        assert_eq!(
            (src.seeds, src.copies),
            (1, 0),
            "first frame is never copied"
        );
        assert!(st.seen_first && st.has_image && !st.must_seed(d.epoch()));
    }

    #[test]
    fn static_screen_serves_the_shadow_and_real_frames_replace_it() {
        let d = FakeDisplays::on();
        let mut src = FakeSource::default();
        let mut st = warmed(&d, &mut src);
        // Nothing changed: no frame, the shadow is current.
        assert!(prime_output(&mut st, &d, &mut src, 2).unwrap());
        // A mouse-only update carries no pixels.
        src.deliver(Frame::PointerOnly);
        assert!(prime_output(&mut st, &d, &mut src, 2).unwrap());
        // A real frame is the whole desktop now: copied, no seed.
        src.deliver(Frame::Copied);
        assert!(prime_output(&mut st, &d, &mut src, 2).unwrap());
        assert_eq!((src.seeds, src.copies), (1, 1));
    }

    /// Stale-frame cause 1: the displays go off. DWM delivers nothing, which looked like an
    /// unchanged screen, so every capture returned the picture from before.
    #[test]
    fn displays_off_uses_gdi_and_reseeds_after_wake() {
        let d = FakeDisplays::on();
        let mut src = FakeSource::default();
        let mut st = warmed(&d, &mut src);
        d.set_off(true);
        // The screen changes while off, but no frame arrives.
        assert!(
            !prime_output(&mut st, &d, &mut src, 2).unwrap(),
            "GDI serves it"
        );
        assert_eq!(
            session_source(&d, true),
            SessionSource::GdiWholeRect,
            "one GDI copy of the whole rect"
        );
        assert_eq!(session_source(&d, false), SessionSource::PerOutput);
        assert!(st.needs_seed);
        d.set_off(false);
        assert_eq!(session_source(&d, true), SessionSource::PerOutput);
        // Back on, still no frame (static screen): the old shadow must not be served.
        assert!(prime_output(&mut st, &d, &mut src, 2).unwrap());
        assert_eq!(src.seeds, 2, "re-seeded from GDI after the wake");
        // And only once.
        assert!(prime_output(&mut st, &d, &mut src, 2).unwrap());
        assert_eq!(src.seeds, 2);
    }

    /// The displays went off and on again with no capture in between: the epoch moved, so
    /// the shadow (validated before) is re-seeded even though no capture saw them off.
    #[test]
    fn off_and_on_between_captures_reseeds() {
        let d = FakeDisplays::on();
        let mut src = FakeSource::default();
        let mut st = warmed(&d, &mut src);
        d.set_off(true);
        d.set_off(false);
        assert!(prime_output(&mut st, &d, &mut src, 2).unwrap());
        assert_eq!(src.seeds, 2);
    }

    /// Stale-frame cause 2: the warm-up's wait for the first frame timed out, the shadow was
    /// seeded, and the placeholder arrived later carrying every change since. Dropping it
    /// silently kept the warm-up picture until the next change on screen.
    #[test]
    fn late_first_frame_forces_a_new_seed() {
        let d = FakeDisplays::on();
        let mut src = FakeSource::default();
        let mut st = ShadowState::new(d.epoch());
        // Warm-up: the first frame has not arrived yet.
        assert!(prime_output(&mut st, &d, &mut src, 0).unwrap());
        assert_eq!(src.seeds, 1);
        assert!(!st.seen_first);
        // The screen changes; the first frame arrives late and is the only one.
        src.deliver(Frame::Copied);
        assert!(prime_output(&mut st, &d, &mut src, 2).unwrap());
        assert_eq!(
            (src.seeds, src.copies),
            (2, 0),
            "placeholder -> fresh seed, not copied"
        );
        assert!(st.seen_first);
        // Later frames are trusted.
        src.deliver(Frame::Copied);
        assert!(prime_output(&mut st, &d, &mut src, 2).unwrap());
        assert_eq!((src.seeds, src.copies), (2, 1));
    }

    #[test]
    fn released_shadow_is_refilled_by_a_frame_or_a_seed() {
        let d = FakeDisplays::on();
        let mut src = FakeSource::default();
        let mut st = warmed(&d, &mut src);
        // `release_idle_shadows`: the texture and its picture are gone.
        st.has_image = false;
        src.deliver(Frame::Copied);
        assert!(prime_output(&mut st, &d, &mut src, 2).unwrap());
        assert_eq!(
            (src.seeds, src.copies),
            (1, 1),
            "a pending frame refills it"
        );
        st.has_image = false;
        assert!(prime_output(&mut st, &d, &mut src, 2).unwrap());
        assert_eq!(src.seeds, 2, "nothing changed: seeded from GDI");
    }

    #[test]
    fn failed_seed_falls_back_to_gdi() {
        struct NoSeed;
        impl FrameSource for NoSeed {
            fn acquire(&mut self, _: u32, _: bool) -> anyhow::Result<Frame> {
                Ok(Frame::None)
            }
            fn seed(&mut self) -> anyhow::Result<bool> {
                anyhow::bail!("GDI failed")
            }
        }
        let d = FakeDisplays::on();
        let mut st = ShadowState::new(0);
        assert!(!prime_output(&mut st, &d, &mut NoSeed, 300).unwrap());
    }

    #[test]
    fn captures_wait_for_captures_but_not_for_warm_ups() {
        assert_eq!(on_lock_busy(false), LockBusy::Wait);
        assert_eq!(on_lock_busy(true), LockBusy::Gdi);
    }

    #[test]
    fn union_coverage_ignores_overlap() {
        let rect = PxRect::new(0, 0, 200, 100);
        let left = PxRect::new(0, 0, 100, 100);
        assert!(union_covers(rect, &[left, PxRect::new(100, 0, 100, 100)]));
        // Overlapping copies of one output add up to the rect's area but leave a gap.
        assert!(!union_covers(rect, &[left, left]));
        assert!(!union_covers(rect, &[left, PxRect::new(120, 0, 80, 100)]));
        // 1440p next to 2160p: the area below the short one is a gap.
        let tall = PxRect::new(0, 0, 3840, 2160);
        let short = PxRect::new(3840, 0, 2560, 1440);
        assert!(!union_covers(PxRect::new(0, 0, 6400, 2160), &[tall, short]));
        assert!(union_covers(PxRect::new(0, 0, 6400, 1440), &[tall, short]));
        // Parts outside the rect are clipped; a rect inside one part is covered.
        let big = PxRect::new(-100, -100, 1000, 1000);
        assert!(union_covers(PxRect::new(10, 10, 5, 5), &[big]));
        assert!(!union_covers(rect, &[]));
        assert!(union_covers(PxRect::new(0, 0, 0, 0), &[]));
    }

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
        convert_rows(
            DXGI_FORMAT_B8G8R8A8_UNORM,
            &src,
            pitch,
            w,
            h,
            &mut ser,
            w * 4,
            false,
        );
        assert_eq!(par.data, ser);
        // Into a sub-rectangle of a wider image: same pixels, the rest untouched.
        let (dw, ox, oy) = (w + 50, 20usize, 3usize);
        let mut big = vec![7u8; dw * 4 * (h + 5)];
        let off = (oy * dw + ox) * 4;
        assert!(convert_into(
            DXGI_FORMAT_B8G8R8A8_UNORM,
            &src,
            pitch,
            w,
            h,
            false,
            80.0,
            &mut big[off..],
            dw * 4
        ));
        for y in 0..h {
            let row = &big[((oy + y) * dw + ox) * 4..][..w * 4];
            assert_eq!(row, &ser[y * w * 4..][..w * 4]);
        }
        assert_eq!(big[off - 1], 7);
        assert_eq!(big[off + w * 4], 7);
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

#[cfg(test)]
mod cursor_scale_tests {
    use super::cursor_draw_scale;

    #[test]
    fn standard_cursors_follow_the_monitor_scale() {
        assert_eq!(cursor_draw_scale(32, 32, 1.5), 1.5);
        assert_eq!(cursor_draw_scale(32, 32, 1.0), 1.0);
        // Already-big accessibility pointers are drawn as they are.
        assert_eq!(cursor_draw_scale(64, 32, 1.5), 1.0);
    }
}
