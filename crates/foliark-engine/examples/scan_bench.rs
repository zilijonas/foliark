//! Scanned-page render timing (NEW-81): `cargo run --release -p foliark-engine --example scan_bench -- file.pdf [scale] [rounds] [out-prefix]`.
//! Per page: median ms of `render_page_exact` (full-size decode, the renderer resamples), of the first `render_page`
//! (first draw: left as is) of the second (reduced, stored) and of the third (image store hit); plus the pixel difference
//! between exact and reduced output (mean abs, max, share of channel values off by more than 24). With an
//! out-prefix, page 0 of both goes to `<prefix>-exact.rgba` / `<prefix>-fast.rgba` (WxH printed).
use foliark_engine::{Document, RenderOptions};
use std::time::Instant;

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn ms<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let t = Instant::now();
    let r = f();
    (r, t.elapsed().as_secs_f64() * 1e3)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).unwrap();
    let scale: f64 = a.get(2).map_or(1.35, |s| s.parse().unwrap());
    let rounds: usize = a.get(3).map_or(5, |s| s.parse().unwrap());
    let opts = || RenderOptions::at(scale);
    let doc = Document::open(bytes.clone(), None).unwrap();
    let pages = doc.page_count();
    let (mut exact, mut first, mut second, mut cached) = (vec![], vec![], vec![], vec![]);
    for r in 0..=rounds {
        let fresh = Document::open(bytes.clone(), None).unwrap();
        for p in 0..pages {
            let (_, e) = ms(|| doc.render_page_exact(p, opts()).unwrap());
            let (_, f) = ms(|| fresh.render_page(p, opts()).unwrap());
            let (_, s2) = ms(|| fresh.render_page(p, opts()).unwrap());
            let (_, c) = ms(|| fresh.render_page(p, opts()).unwrap());
            if r > 0 {
                exact.push(e);
                first.push(f);
                second.push(s2);
                cached.push(c);
            }
        }
    }
    let e = doc.render_page_exact(0, opts()).unwrap();
    // a draw reduces from the second on: compare the one served from the kept decode
    let f = (0..3)
        .map(|_| doc.render_page(0, opts()).unwrap())
        .last()
        .unwrap();
    assert_eq!(
        (e.header.width, e.header.height),
        (f.header.width, f.header.height)
    );
    let diffs: Vec<u32> = e
        .data
        .iter()
        .zip(&f.data)
        .map(|(x, y)| u32::from(x.abs_diff(*y)))
        .collect();
    println!(
        "{}x{} exact {:.1} ms | 1st draw {:.1} | 2nd (reduces) {:.1} | 3rd (kept) {:.1} | diff mean {:.2} max {} off>24 {:.3}%",
        e.header.width,
        e.header.height,
        median(exact),
        median(first),
        median(second),
        median(cached),
        diffs.iter().sum::<u32>() as f64 / diffs.len() as f64,
        diffs.iter().max().unwrap(),
        100.0 * diffs.iter().filter(|d| **d > 24).count() as f64 / diffs.len() as f64,
    );
    if let Some(prefix) = a.get(4) {
        std::fs::write(format!("{prefix}-exact.rgba"), &e.data).unwrap();
        std::fs::write(format!("{prefix}-fast.rgba"), &f.data).unwrap();
    }
}
