//! Raster eligibility (spec §7; NEW-54): independent of the native pipeline, consent- and
//! limit-gated, refusing incomplete renders by page, bound to its revision like a native plan.
mod common;
use common::{Font, fixture, pdf_bytes, run};
use foliark_engine::{Engine, Step};
use foliark_model::{
    CapabilityStatus, ChangeSet, ErrorCode, ExportMode, ExportPlan, ExportPolicy, Limits, PdfError,
    Rect, Redaction, SignedInputPolicy,
};
use pdf_writer::{Finish, Name, Pdf, Rect as PdfRect, Ref};

fn engine(bytes: Vec<u8>, limits: Limits) -> (Engine, String) {
    let mut e = Engine::new(limits);
    let (doc, _) = e.open(bytes, None, false).unwrap();
    (e, doc)
}

fn prepare(
    e: &mut Engine,
    doc: &str,
    mode: ExportMode,
    policy: ExportPolicy,
) -> Result<ExportPlan, PdfError> {
    let t = e.prepare_export(doc, mode, policy);
    run(e, t)
}

fn consenting() -> ExportPolicy {
    let mut p = ExportPolicy::default();
    p.raster.acknowledge_losses = true;
    p
}

fn redact_all(e: &mut Engine, doc: &str, base: u32, page: u32) {
    let task = e.apply_changes(
        doc,
        ChangeSet {
            base_revision: base,
            text_edits: None,
            redactions: Some(vec![Redaction::Region {
                id: format!("r{base}"),
                page_index: page,
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 600.0,
                    height: 800.0,
                },
                fill: None,
            }]),
        },
    );
    assert!(run(e, task).unwrap().accepted);
}

/// One page per entry: `true` shows text in an unembedded font that is not one of the 14
/// standard faces (the renderer draws a stand-in and warns), `false` in Helvetica.
fn pages_with_fonts(stand_in: &[bool]) -> Vec<u8> {
    let mut pdf = Pdf::new();
    pdf.catalog(Ref::new(1)).pages(Ref::new(2));
    let kids: Vec<Ref> = (0..stand_in.len() as i32)
        .map(|i| Ref::new(10 + 2 * i))
        .collect();
    pdf.pages(Ref::new(2))
        .kids(kids.iter().copied())
        .count(kids.len() as i32);
    for (i, odd) in stand_in.iter().enumerate() {
        let mut p = pdf.page(kids[i]);
        p.parent(Ref::new(2))
            .media_box(PdfRect::new(0.0, 0.0, 200.0, 200.0))
            .contents(Ref::new(11 + 2 * i as i32));
        p.resources().fonts().pair(Name(b"F1"), Ref::new(3));
        p.resources().fonts().pair(Name(b"F2"), Ref::new(4));
        p.finish();
        let font = if *odd { "F2" } else { "F1" };
        pdf.stream(
            Ref::new(11 + 2 * i as i32),
            format!("BT /{font} 12 Tf 20 100 Td (page {i}) Tj ET").as_bytes(),
        );
    }
    for (id, name) in [(3, "Helvetica"), (4, "ZzQuuxSerifBook")] {
        pdf.type1_font(Ref::new(id))
            .base_font(Name(name.as_bytes()));
    }
    pdf.finish()
}

