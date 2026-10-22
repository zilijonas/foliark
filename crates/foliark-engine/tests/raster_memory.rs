//! Raster export memory (spec §7, §10.1; NEW-55): one page buffer at a time, so the peak does
//! not grow with the page count. Its own test binary: the counting allocator is process-wide.
mod common;
use common::{Font, pdf_bytes, run};
use foliark_engine::Engine;
use foliark_model::{ExportMode, ExportPolicy, Limits};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

struct Counting;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let live = LIVE.fetch_add(l.size(), Relaxed) + l.size();
        PEAK.fetch_max(live, Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size(), Relaxed);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let live = LIVE.fetch_add(new, Relaxed) + new - l.size();
        LIVE.fetch_sub(l.size(), Relaxed);
        PEAK.fetch_max(live, Relaxed);
        unsafe { System.realloc(p, l, new) }
    }
}

#[global_allocator]
static A: Counting = Counting;

/// Peak heap above the pre-export baseline for a raster export of `pages` pages at scale 4
/// (a 1200 x 1600 px page: 7.68 MB as RGBA).
fn peak(pages: usize) -> usize {
    let page = "BT /F1 12 Tf 20 300 Td (some text) Tj ET";
    let mut e = Engine::new(Limits::default());
    let (doc, _) = e
        .open(pdf_bytes(&vec![page; pages], &Font::default(), 0), None, false)
        .unwrap();
    let mut policy = ExportPolicy::default();
    policy.raster.acknowledge_losses = true;
    policy.raster.scale = 4.0;
    let base = LIVE.load(Relaxed);
    PEAK.store(base, Relaxed);
    let t = e.prepare_export(&doc, ExportMode::Raster, policy);
    run(&mut e, t).unwrap();
    PEAK.load(Relaxed) - base
}

#[test]
fn peak_memory_is_one_page_buffer_whatever_the_page_count() {
    let one_page = 1200 * 1600 * 4;
    let (few, many) = (peak(2), peak(30));
    assert!(few >= one_page, "the probe sees the page buffer: {few}");
    // 28 more pages add their encoded size and nothing like another page buffer.
    assert!(many - few < one_page / 4, "peak grew {few} -> {many}");
    assert!(many < 2 * one_page + one_page / 2, "more than ~two buffers: {many}");
}
