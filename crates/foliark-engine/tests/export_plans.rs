//! Revision-bound export plans and the native pipeline (spec §4, §5, §11; NEW-40). Plans are
//! judged through the public engine ops; output bytes by the independent oracles.
mod common;
use common::{Font, fidelity_oracle, fixture, objects_oracle, pdf_bytes, run};
use foliark_engine::{Document, Engine};
use foliark_model::{
    AnnotationPolicy, CapabilityStatus, ChangeSet, ErrorCode, ExportMode, ExportPlan, ExportPolicy,
    ExtractOptions, FontSource, FontStyle, Limits, LossCode, PdfError, Rect, Redaction,
    SanitizedItem,
};

const PAGE: &str = "BT /F1 10 Tf 1 0 0 1 20 380 Tm (alpha SECRETONE omega) Tj ET\n\
BT /F1 10 Tf 1 0 0 1 20 360 Tm (visible line) Tj ET";

fn engine(bytes: Vec<u8>) -> (Engine, String) {
    let mut e = Engine::new(Limits::default());
    let (doc, _) = e.open(bytes, None, false).unwrap();
    (e, doc)
}

fn redact_word(e: &mut Engine, doc: &str, base: u32, word: &str) {
    let text = e.extract_text(doc, 0, ExtractOptions::default());
    let text = run(e, text).unwrap().text;
    let start = text.find(word).unwrap() as u32;
    let task = e.apply_changes(
        doc,
        ChangeSet {
            base_revision: base,
            text_edits: None,
            redactions: Some(vec![Redaction::Text {
                id: format!("r{base}"),
                page_index: 0,
                start,
                end: start + word.len() as u32,
                fill: None,
            }]),
        },
    );
    let r = run(e, task).unwrap();
    assert!(r.accepted, "{:?}", r.rejected);
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

fn native(e: &mut Engine, doc: &str) -> ExportPlan {
    prepare(e, doc, ExportMode::Native, ExportPolicy::default()).unwrap()
}

fn sans() -> FontSource {
    FontSource {
        bytes: std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../foliark-text/tests/data/LiberationSans-Regular.ttf"
        ))
        .unwrap(),
        family: "Liberation Sans".into(),
        weight: 400,
        style: FontStyle::Normal,
    }
}

fn feature(e: &PdfError) -> Option<&str> {
    e.details.as_ref().and_then(|d| d.feature.as_deref())
}

#[test]
fn a_plan_dies_with_its_revision_fonts_or_document_and_exports_once() {
    let bytes = pdf_bytes(&[PAGE], &Font::default(), 0);
    let (mut e, doc) = engine(bytes);
    redact_word(&mut e, &doc, 0, "SECRETONE");

    // Single use: the first export hands over the bytes and frees the memory, the second is
    // PLAN_CONSUMED; the plan names the binding it was made for.
    let p = native(&mut e, &doc);
    assert_eq!(
        (p.revision, p.generation, p.mode),
        (1, 1, ExportMode::Native)
    );
    assert_eq!(p.doc_id, doc);
    assert_eq!(e.stats().plan_bytes, p.byte_length as usize);
    let out = e.take_export(&doc, &p.plan_id).unwrap();
    assert_eq!(out.len(), p.byte_length as usize);
    assert_eq!(e.stats().plan_bytes, 0);
    assert_eq!(
        e.take_export(&doc, &p.plan_id).unwrap_err().code,
        ErrorCode::PlanConsumed
    );

    // A committed change frees the plan: STALE_PLAN and no held bytes.
    let p = native(&mut e, &doc);
    redact_word(&mut e, &doc, 1, "visible");
    assert_eq!(e.stats().plan_bytes, 0);
    assert_eq!(
        e.take_export(&doc, &p.plan_id).unwrap_err().code,
        ErrorCode::StalePlan
    );

    // Registering a font moves the registry version the plan is bound to.
    let p = native(&mut e, &doc);
    assert_eq!(p.font_registry_version, e.font_registry_version());
    e.register_fonts(vec![sans()]).unwrap();
    assert_eq!(e.stats().plan_bytes, 0);
    assert_eq!(
        e.take_export(&doc, &p.plan_id).unwrap_err().code,
        ErrorCode::StalePlan
    );

    // A newer plan frees the older one, and a plan only exports for its own document.
    let (old, new) = (native(&mut e, &doc), native(&mut e, &doc));
    assert_eq!(
        e.take_export(&doc, &old.plan_id).unwrap_err().code,
        ErrorCode::StalePlan
    );
    assert_eq!(
        e.take_export("1-99", &new.plan_id).unwrap_err().code,
        ErrorCode::DocumentGone
    );
    e.close(&doc);
    assert_eq!(e.stats().plan_bytes, 0);
}

