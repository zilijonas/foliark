//! Raster pixels (spec §7; NEW-55): regions burned into the drawn page, checked on the pixels
//! PyMuPDF decodes from the output file and against the engine's own unburned render of the same
//! revision. Geometry (CropBox, /Rotate, /UserUnit) is placed by a MuPDF render of the SOURCE.
mod common;
use common::{Font, pdf_bytes, python, run, tmp};
use foliark_engine::{Applied, Document, EditApplier, EditContext, Engine, RenderOptions};
use foliark_model::{
    ChangeSet, ExportMode, ExportPolicy, ExtractOptions, Limits, Rect, Redaction, RejectedChange,
    TextEdit,
};
use pdf_writer::{Finish, Pdf, Rect as PdfRect, Ref};

const SCALE: f64 = 2.0;

fn consenting() -> ExportPolicy {
    let mut p = ExportPolicy::default();
    p.raster.acknowledge_losses = true;
    p.raster.scale = SCALE;
    p
}

/// 300 x 400 page with a red square (user 40,60 60x60) and a blue one (user 200,300 60x60).
fn marker_pdf(rotate: i32, crop: Option<[f32; 4]>, unit: Option<f32>) -> Vec<u8> {
    let mut pdf = Pdf::new();
    pdf.catalog(Ref::new(1)).pages(Ref::new(2));
    pdf.pages(Ref::new(2)).kids([Ref::new(3)]).count(1);
    let mut p = pdf.page(Ref::new(3));
    p.parent(Ref::new(2))
        .media_box(PdfRect::new(0.0, 0.0, 300.0, 400.0))
        .contents(Ref::new(4));
    if let Some([a, b, c, d]) = crop {
        p.crop_box(PdfRect::new(a, b, c, d));
    }
    if rotate != 0 {
        p.rotate(rotate);
    }
    if let Some(u) = unit {
        p.pair(pdf_writer::Name(b"UserUnit"), u);
    }
    p.finish();
    pdf.stream(
        Ref::new(4),
        b"1 0 0 rg 40 60 60 60 re f 0 0 1 rg 200 300 60 60 re f",
    );
    pdf.finish()
}

fn at(bytes: &[u8], name: &str) -> String {
    let p = tmp("raster-pixels", name);
    std::fs::write(&p, bytes).unwrap();
    p.to_str().unwrap().to_owned()
}

struct Decoded {
    width: usize,
    height: usize,
    rgb: Vec<u8>,
}

/// The output page's single image as PyMuPDF decodes it.
fn decode(pdf: &[u8], name: &str) -> Decoded {
    let (file, raw) = (at(pdf, name), tmp("raster-pixels", &format!("{name}.rgb")));
    let (ok, log) = python(&[
        "tests/oracles/raster.py",
        "image",
        &file,
        "0",
        raw.to_str().unwrap(),
    ]);
    assert!(ok, "raster.py image: {log}");
    let dims: Vec<usize> = log.split_whitespace().map(|v| v.parse().unwrap()).collect();
    let rgb = std::fs::read(raw).unwrap();
    assert_eq!(rgb.len(), dims[0] * dims[1] * 3);
    Decoded {
        width: dims[0],
        height: dims[1],
        rgb,
    }
}

fn red_box_pt(src: &[u8], name: &str) -> Rect {
    let (ok, log) = python(&[
        "tests/oracles/raster.py",
        "redbox",
        &at(src, name),
        "0",
        "2",
    ]);
    assert!(ok, "{log}");
    let v: Vec<f64> = log.split_whitespace().map(|v| v.parse().unwrap()).collect();
    Rect {
        x: v[0],
        y: v[1],
        width: v[2] - v[0],
        height: v[3] - v[1],
    }
}

fn redact(e: &mut Engine, doc: &str, base: u32, rect: Rect, fill: Option<[f64; 3]>) {
    let t = e.apply_changes(
        doc,
        ChangeSet {
            base_revision: base,
            text_edits: None,
            redactions: Some(vec![Redaction::Region {
                id: "r".into(),
                page_index: 0,
                rect,
                fill: Some(fill),
            }]),
        },
    );
    assert!(run(e, t).unwrap().accepted);
}

fn export(e: &mut Engine, doc: &str) -> Vec<u8> {
    let t = e.prepare_export(doc, ExportMode::Raster, consenting());
    let plan = run(e, t).unwrap();
    e.take_export(doc, &plan.plan_id).unwrap()
}

/// Pixel range `(x0, y0, x1, y1)` a region burns at `SCALE`: the squares that intersect it.
fn burned(r: &Rect, w: usize, h: usize) -> (usize, usize, usize, usize) {
    let lo = |v: f64, max: usize| ((v * SCALE).floor().max(0.0) as usize).min(max);
    let hi = |v: f64, max: usize| ((v * SCALE).ceil().max(0.0) as usize).min(max);
    (
        lo(r.x, w),
        lo(r.y, h),
        hi(r.x + r.width, w),
        hi(r.y + r.height, h),
    )
}

