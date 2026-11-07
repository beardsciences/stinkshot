//! HDR check for the recorder: records 2 s of a virtual-desktop region, decodes the frame at
//! 1 s and prints the average color of `N` equal-width columns (one per color swatch).
//!
//! `cargo run -p stinkshot-record --example hdr_record_probe --release -- X Y W H N`

use std::time::Duration;

use stinkshot_core::PxRect;
use stinkshot_record::*;

fn main() -> anyhow::Result<()> {
    stinkshot_platform::dpi::init();
    let a: Vec<i32> = std::env::args()
        .skip(1)
        .map(|s| s.parse().expect("X Y W H N"))
        .collect();
    let region = PxRect::new(a[0], a[1], a[2] & !1, a[3] & !1);
    let n = a.get(4).copied().unwrap_or(3).max(1);
    let out = std::env::temp_dir().join(format!("stinkshot-hdr-probe-{}.mp4", std::process::id()));
    let rec = Recorder::start(RecordOptions {
        region,
        output: out.clone(),
        fps: 30,
        bitrate_kbps: 20000,
        system_audio: false,
        microphone: false,
        mic_device: None,
        webcam: None,
        show_clicks: false,
        show_keys: false,
        include_cursor: false,
        accent: [0x4C, 0xA3, 0xFF, 255],
    })?;
    std::thread::sleep(Duration::from_millis(2000));
    let backend = rec.capture_backend();
    let res = rec.stop()?;
    println!("backend {backend}, {}x{}", res.width, res.height);
    let f = frame_at(&res.path, Duration::from_millis(1000), res.height)?;
    let cw = f.width as i32 / n;
    for i in 0..n {
        // central half of each column, avoiding edges and the corner tick
        let r = PxRect::new(
            i * cw + cw / 4,
            f.height as i32 / 4,
            cw / 2,
            f.height as i32 / 2,
        );
        let c = f.crop(r);
        let mut s = [0f64; 3];
        for p in c.data.chunks_exact(4) {
            for k in 0..3 {
                s[k] += p[k] as f64;
            }
        }
        let px = (c.width * c.height).max(1) as f64;
        println!(
            "column {i}: avg {:?}",
            s.map(|v| (v / px * 10.0).round() / 10.0)
        );
    }
    let _ = std::fs::remove_file(&res.path);
    Ok(())
}
