//! Size and time of the background PNG re-compression (`png_opt`) on real screenshots:
//! `cargo run -p stinkshot-core --release --example png_opt_bench -- [--zopfli N,N] <png>...`
//!
//! Each file is decoded and saved with Stinkshot's fast encoder (what the capture path writes
//! first), then optimized from that file. The input file's own size is printed too, so a
//! screenshot saved by another tool compares on identical pixels.
use std::time::Instant;

use stinkshot_core::Rgba;
use stinkshot_core::png_opt::{Effort, Outcome, optimize};

fn mb(n: usize) -> f64 {
    n as f64 / 1_048_576.0
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut efforts = vec![Effort::Miniz, Effort::Zopfli(1)];
    if let Some(i) = args.iter().position(|a| a == "--zopfli") {
        let list = args.remove(i + 1);
        args.remove(i);
        efforts = vec![Effort::Miniz];
        efforts.extend(
            list.split(',')
                .filter_map(|v| v.parse::<u8>().ok())
                .map(Effort::Zopfli),
        );
    }
    for path in args {
        let input = std::fs::read(&path).expect("read");
        let img = Rgba::decode(&input).expect("decode");
        let mp = img.width as f64 * img.height as f64 / 1e6;
        println!(
            "{path}: {}x{} ({mp:.1} MP), input file {:.2} MB ({:.3} MB/MP)",
            img.width,
            img.height,
            mb(input.len()),
            mb(input.len()) / mp
        );
        let t = Instant::now();
        let fast = img.to_png().expect("encode");
        println!(
            "  fast encode        {:8.0} ms  {:6.2} MB  {:.3} MB/MP",
            t.elapsed().as_secs_f64() * 1000.0,
            mb(fast.len()),
            mb(fast.len()) / mp
        );
        for &e in &efforts {
            let t = Instant::now();
            let (outcome, report) = optimize(&fast, e).expect("optimize");
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            let size = match &outcome {
                Outcome::Smaller(b) => b.len(),
                _ => fast.len(),
            };
            println!(
                "  {:<18} {ms:8.0} ms  {:6.2} MB  {:.3} MB/MP  ({} / {}, {:.0}% of fast; trials {:.0} ms, deflate {:.0} ms)",
                format!("{e:?}"),
                mb(size),
                mb(size) / mp,
                report.color,
                report.filter,
                100.0 * size as f64 / fast.len() as f64,
                report.trial_ms,
                report.final_ms
            );
        }
    }
}
