//! Diagnoses Desktop Duplication failures: tries `DuplicateOutput1` on every output while the
//! process is DPI unaware, with a per-monitor-v2 thread context, after making the process
//! per-monitor-v2 aware, and a second time while the first duplication is still alive.
//!
//! `cargo run -p stinkshot-record --example dup_probe`

use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput1, IDXGIOutput5, IDXGIOutputDuplication,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};
use windows::core::Interface;

fn outputs() -> Vec<(ID3D11Device, IDXGIOutput1, String, bool)> {
    let mut v = Vec::new();
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().unwrap();
        let colors = stinkshot_platform::hdr::displays();
        let mut ai = 0;
        while let Ok(adapter) = factory.EnumAdapters1(ai) {
            ai += 1;
            let mut oi = 0;
            while let Ok(out) = adapter.EnumOutputs(oi) {
                oi += 1;
                let desc = out.GetDesc().unwrap();
                let name = String::from_utf16_lossy(&desc.DeviceName)
                    .trim_end_matches('\0')
                    .to_string();
                let hdr = colors.iter().any(|c| c.device == name && c.advanced_color);
                let mut device: Option<ID3D11Device> = None;
                D3D11CreateDevice(
                    &adapter,
                    D3D_DRIVER_TYPE_UNKNOWN,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    None,
                )
                .unwrap();
                v.push((device.unwrap(), out.cast().unwrap(), name, hdr));
            }
        }
    }
    v
}

fn try_all(label: &str, keep: bool) -> Vec<IDXGIOutputDuplication> {
    let mut kept = Vec::new();
    for (dev, out, name, hdr) in outputs() {
        let fmt = if hdr {
            DXGI_FORMAT_R16G16B16A16_FLOAT
        } else {
            DXGI_FORMAT_B8G8R8A8_UNORM
        };
        let r = unsafe {
            out.cast::<IDXGIOutput5>()
                .and_then(|o5| o5.DuplicateOutput1(&dev, 0, &[fmt]))
        };
        let r1 = unsafe { out.DuplicateOutput(&dev) };
        println!(
            "{label:<28} {name} hdr={hdr}: DuplicateOutput1 -> {}, DuplicateOutput -> {}",
            match &r {
                Ok(_) => "ok".to_string(),
                Err(e) => format!("{:#X}", e.code().0),
            },
            match &r1 {
                Ok(_) => "ok".to_string(),
                Err(e) => format!("{:#X}", e.code().0),
            }
        );
        if keep {
            kept.extend(r.ok());
            kept.extend(r1.ok());
        }
    }
    kept
}

fn main() {
    try_all("process unaware", false);
    unsafe {
        let old = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        try_all("thread PMv2 context", false);
        SetThreadDpiAwarenessContext(old);
    }
    stinkshot_platform::dpi::init();
    let held = try_all("process PMv2", true);
    try_all("PMv2, already duplicated", false);
    drop(held);
    stinkshot_platform::capture::warm_up();
    try_all("after platform warm_up", false);
    stinkshot_platform::capture::release_cached_capture();
    try_all("after release", false);
}
