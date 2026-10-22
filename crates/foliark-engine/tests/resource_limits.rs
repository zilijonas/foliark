//! Budgets (spec §9): each limit refuses before it allocates, and sits exactly at its edge.
use flate2::{Compression, write::ZlibEncoder};
use foliark_engine::reader::Document;
use foliark_engine::{Document as Engine, RenderOptions};
use foliark_model::{ErrorCode, FoliarkError, Limits, PdfError};
use pdf_writer::{Filter, Finish, Name, Pdf, Rect, Ref};
use std::io::Write;

const MIB: usize = 1 << 20;

fn limit(limit: &'static str, actual: u64, max: u64) -> PdfError {
    PdfError::limit(limit, actual, max)
}

/// The `LIMIT_EXCEEDED` details of a failed result: (limit, actual, max).
fn limit_of<T>(r: Result<T, PdfError>) -> (String, u64, u64) {
    let e = r.err().expect("a limit error");
    assert_eq!(e.code, ErrorCode::LimitExceeded);
    let d = e.details.expect("details");
    (
        d.limit.unwrap(),
        d.actual.unwrap() as u64,
        d.max.unwrap() as u64,
    )
}

fn open(bytes: Vec<u8>, limits: Limits) -> Result<Document, PdfError> {
    Document::open_with_limits(bytes, None, false, limits)
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut e = ZlibEncoder::new(Vec::new(), Compression::best());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

/// One 100x100 page whose content is `content`, optionally with a gray `w`x`h` image `/Im`.
fn page_pdf(content: &[u8], filter: Option<Filter>, image: Option<(i32, i32)>) -> Vec<u8> {
    let (cat, tree, page, c, im) = (
        Ref::new(1),
        Ref::new(2),
        Ref::new(3),
        Ref::new(4),
        Ref::new(5),
    );
    let mut pdf = Pdf::new();
    pdf.catalog(cat).pages(tree);
    pdf.pages(tree).kids([page]).count(1);
    let mut p = pdf.page(page);
    p.parent(tree)
        .media_box(Rect::new(0.0, 0.0, 100.0, 100.0))
        .contents(c);
    if image.is_some() {
        p.resources().x_objects().pair(Name(b"Im"), im);
    }
    p.finish();
    let mut s = pdf.stream(c, content);
    if let Some(f) = filter {
        s.filter(f);
    }
    s.finish();
    if let Some((w, h)) = image {
        let mut i = pdf.image_xobject(im, &[0x80; 64]);
        i.width(w).height(h).bits_per_component(8);
        i.color_space().device_gray();
    }
    pdf.finish()
}

fn pages_pdf(n: i32, claimed: i32) -> Vec<u8> {
    let mut pdf = Pdf::new();
    pdf.catalog(Ref::new(1)).pages(Ref::new(2));
    let kids: Vec<Ref> = (0..n).map(|i| Ref::new(10 + i)).collect();
    pdf.pages(Ref::new(2))
        .kids(kids.iter().copied())
        .count(claimed);
    for k in kids {
        pdf.page(k)
            .parent(Ref::new(2))
            .media_box(Rect::new(0.0, 0.0, 10.0, 10.0));
    }
    pdf.finish()
}

#[test]
fn input_size_is_checked_before_parsing() {
    let bytes = page_pdf(b"", None, None);
    let n = bytes.len() as u32;
    let at = |max_input_bytes| Limits {
        max_input_bytes,
        ..Default::default()
    };
    assert!(open(bytes.clone(), at(n)).is_ok());
    // Not a PDF at all: the size error wins because nothing was parsed.
    let garbage = vec![0u8; 100];
    assert_eq!(
        open(garbage, at(99)).err(),
        Some(limit("maxInputBytes", 100, 99))
    );
    let wire = FoliarkError::from(open(bytes, at(n - 1)).err().unwrap());
    assert_eq!(wire.code, ErrorCode::LimitExceeded);
    let d = wire.details.unwrap();
    assert_eq!(
        (d.limit.as_deref(), d.actual, d.max),
        (
            Some("maxInputBytes"),
            Some(f64::from(n)),
            Some(f64::from(n - 1))
        )
    );
}

#[test]
fn page_count_is_the_real_one_not_the_claimed_one() {
    let at = |max_pages| Limits {
        max_pages,
        ..Default::default()
    };
    assert_eq!(open(pages_pdf(50, 50), at(50)).unwrap().page_count(), 50);
    assert_eq!(
        open(pages_pdf(51, 51), at(50)).err(),
        Some(limit("maxPages", 51, 50))
    );
    // /Count says 5: the kids are what counts.
    assert_eq!(
        open(pages_pdf(51, 5), at(50)).err(),
        Some(limit("maxPages", 51, 50))
    );
    // The 1001-page HDDN fixture against the SDK default of 1000.
    let big = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/generated/many-pages-1001.pdf"
    ))
    .unwrap();
    assert_eq!(
        Document::open(big, None, false).err(),
        Some(limit("maxPages", 1001, 1000))
    );
}

