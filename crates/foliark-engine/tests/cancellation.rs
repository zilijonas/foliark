//! A hostile page must not burn CPU unbounded inside one call: the cancel flag and the work
//! budget stop the interpreter mid-page (hayro P15), and the stopped operation never returns a
//! partial page.
use flate2::{Compression, write::ZlibEncoder};
use foliark_engine::{Document, RenderOptions};
use foliark_model::{ErrorCode, Limits};
use pdf_writer::{Filter, Pdf, Rect, Ref};
use std::io::Write;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// One page of `pairs` times `q Q`: cheap to store (Flate), long to interpret.
fn long_page(pairs: usize) -> Vec<u8> {
    let mut z = ZlibEncoder::new(Vec::new(), Compression::best());
    z.write_all(&b"q Q ".repeat(pairs)).unwrap();
    let data = z.finish().unwrap();
    let mut pdf = Pdf::new();
    pdf.catalog(Ref::new(1)).pages(Ref::new(2));
    pdf.pages(Ref::new(2)).kids([Ref::new(3)]).count(1);
    pdf.page(Ref::new(3))
        .parent(Ref::new(2))
        .media_box(Rect::new(0.0, 0.0, 100.0, 100.0))
        .contents(Ref::new(4));
    pdf.stream(Ref::new(4), &data).filter(Filter::FlateDecode);
    pdf.finish()
}

const PAIRS: usize = 1_500_000;

fn limits(max_work_units: u64) -> Limits {
    Limits {
        // `q Q` repeated compresses far beyond the default ratio guard.
        max_decode_ratio: u32::MAX,
        max_work_units,
        ..Default::default()
    }
}

fn open(max_work_units: u64) -> Document {
    Document::open_with_limits(long_page(PAIRS), None, false, limits(max_work_units)).unwrap()
}

/// Run `f`, returning its result and how long it took.
fn timed<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let start = Instant::now();
    let out = f();
    (out, start.elapsed())
}

#[test]
fn interpretation_stops_mid_page_on_cancel_and_on_the_work_budget() {
    let full = open(u64::MAX);
    let (page, whole) = timed(|| full.page_model(0));
    page.expect("the page interprets in full");

    // Another thread sets the flag while the page is being interpreted.
    let doc = open(u64::MAX);
    let flag = doc.cancel_handle();
    let stopper = std::thread::spawn({
        let flag = flag.clone();
        move || {
            std::thread::sleep(whole / 10);
            flag.store(true, Ordering::Relaxed);
        }
    });
    let (cancelled, took) = timed(|| doc.page_model(0));
    stopper.join().unwrap();
    assert_eq!(cancelled.err().map(|e| e.code), Some(ErrorCode::Cancelled));
    assert!(took < whole / 2, "cancel took {took:?} of {whole:?}");
    // The flag is the host's to clear; afterwards the same document works again.
    flag.store(false, Ordering::Relaxed);
    doc.page_model(0)
        .expect("a cancelled call leaves the document usable");

    // The work budget ends the page the same way instead of at its end.
    let small = open(PAIRS as u64 / 4);
    let (tripped, took) = timed(|| small.page_model(0));
    let e = tripped.expect_err("budget trips");
    assert_eq!(e.code, ErrorCode::LimitExceeded);
    assert_eq!(e.details.unwrap().limit.as_deref(), Some("maxWorkUnits"));
    assert!(took < whole / 2, "budget stop took {took:?} of {whole:?}");
}

#[test]
fn rendering_honours_the_flag_and_counts_operators_against_the_budget() {
    let e = open(PAIRS as u64 / 4)
        .render_page(0, RenderOptions::at(1.0))
        .expect_err("budget trips");
    assert_eq!(e.code, ErrorCode::LimitExceeded);
    assert_eq!(e.details.unwrap().limit.as_deref(), Some("maxWorkUnits"));

    let doc = open(u64::MAX);
    doc.cancel_handle().store(true, Ordering::Relaxed);
    let e = doc.render_page(0, RenderOptions::at(1.0)).err().unwrap();
    assert_eq!(e.code, ErrorCode::Cancelled);
    doc.cancel_handle().store(false, Ordering::Relaxed);
    doc.render_page(0, RenderOptions::at(1.0))
        .expect("renders once the flag is clear");
}
