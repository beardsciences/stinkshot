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
use stinkshot_platform::{capture, dpi, hdr};
use windows::Foundation::TimeSpan;
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{E_ACCESSDENIED, E_INVALIDARG, HMODULE, LPARAM, RECT, TRUE};
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

/// How long Desktop Duplication gets to deliver a first frame per monitor before the monitor
/// is filled from GDI (it delivers nothing while the screen is static).
const GDI_SEED_AFTER: Duration = Duration::from_millis(150);
/// Duplication silent this long: start checking with GDI whether the screen really is static.
const DUP_SILENT_AFTER: Duration = Duration::from_secs(1);
/// How often a silent duplication is cross-checked with a GDI grab.
const GDI_PROBE_EVERY: Duration = Duration::from_millis(500);

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
    let _dpi = dpi::ThreadPerMonitorAware::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut list as *mut _ as isize));
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
    /// The monitor's top-left in virtual-desktop px.
    mon_origin: (i32, i32),
    /// DXGI backend: GDI seeding and the fallback for a duplication that stays silent while
    /// the screen changes.
    gdi: GdiWatch,
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
    /// DXGI backend: keeps the app's cached capture session off the duplications this
    /// recording owns (one duplication per output per process). Dropped with the capturer.
    _dup_lease: Option<capture::DuplicationLease>,
    /// Frame interval (GDI fallback grabs at most this often).
    frame_interval: Duration,
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
                Self::new_dxgi(region, &plan, include_cursor, fps)
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
                    mon_origin: (m.rect.x, m.rect.y),
                    gdi: GdiWatch::new(),
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
                _dup_lease: None,
                frame_interval: Duration::from_secs(1) / fps.max(1),
            })
        }
    }

    fn new_dxgi(region: PxRect, plan: &Plan, include_cursor: bool, fps: u32) -> Result<Capturer> {
        // Before duplicating: the app's warm capture session holds every output.
        let lease = capture::lend_duplication();
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
                let dup = duplicate_output_retrying(&out, &device, hdr_stage.is_some())
                    .with_context(|| format!("screen duplication of {}", color.device))?;
                mons.push(MonCap {
                    backend: Backend::Dup {
                        output: out,
                        dup: Some(dup),
                        bad_format: 0,
                    },
                    got_frame: false,
                    src: *src,
                    mon_origin: (m.rect.x, m.rect.y),
                    gdi: GdiWatch::new(),
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
                _dup_lease: Some(lease),
                frame_interval: Duration::from_secs(1) / fps.max(1),
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
            if matches!(m.backend, Backend::Dup { .. })
                && gdi_fallback(m, got, &self.ctx, &self.stage, self.frame_interval)
            {
                stats.new_frames += 1;
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
                // LastPresentTime 0: only the pointer changed, the desktop image did not (and
                // a new duplication's very first surface can be an uninitialized
                // placeholder), so there is nothing to copy.
                if info.LastPresentTime != 0
                    && let Some(res) = res
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

/// Attempts and pause for `duplicate_output_retrying`: duplication is briefly unavailable
/// while the desktop switches (UAC, lock screen, mode change) or another capture is still
/// letting go of the display.
const DUP_ATTEMPTS: u32 = 4;
const DUP_RETRY_PAUSE: Duration = Duration::from_millis(150);

/// `duplicate_output`, retried a few times for errors that are usually transient.
fn duplicate_output_retrying(
    output: &IDXGIOutput1,
    device: &ID3D11Device,
    hdr: bool,
) -> Result<IDXGIOutputDuplication> {
    let mut attempt = 1;
    loop {
        match duplicate_output(output, device, hdr) {
            Ok(d) => return Ok(d),
            Err(e) if attempt < DUP_ATTEMPTS && is_transient_dup_error(&e) => {
                log::debug!("duplication attempt {attempt} failed ({e:#}); retrying");
                attempt += 1;
                std::thread::sleep(DUP_RETRY_PAUSE);
            }
            Err(e) => return Err(e),
        }
    }
}

fn dup_hresult(e: &anyhow::Error) -> Option<windows::core::HRESULT> {
    e.chain()
        .find_map(|c| c.downcast_ref::<windows::core::Error>())
        .map(|w| w.code())
}

fn is_transient_dup_error(e: &anyhow::Error) -> bool {
    dup_hresult(e).is_some_and(|c| {
        c == DXGI_ERROR_NOT_CURRENTLY_AVAILABLE
            || c == DXGI_ERROR_SESSION_DISCONNECTED
            || c == E_ACCESSDENIED
            || c == E_INVALIDARG
    })
}

/// Why a `DuplicateOutput(1)` failed, by HRESULT.
fn explain_dup_error(code: windows::core::HRESULT) -> &'static str {
    if code == E_INVALIDARG {
        "this process already duplicates the display (another capture is still holding it)"
    } else if code == DXGI_ERROR_UNSUPPORTED {
        "Desktop Duplication does not support this display mode or a DPI-unaware caller"
    } else if code == DXGI_ERROR_NOT_CURRENTLY_AVAILABLE {
        "too many applications are duplicating the display right now"
    } else if code == E_ACCESSDENIED {
        "the secure desktop (UAC or lock screen) is showing"
    } else if code == DXGI_ERROR_SESSION_DISCONNECTED {
        "the session is disconnected (remote desktop)"
    } else {
        "Desktop Duplication is unavailable"
    }
}

/// Duplicate `output` as BGRA8, or as FP16 scRGB for an HDR output (an 8-bit duplication of
/// an HDR output is clipped without the SDR white scaling, so it looks blown out), and refuse
/// rotated outputs (the duplicated surface is unrotated, so the crop would be wrong; WGC
/// handles those).
///
/// Runs Per-Monitor-V2 DPI aware: `DuplicateOutput1` returns `DXGI_ERROR_UNSUPPORTED` to a
/// DPI-unaware caller on a display scaled above 100% (test binaries have no manifest).
fn duplicate_output(
    output: &IDXGIOutput1,
    device: &ID3D11Device,
    hdr: bool,
) -> Result<IDXGIOutputDuplication> {
    let _dpi = dpi::ThreadPerMonitorAware::new();
    let explain = |e: windows::core::Error, what: &str| {
        let why = explain_dup_error(e.code());
        anyhow::Error::new(e).context(format!("{what}: {why}"))
    };
    unsafe {
        let dup = if hdr {
            output
                .cast::<IDXGIOutput5>()
                .and_then(|o5| o5.DuplicateOutput1(device, 0, &[DXGI_FORMAT_R16G16B16A16_FLOAT]))
                .map_err(|e| explain(e, "DuplicateOutput1(FP16) for an HDR display"))?
        } else {
            match output
                .cast::<IDXGIOutput5>()
                .and_then(|o5| o5.DuplicateOutput1(device, 0, &[DXGI_FORMAT_B8G8R8A8_UNORM]))
            {
                Ok(d) => d,
                Err(_) => output
                    .DuplicateOutput(device)
                    .map_err(|e| explain(e, "DuplicateOutput"))?,
            }
        };
        let rot = dup.GetDesc().Rotation;
        if rot != DXGI_MODE_ROTATION_IDENTITY && rot != DXGI_MODE_ROTATION_UNSPECIFIED {
            bail!("the display is rotated ({rot:?}), which the fallback capture does not support");
        }
        Ok(dup)
    }
}

/// GDI help for one duplicated monitor.
///
/// - Seed: Desktop Duplication delivers nothing until the screen changes, so a static
///   desktop would record no frames at all; after `GDI_SEED_AFTER` without a frame the
///   monitor is filled from GDI (which returns correct SDR colors on HDR displays too).
/// - Fallback: a duplication can also stay silent while the screen does change (measured
///   Nov 7 2026 with the displays idle-off: DWM stops feeding duplications, GDI still sees
///   windows repaint). After `DUP_SILENT_AFTER` of silence a GDI grab is taken every
///   `GDI_PROBE_EVERY`. The first grab of a silent period is written to the stage (the
///   screen may have changed after the duplication's last frame), and later grabs are
///   compared in full with the previous one; when they differ the monitor is captured with
///   GDI at the frame rate, every grab written, until the duplication delivers again.
struct GdiWatch {
    created: Instant,
    last_dup: Instant,
    next_probe: Instant,
    /// The last GDI grab, which is also what the stage shows for this monitor (every grab
    /// that is kept here was written). None after a duplication frame.
    prev: Option<Vec<u8>>,
    active: bool,
    last_grab: Instant,
    /// After a failed grab no new attempt before this (secure desktop, mode change).
    retry_at: Instant,
    /// A failure streak is in progress (logged once).
    failing: bool,
    /// Pictures taken from the duplication / from GDI (logged when the capture ends).
    dup_frames: u64,
    gdi_frames: u64,
}

impl GdiWatch {
    fn new() -> GdiWatch {
        let now = Instant::now();
        GdiWatch {
            created: now,
            last_dup: now,
            next_probe: now,
            prev: None,
            active: false,
            last_grab: now,
            retry_at: now,
            failing: false,
            dup_frames: 0,
            gdi_frames: 0,
        }
    }
}

/// What to do with a GDI grab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GdiStep {
    /// Nothing new on screen: keep the stage.
    Skip,
    /// Write the grab to the stage.
    Write,
    /// The screen changes while the duplication is silent: write and turn the fallback on.
    Activate,
}

/// Decide what a GDI grab `cur` means. `prev` is the previous grab of this silent period
/// (what the stage shows), None when the stage holds a duplication picture or nothing.
fn gdi_step(got_frame: bool, active: bool, prev: Option<&[u8]>, cur: &[u8]) -> GdiStep {
    if !got_frame || active {
        // Seeding a static screen, or capturing with GDI: every grab is written (the
        // expensive BitBlt already ran; a tiny change such as a caret must not be lost).
        return GdiStep::Write;
    }
    match prev {
        // First probe of a silent period: the screen may have changed after the
        // duplication's last frame, and the stage still shows that older picture.
        None => GdiStep::Write,
        Some(p) if p != cur => GdiStep::Activate,
        Some(_) => GdiStep::Skip,
    }
}

/// Runs the seed / fallback logic for `m` after a duplication poll. True when the stage
/// received a new picture from GDI.
fn gdi_fallback(
    m: &mut MonCap,
    got: bool,
    ctx: &ID3D11DeviceContext,
    stage: &ID3D11Texture2D,
    interval: Duration,
) -> bool {
    let now = Instant::now();
    let g = &mut m.gdi;
    if got {
        g.dup_frames += 1;
        g.last_dup = now;
        g.prev = None;
        if g.active {
            g.active = false;
            log::info!(
                "{}: duplication frames again; GDI fallback off",
                m.color.device
            );
        }
        return false;
    }
    let grab_now = if now < g.retry_at {
        false
    } else if !m.got_frame {
        now.duration_since(g.created) >= GDI_SEED_AFTER && now >= g.next_probe
    } else if g.active {
        // The video loop wakes about once per frame deadline; a full interval would miss
        // every other one to timer jitter.
        now.duration_since(g.last_grab) >= interval / 2
    } else {
        now.duration_since(g.last_dup) >= DUP_SILENT_AFTER && now >= g.next_probe
    };
    if !grab_now {
        return false;
    }
    let rect = PxRect::new(
        m.mon_origin.0 + m.src.x,
        m.mon_origin.1 + m.src.y,
        m.src.w,
        m.src.h,
    );
    g.last_grab = now;
    g.next_probe = now + GDI_PROBE_EVERY;
    let failure = match capture::capture_rect(rect, false) {
        Ok(i) if i.width == rect.w as u32 && i.height == rect.h as u32 => Ok(i),
        Ok(i) => Err(format!("got {}x{}", i.width, i.height)),
        Err(e) => Err(format!("failed: {e:#}")),
    };
    let img = match failure {
        Ok(i) => {
            if g.failing {
                g.failing = false;
                log::info!("GDI grab of {} works again", m.color.device);
            }
            i
        }
        Err(why) => {
            // Back off and log once per streak, not on every poll.
            g.retry_at = now + GDI_PROBE_EVERY;
            if !g.failing {
                g.failing = true;
                log::warn!(
                    "GDI grab of {} {why}; retrying every {} ms",
                    m.color.device,
                    GDI_PROBE_EVERY.as_millis()
                );
            }
            return false;
        }
    };
    let step = gdi_step(m.got_frame, g.active, g.prev.as_deref(), &img.data);
    match step {
        GdiStep::Skip => return false,
        GdiStep::Write => {
            if !m.got_frame {
                log::debug!(
                    "{} delivered no duplication frame yet (static screen); seeded from GDI",
                    m.color.device
                );
            }
        }
        GdiStep::Activate => {
            g.active = true;
            log::info!(
                "{}: duplication silent while the screen changes; capturing with GDI",
                m.color.device
            );
        }
    }
    if let Err(e) = write_rgba(ctx, stage, m.dst, &img) {
        log::warn!("GDI grab of {}: {e:#}", m.color.device);
        // The stage may not show this grab: compare the next one against nothing.
        g.prev = None;
        return false;
    }
    g.prev = Some(img.data);
    m.got_frame = true;
    m.gdi.gdi_frames += 1;
    true
}

/// Write an RGBA image into the BGRA staging texture at `dst`.
fn write_rgba(
    ctx: &ID3D11DeviceContext,
    stage: &ID3D11Texture2D,
    dst: (u32, u32),
    img: &stinkshot_core::Rgba,
) -> Result<()> {
    let (w, h) = (img.width as usize, img.height as usize);
    unsafe {
        let mut to = D3D11_MAPPED_SUBRESOURCE::default();
        ctx.Map(stage, 0, D3D11_MAP_READ_WRITE, 0, Some(&mut to))
            .context("Map(staging for GDI)")?;
        let tp = to.RowPitch as usize;
        for y in 0..h {
            let row = std::slice::from_raw_parts_mut(
                (to.pData as *mut u8).add((dst.1 as usize + y) * tp + dst.0 as usize * 4),
                w * 4,
            );
            rgba_row_to_bgra(&img.data[y * w * 4..(y + 1) * w * 4], row);
        }
        ctx.Unmap(stage, 0);
    }
    Ok(())
}

/// Copy one row of RGBA pixels into BGRA (opaque).
fn rgba_row_to_bgra(src: &[u8], dst: &mut [u8]) {
    for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
        d[0] = s[2];
        d[1] = s[1];
        d[2] = s[0];
        d[3] = 255;
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
            match &m.backend {
                Backend::Wgc { session, pool, .. } => {
                    let _ = session.Close();
                    let _ = pool.Close();
                }
                Backend::Dup { .. } => log::info!(
                    "{}: {} pictures from the duplication, {} from GDI",
                    m.color.device,
                    m.gdi.dup_frames,
                    m.gdi.gdi_frames
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gdi_seed_and_active_write_every_grab() {
        let a = [1u8; 64];
        assert_eq!(gdi_step(false, false, None, &a), GdiStep::Write);
        assert_eq!(gdi_step(false, false, Some(&a), &a), GdiStep::Write);
        // Active: even an unchanged grab is written.
        assert_eq!(gdi_step(true, true, Some(&a), &a), GdiStep::Write);
    }

    #[test]
    fn first_probe_of_a_silent_period_is_written() {
        // The stage holds the duplication's last picture; the screen may have changed
        // since, so the first grab must reach the stage.
        assert_eq!(gdi_step(true, false, None, &[7u8; 64]), GdiStep::Write);
    }

    #[test]
    fn a_one_pixel_change_turns_the_fallback_on() {
        // 1920 px wide rows: a change in a column the old sampled hash skipped (x = 5).
        let w = 1920 * 4;
        let prev = vec![0u8; w * 4];
        let mut cur = prev.clone();
        cur[w * 2 + 5 * 4] = 255;
        assert_eq!(gdi_step(true, false, Some(&prev), &cur), GdiStep::Activate);
        assert_eq!(gdi_step(true, false, Some(&prev), &prev), GdiStep::Skip);
    }
}