#[test]
fn object_count_limit() {
    let bytes = pages_pdf(3, 3);
    let n = open(bytes.clone(), Limits::default()).unwrap().pdf().len() as u32;
    let at = |max_objects| Limits {
        max_objects,
        ..Default::default()
    };
    assert!(open(bytes.clone(), at(n)).is_ok());
    assert_eq!(
        open(bytes, at(n - 1)).err(),
        Some(limit("maxObjects", n.into(), (n - 1).into()))
    );
}

#[test]
fn expansion_bomb_stops_at_the_cap_and_the_edge_passes() {
    let bomb = zlib(&vec![0u8; 10 * MIB]);
    assert!(bomb.len() < 20_000, "10 MiB of zeros must compress hard");
    let pdf = page_pdf(&bomb, Some(Filter::FlateDecode), None);

    // Per-stream byte cap: decoding stops one byte past it, it never builds the 10 MiB.
    let capped = Limits {
        max_stream_decoded_bytes: MIB as u32,
        max_decode_ratio: u32::MAX,
        ..Default::default()
    };
    let err = open(pdf.clone(), capped.clone())
        .unwrap()
        .page_contents(0)
        .unwrap_err();
    assert_eq!(
        err,
        limit("maxStreamDecodedBytes", MIB as u64 + 1, MIB as u64)
    );
    assert_eq!(open(pdf.clone(), capped).unwrap().info().err(), Some(err));

    // Ratio cap: floor of 1 MiB, then raw length x ratio.
    let ratio = Limits {
        max_decode_ratio: 100,
        ..Default::default()
    };
    let (name, actual, max) = limit_of(open(pdf.clone(), ratio).unwrap().page_contents(0));
    assert_eq!((name.as_str(), max), ("maxDecodeRatio", 100));
    assert!(actual > 100);
    // The default ratio (1000) already refuses it: deflate manages about 1030 on zeros. The
    // same bytes pass a ratio limit above that, and the zeros parse as whitespace.
    let (name, _, max) = limit_of(
        open(pdf.clone(), Limits::default())
            .unwrap()
            .page_contents(0),
    );
    assert_eq!((name.as_str(), max), ("maxDecodeRatio", 1000));
    let loose = Limits {
        max_decode_ratio: 2000,
        ..Default::default()
    };
    assert!(open(pdf, loose).unwrap().page_contents(0).is_ok());

    // Exactly the cap passes, one byte more does not.
    let spaces = |n| page_pdf(&zlib(&vec![b' '; n]), Some(Filter::FlateDecode), None);
    let at = |max| Limits {
        max_stream_decoded_bytes: max,
        max_decode_ratio: u32::MAX,
        ..Default::default()
    };
    let n = 64 * 1024;
    assert!(
        open(spaces(n), at(n as u32))
            .unwrap()
            .page_contents(0)
            .is_ok()
    );
    assert_eq!(
        open(spaces(n + 1), at(n as u32))
            .unwrap()
            .page_contents(0)
            .err(),
        Some(limit("maxStreamDecodedBytes", n as u64 + 1, n as u64))
    );
}

#[test]
fn image_dimension_bomb_is_refused_from_the_dictionary() {
    // 100,000,000 pixels is the default: 10000x10000 passes, one more row does not.
    let image = |w, h| page_pdf(b"", None, Some((w, h)));
    assert!(
        open(image(10_000, 10_000), Limits::default())
            .unwrap()
            .info()
            .is_ok()
    );
    assert_eq!(
        open(image(10_000, 10_001), Limits::default())
            .unwrap()
            .info()
            .err(),
        Some(limit("maxImagePixels", 100_010_000, 100_000_000))
    );
    // Both sides near u32::MAX: the product must not wrap.
    let (name, ..) = limit_of(
        open(image(i32::MAX, i32::MAX), Limits::default())
            .unwrap()
            .info(),
    );
    assert_eq!(name, "maxImagePixels");
    // The renderer refuses it too, without decoding the image.
    let doc = Engine::open(image(40_000, 40_000), None).unwrap();
    let (name, ..) = limit_of(doc.render_page(0, RenderOptions::at(1.0)));
    assert_eq!(name, "maxImagePixels");
    // Drawn images are checked as they are met, including ones the scan cannot see.
    let tight = Limits {
        max_image_pixels: 64,
        ..Default::default()
    };
    assert!(
        open(
            page_pdf(b"q 5 0 0 5 0 0 cm /Im Do Q", None, Some((8, 8))),
            tight.clone()
        )
        .unwrap()
        .page_model(0)
        .is_ok()
    );
    assert_eq!(
        open(
            page_pdf(b"q 5 0 0 5 0 0 cm /Im Do Q", None, Some((9, 8))),
            tight
        )
        .unwrap()
        .page_model(0)
        .err(),
        Some(limit("maxImagePixels", 72, 64))
    );
}

