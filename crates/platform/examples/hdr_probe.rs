//! HDR diagnostics: prints each display's advanced-color state and SDR white level, then
//! samples the average color of a small virtual-desktop rect through every capture path.
//!
//! `cargo run -p stinkshot-platform --example hdr_probe --release -- X Y W H`
//!
//! The probed area should be updating (for example a window that repaints every few hundred
//! ms) so Desktop Duplication delivers real frames.

use stinkshot_core::PxRect;
use stinkshot_platform::{capture, hdr, monitors};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::core::Interface;

fn avg(img: &stinkshot_core::Rgba) -> [f32; 3] {
    let mut s = [0f64; 3];
    let n = (img.width * img.height).max(1) as f64;
    for p in img.data.chunks_exact(4) {
        for i in 0..3 {
            s[i] += p[i] as f64;
        }
    }
    s.map(|v| (v / n) as f32)
}

/// Duplicate the output that contains `rect` with `formats`; return (format, raw bytes of the
/// rect, row pitch).
fn dup_sample(
    rect: PxRect,
    formats: &[DXGI_FORMAT],
) -> anyhow::Result<(DXGI_FORMAT, Vec<u8>, usize)> {
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut ai = 0;
        while let Ok(adapter) = factory.EnumAdapters1(ai) {
            ai += 1;
            let mut oi = 0;
            while let Ok(out) = adapter.EnumOutputs(oi) {
                oi += 1;
                let d = out.GetDesc()?;
                let r = d.DesktopCoordinates;
                let orect = PxRect::new(r.left, r.top, r.right - r.left, r.bottom - r.top);
                if orect.intersect(&rect) != Some(rect) {
                    continue;
                }
                let mut device = None;
                let mut ctx = None;
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
                )?;
                let device: ID3D11Device = device.unwrap();
                let ctx: ID3D11DeviceContext = ctx.unwrap();
                let o5: IDXGIOutput5 = out.cast()?;
                let dup = o5.DuplicateOutput1(&device, 0, formats)?;
                let mut seen = 0;
                for _ in 0..200 {
                    let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
                    let mut res = None;
                    if dup.AcquireNextFrame(100, &mut info, &mut res).is_err() {
                        continue;
                    }
                    seen += 1;
                    if seen < 2 || info.LastPresentTime == 0 {
                        let _ = dup.ReleaseFrame();
                        continue;
                    }
                    let tex: ID3D11Texture2D = res.unwrap().cast()?;
                    let mut td = D3D11_TEXTURE2D_DESC::default();
                    tex.GetDesc(&mut td);
                    let mut sd = td;
                    sd.Width = rect.w as u32;
                    sd.Height = rect.h as u32;
                    sd.Usage = D3D11_USAGE_STAGING;
                    sd.BindFlags = 0;
                    sd.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                    sd.MiscFlags = 0;
                    let mut st = None;
                    device.CreateTexture2D(&sd, None, Some(&mut st))?;
                    let st = st.unwrap();
                    let lx = (rect.x - orect.x) as u32;
                    let ly = (rect.y - orect.y) as u32;
                    let b = D3D11_BOX {
                        left: lx,
                        top: ly,
                        front: 0,
                        right: lx + rect.w as u32,
                        bottom: ly + rect.h as u32,
                        back: 1,
                    };
                    ctx.CopySubresourceRegion(&st, 0, 0, 0, 0, &tex, 0, Some(&b));
                    let _ = dup.ReleaseFrame();
                    let mut m = D3D11_MAPPED_SUBRESOURCE::default();
                    ctx.Map(&st, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
                    let pitch = m.RowPitch as usize;
                    let bytes =
                        std::slice::from_raw_parts(m.pData as *const u8, pitch * rect.h as usize)
                            .to_vec();
                    ctx.Unmap(&st, 0);
                    return Ok((td.Format, bytes, pitch));
                }
                anyhow::bail!("no real frame within 20 s (is the area updating?)");
            }
        }
        anyhow::bail!("rect is not inside one output")
    }
}

struct Stderr;
impl log::Log for Stderr {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.target().starts_with("stinkshot")
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            eprintln!("  [{}] {}", r.level(), r.args());
        }
    }
    fn flush(&self) {}
}

fn bench_conversion() {
    let (w, h) = (3840usize, 2160usize);
    let pitch = w * 8;
    let mut src = vec![0u8; pitch * h];
    for (i, px) in src.chunks_exact_mut(8).enumerate() {
        // mostly SDR content with a band of HDR highlights
        let v = if i % w < 200 {
            9.0
        } else {
            (i % 997) as f32 / 997.0 * 6.0
        };
        let b = hdr::f32_to_half(v).to_le_bytes();
        px[..2].copy_from_slice(&b);
        px[2..4].copy_from_slice(&b);
        px[4..6].copy_from_slice(&b);
    }
    let conv = hdr::ScrgbToSrgb::shared(480.0);
    let mut out = vec![0u8; w * h * 4];
    for _ in 0..5 {
        let t = std::time::Instant::now();
        conv.convert_parallel(&src, pitch, w, h, &mut out, w * 4, false);
        println!("convert 3840x2160 FP16 -> RGBA8: {:?}", t.elapsed());
    }
}

