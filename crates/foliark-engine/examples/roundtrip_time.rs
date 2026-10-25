//! Times the fuzz `roundtrip` calls: `cargo run --release -p foliark-engine --example roundtrip_time -- file.pdf...`.
use foliark_engine::{Document, writer::Writer};
use foliark_model::Limits;
use std::time::Instant;

fn main() {
    const MIB: u32 = 1 << 20;
    let limits = Limits {
        max_input_bytes: MIB,
        max_pages: 8,
        max_objects: 20_000,
        max_stream_decoded_bytes: 4 * MIB,
        max_decode_ratio: 1000,
        max_image_pixels: 1_000_000,
        max_render_pixels: 250_000,
        max_nesting: 8,
        max_work_units: 2_000_000,
        max_output_bytes: 8 * MIB,
        ..Limits::default()
    };
    let rounds: usize = std::env::var("ROUNDS").map_or(1, |s| s.parse().unwrap());
    for path in std::env::args()
        .skip(1)
        .flat_map(|p| std::iter::repeat_n(p, rounds))
    {
        let data = std::fs::read(&path).unwrap();
        let t = Instant::now();
        let doc = Document::open_with_limits(data, None, true, limits.clone());
        let t_open = t.elapsed().as_secs_f64() * 1e3;
        let Ok(doc) = doc else {
            println!("{path}: open refused {t_open:.1} ms");
            continue;
        };
        let t = Instant::now();
        let out = Writer::new(doc.reader(), limits.clone()).write();
        let t_write = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        let again = out.as_ref().map(|o| {
            Document::open_with_limits(o.clone(), None, false, limits.clone()).map(|_| ())
        });
        let t_reopen = t.elapsed().as_secs_f64() * 1e3;
        println!(
            "{path}: open {t_open:.1} ms, write {t_write:.1} ms ({}), reopen {t_reopen:.1} ms ({:?})",
            out.as_ref()
                .map(|o| o.len().to_string())
                .unwrap_or_else(|e| format!("{:?}", e.code)),
            again.map(|r| r.is_ok())
        );
    }
}
