//! Spec §1.3: pixels are placed by the canonical display-space transform, and a render says
//! when it is not faithful.
use foliark_engine::{Document, RenderOptions, RenderedPage};
use foliark_model::{ErrorCode, Rect, RenderWarningCode};
use pdf_writer::{Content, Finish, Name, Pdf, Rect as PdfRect, Ref, Str};

const PAGE: Ref = Ref::new(3);
const RED: [u8; 4] = [255, 0, 0, 255];
const WHITE: [u8; 4] = [255; 4];

#[derive(Default)]
struct Spec {
    rotate: i32,
    unit: Option<f32>,
    crop: Option<[f32; 4]>,
    /// Pdf-level extras: font and annotation objects.
    font: Option<&'static [u8]>,
    annot: Option<Option<()>>,
}

/// Media [0 0 300 200], red 40x20 at the bottom-left of the crop and blue 20x10 at its
/// top-right (crop defaults to [50 40 250 140]), so every rotation puts them somewhere else.
fn pdf(spec: &Spec) -> Document {
    let mut pdf = Pdf::new();
    pdf.catalog(Ref::new(1)).pages(Ref::new(2));
    pdf.pages(Ref::new(2)).kids([PAGE]).count(1);
    let mut page = pdf.page(PAGE);
    page.parent(Ref::new(2))
        .media_box(PdfRect::new(0.0, 0.0, 300.0, 200.0))
        .contents(Ref::new(4));
    let [x0, y0, x1, y1] = spec.crop.unwrap_or([50.0, 40.0, 250.0, 140.0]);
    page.crop_box(PdfRect::new(x0, y0, x1, y1));
    if spec.rotate != 0 {
        page.rotate(spec.rotate);
    }
    if let Some(u) = spec.unit {
        page.pair(Name(b"UserUnit"), u);
    }
    if spec.font.is_some() {
        page.resources().fonts().pair(Name(b"F1"), Ref::new(5));
    }
    if spec.annot.is_some() {
        page.annotations([Ref::new(6)]);
    }
    page.finish();
    let mut c = Content::new();
    c.set_fill_rgb(1.0, 0.0, 0.0)
        .rect(x0, y0, 40.0, 20.0)
        .fill_nonzero()
        .set_fill_rgb(0.0, 0.0, 1.0)
        .rect(x1 - 20.0, y1 - 10.0, 20.0, 10.0)
        .fill_nonzero();
    if spec.font.is_some() {
        c.begin_text()
            .set_fill_gray(0.0)
            .set_font(Name(b"F1"), 12.0)
            .next_line(x0 + 60.0, y0 + 40.0)
            .show(Str(b"Hello"))
            .end_text();
    }
    pdf.stream(Ref::new(4), &c.finish());
    if let Some(name) = spec.font {
        pdf.type1_font(Ref::new(5)).base_font(Name(name));
    }
    if let Some(with_ap) = &spec.annot {
        let mut a = pdf.annotation(Ref::new(6));
        a.subtype(pdf_writer::types::AnnotationType::Square)
            .rect(PdfRect::new(x0 + 80.0, y0 + 10.0, x0 + 100.0, y0 + 30.0));
        if with_ap.is_some() {
            a.appearance().normal().stream(Ref::new(7));
        }
        a.finish();
        if with_ap.is_some() {
            let mut ap = pdf.form_xobject(Ref::new(7), b"0 1 0 rg 0 0 20 20 re f");
            ap.bbox(PdfRect::new(0.0, 0.0, 20.0, 20.0));
            ap.finish();
        }
    }
    Document::open(pdf.finish(), None).unwrap()
}

fn render(doc: &Document, options: RenderOptions) -> RenderedPage {
    doc.render_page(0, options).unwrap()
}

fn px(r: &RenderedPage, x: usize, y: usize) -> [u8; 4] {
    let w = r.header.width as usize;
    r.data[(y * w + x) * 4..][..4].try_into().unwrap()
}

/// The image turned `degrees` clockwise, as a PDF viewer turns a page.
fn rotate_cw(r: &RenderedPage, degrees: u32) -> (usize, usize, Vec<u8>) {
    let (mut w, mut h) = (r.header.width as usize, r.header.height as usize);
    let mut data = r.data.clone();
    for _ in 0..degrees / 90 {
        let mut next = vec![0; data.len()];
        for y in 0..h {
            for x in 0..w {
                let to = (x * h + (h - 1 - y)) * 4;
                next[to..to + 4].copy_from_slice(&data[(y * w + x) * 4..][..4]);
            }
        }
        data = next;
        (w, h) = (h, w);
    }
    (w, h, data)
}

