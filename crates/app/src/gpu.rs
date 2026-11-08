//! wgpu setup for eframe: backend, adapter and device, created on a worker thread so the tray,
//! hotkeys and IPC are up while the GPU driver loads (docs/notes/app-shell.md, "Idle memory
//! and start-up", Nov 7 2026).
//!
//! - **Direct3D 12 first.** wgpu's default enables Vulkan, DX12 and GL and picks Vulkan on
//!   NVIDIA. Vulkan loads every implicit layer installed on the machine (Overwolf, OBS, GOG
//!   overlays here) into our process, and its device committed about 350 MB of private
//!   memory. DX12 also shares the D3D user-mode driver that the capture session (D3D11) loads
//!   anyway. Vulkan, then GL, remain as fallbacks when no DX12 adapter exists.
//! - **Small memory blocks and descriptor heap** (`memory_hints`, `device_descriptor`). The
//!   default (`Performance`) sub-allocates in 128-256 MB blocks: 337 MB private on DX12.
//!   `MemoryUsage` brought that to 139 MB but still kept a 64 MB upload block after the first
//!   overlay; 4-16 MB blocks and a 64 K descriptor heap (instead of 1 M, 30.5 MB) cut the
//!   device further (Nov 7 2026, see app-shell.md).
//! - **DirectComposition swapchains** (`DxgiFromVisual`). The default HWND swapchain has no
//!   alpha: pins, the recording frame and the countdown are transparent windows, and the
//!   recording frame would cover the recorded area with black. Vulkan had alpha here.
//! - **FXC, not "Auto".** Auto loads any `dxcompiler.dll` found on `PATH` (the Vulkan SDK's
//!   here, 21 MB). egui's one shader compiles fine with the system FXC.
//! - **Instance flags** (`instance_flags`): no indirect-call validation, unoptimized FXC.
//! - **Low-power adapter.** A tray tool should not wake a laptop's discrete GPU; on a desktop
//!   with one GPU it changes nothing.
//!
//! `WGPU_BACKEND` / `WGPU_POWER_PREF` still override backend and adapter choice for
//! troubleshooting.
//!
//! **Device loss** (docs/notes/app-shell.md, "GPU device loss, memory and cold start",
//! Nov 7 2026). A lost wgpu device cannot be used again, and egui-wgpu 0.37 panics on the
//! next mesh upload once it is lost ("Failed to create staging buffer for index data"):
//! `Queue::write_buffer_with` returns None for a lost device without reporting an error.
//! wgpu-core loses the device on a driver `DXGI_ERROR_DEVICE_REMOVED` / `RESET`, on any
//! unexpected HRESULT from a D3D12 call (Present, fence signal, swapchain buffers) and on
//! out-of-memory from the allocator. eframe cannot swap its device, so recovery is a restart
//! of the process: `install_handlers` marks the loss, the shell restarts before the next
//! frame when it can (`lost`), and `run_app` catches the egui-wgpu panic and restarts when
//! the loss happened inside a paint.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew, WgpuSetupExisting};
use eframe::wgpu;

use crate::timing;

/// Device descriptor shared by the pre-created device and the eframe fallback. Matches egui's
/// default (8192 px textures, enough for one 8K monitor; the overlay tiles anything bigger)
/// apart from the memory hint and a smaller descriptor heap.
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
            // The D3D12 backend sizes its shader-visible CBV/SRV/UAV descriptor heap to this
            // limit: the default 1,000,000 descriptors is a 30.5 MB heap (32 bytes each on
            // NVIDIA), committed from start-up. egui uses one per texture; 64 K is 2 MB.
            max_non_sampler_bindings: base_limits.max_non_sampler_bindings.min(65_536),
            ..base_limits
        },
        memory_hints: memory_hints(),
        ..Default::default()
    }
}

/// Small sub-allocation blocks (4-16 MB device, 4-8 MB host). egui needs a handful of small
/// buffers and textures; anything bigger (the overlay's frozen desktop, its upload) gets a
/// dedicated allocation that is freed with it. gpu-allocator keeps the last block of each
/// memory type, so `MemoryUsage` (up to 64 MB device / 32 MB host blocks) kept a 64 MB
/// write-combined upload block committed after the first overlay. Neither hint sets a memory
/// budget, so neither makes an allocation fail (see the device loss notes above).
fn memory_hints() -> wgpu::MemoryHints {
    const MB: u64 = 1 << 20;
    wgpu::MemoryHints::Manual {
        suballocated_device_memory_block_size: 4 * MB..16 * MB,
    }
}

