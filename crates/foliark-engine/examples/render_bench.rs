//! Native reference for the render spike (NEW-63): `cargo run --release -p foliark-engine --example render_bench -- file.pdf [scale] [rounds]`.
//! Prints median ms/page with a fresh `RenderCache` per call (what the engine did) and with one shared per document.
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings, render};
use std::time::Instant;

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let pdf = Pdf::new(std::fs::read(&a[1]).unwrap()).unwrap();
    let scale: f32 = a.get(2).map_or(1.35, |s| s.parse().unwrap());
    let rounds: usize = a.get(3).map_or(5, |s| s.parse().unwrap());
    let pages = pdf.pages();
    let px = PixmapSettings {
        x_scale: scale,
        y_scale: scale,
        bg_color: WHITE,
    };
    let shared = RenderCache::new();
    for (name, share) in [("fresh cache per call", false), ("shared cache", true)] {
        let mut t = vec![];
        for r in 0..=rounds {
            for p in pages.iter() {
                let t0 = Instant::now();
                let fresh = RenderCache::new();
                let c = if share { &shared } else { &fresh };
                let _ = render(
                    p,
                    c,
                    &InterpreterSettings::default(),
                    &RenderSettings::default(),
                    &px,
                )
                .take_rgba8(hayro::vello_cpu::peniko::ImageAlphaType::Alpha);
                if r > 0 {
                    t.push(t0.elapsed().as_secs_f64() * 1e3);
                }
            }
        }
        println!(
            "{name}: {:.2} ms/page median ({} pages)",
            median(t),
            pages.len()
        );
    }
}
