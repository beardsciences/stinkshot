//! PNG encode timing for real screenshots:
//! `cargo run -p stinkshot-core --release --example encode_bench -- <png>...`
use std::time::Instant;

use image::ImageEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use stinkshot_core::Rgba;

fn main() {
    for path in std::env::args().skip(1) {
        let img = Rgba::load(std::path::Path::new(&path)).expect("load");
        println!("{path}: {}x{}", img.width, img.height);
        for (name, c, f) in [
            (
                "default+adaptive",
                CompressionType::Default,
                FilterType::Adaptive,
            ),
            ("fast+adaptive", CompressionType::Fast, FilterType::Adaptive),
            ("fast+up", CompressionType::Fast, FilterType::Up),
            ("fast+paeth", CompressionType::Fast, FilterType::Paeth),
            ("fast+sub", CompressionType::Fast, FilterType::Sub),
        ] {
            let mut best = f64::MAX;
            let mut size = 0;
            for _ in 0..3 {
                let t = Instant::now();
                let mut buf = Vec::with_capacity(img.data.len() / 3);
                PngEncoder::new_with_quality(&mut buf, c, f)
                    .write_image(
                        &img.data,
                        img.width,
                        img.height,
                        image::ExtendedColorType::Rgba8,
                    )
                    .unwrap();
                best = best.min(t.elapsed().as_secs_f64() * 1000.0);
                size = buf.len();
            }
            println!("  {name:18} {best:7.1} ms  {:6.2} MB", size as f64 / 1e6);
        }
        let mut best = f64::MAX;
        let mut size = 0;
        for _ in 0..3 {
            let t = Instant::now();
            let buf = img.to_png().unwrap();
            best = best.min(t.elapsed().as_secs_f64() * 1000.0);
            size = buf.len();
        }
        println!(
            "  {:18} {best:7.1} ms  {:6.2} MB",
            "to_png()",
            size as f64 / 1e6
        );
        let t = Instant::now();
        let thumb = img.thumbnail(360, 220);
        println!(
            "  thumbnail {}x{}    {:7.1} ms",
            thumb.width,
            thumb.height,
            t.elapsed().as_secs_f64() * 1000.0
        );
    }
}