#[test]
fn the_policy_hash_follows_the_effective_policy_and_mode() {
    let (mut e, doc) = engine(pdf_bytes(&[PAGE], &Font::default(), 0));
    let default = native(&mut e, &doc);
    let mut spelled = ExportPolicy::default();
    spelled.raster.scale = 4.0; // not part of a native export
    assert_eq!(
        prepare(&mut e, &doc, ExportMode::Native, spelled)
            .unwrap()
            .policy_hash,
        default.policy_hash
    );
    let remove = ExportPolicy {
        annotations: AnnotationPolicy::Remove,
        ..ExportPolicy::default()
    };
    let other = prepare(&mut e, &doc, ExportMode::Native, remove).unwrap();
    assert_ne!(other.policy_hash, default.policy_hash);
    assert_eq!(default.policy_hash.len(), 16);
}

#[test]
fn a_text_redaction_export_passes_the_oracles_and_reports_what_it_did() {
    let input = pdf_bytes(&[PAGE], &Font::default(), 0);
    let (mut e, doc) = engine(input.clone());
    redact_word(&mut e, &doc, 0, "SECRETONE");
    let p = native(&mut e, &doc);
    let out = e.take_export(&doc, &p.plan_id).unwrap();

    let r = &p.report;
    assert_eq!((r.pages, r.output_bytes as usize), (1, out.len()));
    assert_eq!((r.redaction.regions, r.redaction.clusters_removed), (1, 9));
    assert_eq!(r.edits.applied, 0);
    assert!(r.removed.is_empty() && r.kept.invisible_text_clusters == 0);

    let (ok, log) = objects_oracle("export-plans", "text.pdf", &out, &["SECRETONE"]);
    assert!(ok, "objects.py: {log}");
    let (ok, log) = objects_oracle("export-plans", "text-in.pdf", &input, &["SECRETONE"]);
    assert!(!ok, "the oracle found nothing in the INPUT: {log}");
    // The word's line stays within 0.5 pt of the region; everything else must not move.
    let words = Document::open(input.clone(), None).unwrap();
    let w = words
        .extract_text(0, &ExtractOptions::default())
        .unwrap()
        .words
        .into_iter()
        .find(|w| w.text == "SECRETONE")
        .unwrap()
        .bounds;
    let (ok, log) = fidelity_oracle(
        "export-plans",
        "text",
        &input,
        &out,
        &[(
            0,
            w.x - 0.5,
            w.y - 0.5,
            w.x + w.width + 0.5,
            w.y + w.height + 0.5,
        )],
        &["alpha", "omega", "visible"],
    );
    assert!(ok, "fidelity.py: {log}");
    // The black box is painted: the exported page is dark at the word's centre and white
    // (background) at the surviving words.
    let reopened = Document::open(out, None).unwrap();
    let text = reopened
        .extract_text(0, &ExtractOptions::default())
        .unwrap()
        .text;
    assert!(!text.contains("SECRET") && text.contains("alpha") && text.contains("omega"));
    let img = reopened
        .render_page(0, foliark_engine::RenderOptions::default())
        .unwrap();
    let px = |x: f64, y: f64| img.data[((y as usize) * img.header.width as usize + x as usize) * 4];
    assert!(px(w.x + w.width / 2.0, w.y + w.height / 2.0) < 40);
    assert!(px(w.x + w.width / 2.0, w.y - 8.0) > 200);
}

#[test]
fn sanitize_only_export_without_redactions_is_valid_and_reports_removals() {
    let input = fixture("generated/leaks-metadata-attachment-js.pdf");
    let (mut e, doc) = engine(input);
    let p = native(&mut e, &doc);
    let out = e.take_export(&doc, &p.plan_id).unwrap();
    let reopened = Document::open(out.clone(), None).unwrap();
    assert_eq!(reopened.page_count(), 1);
    assert!(
        reopened
            .extract_text(0, &ExtractOptions::default())
            .unwrap()
            .text
            .contains("KEEP_PUBLIC_KITCHEN")
    );
    assert!(p.report.removed.contains_key(&SanitizedItem::Metadata));
    assert_eq!(p.report.redaction.regions, 0);
    assert!(
        p.report
            .losses
            .iter()
            .any(|l| l.code == LossCode::ScriptsRemoved)
    );
}