/// Burned pixels are the fill (white for null, spec §7), nothing else differs from the engine's unburned render of the same
/// document, and the pure-red marker is gone from the decoded output.
fn check_burn(src: &[u8], region: Rect, fill: Option<[f64; 3]>, label: &str) -> Decoded {
    let mut e = Engine::new(Limits::default());
    let (doc, _) = e.open(src.to_vec(), None, false).unwrap();
    let t = e.render(&doc, 0, RenderOptions::at(SCALE));
    let reference = run(&mut e, t).unwrap();
    redact(&mut e, &doc, 0, region, fill);
    let out = decode(&export(&mut e, &doc), label);
    let (w, h) = (
        reference.header.width as usize,
        reference.header.height as usize,
    );
    assert_eq!((out.width, out.height), (w, h), "{label}: pixel grid");
    let (x0, y0, x1, y1) = burned(&region, w, h);
    assert!(x1 > x0 && y1 > y0);
    let burn = fill.map_or([255u8; 3], |f| f.map(|c| (c * 255.0).round() as u8));
    let mut wrong = 0;
    for y in 0..h {
        for x in 0..w {
            let got = &out.rgb[(y * w + x) * 3..][..3];
            let inside = (x0..x1).contains(&x) && (y0..y1).contains(&y);
            let want = if inside {
                &burn[..]
            } else {
                &reference.data[(y * w + x) * 4..][..3]
            };
            wrong += usize::from(got != want);
        }
    }
    assert_eq!(wrong, 0, "{label}: pixels differ from burned reference");
    let red = out
        .rgb
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|p| p[0] > 200 && p[1] < 60 && p[2] < 60)
        .count();
    assert_eq!(red, 0, "{label}: red marker survived");
    let blue = out
        .rgb
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|p| p[2] > 200 && p[0] < 60 && p[1] < 60)
        .count();
    assert!(blue > 100, "{label}: public marker lost");
    out
}