#[test]
fn rotated_pages_are_the_unrotated_render_turned_clockwise() {
    let base = render(&pdf(&Spec::default()), RenderOptions::at(1.0));
    assert_eq!((base.header.width, base.header.height), (200, 100));
    assert_eq!(px(&base, 5, 95), RED);
    for degrees in [90, 180, 270] {
        let doc = pdf(&Spec {
            rotate: degrees as i32,
            ..Spec::default()
        });
        let r = render(&doc, RenderOptions::at(1.0));
        let (w, h, expected) = rotate_cw(&base, degrees);
        assert_eq!((r.header.width as usize, r.header.height as usize), (w, h));
        assert!(
            r.data == expected,
            "rotate {degrees} differs from the turned 0 degree render"
        );
    }
}

#[test]
fn user_unit_scales_the_page_like_the_render_scale() {
    let big = render(
        &pdf(&Spec {
            unit: Some(2.0),
            ..Spec::default()
        }),
        RenderOptions::at(1.0),
    );
    let twice = render(&pdf(&Spec::default()), RenderOptions::at(2.0));
    assert_eq!((big.header.width, big.header.height), (400, 200));
    assert!(big.data == twice.data);
}

#[test]
fn clip_is_a_display_rect_and_rounds_per_the_spec() {
    let doc = pdf(&Spec {
        rotate: 90,
        ..Spec::default()
    });
    let full = render(&doc, RenderOptions::at(2.0));
    // Display space of a /Rotate 90 page is 100 x 200; take a window and compare its pixels
    // with the same window cut out of the full render.
    let clip = Rect {
        x: 10.0,
        y: 30.0,
        width: 40.0,
        height: 50.0,
    };
    let part = render(
        &doc,
        RenderOptions {
            scale: 2.0,
            clip: Some(clip),
            ..Default::default()
        },
    );
    assert_eq!((part.header.width, part.header.height), (80, 100));
    assert_eq!(part.header.clip, clip);
    for y in 0..100 {
        for x in 0..80 {
            assert_eq!(px(&part, x, y), px(&full, 20 + x, 60 + y), "pixel {x},{y}");
        }
    }
    // round(10.3 * 1.35) = 14, round(0.1 * 1.35) clamps to 1.
    let odd = Rect {
        x: 0.0,
        y: 0.0,
        width: 10.3,
        height: 0.1,
    };
    let r = render(
        &doc,
        RenderOptions {
            scale: 1.35,
            clip: Some(odd),
            ..Default::default()
        },
    );
    assert_eq!((r.header.width, r.header.height), (14, 1));
    // A whole-page render reports the page as its clip.
    assert_eq!(
        full.header.clip,
        Rect {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 200.0
        }
    );
}

#[test]
fn background_is_straight_alpha_and_transparent_pages_stay_transparent() {
    let doc = pdf(&Spec::default());
    let clear = render(
        &doc,
        RenderOptions {
            background: [0.0; 4],
            ..Default::default()
        },
    );
    assert_eq!(px(&clear, 100, 50), [0, 0, 0, 0]);
    assert_eq!(px(&clear, 5, 95), RED);
    let half = render(
        &doc,
        RenderOptions {
            background: [0.0, 0.0, 1.0, 0.5],
            ..Default::default()
        },
    );
    let [r, g, b, a] = px(&half, 100, 50);
    assert_eq!((r, g, b), (0, 0, 255), "straight, not premultiplied");
    assert!((127..=128).contains(&a));
    assert_eq!(px(&render(&doc, RenderOptions::default()), 100, 50), WHITE);
}

#[test]
fn bad_requests_and_oversized_renders_are_refused() {
    let doc = pdf(&Spec::default());
    let code = |o: RenderOptions| doc.render_page(0, o).err().unwrap().code;
    assert_eq!(code(RenderOptions::at(1e6)), ErrorCode::LimitExceeded);
    // 100 x 80 pt page at 100/pt is 10000 x 8000: inside the side cap, over maxRenderPixels.
    let clip = Some(Rect {
        x: 0.0,
        y: 0.0,
        width: 100.0,
        height: 80.0,
    });
    assert_eq!(
        code(RenderOptions {
            scale: 100.0,
            clip,
            ..Default::default()
        }),
        ErrorCode::LimitExceeded
    );
    for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert_eq!(code(RenderOptions::at(scale)), ErrorCode::InvalidArgument);
    }
    let empty = Some(Rect {
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 5.0,
    });
    assert_eq!(
        code(RenderOptions {
            clip: empty,
            ..Default::default()
        }),
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        code(RenderOptions {
            background: [0.0, 0.0, 0.0, 2.0],
            ..Default::default()
        }),
        ErrorCode::InvalidArgument
    );
    assert_eq!(
        doc.render_page(1, RenderOptions::default())
            .err()
            .unwrap()
            .code,
        ErrorCode::InvalidArgument
    );
}

