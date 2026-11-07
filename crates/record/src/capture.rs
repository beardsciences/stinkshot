//! Screen capture of the monitor(s) a region spans, cropped into one CPU-readable staging
//! texture.
//!
//! Primary backend: Windows.Graphics.Capture (one frame pool per monitor, free threaded,
//! polled by the video thread). Fallback backend: DXGI Desktop Duplication, used when WGC
//! cannot create a session (it errors, or on some systems simply never answers; session
//! creation therefore runs on a helper thread with a timeout).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use stinkshot_core::PxRect;
use stinkshot_platform::hdr;
use windows::Foundation::TimeSpan;
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{HMODULE, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT,
    DXGI_MODE_ROTATION_IDENTITY, DXGI_MODE_ROTATION_UNSPECIFIED, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};
use windows::core::{BOOL, Interface};

use crate::mf::{ComGuard, SendCom};

/// How long WGC gets to create its sessions before we fall back to Desktop Duplication.
const WGC_INIT_TIMEOUT: Duration = Duration::from_secs(3);

/// When WGC session creation last hung. A hang (not an ordinary error, which may be
/// transient such as a display reconfiguration) makes later recordings skip the wait for a
/// while.
static WGC_HUNG_AT: Mutex<Option<Instant>> = Mutex::new(None);
const WGC_COOLDOWN: Duration = Duration::from_secs(300);

/// Consecutive duplicated frames in an unsupported pixel format before giving up.
const MAX_BAD_FORMAT_FRAMES: u32 = 30;

/// How often a recording re-reads each display's HDR state and SDR white level, so turning
/// HDR on or off or moving the SDR brightness slider mid-recording keeps the colors right.
const COLOR_RECHECK: Duration = Duration::from_secs(1);
/// Soonest re-read after a sign that the color state changed (lost duplication, frames in an
/// unexpected format).
const COLOR_RECHECK_URGENT: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug)]
pub struct MonitorRect {
    pub handle: isize,
    /// Virtual-desktop physical pixels.
    pub rect: PxRect,
}

/// Monitors in physical pixels regardless of the calling process' DPI awareness.
pub fn enumerate_monitors() -> Vec<MonitorRect> {
    unsafe extern "system" fn cb(hmon: HMONITOR, _dc: HDC, _r: *mut RECT, data: LPARAM) -> BOOL {
        let list = unsafe { &mut *(data.0 as *mut Vec<MonitorRect>) };
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if unsafe { GetMonitorInfoW(hmon, &mut mi) }.as_bool() {
            let r = mi.rcMonitor;
            list.push(MonitorRect {
                handle: hmon.0 as isize,
                rect: PxRect::new(r.left, r.top, r.right - r.left, r.bottom - r.top),
            });
        }
        TRUE
    }
    let mut list: Vec<MonitorRect> = Vec::new();
    unsafe {
        let old = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut list as *mut _ as isize));
        if !old.0.is_null() {
            SetThreadDpiAwarenessContext(old);
        }
    }
    list
}

/// Primary monitor rectangle (origin 0,0).
pub fn primary_monitor_rect() -> Option<PxRect> {
    enumerate_monitors()
        .into_iter()
        .find(|m| m.rect.x == 0 && m.rect.y == 0)
        .map(|m| m.rect)
}

enum Backend {
    Wgc {
        _item: GraphicsCaptureItem,
        pool: Direct3D11CaptureFramePool,
        session: GraphicsCaptureSession,
        size: SizeInt32,
        /// Set by `GraphicsCaptureItem.Closed` (monitor unplugged, topology change).
        closed: Arc<AtomicBool>,
    },
    Dup {
        output: IDXGIOutput1,
        dup: Option<IDXGIOutputDuplication>,
        /// Consecutive frames delivered in a format other than BGRA8.
        bad_format: u32,
    },
}