#[test]
fn burned_pixels_follow_display_space_on_rotated_cropped_and_scaled_pages() {
    type Case = (&'static str, i32, Option<[f32; 4]>, Option<f32>);
    let cases: [Case; 6] = [
        ("plain", 0, None, None),
        ("r90", 90, None, None),
        ("r270", 270, None, None),
        ("r180-crop", 180, Some([20.0, 30.0, 280.0, 390.0]), None),
        ("r90-crop", 90, Some([20.0, 30.0, 280.0, 390.0]), None),
        ("unit2", 270, Some([20.0, 30.0, 280.0, 390.0]), Some(2.0)),
    ];
    for (name, rotate, crop, unit) in cases {
        let src = marker_pdf(rotate, crop, unit);
        // Where MuPDF puts the red square in display space, padded by one point.
        let m = red_box_pt(&src, &format!("{name}.src.pdf"));
        let region = Rect {
            x: m.x - 1.0,
            y: m.y - 1.0,
            width: m.width + 2.0,
            height: m.height + 2.0,
        };
        check_burn(&src, region, None, name);
        if name == "r90-crop" {
            // A colour fill burns that colour, not white.
            check_burn(&src, region, Some([0.0, 1.0, 0.0]), "r90-crop-green");
        }
    }
}

#[test]
fn a_region_on_a_pixel_edge_burns_exactly_the_squares_it_touches_with_area() {
    // x 10..20.25 pt, y 30.25..40 pt at scale 2 is columns 20..41 and rows 60..80: the region
    // grazes row 60 (30.25*2 = 60.5) and column 40 (20.25*2 = 40.5), each erased whole.
    let src = marker_pdf(0, None, None);
    let mut e = Engine::new(Limits::default());
    let (doc, _) = e.open(src, None, false).unwrap();
    let region = Rect {
        x: 10.0,
        y: 30.25,
        width: 10.25,
        height: 9.75,
    };
    redact(&mut e, &doc, 0, region, Some([0.0; 3]));
    let out = decode(&export(&mut e, &doc), "edge");
    let black = |x: usize, y: usize| out.rgb[(y * out.width + x) * 3..][..3] == [0, 0, 0];
    assert!(black(20, 60) && black(40, 79));
    assert!(!black(19, 60) && !black(41, 60) && !black(20, 59) && !black(20, 80));
}

// ---- edited text -------------------------------------------------------------------------------

/// Stands in for NEW-51: every edit list yields a working copy whose line reads "EDITED text".
struct Edited;

const EDITED: &str = "BT /F1 20 Tf 1 0 0 1 20 300 Tm (EDITED text) Tj ET";
const ORIGINAL: &str = "BT /F1 20 Tf 1 0 0 1 20 300 Tm (ORIGINAL xx) Tj ET";

impl EditApplier for Edited {
    fn apply(
        &self,
        _: &Document,
        _: &[TextEdit],
        _: &EditContext<'_>,
    ) -> Result<Applied, Vec<RejectedChange>> {
        Ok(Applied {
            bytes: pdf_bytes(&[EDITED], &Font::default(), 0),
            layouts: vec![],
        })
    }
}

#[test]
fn a_redaction_over_edited_text_burns_the_edited_pixels_not_the_original_ones() {
    let src = pdf_bytes(&[ORIGINAL], &Font::default(), 0);
    let mut e = Engine::new(Limits::default()).with_edit_applier(Box::new(Edited));
    let (doc, _) = e.open(src.clone(), None, false).unwrap();
    let t = e.apply_changes(
        &doc,
        ChangeSet {
            base_revision: 0,
            text_edits: Some(vec![TextEdit {
                id: "e".into(),
                page_index: 0,
                rects: vec![Rect {
                    x: 20.0,
                    y: 80.0,
                    width: 100.0,
                    height: 20.0,
                }],
                text: "EDITED".into(),
                font: None,
            }]),
            redactions: None,
        },
    );
    assert!(run(&mut e, t).unwrap().accepted);

    // Without a redaction the export shows the edit: it differs from the source's own render.
    let edited = decode(&export(&mut e, &doc), "edited-plain");
    let t = e.render(&doc, 0, RenderOptions::at(SCALE));
    let working = run(&mut e, t).unwrap();
    let source = foliark_engine::Document::open(src, None)
        .unwrap()
        .render_page(0, RenderOptions::at(SCALE))
        .unwrap();
    let rgb = |d: &[u8]| {
        d.as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| p[..3].to_vec())
            .collect::<Vec<u8>>()
    };
    assert_eq!(
        edited.rgb,
        rgb(&working.data),
        "export draws the working copy"
    );
    assert_ne!(
        edited.rgb,
        rgb(&source.data),
        "export is not the stale original"
    );

    // Redact the 4th and 5th glyph boxes of the edited line: those boxes are the (black) fill, whole.
    let t = e.extract_text(
        &doc,
        0,
        ExtractOptions {
            chars: true,
            ..Default::default()
        },
    );
    let page = run(&mut e, t).unwrap();
    let glyphs: Vec<_> = page.clusters.into_iter().flatten().collect();
    let (a, b) = (glyphs[3].quad.bounds(), glyphs[4].quad.bounds());
    // The region reaches only into the middle of glyph 3, yet the whole glyph 3 box burns.
    redact(
        &mut e,
        &doc,
        1,
        Rect {
            x: a.x + a.width / 2.0,
            y: a.y,
            width: a.width / 2.0 + b.width,
            height: a.height,
        },
        Some([0.0; 3]),
    );
    let out = decode(&export(&mut e, &doc), "edited-redacted");
    let black_in = |r: &Rect| {
        let (x0, y0, x1, y1) = burned(r, out.width, out.height);
        (y0..y1).all(|y| (x0..x1).all(|x| out.rgb[(y * out.width + x) * 3..][..3] == [0, 0, 0]))
    };
    assert!(black_in(&a), "the grazed glyph's whole box is burned");
    assert!(black_in(&b));
    // The first glyph, far from the region, is untouched ink.
    let first = glyphs[0].quad.bounds();
    let (x0, y0, x1, y1) = burned(&first, out.width, out.height);
    let ink = (y0..y1)
        .any(|y| (x0..x1).any(|x| out.rgb[(y * out.width + x) * 3..][..3] != [255, 255, 255]));
    assert!(ink, "outside the region the edited text is still drawn");
}

// ---- cancel -------------------------------------------------------------------------------------

#[test]
fn a_cancel_between_pages_leaves_no_plan_and_the_engine_usable() {
    use foliark_engine::Step;
    let page = "BT /F1 12 Tf 20 300 Td (page) Tj ET";
    let (mut e, doc) = {
        let mut e = Engine::new(Limits::default());
        let (doc, _) = e
            .open(
                pdf_bytes(&[page, page, page], &Font::default(), 0),
                None,
                false,
            )
            .unwrap();
        (e, doc)
    };
    // An older plan is freed when the new export starts, and a task dropped after its first
    // page (the worker's cancel) holds nothing.
    let old = e.prepare_export(&doc, ExportMode::Raster, consenting());
    run(&mut e, old).unwrap();
    assert!(e.stats().plan_bytes > 0);
    let mut task = e.prepare_export(&doc, ExportMode::Raster, consenting());
    let Step::Pending(p) = task.step(&mut e, 1).unwrap() else {
        panic!("one page of three is not done");
    };
    assert_eq!((p.done, p.total), (1, 3));
    drop(task);
    assert_eq!(e.stats().plan_bytes, 0);

    // The cooperative flag stops the next page's draw: typed error, still no plan.
    let mut task = e.prepare_export(&doc, ExportMode::Raster, consenting());
    assert!(matches!(task.step(&mut e, 1).unwrap(), Step::Pending(_)));
    e.cancel_handle()
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let err = task.step(&mut e, 1).err().expect("cancelled");
    assert_eq!(err.code, foliark_model::ErrorCode::Cancelled);
    assert_eq!(e.stats().plan_bytes, 0);
    e.cancel_handle()
        .store(false, std::sync::atomic::Ordering::Relaxed);
    assert!(export(&mut e, &doc).starts_with(b"%PDF-"));
}