#[test]
fn stages_this_build_cannot_run_refuse_by_name_with_no_bytes() {
    let refuse = |name: &str, policy: ExportPolicy| {
        let (mut e, doc) = engine(fixture(name));
        let err = prepare(&mut e, &doc, ExportMode::Native, policy).unwrap_err();
        assert_eq!(e.stats().plan_bytes, 0, "{name} held bytes after a refusal");
        let caps = e.capabilities(&doc, false);
        (err, run(&mut e, caps).unwrap())
    };
    let (err, caps) = refuse("hddn/acroform.pdf", ExportPolicy::default());
    assert_eq!(
        (err.code, feature(&err)),
        (ErrorCode::InvalidArgument, Some("flattenWidgets"))
    );
    // Capabilities tell native-refused from raster-eligible: the native refusal is known now
    // (with its code), raster is independent and still to be established.
    assert_eq!(caps.export_native.status, CapabilityStatus::Unsupported);
    assert_eq!(
        caps.export_native.reasons[0].code,
        ErrorCode::InvalidArgument
    );
    assert_eq!(caps.export_raster.status, CapabilityStatus::Unknown);
    assert!(caps.export_raster.reasons.is_empty());

    let (err, _) = refuse("generated/annot-appearance.pdf", ExportPolicy::default());
    assert_eq!(feature(&err), Some("flattenAnnotations"));
    let (err, _) = refuse("generated/ocg-hidden-text.pdf", ExportPolicy::default());
    assert_eq!(err.code, ErrorCode::OptionalContentUnsupported);
    let refuse_signed = ExportPolicy {
        signed_input: foliark_model::SignedInputPolicy::Refuse,
        invisible_text: foliark_model::InvisibleTextPolicy::Remove,
        ..ExportPolicy::default()
    };
    let (err, _) = refuse("generated/ocr-over-image.pdf", refuse_signed);
    assert_eq!(feature(&err), Some("invisibleTextRemove"));
}

#[test]
fn annotations_remove_policy_drops_them_and_says_so() {
    let (mut e, doc) = engine(fixture("generated/annot-appearance.pdf"));
    let policy = ExportPolicy {
        annotations: AnnotationPolicy::Remove,
        ..ExportPolicy::default()
    };
    let p = prepare(&mut e, &doc, ExportMode::Native, policy).unwrap();
    assert!(p.report.removed[&SanitizedItem::Annotations] >= 1);
    assert!(
        p.report
            .losses
            .iter()
            .any(|l| l.code == LossCode::AnnotationsRemoved)
    );
    let out = e.take_export(&doc, &p.plan_id).unwrap();
    let (ok, log) = objects_oracle(
        "export-plans",
        "annot.pdf",
        &out,
        &["SECRET_ANNOT_CONTENTS", "SECRET_LINK_URI"],
    );
    assert!(ok, "{log}");
}

#[test]
fn redaction_over_an_image_refuses_until_the_image_executor_exists() {
    let (mut e, doc) = engine(fixture("hddn/shared-image.pdf"));
    let task = e.apply_changes(
        &doc,
        ChangeSet {
            base_revision: 0,
            text_edits: None,
            redactions: Some(vec![Redaction::Region {
                id: "all".into(),
                page_index: 0,
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
    assert!(run(&mut e, task).unwrap().accepted);
    let err = prepare(&mut e, &doc, ExportMode::Native, ExportPolicy::default()).unwrap_err();
    assert_eq!(feature(&err), Some("redactionImages"));
    let caps = e.capabilities(&doc, false);
    let caps = run(&mut e, caps).unwrap();
    assert_eq!(caps.redact.status, CapabilityStatus::Unsupported);
}

#[test]
fn raster_needs_consent_then_is_refused_until_it_is_built() {
    let (mut e, doc) = engine(pdf_bytes(&[PAGE], &Font::default(), 0));
    let err = prepare(&mut e, &doc, ExportMode::Raster, ExportPolicy::default()).unwrap_err();
    assert_eq!(err.code, ErrorCode::ConsentRequired);
    let mut ack = ExportPolicy::default();
    ack.raster.acknowledge_losses = true;
    let err = prepare(&mut e, &doc, ExportMode::Raster, ack).unwrap_err();
    assert_eq!(feature(&err), Some("exportRaster"));
    assert_eq!(e.stats().plan_bytes, 0);
}