struct MonCap {
    backend: Backend,
    got_frame: bool,
    /// Intersection of this monitor with the region, in monitor-local px.
    src: PxRect,
    /// Where that intersection lands inside the region.
    dst: (u32, u32),
    /// Set for HDR monitors: frames arrive as FP16 scRGB, are copied here and converted to
    /// SDR BGRA into the shared stage on the CPU.
    hdr: Option<HdrStage>,
    /// The display's color state as last read (device name, HDR on, SDR white level).
    color: hdr::DisplayColor,
}

/// FP16 staging copy of one HDR monitor's part of the region.
struct HdrStage {
    tex: ID3D11Texture2D,
    conv: Arc<hdr::ScrgbToSrgb>,
    /// A new frame was copied into `tex` and not yet converted.
    pending: bool,
}

impl HdrStage {
    fn new(device: &ID3D11Device, src: PxRect, color: &hdr::DisplayColor) -> Result<HdrStage> {
        log::info!(
            "{} is HDR (SDR white {} nits); capturing FP16 and converting to SDR",
            color.device,
            color.sdr_white_nits
        );
        let desc = D3D11_TEXTURE2D_DESC {
            Width: src.w as u32,
            Height: src.h as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R16G16B16A16_FLOAT,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut tex: Option<ID3D11Texture2D> = None;
        unsafe { device.CreateTexture2D(&desc, None, Some(&mut tex)) }
            .context("CreateTexture2D(HDR staging)")?;
        Ok(HdrStage {
            tex: tex.context("no HDR staging texture")?,
            conv: hdr::ScrgbToSrgb::shared(color.sdr_white_nits),
            pending: false,
        })
    }

    /// Convert the pending FP16 picture into `stage` (BGRA8) at `dst`.
    fn resolve(
        &mut self,
        ctx: &ID3D11DeviceContext,
        stage: &ID3D11Texture2D,
        src: PxRect,
        dst: (u32, u32),
    ) -> Result<()> {
        if !self.pending {
            return Ok(());
        }
        self.pending = false;
        let (w, h) = (src.w as usize, src.h as usize);
        unsafe {
            let mut from = D3D11_MAPPED_SUBRESOURCE::default();
            ctx.Map(&self.tex, 0, D3D11_MAP_READ, 0, Some(&mut from))
                .context("Map(HDR staging)")?;
            let mut to = D3D11_MAPPED_SUBRESOURCE::default();
            if let Err(e) = ctx.Map(stage, 0, D3D11_MAP_READ_WRITE, 0, Some(&mut to)) {
                ctx.Unmap(&self.tex, 0);
                return Err(e).context("Map(staging for HDR)");
            }
            let fp = from.RowPitch as usize;
            let tp = to.RowPitch as usize;
            let src_bytes =
                std::slice::from_raw_parts(from.pData as *const u8, fp * (h - 1) + w * 8);
            let base = (to.pData as *mut u8).add(dst.1 as usize * tp + dst.0 as usize * 4);
            let dst_bytes = std::slice::from_raw_parts_mut(base, tp * (h - 1) + w * 4);
            self.conv
                .convert_parallel(src_bytes, fp, w, h, dst_bytes, tp, true);
            ctx.Unmap(stage, 0);
            ctx.Unmap(&self.tex, 0);
        }
        Ok(())
    }
}

pub struct Capturer {
    device: ID3D11Device,
    winrt_device: Option<IDirect3DDevice>,
    ctx: ID3D11DeviceContext,
    mons: Vec<MonCap>,
    stage: ID3D11Texture2D,
    /// Name of the backend in use, for logs.
    pub backend_name: &'static str,
    /// Region in virtual-desktop px (origin for cursor compositing).
    region: PxRect,
    /// The backend cannot draw the pointer itself and the user wants it: the video loop
    /// composites it with `composite_cursor`.
    cursor_overlay: bool,
    /// When the displays' color state was last read.
    color_checked: Instant,
    /// Something suggests the color state changed: re-read it soon.
    color_due: bool,
}

// Used from the single video thread only; moved there once after construction.
unsafe impl Send for Capturer {}

#[derive(Default, Debug, Clone, Copy)]
pub struct PollStats {
    /// Frames that updated the staging picture.
    pub new_frames: u32,
    /// Frames delivered by the OS but superseded before we could use them.
    pub dropped: u32,
}

pub struct Mapped<'a> {
    cap: &'a Capturer,
    pub data: *const u8,
    pub stride: usize,
}