/// Instance flags for a tray app that draws egui:
/// - no `VALIDATION_INDIRECT_CALL` (wgpu's release default): egui never draws indirect, and
///   the validation compiled two compute shaders while the device was created, about 30 ms
///   on the cold-start path;
/// - `DEBUG` on DX12, where it only makes FXC skip optimization (and embed the source):
///   egui's shaders are trivial, and eframe compiles them before the first frame (about
///   20 ms less on the cold-start path).
///
/// `WGPU_DEBUG` / `WGPU_VALIDATION` and the other wgpu variables still apply on top.
fn instance_flags(backends: wgpu::Backends) -> wgpu::InstanceFlags {
    let mut flags = wgpu::InstanceFlags::from_build_config();
    flags.remove(wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL);
    if backends == wgpu::Backends::DX12 && !cfg!(debug_assertions) {
        flags |= wgpu::InstanceFlags::DEBUG;
    }
    flags.with_env()
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
        flags: instance_flags(backends),
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

static LOST: AtomicBool = AtomicBool::new(false);

/// Whether the wgpu device was lost (or hit out-of-memory, which loses it too).
pub fn lost() -> bool {
    LOST.load(Ordering::SeqCst)
}

fn mark_lost(why: &str) {
    if !LOST.swap(true, Ordering::SeqCst) {
        log::error!("GPU device lost: {why}");
    }
    crate::wake::wake();
}

/// Replace wgpu's default error handling: the default handler panics on any uncaptured
/// error. Validation errors are logged (the failed call is skipped, the app keeps running);
/// out-of-memory and device loss mark the device lost so the shell restarts.
pub fn install_handlers(device: &wgpu::Device) {
    device.set_device_lost_callback(|reason, message| {
        mark_lost(&format!("{reason:?}: {message}"));
    });
    device.on_uncaptured_error(Arc::new(|e: wgpu::Error| match e {
        wgpu::Error::OutOfMemory { .. } => mark_lost(&format!("{e}")),
        e => log::error!("wgpu error: {e}"),
    }));
}

/// Whether a panic payload is egui-wgpu or wgpu giving up on the device.
pub fn is_device_panic(payload: &(dyn std::any::Any + Send)) -> bool {
    let msg = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("");
    is_device_panic_message(msg)
}

fn is_device_panic_message(msg: &str) -> bool {
    msg.starts_with("Failed to create staging buffer")
        || msg.starts_with("wgpu error")
        || msg.contains("Parent device is lost")
        || msg.contains("Device lost")
}

/// Environment variable naming the process a restarted instance replaces (`relaunch`).
const RESTART_ENV: &str = "STINKSHOT_RESTART_AFTER";

/// Start a new instance (`--tray`, same profile) that takes over once this process has
/// exited. Returns whether it was started.
pub fn relaunch() -> bool {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            log::error!("restart: no exe path: {e}");
            return false;
        }
    };
    match std::process::Command::new(exe)
        .arg("--tray")
        .env(RESTART_ENV, std::process::id().to_string())
        .env_remove("STINKSHOT_TEST_GPU_LOSS")
        .spawn()
    {
        Ok(c) => {
            log::warn!("restarting after GPU device loss (new pid {})", c.id());
            true
        }
        Err(e) => {
            log::error!("restart failed: {e}");
            false
        }
    }
}

/// In a restarted instance: wait (up to 10 s) for the process it replaces to exit, so that
/// one has released the single-instance mutex. Does nothing for a normal launch.
pub fn wait_for_predecessor() {
    let Some(pid) = std::env::var(RESTART_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
    else {
        return;
    };
    // SAFETY: removing our own variable before any other thread reads the environment.
    unsafe { std::env::remove_var(RESTART_ENV) };
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    // SAFETY: the handle is checked and closed; waiting on a process handle is harmless.
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            let _ = WaitForSingleObject(h, 10_000);
            let _ = CloseHandle(h);
        }
    }
    log::info!("restarted after GPU device loss in process {pid}");
}

/// Diagnostics only: `STINKSHOT_TEST_GPU_LOSS=N` destroys the device while the Nth overlay
/// paints (the in-paint loss path), `idle:S` S seconds after start-up from a worker thread
/// (the between-frames path). Returns the overlay number, if any.
pub fn test_loss_plan(device: &wgpu::Device) -> Option<u32> {
    let v = std::env::var("STINKSHOT_TEST_GPU_LOSS").ok()?;
    if let Some(secs) = v.strip_prefix("idle:").and_then(|s| s.parse::<u64>().ok()) {
        let device = device.clone();
        let _ = std::thread::Builder::new()
            .name("test-gpu-loss".into())
            .spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(secs));
                log::warn!("test: destroying the GPU device");
                device.destroy();
                // `destroy` reports the loss on the next maintenance pass; a real loss
                // reports it at once.
                let _ = device.poll(wgpu::PollType::Poll);
            });
        return None;
    }
    v.parse().ok()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_panics_are_recognized() {
        assert!(is_device_panic_message(
            "Failed to create staging buffer for index data. Index count: 6."
        ));
        assert!(is_device_panic_message(
            "wgpu error: Out of Memory
"
        ));
        assert!(is_device_panic_message(
            "Error in Surface::present: Parent device is lost"
        ));
        assert!(!is_device_panic_message("index out of bounds"));
        let p: Box<dyn std::any::Any + Send> = Box::new(String::from(
            "Failed to create staging buffer for vertex data",
        ));
        assert!(is_device_panic(p.as_ref()));
        let p: Box<dyn std::any::Any + Send> = Box::new("called `Option::unwrap()`");
        assert!(!is_device_panic(p.as_ref()));
    }
}
