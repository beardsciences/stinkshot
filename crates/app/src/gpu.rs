//! wgpu setup for eframe: backend, adapter and device, created on a worker thread so the tray,
//! hotkeys and IPC are up while the GPU driver loads (docs/notes/app-shell.md, "Idle memory
//! and start-up", Nov 7 2026).
//!
//! - **Direct3D 12 first.** wgpu's default enables Vulkan, DX12 and GL and picks Vulkan on
//!   NVIDIA. Vulkan loads every implicit layer installed on the machine (Overwolf, OBS, GOG
//!   overlays here) into our process, and its device committed about 350 MB of private
//!   memory. DX12 also shares the D3D user-mode driver that the capture session (D3D11) loads
//!   anyway. Vulkan, then GL, remain as fallbacks when no DX12 adapter exists.
//! - **`MemoryHints::MemoryUsage`.** The default (`Performance`) sub-allocates in large
//!   blocks; egui needs a handful of textures. Measured on DX12: 337 MB -> 139 MB private.
//! - **DirectComposition swapchains** (`DxgiFromVisual`). The default HWND swapchain has no
//!   alpha: pins, the recording frame and the countdown are transparent windows, and the
//!   recording frame would cover the recorded area with black. Vulkan had alpha here.
//! - **FXC, not "Auto".** Auto loads any `dxcompiler.dll` found on `PATH` (the Vulkan SDK's
//!   here, 21 MB). egui's one shader compiles fine with the system FXC.
//! - **Low-power adapter.** A tray tool should not wake a laptop's discrete GPU; on a desktop
//!   with one GPU it changes nothing.
//!
//! `WGPU_BACKEND` / `WGPU_POWER_PREF` still override backend and adapter choice for
//! troubleshooting.

use std::sync::Arc;
use std::thread::JoinHandle;

use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew, WgpuSetupExisting};
use eframe::wgpu;

use crate::timing;

/// Device descriptor shared by the pre-created device and the eframe fallback. Matches egui's
/// default (8192 px textures, enough for one 8K monitor; the overlay tiles anything bigger)
/// apart from the memory hint.
fn device_descriptor(adapter: &wgpu::Adapter) -> wgpu::DeviceDescriptor<'static> {
    let base_limits = if adapter.get_info().backend == wgpu::Backend::Gl {
        wgpu::Limits::downlevel_webgl2_defaults()
    } else {
        wgpu::Limits::default()
    };
    wgpu::DeviceDescriptor {
        label: Some("stinkshot egui device"),
        required_limits: wgpu::Limits {
            max_texture_dimension_2d: 8192,
            ..base_limits
        },
        memory_hints: wgpu::MemoryHints::MemoryUsage,
        ..Default::default()
    }
}

fn instance(backends: wgpu::Backends) -> wgpu::Instance {
    let mut backend_options = wgpu::BackendOptions::from_env_or_default();
    if wgpu::Dx12Compiler::from_env().is_none() {
        backend_options.dx12.shader_compiler = wgpu::Dx12Compiler::Fxc;
    }
    if wgpu::Dx12SwapchainKind::from_env().is_none() {
        backend_options.dx12.presentation_system = wgpu::Dx12SwapchainKind::DxgiFromVisual;
    }
    wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        flags: wgpu::InstanceFlags::from_build_config().with_env(),
        backend_options,
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        display: None,
    })
}

fn power() -> wgpu::PowerPreference {
    wgpu::PowerPreference::from_env().unwrap_or(wgpu::PowerPreference::LowPower)
}

/// Backends to try, in order.
fn backend_order() -> Vec<wgpu::Backends> {
    match wgpu::Backends::from_env() {
        Some(b) => vec![b],
        None => vec![
            wgpu::Backends::DX12,
            wgpu::Backends::VULKAN,
            wgpu::Backends::GL,
        ],
    }
}

fn create() -> Option<WgpuSetupExisting> {
    for backends in backend_order() {
        let instance = instance(backends);
        let adapter =
            match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: power(),
                force_fallback_adapter: false,
                compatible_surface: None,
                ..Default::default()
            })) {
                Ok(a) => a,
                Err(e) => {
                    log::info!("wgpu: no {backends:?} adapter: {e}");
                    continue;
                }
            };
        timing::point(format_args!("gpu: adapter ({backends:?})"));
        let desc = device_descriptor(&adapter);
        match pollster::block_on(adapter.request_device(&desc)) {
            Ok((device, queue)) => {
                let i = adapter.get_info();
                log::info!(
                    "wgpu adapter: {} ({:?}, {:?})",
                    i.name,
                    i.backend,
                    i.device_type
                );
                return Some(WgpuSetupExisting {
                    instance,
                    adapter,
                    device,
                    queue,
                });
            }
            Err(e) => log::warn!("wgpu: {backends:?} device failed: {e}"),
        }
    }
    None
}

/// Start creating the wgpu device on a worker. `options` takes the result.
pub fn start() -> Option<JoinHandle<Option<WgpuSetupExisting>>> {
    std::thread::Builder::new()
        .name("gpu-init".into())
        .spawn(|| {
            let r = create();
            timing::point(format_args!("gpu: device ready"));
            r
        })
        .map_err(|e| log::warn!("gpu-init thread: {e}"))
        .ok()
}

/// eframe's wgpu configuration: the device made by `start` when it succeeded, otherwise
/// eframe creates one itself (all backends, still with the low-memory hint).
pub fn options(pending: Option<JoinHandle<Option<WgpuSetupExisting>>>) -> WgpuConfiguration {
    let existing = pending.and_then(|h| h.join().ok()).flatten();
    let wgpu_setup = match existing {
        Some(e) => WgpuSetup::Existing(e),
        None => {
            log::warn!("wgpu: pre-created device unavailable; letting eframe pick one");
            let mut setup = WgpuSetupCreateNew::without_display_handle();
            setup.power_preference = power();
            setup.device_descriptor = Arc::new(device_descriptor);
            WgpuSetup::CreateNew(setup)
        }
    };
    WgpuConfiguration {
        wgpu_setup,
        ..Default::default()
    }
}