impl Drop for Mapped<'_> {
    fn drop(&mut self) {
        unsafe { self.cap.ctx.Unmap(&self.cap.stage, 0) };
    }
}

impl Mapped<'_> {
    pub fn bytes(&self, height: usize) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.data, self.stride * height) }
    }
}

type Plan = Vec<(MonitorRect, PxRect, (u32, u32))>;

fn create_stage(
    device: &ID3D11Device,
    ctx: &ID3D11DeviceContext,
    w: u32,
    h: u32,
) -> Result<ID3D11Texture2D> {
    unsafe {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: (D3D11_CPU_ACCESS_READ.0 | D3D11_CPU_ACCESS_WRITE.0) as u32,
            MiscFlags: 0,
        };
        let mut stage: Option<ID3D11Texture2D> = None;
        device
            .CreateTexture2D(&desc, None, Some(&mut stage))
            .context("CreateTexture2D(staging)")?;
        let stage = stage.context("no staging texture")?;
        // Zero (opaque black) so parts of the region outside every monitor are black.
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        ctx.Map(&stage, 0, D3D11_MAP_READ_WRITE, 0, Some(&mut mapped))
            .context("Map(staging init)")?;
        for y in 0..h as usize {
            let row = (mapped.pData as *mut u8).add(y * mapped.RowPitch as usize);
            for x in 0..w as usize {
                std::ptr::write_unaligned(row.add(x * 4) as *mut u32, 0xFF00_0000);
            }
        }
        ctx.Unmap(&stage, 0);
        Ok(stage)
    }
}

fn make_plan(region: PxRect, monitors: &[MonitorRect]) -> Plan {
    monitors
        .iter()
        .filter_map(|m| {
            let i = m.rect.intersect(&region)?;
            Some((
                *m,
                PxRect::new(i.x - m.rect.x, i.y - m.rect.y, i.w, i.h),
                ((i.x - region.x) as u32, (i.y - region.y) as u32),
            ))
        })
        .collect()
}

impl Capturer {
    /// `region` in virtual-desktop physical px; width/height must already be even.
    pub fn new(region: PxRect, include_cursor: bool, fps: u32) -> Result<Capturer> {
        if region.is_empty() {
            bail!("recording region is empty");
        }
        let monitors = enumerate_monitors();
        if monitors.is_empty() {
            bail!("no monitors found");
        }
        let plan = make_plan(region, &monitors);
        if plan.is_empty() {
            bail!("region {region:?} does not intersect any monitor");
        }
        let recently_hung = WGC_HUNG_AT
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some_and(|t| t.elapsed() < WGC_COOLDOWN);
        let wgc = if recently_hung {
            Err(anyhow!("WGC hung earlier in this process"))
        } else {
            Self::new_wgc_with_timeout(region, &plan, include_cursor, fps)
        };
        match wgc {
            Ok(c) => Ok(c),
            Err(wgc_err) => {
                log::warn!(
                    "Windows Graphics Capture unavailable ({wgc_err:#}); falling back to DXGI Desktop Duplication"
                );
                Self::new_dxgi(region, &plan, include_cursor)
                    .with_context(|| format!("screen capture failed (WGC: {wgc_err:#})"))
            }
        }
    }

    fn new_wgc_with_timeout(
        region: PxRect,
        plan: &Plan,
        include_cursor: bool,
        fps: u32,
    ) -> Result<Capturer> {
        let (tx, rx) = mpsc::channel::<Result<SendCom<Capturer>, String>>();
        let plan = plan.clone();
        std::thread::Builder::new()
            .name("record-wgc-init".into())
            .spawn(move || {
                let _com = ComGuard::new();
                let r = Self::new_wgc(region, &plan, include_cursor, fps)
                    .map(SendCom)
                    .map_err(|e| format!("{e:#}"));
                let _ = tx.send(r);
            })
            .context("spawn WGC init thread")?;
        match rx.recv_timeout(WGC_INIT_TIMEOUT) {
            Ok(Ok(c)) => Ok(c.0),
            Ok(Err(e)) => Err(anyhow!(e)),
            // The helper thread stays blocked inside the OS call; it holds nothing we need.
            Err(_) => {
                *WGC_HUNG_AT.lock().unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
                Err(anyhow!(
                    "WGC session creation did not complete within {WGC_INIT_TIMEOUT:?}"
                ))
            }
        }
    }