#[test]
fn render_pixels_are_limited_per_render() {
    let at = Limits {
        max_render_pixels: 100 * 100,
        ..Default::default()
    };
    let doc = Engine::open_with_limits(page_pdf(b"", None, None), None, false, at).unwrap();
    assert!(doc.render_page(0, RenderOptions::at(1.0)).is_ok());
    assert_eq!(
        limit_of(doc.render_page(0, RenderOptions::at(1.01))),
        ("maxRenderPixels".to_owned(), 10201, 10000)
    );
}

#[test]
fn deep_and_cyclic_form_graphs_are_typed_errors() {
    let fixture = |name: &str| {
        std::fs::read(format!(
            "{}/../../tests/fixtures/generated/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    };
    let cycle = open(fixture("forms-cyclic.pdf"), Limits::default())
        .unwrap()
        .page_model(0)
        .unwrap_err();
    assert_eq!(cycle.code, ErrorCode::MalformedPdf);
    let deep = fixture("forms-deep-40.pdf");
    assert_eq!(
        open(deep.clone(), Limits::default())
            .unwrap()
            .page_model(0)
            .err(),
        Some(limit("maxNesting", 33, 32))
    );
    let at = |max_nesting| Limits {
        max_nesting,
        ..Default::default()
    };
    assert!(open(deep.clone(), at(40)).unwrap().page_model(0).is_ok());
    assert_eq!(
        open(deep, at(39)).unwrap().page_model(0).err(),
        Some(limit("maxNesting", 40, 39))
    );
}

#[test]
fn object_nesting_far_past_the_parser_limit_does_not_blow_the_stack() {
    let mut pdf = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R /X ".to_vec();
    pdf.extend(std::iter::repeat_n(b'[', 100_000));
    pdf.extend(std::iter::repeat_n(b']', 100_000));
    pdf.extend(b" >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n");
    let outcome = Document::open(pdf, None, true).and_then(|d| d.info());
    assert!(outcome.is_ok() || outcome.is_err()); // reaching this line is the test
}

#[test]
fn a_lying_stream_length_neither_allocates_nor_reads_past_the_stream() {
    let content = b"1 0 0 rg 0 0 5 5 re f";
    for bad in ["4000000000", "-5", "3"] {
        let mut bytes = page_pdf(content, None, None);
        let want = format!("/Length {}", content.len());
        let at = bytes
            .windows(want.len())
            .position(|w| w == want.as_bytes())
            .unwrap();
        bytes.splice(at..at + want.len(), format!("/Length {bad}").into_bytes());
        let outcome = Document::open(bytes, None, true).and_then(|d| d.page_contents(0));
        // Either the parser recovered the real stream or it said so; nothing in between.
        match outcome {
            Ok(streams) => assert_eq!(streams[0].data, content, "/Length {bad}"),
            Err(e) => assert_ne!(e.code, ErrorCode::Internal, "/Length {bad}"),
        }
    }
}

#[test]
fn work_units_cap_the_interpreter_exactly() {
    let content: Vec<u8> = b"0 0 m 5 5 l S ".repeat(50);
    let pdf = page_pdf(&content, None, None);
    let items = |max_work_units| {
        let limits = Limits {
            max_work_units,
            ..Default::default()
        };
        open(pdf.clone(), limits).unwrap().page_model(0)
    };
    assert!(items(5_000_000_000).is_ok());
    // Find the smallest budget that passes; one less must fail with the work-unit limit.
    let edge = (1..2000u64)
        .find(|&n| items(n).is_ok())
        .expect("some budget passes");
    assert!(
        edge > 100,
        "operators are charged, not just the stream: {edge}"
    );
    assert_eq!(
        items(edge - 1).err(),
        Some(limit("maxWorkUnits", edge, edge - 1))
    );
}
