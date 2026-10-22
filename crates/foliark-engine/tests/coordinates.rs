//! Spec §2: the page transforms, checked against geometry (where a corner of the page goes) and
//! against PyMuPDF numbers, not against the table they are computed from.
use foliark_engine::Document;
use foliark_engine::interpret::ItemKind;
use foliark_model::{ErrorCode, Matrix, PageGeometry, Point, Quad, Rect, page_geometry};

const EPS: f64 = 1e-9;

fn p(x: f64, y: f64) -> Point {
    Point { x, y }
}

fn near(a: Point, b: Point) {
    assert!(
        (a.x - b.x).abs() < EPS && (a.y - b.y).abs() < EPS,
        "{a:?} != {b:?}"
    );
}

/// Media [-50, -40, 650, 760], crop [20, 30, 580, 770] (so crop ∩ media is [20, 30, 580, 760]),
/// UserUnit 2.
fn geometry(rotate: i64) -> PageGeometry {
    page_geometry(
        0,
        [-50.0, -40.0, 650.0, 760.0],
        Some([20.0, 30.0, 580.0, 770.0]),
        rotate,
        2.0,
    )
    .unwrap()
}

#[test]
fn every_rotation_turns_the_page_clockwise_around_its_crop_box() {
    let (w0, h0, u) = (560.0, 730.0, 2.0);
    // Crop corners in user space: bottom-left, bottom-right, top-right, top-left.
    let (bl, br, tr, tl) = (
        p(20.0, 30.0),
        p(580.0, 30.0),
        p(580.0, 760.0),
        p(20.0, 760.0),
    );
    // Which crop corner is at the display origin, and the display size, per /Rotate. Turning a
    // page 90 degrees clockwise puts its bottom-left corner at the top-left.
    for (rotate, origin, width, height) in [
        (0, tl, u * w0, u * h0),
        (90, bl, u * h0, u * w0),
        (180, br, u * w0, u * h0),
        (270, tr, u * h0, u * w0),
    ] {
        let g = geometry(rotate);
        assert_eq!(
            (g.width, g.height, g.rotation),
            (width, height, rotate as u16)
        );
        assert_eq!(
            (g.user_unit, g.media_box, g.crop_box),
            (
                2.0,
                [-50.0, -40.0, 650.0, 760.0],
                [20.0, 30.0, 580.0, 760.0]
            )
        );
        near(g.user_to_display.apply(origin), p(0.0, 0.0));
        let opposite = p(580.0 + 20.0 - origin.x, 30.0 + 760.0 - origin.y);
        near(g.user_to_display.apply(opposite), p(width, height));

        // Round trips, point and quad, user -> display -> user and back.
        let quad = Quad([
            p(100.5, 200.25),
            p(180.5, 200.25),
            p(180.5, 180.0),
            p(100.5, 180.0),
        ]);
        let there = quad.map(g.user_to_display);
        let back = there.map(g.display_to_user);
        for (a, b) in quad.0.iter().zip(&back.0) {
            near(*a, *b);
        }
        // Orientation survives: the top edge of the glyph box is still corner 0 -> 1, and the
        // box is 80 x 20.25 user units = 2x that in display points, whatever the rotation.
        let b = there.bounds();
        let (bw, bh) = if rotate % 180 == 0 {
            (160.0, 40.5)
        } else {
            (40.5, 160.0)
        };
        assert!((b.width - bw).abs() < EPS && (b.height - bh).abs() < EPS);
        let top = (there.0[1].x - there.0[0].x).hypot(there.0[1].y - there.0[0].y);
        assert!((top - 160.0).abs() < EPS);
    }
}

#[test]
fn pixel_transforms_scale_display_and_invert() {
    let g = geometry(90);
    let m = g.user_to_pixel(1.5);
    let q = p(100.0, 200.0);
    let d = g.user_to_display.apply(q);
    near(m.apply(q), p(d.x * 1.5, d.y * 1.5));
    near(g.pixel_to_user(1.5).unwrap().apply(m.apply(q)), q);
    assert!(g.pixel_to_user(0.0).is_none());
}

#[test]
fn malformed_geometry_is_refused_not_repaired() {
    let media = [0.0, 0.0, 100.0, 100.0];
    for (rotate, unit, crop) in [
        (45, 1.0, None),
        (0, 0.0, None),
        (0, f64::NAN, None),
        (0, 1.0, Some([500.0, 500.0, 600.0, 600.0])), // crop outside media: empty
        (0, 1.0, Some([0.0, f64::INFINITY, 1.0, 1.0])),
    ] {
        let e = page_geometry(0, media, crop, rotate, unit).unwrap_err();
        assert_eq!(e.code, ErrorCode::MalformedPdf, "{rotate} {unit} {crop:?}");
    }
    // Rotate is taken modulo 360, and corner order in a box does not matter.
    let g = page_geometry(0, [100.0, 100.0, 0.0, 0.0], None, -90, 1.0).unwrap();
    assert_eq!((g.rotation, g.media_box), (270, [0.0, 0.0, 100.0, 100.0]));
}