fn codes(r: &RenderedPage) -> Vec<RenderWarningCode> {
    r.header.warnings.iter().map(|w| w.code).collect()
}

#[test]
fn a_font_the_pdf_does_not_carry_is_reported_but_the_standard_ones_are_not() {
    let standard = render(
        &pdf(&Spec {
            font: Some(b"Helvetica"),
            ..Spec::default()
        }),
        RenderOptions::default(),
    );
    assert!(standard.header.complete && standard.header.warnings.is_empty());
    let r = render(
        &pdf(&Spec {
            font: Some(b"Garamond-Bold"),
            ..Spec::default()
        }),
        RenderOptions::default(),
    );
    assert_eq!(codes(&r), [RenderWarningCode::FontSubstituted]);
    assert!(!r.header.complete);
    assert!(r.header.warnings[0].detail.contains("Garamond"));
}

#[test]
fn annotations_draw_when_asked_and_a_missing_appearance_is_reported() {
    let with = pdf(&Spec {
        annot: Some(Some(())),
        ..Spec::default()
    });
    // Annotation rect [130 50 150 70] in user space is x 80..100, y 70..90 in the crop.
    let green = [0, 255, 0, 255];
    let on = render(&with, RenderOptions::default());
    assert_eq!(px(&on, 90, 80), green);
    assert!(on.header.complete);
    assert_eq!(
        px(
            &render(
                &with,
                RenderOptions {
                    annotations: false,
                    ..Default::default()
                }
            ),
            90,
            80
        ),
        WHITE
    );
    let without = pdf(&Spec {
        annot: Some(None),
        ..Spec::default()
    });
    assert_eq!(
        codes(&render(&without, RenderOptions::default())),
        [RenderWarningCode::AnnotationAppearanceMissing]
    );
    assert!(
        render(
            &without,
            RenderOptions {
                annotations: false,
                ..Default::default()
            }
        )
        .header
        .complete
    );
}

/// A page whose only content is `content`, with `/F1` = object 5 and `/Im` = object 8 left to
/// the caller.
fn bare(content: &[u8], extra: impl FnOnce(&mut Pdf)) -> Document {
    let mut pdf = Pdf::new();
    pdf.catalog(Ref::new(1)).pages(Ref::new(2));
    pdf.pages(Ref::new(2)).kids([PAGE]).count(1);
    let mut page = pdf.page(PAGE);
    page.parent(Ref::new(2))
        .media_box(PdfRect::new(0.0, 0.0, 100.0, 100.0))
        .contents(Ref::new(4));
    let mut res = page.resources();
    res.fonts().pair(Name(b"F1"), Ref::new(5));
    res.x_objects().pair(Name(b"Im"), Ref::new(8));
    res.finish();
    page.finish();
    pdf.stream(Ref::new(4), content);
    extra(&mut pdf);
    Document::open(pdf.finish(), None).unwrap()
}

#[test]
fn an_image_that_will_not_decode_and_a_cmap_the_build_lacks_are_reported() {
    let doc = bare(b"q 50 0 0 50 10 10 cm /Im Do Q", |pdf| {
        let mut im = pdf.image_xobject(Ref::new(8), b"not a jpeg");
        im.filter(pdf_writer::Filter::DctDecode);
        im.width(8).height(8).color_space().device_rgb();
        im.bits_per_component(8);
    });
    let r = render(&doc, RenderOptions::default());
    assert_eq!(codes(&r), [RenderWarningCode::ImageDecodeFailed]);
    assert!(!r.header.complete);

    let doc = bare(b"BT /F1 12 Tf 10 50 Td <8140> Tj ET", |pdf| {
        let mut f = pdf.type0_font(Ref::new(5));
        f.base_font(Name(b"MS-Mincho"))
            .encoding_predefined(Name(b"90ms-RKSJ-H"))
            .descendant_font(Ref::new(6));
        f.finish();
        pdf.cid_font(Ref::new(6))
            .subtype(pdf_writer::types::CidFontType::Type0)
            .base_font(Name(b"MS-Mincho"))
            .system_info(pdf_writer::types::SystemInfo {
                registry: Str(b"Adobe"),
                ordering: Str(b"Japan1"),
                supplement: 2,
            });
    });
    assert_eq!(
        codes(&render(&doc, RenderOptions::default())),
        [RenderWarningCode::CmapMissing]
    );
}