#[test]
fn a_document_native_export_refuses_is_still_raster_eligible_and_never_falls_back() {
    // A layer the native stage cannot decide (same-length patch: the page draws a layer name its
    // /Properties no longer defines): native refuses by code. Raster draws what is visible.
    let mut undecidable = fixture("generated/ocg-hidden-text.pdf");
    let at = undecidable
        .windows(10)
        .position(|w| w == b"/oc2 7 0 R")
        .unwrap();
    undecidable[at + 3] = b'X';
    let (mut e, doc) = engine(undecidable, Limits::default());
    let err = prepare(&mut e, &doc, ExportMode::Native, consenting()).unwrap_err();
    assert_eq!(err.code, ErrorCode::OptionalContentUnsupported);
    // The refusal held nothing and asked nobody to rasterize: no plan, raster still unproven.
    assert_eq!(e.stats().plan_bytes, 0);
    let t = e.capabilities(&doc, false);
    let caps = run(&mut e, t).unwrap();
    assert_eq!(caps.export_native.status, CapabilityStatus::Unsupported);
    assert_ne!(caps.export_raster.status, CapabilityStatus::Supported);
    let p = prepare(&mut e, &doc, ExportMode::Raster, consenting()).unwrap();
    assert_eq!(p.mode, ExportMode::Raster);
    assert!(p.report.flattened.ocg_layers > 0);
    let t = e.capabilities(&doc, false);
    let caps = run(&mut e, t).unwrap();
    assert_eq!(caps.export_raster.status, CapabilityStatus::Supported);

    // A redaction over an image: native erases the pixels, raster burns the region; both apply it.
    let (mut e, doc) = engine(fixture("hddn/shared-image.pdf"), Limits::default());
    redact_all(&mut e, &doc, 0, 0);
    let p = prepare(&mut e, &doc, ExportMode::Native, ExportPolicy::default()).unwrap();
    assert_eq!(p.report.redaction.images_erased, 1);
    let p = prepare(&mut e, &doc, ExportMode::Raster, consenting()).unwrap();
    assert_eq!(p.report.redaction.regions, 1);
}

#[test]
fn raster_needs_consent_a_sane_scale_and_a_page_that_fits_the_limits() {
    let bytes = pdf_bytes(
        &["BT /F1 10 Tf 20 380 Td (hello) Tj ET"],
        &Font::default(),
        0,
    );
    let (mut e, doc) = engine(bytes.clone(), Limits::default());
    let refuse = |e: &mut Engine, p: ExportPolicy| {
        let err = prepare(e, &doc, ExportMode::Raster, p).unwrap_err();
        assert_eq!(e.stats().plan_bytes, 0);
        err
    };
    assert_eq!(
        refuse(&mut e, ExportPolicy::default()).code,
        ErrorCode::ConsentRequired
    );
    for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let mut p = consenting();
        p.raster.scale = scale;
        assert_eq!(
            refuse(&mut e, p).code,
            ErrorCode::InvalidArgument,
            "{scale}"
        );
    }
    // A 300 x 400 pt page at scale 40 is 12000 x 16000 px: over maxRenderPixels (50 MP default).
    let mut huge = consenting();
    huge.raster.scale = 40.0;
    let err = refuse(&mut e, huge);
    assert_eq!(err.code, ErrorCode::LimitExceeded);
    assert_eq!(
        err.details.unwrap().limit.as_deref(),
        Some("maxRenderPixels")
    );
    let mut wide = consenting();
    wide.raster.scale = 100.0;
    let err = refuse(&mut e, wide);
    assert_eq!(err.details.unwrap().limit.as_deref(), Some("maxRenderSide"));
    // The same page passes at scale 2 and the policy limit is the document's own limit.
    assert!(prepare(&mut e, &doc, ExportMode::Raster, consenting()).is_ok());
    let tight = Limits {
        max_render_pixels: 100_000,
        ..Limits::default()
    };
    let (mut e, doc) = engine(bytes, tight);
    let err = prepare(&mut e, &doc, ExportMode::Raster, consenting()).unwrap_err();
    assert_eq!(
        err.details.unwrap().limit.as_deref(),
        Some("maxRenderPixels")
    );
}