    fn new_wgc(region: PxRect, plan: &Plan, include_cursor: bool, fps: u32) -> Result<Capturer> {
        unsafe {
            let mut device: Option<ID3D11Device> = None;
            let mut ctx: Option<ID3D11DeviceContext> = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            )
            .context("D3D11CreateDevice")?;
            let device = device.context("no D3D11 device")?;
            let ctx = ctx.context("no D3D11 context")?;
            let dxgi: IDXGIDevice = device.cast().context("IDXGIDevice")?;
            let winrt_device: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)
                .context("CreateDirect3D11DeviceFromDXGIDevice")?
                .cast()?;
            let interop =
                windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
                    .context("GraphicsCaptureItem interop factory")?;

            let mut mons = Vec::new();
            for (m, src, dst) in plan {
                let color = hdr::display_for_monitor(m.handle);
                let hdr_stage = if color.advanced_color {
                    Some(HdrStage::new(&device, *src, &color)?)
                } else {
                    None
                };
                let pixel_format = wgc_format(hdr_stage.is_some());
                let item: GraphicsCaptureItem = interop
                    .CreateForMonitor(HMONITOR(m.handle as *mut _))
                    .context("CreateForMonitor")?;
                let size = item.Size()?;
                let closed = Arc::new(AtomicBool::new(false));
                {
                    let flag = Arc::clone(&closed);
                    item.Closed(&windows::Foundation::TypedEventHandler::new(move |_, _| {
                        flag.store(true, Ordering::Relaxed);
                        Ok(())
                    }))
                    .context("GraphicsCaptureItem.Closed")?;
                }
                let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
                    &winrt_device,
                    pixel_format,
                    3,
                    size,
                )
                .context("CreateFreeThreaded")?;
                let session = pool
                    .CreateCaptureSession(&item)
                    .context("CreateCaptureSession")?;
                let _ = session.SetIsCursorCaptureEnabled(include_cursor);
                let _ = session.SetIsBorderRequired(false);
                if fps > 0 {
                    let _ = session.SetMinUpdateInterval(TimeSpan {
                        Duration: 10_000_000 / fps as i64 / 2,
                    });
                }
                mons.push(MonCap {
                    backend: Backend::Wgc {
                        _item: item,
                        pool,
                        session,
                        size,
                        closed,
                    },
                    got_frame: false,
                    src: *src,
                    dst: *dst,
                    hdr: hdr_stage,
                    color,
                });
            }
            let stage = create_stage(&device, &ctx, region.w as u32, region.h as u32)?;
            for m in &mons {
                if let Backend::Wgc { session, .. } = &m.backend {
                    session.StartCapture().context("StartCapture")?;
                }
            }
            Ok(Capturer {
                device,
                winrt_device: Some(winrt_device),
                ctx,
                mons,
                stage,
                backend_name: "Windows Graphics Capture",
                region,
                cursor_overlay: false,
                color_checked: Instant::now(),
                color_due: false,
            })
        }
    }

    fn new_dxgi(region: PxRect, plan: &Plan, include_cursor: bool) -> Result<Capturer> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1().context("CreateDXGIFactory1")?;
            // All duplicated monitors must live on one adapter (they share one D3D11 device);
            // pick the adapter that owns the most of the wanted monitors.
            let mut chosen: Option<(IDXGIAdapter1, Vec<(IDXGIOutput1, usize)>)> = None;
            let mut ai = 0;
            while let Ok(adapter) = factory.EnumAdapters1(ai) {
                ai += 1;
                let mut outs = Vec::new();
                let mut oi = 0;
                while let Ok(out) = adapter.EnumOutputs(oi) {
                    oi += 1;
                    let Ok(desc) = out.GetDesc() else { continue };
                    if let Some(pi) = plan
                        .iter()
                        .position(|(m, _, _)| m.handle == desc.Monitor.0 as isize)
                        && let Ok(o1) = out.cast::<IDXGIOutput1>()
                    {
                        outs.push((o1, pi));
                    }
                }
                if !outs.is_empty() && chosen.as_ref().is_none_or(|c| outs.len() > c.1.len()) {
                    chosen = Some((adapter, outs));
                }
            }
            let (adapter, outs) = chosen.context("no DXGI output matches the recording region")?;
            if outs.len() < plan.len() {
                log::warn!(
                    "only {} of {} monitors are on the capture adapter",
                    outs.len(),
                    plan.len()
                );
            }
            let mut device: Option<ID3D11Device> = None;
            let mut ctx: Option<ID3D11DeviceContext> = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            )
            .context("D3D11CreateDevice(adapter)")?;
            let device = device.context("no D3D11 device")?;
            let ctx = ctx.context("no D3D11 context")?;
            let mut mons = Vec::new();
            for (out, pi) in outs {
                let (m, src, dst) = &plan[pi];
                let color = hdr::display_for_monitor(m.handle);
                let hdr_stage = if color.advanced_color {
                    Some(HdrStage::new(&device, *src, &color)?)
                } else {
                    None
                };
                let dup = duplicate_output(&out, &device, hdr_stage.is_some())
                    .context("DuplicateOutput (secure desktop or another duplication in use?)")?;
                mons.push(MonCap {
                    backend: Backend::Dup {
                        output: out,
                        dup: Some(dup),
                        bad_format: 0,
                    },
                    got_frame: false,
                    src: *src,
                    dst: *dst,
                    hdr: hdr_stage,
                    color,
                });
            }
            let stage = create_stage(&device, &ctx, region.w as u32, region.h as u32)?;
            Ok(Capturer {
                device,
                winrt_device: None,
                ctx,
                mons,
                stage,
                backend_name: "DXGI Desktop Duplication",
                region,
                // Desktop Duplication frames never contain the pointer.
                cursor_overlay: include_cursor,
                color_checked: Instant::now(),
                color_due: false,
            })
        }
    }

    /// True when the caller must draw the pointer onto each frame (`composite_cursor`).
    pub fn needs_cursor_overlay(&self) -> bool {
        self.cursor_overlay
    }

    /// Alpha-blend the current pointer into a BGRA frame of the capture region.
    pub fn composite_cursor(&self, bgra: &mut [u8], w: usize, h: usize) {
        if !self.cursor_overlay || bgra.len() < w * h * 4 {
            return;
        }
        // Pointer shapes are at most a few hundred px; draw into a small transparent patch
        // centered on the pointer instead of converting the whole frame.
        const HALF: i32 = 160;
        let pos = stinkshot_platform::input::cursor_pos();
        let origin = stinkshot_core::PxPoint {
            x: pos.x - HALF,
            y: pos.y - HALF,
        };
        let mut patch = stinkshot_core::Rgba::new(HALF as u32 * 2, HALF as u32 * 2);
        stinkshot_platform::capture::draw_cursor(&mut patch, origin);
        let pw = patch.width as i32;
        for py in 0..patch.height as i32 {
            let fy = origin.y + py - self.region.y;
            if fy < 0 || fy >= h as i32 {
                continue;
            }
            for px in 0..pw {
                let s = &patch.data[((py * pw + px) * 4) as usize..][..4];
                let a = s[3] as u32;
                if a == 0 {
                    continue;
                }
                let fx = origin.x + px - self.region.x;
                if fx < 0 || fx >= w as i32 {
                    continue;
                }
                let d = &mut bgra[(fy as usize * w + fx as usize) * 4..][..4];
                // s is RGBA, d is BGRA.
                for (di, si) in [(0usize, 2usize), (1, 1), (2, 0)] {
                    d[di] = ((s[si] as u32 * a + d[di] as u32 * (255 - a)) / 255) as u8;
                }
                d[3] = 255;
            }
        }
    }

    /// True once every captured monitor has delivered at least one frame.
    pub fn has_all_frames(&self) -> bool {
        self.mons.iter().all(|m| m.got_frame)
    }

    pub fn has_any_frame(&self) -> bool {
        self.mons.iter().any(|m| m.got_frame)
    }

    /// Drain every source; the newest picture of each monitor ends up in the staging texture.
    pub fn poll(&mut self) -> Result<PollStats> {
        self.recheck_colors()?;
        let mut stats = PollStats::default();
        for m in &mut self.mons {
            // HDR monitors copy into their FP16 staging texture at its origin instead.
            let target = match &m.hdr {
                Some(h) => CopyTarget {
                    tex: &h.tex,
                    dst: (0, 0),
                    format: DXGI_FORMAT_R16G16B16A16_FLOAT,
                },
                None => CopyTarget {
                    tex: &self.stage,
                    dst: m.dst,
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                },
            };
            let got = match &mut m.backend {
                Backend::Wgc {
                    pool, size, closed, ..
                } => poll_wgc(
                    &self.ctx,
                    &target,
                    self.winrt_device.as_ref(),
                    pool,
                    size,
                    closed,
                    m.src,
                    &mut stats,
                )?,
                Backend::Dup {
                    output,
                    dup,
                    bad_format,
                } => poll_dup(
                    &self.device,
                    &self.ctx,
                    &target,
                    output,
                    dup,
                    bad_format,
                    m.src,
                    &mut stats,
                )?,
            };
            // A lost duplication (HDR toggles and mode changes cause one) or frames in the
            // other format mean the color state may have changed: re-read it before the
            // duplication is re-created.
            if let Backend::Dup {
                dup, bad_format, ..
            } = &m.backend
                && (dup.is_none() || *bad_format > 0)
            {
                self.color_due = true;
            }
            if got {
                m.got_frame = true;
                if let Some(h) = &mut m.hdr {
                    h.pending = true;
                    h.resolve(&self.ctx, &self.stage, m.src, m.dst)?;
                }
            }
        }
        Ok(stats)
    }

    /// Re-read each display's HDR state and SDR white level (once a second, sooner when
    /// `color_due`) and follow changes: add or drop the FP16 stage, switch the WGC pool or
    /// duplication format, or swap the converter for a new white level.
    fn recheck_colors(&mut self) -> Result<()> {
        let wait = if self.color_due {
            COLOR_RECHECK_URGENT
        } else {
            COLOR_RECHECK
        };
        if self.color_checked.elapsed() < wait {
            return Ok(());
        }
        self.color_checked = Instant::now();
        self.color_due = false;
        let colors = hdr::displays();
        for m in &mut self.mons {
            // A display missing from the query (it failed, or the display is mid-change)
            // keeps its last known state rather than being taken for SDR.
            let Some(c) = colors.iter().find(|c| {
                !m.color.device.is_empty() && c.device.eq_ignore_ascii_case(&m.color.device)
            }) else {
                continue;
            };
            if c.advanced_color != m.color.advanced_color {
                log::info!(
                    "{} turned HDR {} during the recording",
                    c.device,
                    if c.advanced_color { "on" } else { "off" }
                );
                m.hdr = if c.advanced_color {
                    Some(HdrStage::new(&self.device, m.src, c)?)
                } else {
                    None
                };
                match &mut m.backend {
                    Backend::Wgc { pool, size, .. } => {
                        if let Some(d) = &self.winrt_device {
                            pool.Recreate(d, wgc_format(c.advanced_color), 3, *size)
                                .context("Recreate(capture frame pool) after an HDR change")?;
                        }
                    }
                    Backend::Dup {
                        dup, bad_format, ..
                    } => {
                        // Re-created in the new format by the next poll.
                        *dup = None;
                        *bad_format = 0;
                    }
                }
            } else if c.sdr_white_nits != m.color.sdr_white_nits
                && let Some(h) = &mut m.hdr
            {
                log::info!(
                    "{} SDR white changed to {} nits during the recording",
                    c.device,
                    c.sdr_white_nits
                );
                h.conv = hdr::ScrgbToSrgb::shared(c.sdr_white_nits);
            }
            m.color = c.clone();
        }
        Ok(())
    }

    /// Map the staging picture for reading (BGRA, top-down).
    pub fn map(&self) -> Result<Mapped<'_>> {
        unsafe {
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.ctx
                .Map(&self.stage, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .context("Map(staging)")?;
            Ok(Mapped {
                cap: self,
                data: mapped.pData as *const u8,
                stride: mapped.RowPitch as usize,
            })
        }
    }
}