fn main() -> anyhow::Result<()> {
    stinkshot_platform::dpi::init();
    if std::env::args().nth(1).as_deref() == Some("bench") {
        bench_conversion();
        return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some("window") {
        // `hdr_probe window <title> <columns>`: window capture (WGC or PrintWindow fallback)
        // of a window, average color of each of N equal columns.
        let title = std::env::args().nth(2).unwrap_or_default();
        let n: u32 = std::env::args()
            .nth(3)
            .and_then(|s| s.parse().ok())
            .unwrap_or(3);
        let w = stinkshot_platform::windows_enum::visible_windows()
            .into_iter()
            .find(|w| w.title == title)
            .ok_or_else(|| anyhow::anyhow!("no window titled {title:?}"))?;
        let img = capture::capture_window(w.hwnd, &capture::WindowCaptureOpts::default())?;
        println!("window {:?} captured {}x{}", w.rect, img.width, img.height);
        let cw = img.width / n;
        for i in 0..n {
            let r = PxRect::new(
                (i * cw + cw / 4) as i32,
                img.height as i32 / 4,
                (cw / 2) as i32,
                img.height as i32 / 2,
            );
            println!("column {i}: avg {:?}", avg(&img.crop(r)));
        }
        return Ok(());
    }
    if std::env::var_os("PROBE_LOG").is_some() {
        let _ = log::set_logger(&Stderr);
        log::set_max_level(log::LevelFilter::Trace);
    }
    let a: Vec<i32> = std::env::args()
        .skip(1)
        .map(|s| s.parse().expect("X Y W H"))
        .collect();
    let rect = PxRect::new(a[0], a[1], a[2], a[3]);
    for m in monitors::list() {
        println!("monitor {:?}", m.rect);
    }
    for d in hdr::displays() {
        println!(
            "display {} advanced_color={} sdr_white={} nits",
            d.device, d.advanced_color, d.sdr_white_nits
        );
    }
    let t = std::time::Instant::now();
    let img = capture::capture_rect(rect, false)?;
    println!(
        "capture_rect (Stinkshot)  avg {:?}  {:?}",
        avg(&img),
        t.elapsed()
    );
    for i in 0..3 {
        let t = std::time::Instant::now();
        let img = capture::capture_rect(rect, false)?;
        println!(
            "capture_rect #{} avg {:?}  {:?}",
            i + 2,
            avg(&img),
            t.elapsed()
        );
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    for _ in 0..5 {
        let t = std::time::Instant::now();
        let f = capture::freeze_all(false)?;
        let el = t.elapsed();
        let crop = f.image.crop(rect.translate(-f.bounds.x, -f.bounds.y));
        println!(
            "freeze_all {}x{} in {:?}, probe avg {:?}",
            f.image.width,
            f.image.height,
            el,
            avg(&crop)
        );
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    // The idle path: shadows dropped (`release_idle_shadows`), then refilled by the next
    // frame or a GDI seed.
    for i in 0..2 {
        capture::release_idle_shadows(std::time::Duration::ZERO);
        let t = std::time::Instant::now();
        let f = capture::freeze_all(false)?;
        let el = t.elapsed();
        let crop = f.image.crop(rect.translate(-f.bounds.x, -f.bounds.y));
        println!(
            "freeze_all after shadow release #{} in {:?}, probe avg {:?}",
            i + 1,
            el,
            avg(&crop)
        );
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    // A process can hold one duplication per output; free Stinkshot's before probing.
    capture::release_cached_capture();

    match dup_sample(rect, &[DXGI_FORMAT_B8G8R8A8_UNORM]) {
        Ok((f, b, pitch)) => {
            let mut s = [0f64; 3];
            for y in 0..rect.h as usize {
                for x in 0..rect.w as usize {
                    let p = &b[y * pitch + x * 4..];
                    s[0] += p[2] as f64;
                    s[1] += p[1] as f64;
                    s[2] += p[0] as f64;
                }
            }
            let n = (rect.w * rect.h) as f64;
            println!("dup BGRA8 ({f:?})  avg {:?}", s.map(|v| (v / n) as f32));
        }
        Err(e) => println!("dup BGRA8: {e}"),
    }
    match dup_sample(
        rect,
        &[DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_B8G8R8A8_UNORM],
    ) {
        Ok((f, b, pitch)) if f == DXGI_FORMAT_R16G16B16A16_FLOAT => {
            let mut s = [0f64; 3];
            for y in 0..rect.h as usize {
                for x in 0..rect.w as usize {
                    let p = &b[y * pitch + x * 8..];
                    for (i, v) in s.iter_mut().enumerate() {
                        *v += hdr::half_to_f32(u16::from_le_bytes([p[i * 2], p[i * 2 + 1]])) as f64;
                    }
                }
            }
            let n = (rect.w * rect.h) as f64;
            let lin = s.map(|v| (v / n) as f32);
            println!("dup FP16 raw scRGB avg {lin:?}");
            let white = monitors::list()
                .into_iter()
                .find(|m| m.rect.intersect(&rect) == Some(rect))
                .map(|m| m.rect);
            println!("  (output {white:?})");
            for d in hdr::displays() {
                let c = hdr::ScrgbToSrgb::new(d.sdr_white_nits);
                let mut out = vec![0u8; rect.w as usize * rect.h as usize * 4];
                c.convert_parallel(
                    &b,
                    pitch,
                    rect.w as usize,
                    rect.h as usize,
                    &mut out,
                    rect.w as usize * 4,
                    false,
                );
                let img = stinkshot_core::Rgba {
                    width: rect.w as u32,
                    height: rect.h as u32,
                    data: out,
                };
                println!(
                    "  converted with {} white {} nits: avg {:?}",
                    d.device,
                    d.sdr_white_nits,
                    avg(&img)
                );
            }
        }
        Ok((f, _, _)) => println!("dup FP16 request returned {f:?} (SDR output)"),
        Err(e) => println!("dup FP16: {e}"),
    }
    Ok(())
}