#[test]
fn normalized_boxes_clamp_and_invert() {
    let g = geometry(0); // 1120 x 1460
    let r = Rect {
        x: 112.0,
        y: 146.0,
        width: 224.0,
        height: 292.0,
    };
    let n = g.normalize(r);
    assert_eq!((n.x, n.y), (0.1, 0.1));
    assert!((n.width - 0.2).abs() < EPS && (n.height - 0.2).abs() < EPS);
    let back = g.denormalize(n);
    assert!((back.x - r.x).abs() < 1e-9 && (back.height - r.height).abs() < 1e-9);
    // Hanging off the page is clamped: the box keeps only the part on it.
    let off = g.normalize(Rect {
        x: -100.0,
        y: 1400.0,
        width: 300.0,
        height: 300.0,
    });
    assert_eq!((off.x, off.y), (0.0, 1400.0 / 1460.0));
    assert!((off.width - 200.0 / 1120.0).abs() < EPS && (off.height - 60.0 / 1460.0).abs() < EPS);
}

/// PyMuPDF 1.26.5 with `fitz.TOOLS.set_small_glyph_heights(True)` on tests/fixtures/hddn/
/// rotation-crop.pdf (4 pages, MediaBox 600x800, CropBox [20 30 580 770], /Rotate 0/90/180/270).
/// Per page: `page.rect` size, then the first word ("KEEP_PUBLIC_ROTATION_CROP") as
/// `fitz.Rect(word[:4]) * page.rotation_matrix`, x0 y0 x1 y1 in display space, then the
/// same box as HDDN normalizes it: nx ny nw nh (x/W, y/H, w/W, h/H).
#[allow(clippy::type_complexity)]
const PYMUPDF: [((f64, f64), [f64; 4], [f64; 4]); 4] = [
    (
        (560.0, 740.0),
        [52.0, 20.611, 244.048, 32.611],
        [0.092857, 0.027853, 0.342943, 0.016216],
    ),
    (
        (740.0, 560.0),
        [707.389, 52.0, 719.389, 244.048],
        [0.955931, 0.092857, 0.016216, 0.342943],
    ),
    (
        (560.0, 740.0),
        [315.952, 707.389, 508.0, 719.389],
        [0.5642, 0.955931, 0.342943, 0.016216],
    ),
    (
        (740.0, 560.0),
        [20.611, 315.952, 32.611, 508.0],
        [0.027853, 0.5642, 0.016216, 0.342943],
    ),
];

#[test]
fn rotation_crop_fixture_matches_pymupdf() {
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/hddn/rotation-crop.pdf"
    ))
    .unwrap();
    let doc = Document::open(bytes, None).unwrap();
    for (i, ((w, h), word, norm)) in PYMUPDF.into_iter().enumerate() {
        let g = doc.page_geometry(i).unwrap();
        assert_eq!((g.width, g.height), (w, h), "page {i}");
        assert_eq!(
            (g.user_unit, g.media_box, g.crop_box),
            (1.0, [0.0, 0.0, 600.0, 800.0], [20.0, 30.0, 580.0, 770.0])
        );
        // The first word is the first 25 glyphs: union of their quads in display space.
        let page = doc.page_model(i).unwrap();
        let quads: Vec<Quad> = page
            .items
            .iter()
            .filter_map(|it| match &it.kind {
                ItemKind::Glyph(gl) => Some(gl.quad_user.map(g.user_to_display)),
                _ => None,
            })
            .take("KEEP_PUBLIC_ROTATION_CROP".len())
            .collect();
        assert_eq!(quads.len(), 25);
        let bounds = quads.iter().map(Quad::bounds).fold(
            [
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            ],
            |a, b| {
                [
                    a[0].min(b.x),
                    a[1].min(b.y),
                    a[2].max(b.x + b.width),
                    a[3].max(b.y + b.height),
                ]
            },
        );
        // Spec §3.6: within 0.75 pt.
        for (got, want) in bounds.iter().zip(word) {
            assert!(
                (got - want).abs() < 0.75,
                "page {i}: {bounds:?} vs {word:?}"
            );
        }
        let n = g.normalize(Rect {
            x: bounds[0],
            y: bounds[1],
            width: bounds[2] - bounds[0],
            height: bounds[3] - bounds[1],
        });
        for (got, want) in [n.x, n.y, n.width, n.height].iter().zip(norm) {
            assert!(
                (got - want).abs() < 0.75 / 560.0,
                "page {i}: normalized {got} vs {want}"
            );
        }
    }
}

#[test]
fn engine_reads_user_unit_negative_media_origin_and_inherited_rotate() {
    use pdf_writer::{Finish, Name, Pdf, Rect as PdfRect, Ref};
    let mut pdf = Pdf::new();
    pdf.catalog(Ref::new(1)).pages(Ref::new(2));
    let mut tree = pdf.pages(Ref::new(2));
    tree.kids([Ref::new(3)]).count(1);
    tree.pair(Name(b"Rotate"), 270);
    tree.finish();
    let mut page = pdf.page(Ref::new(3));
    page.parent(Ref::new(2))
        .media_box(PdfRect::new(-50.0, -40.0, 650.0, 760.0))
        .crop_box(PdfRect::new(20.0, 30.0, 580.0, 770.0))
        .pair(Name(b"UserUnit"), 2.0);
    page.finish();
    let doc = Document::open(pdf.finish(), None).unwrap();
    let g = doc.page_geometry(0).unwrap();
    assert_eq!(g, geometry(270));
    // Crop corner (580, 760) is the display origin for 270 (its top-right goes to the top-left).
    near(g.user_to_display.apply(p(580.0, 760.0)), p(0.0, 0.0));
    let m: Matrix = g.display_to_user;
    near(m.apply(p(g.width, g.height)), p(20.0, 30.0));
}