/// Where a monitor's frames are copied: the shared BGRA stage, or an HDR monitor's FP16
/// staging texture.
struct CopyTarget<'a> {
    tex: &'a ID3D11Texture2D,
    dst: (u32, u32),
    /// The frame format this target expects.
    format: DXGI_FORMAT,
}

/// WGC pool format for a monitor: FP16 scRGB on HDR monitors (converted on the CPU with the
/// display's SDR white level), BGRA8 otherwise.
fn wgc_format(hdr: bool) -> DirectXPixelFormat {
    if hdr {
        DirectXPixelFormat::R16G16B16A16Float
    } else {
        DirectXPixelFormat::B8G8R8A8UIntNormalized
    }
}

#[allow(clippy::too_many_arguments)]
fn poll_wgc(
    ctx: &ID3D11DeviceContext,
    target: &CopyTarget,
    winrt_device: Option<&IDirect3DDevice>,
    pool: &Direct3D11CaptureFramePool,
    size: &mut SizeInt32,
    closed: &AtomicBool,
    src: PxRect,
    stats: &mut PollStats,
) -> Result<bool> {
    if closed.load(Ordering::Relaxed) {
        bail!("the captured display was disconnected or changed, so the recording cannot continue");
    }
    let mut latest: Option<windows::Graphics::Capture::Direct3D11CaptureFrame> = None;
    let mut delivered = 0u32;
    while let Ok(frame) = pool.TryGetNextFrame() {
        delivered += 1;
        if let Some(old) = latest.replace(frame) {
            let _ = old.Close();
        }
    }
    let Some(frame) = latest else {
        return Ok(false);
    };
    stats.dropped += delivered.saturating_sub(1);
    unsafe {
        let content = frame.ContentSize().unwrap_or(*size);
        if content.Width != size.Width || content.Height != size.Height {
            log::info!(
                "monitor capture size changed to {}x{}",
                content.Width,
                content.Height
            );
            *size = content;
            let _ = frame.Close();
            if let Some(d) = winrt_device {
                let hdr = target.format == DXGI_FORMAT_R16G16B16A16_FLOAT;
                let _ = pool.Recreate(d, wgc_format(hdr), 3, content);
            }
            return Ok(false);
        }
        let surface = frame.Surface().context("frame surface")?;
        let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
        let tex: ID3D11Texture2D = access.GetInterface().context("frame texture")?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        tex.GetDesc(&mut desc);
        if desc.Format != target.format {
            // Queued before the pool switched format for an HDR change.
            let _ = frame.Close();
            return Ok(false);
        }
        copy_region(
            ctx,
            target.tex,
            &tex,
            src,
            target.dst,
            content.Width,
            content.Height,
        );
        let _ = frame.Close();
    }
    stats.new_frames += 1;
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
fn poll_dup(
    device: &ID3D11Device,
    ctx: &ID3D11DeviceContext,
    target: &CopyTarget,
    output: &IDXGIOutput1,
    dup: &mut Option<IDXGIOutputDuplication>,
    bad_format: &mut u32,
    src: PxRect,
    stats: &mut PollStats,
) -> Result<bool> {
    let mut got = false;
    let hdr = target.format == DXGI_FORMAT_R16G16B16A16_FLOAT;
    if dup.is_none() {
        // lost earlier (mode change, UAC desktop): try to re-establish quietly
        *dup = duplicate_output(output, device, hdr).ok();
    }
    let Some(d) = dup.as_ref() else {
        return Ok(false);
    };
    loop {
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut res: Option<IDXGIResource> = None;
        let r = unsafe { d.AcquireNextFrame(0, &mut info, &mut res) };
        match r {
            Ok(()) => {
                if let Some(res) = res
                    && let Ok(tex) = res.cast::<ID3D11Texture2D>()
                {
                    let mut desc = D3D11_TEXTURE2D_DESC::default();
                    unsafe { tex.GetDesc(&mut desc) };
                    if desc.Format != target.format {
                        // Copying e.g. an HDR (FP16) surface into the BGRA8 stage fails
                        // silently and would record black (and an 8-bit frame of an HDR
                        // monitor is blown out).
                        *bad_format += 1;
                        let fatal = *bad_format > MAX_BAD_FORMAT_FRAMES;
                        unsafe {
                            let _ = d.ReleaseFrame();
                        }
                        if fatal {
                            bail!(
                                "the display is delivering {:?} frames (HDR or another format the fallback capture cannot record)",
                                desc.Format
                            );
                        }
                        continue;
                    }
                    *bad_format = 0;
                    copy_region(
                        ctx,
                        target.tex,
                        &tex,
                        src,
                        target.dst,
                        desc.Width as i32,
                        desc.Height as i32,
                    );
                    if got {
                        stats.dropped += 1;
                    }
                    got = true;
                }
                unsafe {
                    let _ = d.ReleaseFrame();
                }
            }
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => break,
            Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => {
                *dup = None;
                break;
            }
            Err(e) => return Err(anyhow!("AcquireNextFrame failed: {e}")),
        }
    }
    if got {
        stats.new_frames += 1;
    }
    Ok(got)
}

/// Duplicate `output` as BGRA8, or as FP16 scRGB for an HDR output (an 8-bit duplication of
/// an HDR output is clipped without the SDR white scaling, so it looks blown out), and refuse
/// rotated outputs (the duplicated surface is unrotated, so the crop would be wrong; WGC
/// handles those).
fn duplicate_output(
    output: &IDXGIOutput1,
    device: &ID3D11Device,
    hdr: bool,
) -> Result<IDXGIOutputDuplication> {
    unsafe {
        let dup = if hdr {
            output
                .cast::<IDXGIOutput5>()
                .and_then(|o5| o5.DuplicateOutput1(device, 0, &[DXGI_FORMAT_R16G16B16A16_FLOAT]))
                .context("DuplicateOutput1(FP16) for an HDR display")?
        } else {
            match output
                .cast::<IDXGIOutput5>()
                .and_then(|o5| o5.DuplicateOutput1(device, 0, &[DXGI_FORMAT_B8G8R8A8_UNORM]))
            {
                Ok(d) => d,
                Err(_) => output.DuplicateOutput(device)?,
            }
        };
        let rot = dup.GetDesc().Rotation;
        if rot != DXGI_MODE_ROTATION_IDENTITY && rot != DXGI_MODE_ROTATION_UNSPECIFIED {
            bail!("the display is rotated ({rot:?}), which the fallback capture does not support");
        }
        Ok(dup)
    }
}

fn copy_region(
    ctx: &ID3D11DeviceContext,
    stage: &ID3D11Texture2D,
    tex: &ID3D11Texture2D,
    src: PxRect,
    dst: (u32, u32),
    tex_w: i32,
    tex_h: i32,
) {
    let b = D3D11_BOX {
        left: src.x as u32,
        top: src.y as u32,
        front: 0,
        right: (src.x + src.w).min(tex_w) as u32,
        bottom: (src.y + src.h).min(tex_h) as u32,
        back: 1,
    };
    if b.right > b.left && b.bottom > b.top {
        unsafe { ctx.CopySubresourceRegion(stage, 0, dst.0, dst.1, 0, tex, 0, Some(&b)) };
    }
}

impl Drop for Capturer {
    fn drop(&mut self) {
        for m in &self.mons {
            if let Backend::Wgc { session, pool, .. } = &m.backend {
                let _ = session.Close();
                let _ = pool.Close();
            }
        }
    }
}