#[test]
fn signed_input_policy_applies_to_raster_too() {
    // A catalog /Perms (a certification signature) marks the input as signed.
    let mut pdf = Pdf::new();
    pdf.catalog(Ref::new(1))
        .pages(Ref::new(2))
        .insert(Name(b"Perms"))
        .dict();
    pdf.pages(Ref::new(2)).kids([Ref::new(3)]).count(1);
    pdf.page(Ref::new(3))
        .parent(Ref::new(2))
        .media_box(PdfRect::new(0.0, 0.0, 100.0, 100.0));
    let (mut e, doc) = engine(pdf.finish(), Limits::default());
    let refuse = ExportPolicy {
        signed_input: SignedInputPolicy::Refuse,
        ..consenting()
    };
    let err = prepare(&mut e, &doc, ExportMode::Raster, refuse).unwrap_err();
    assert_eq!(err.code, ErrorCode::SignedInput);
    let p = prepare(&mut e, &doc, ExportMode::Raster, consenting()).unwrap();
    assert!(
        p.report
            .losses
            .iter()
            .any(|l| l.code == foliark_model::LossCode::SignaturesInvalidated)
    );
}

#[test]
fn pages_that_draw_with_a_warning_refuse_the_export_and_name_every_one() {
    let (mut e, doc) = engine(
        pages_with_fonts(&[false, true, false, true]),
        Limits::default(),
    );
    let err = prepare(&mut e, &doc, ExportMode::Raster, consenting()).unwrap_err();
    assert_eq!(err.code, ErrorCode::RenderIncomplete);
    assert_eq!(err.details.unwrap().pages, Some(vec![1, 3]));
    assert_eq!(e.stats().plan_bytes, 0);
    // Preview still draws it (complete: false), so the refusal is the export's alone.
    let t = e.render(&doc, 1, Default::default());
    let r = run(&mut e, t).unwrap();
    assert!(!r.header.complete);
    let ok = engine(pages_with_fonts(&[false, false]), Limits::default());
    let (mut e, doc) = ok;
    assert!(prepare(&mut e, &doc, ExportMode::Raster, consenting()).is_ok());
}

#[test]
fn a_raster_plan_is_bound_to_its_revision_mode_and_settings() {
    let (mut e, doc) = engine(
        pdf_bytes(
            &["BT /F1 10 Tf 20 380 Td (hello) Tj ET"],
            &Font::default(),
            0,
        ),
        Limits::default(),
    );
    let native = prepare(&mut e, &doc, ExportMode::Native, consenting()).unwrap();
    let raster = prepare(&mut e, &doc, ExportMode::Raster, consenting()).unwrap();
    // Same policy, different mode: different hash, and the held plan is the raster one, so the
    // native id cannot be exchanged for raster bytes (or the reverse).
    assert_ne!(native.policy_hash, raster.policy_hash);
    assert_eq!(
        e.take_export(&doc, &native.plan_id).unwrap_err().code,
        ErrorCode::StalePlan
    );
    let mut four = consenting();
    four.raster.scale = 4.0;
    let other = prepare(&mut e, &doc, ExportMode::Raster, four).unwrap();
    assert_ne!(other.policy_hash, raster.policy_hash);
    assert_eq!(
        e.take_export(&doc, &raster.plan_id).unwrap_err().code,
        ErrorCode::StalePlan
    );
    // A committed change kills the held raster plan; so does a font registration.
    redact_all(&mut e, &doc, 0, 0);
    assert_eq!(
        e.take_export(&doc, &other.plan_id).unwrap_err().code,
        ErrorCode::StalePlan
    );
    let fresh = prepare(&mut e, &doc, ExportMode::Raster, consenting()).unwrap();
    assert_eq!(fresh.revision, 1);
    assert!(e.take_export(&doc, &fresh.plan_id).is_ok());
}

#[test]
fn a_task_that_moved_revision_between_steps_fails_and_leaves_no_plan() {
    let (mut e, doc) = engine(pages_with_fonts(&[false, false, false]), Limits::default());
    let mut task = e.prepare_export(&doc, ExportMode::Raster, consenting());
    assert!(matches!(task.step(&mut e, 1).unwrap(), Step::Pending(_)));
    redact_all(&mut e, &doc, 0, 0);
    assert!(task.step(&mut e, 1).is_err());
    assert_eq!(e.stats().plan_bytes, 0);
}
