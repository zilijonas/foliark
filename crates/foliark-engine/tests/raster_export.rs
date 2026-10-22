//! Raster export files (spec §7; NEW-56): a fresh image-only PDF with the source's display
//! geometry, judged by the independent oracles (objects.py, fidelity.py --raster, raster.py) and
//! by an accurate loss report. Native refusals do not stop a supported raster export.
mod common;
use common::{Font, fixture, objects_oracle, pdf_bytes, python, repo, run, tmp};
use foliark_engine::Engine;
use foliark_model::{
    AnnotationPolicy, ChangeSet, ErrorCode, ExportMode, ExportPlan, ExportPolicy, Limits, LossCode,
    PdfError, Redaction,
};
use pdf_writer::Finish;

fn consenting() -> ExportPolicy {
    let mut p = ExportPolicy::default();
    p.raster.acknowledge_losses = true;
    p
}

fn raster(
    bytes: &[u8],
    password: Option<&str>,
    policy: ExportPolicy,
    limits: Limits,
) -> Result<(Vec<u8>, ExportPlan), PdfError> {
    let mut e = Engine::new(limits);
    let (doc, _) = e.open(bytes.to_vec(), password, false)?;
    let t = e.prepare_export(&doc, ExportMode::Raster, policy);
    let plan = run(&mut e, t)?;
    Ok((e.take_export(&doc, &plan.plan_id)?, plan))
}

/// `(path, password, mustRemove)` of every corpus fixture that is not the 1001-page stress file.
fn corpus() -> Vec<(String, Option<String>, Vec<String>)> {
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(repo().join("tests/fixtures/manifest.json")).unwrap(),
    )
    .unwrap();
    let strings = |v: &serde_json::Value| -> Vec<String> {
        v.as_array()
            .map(|a| a.iter().map(|s| s.as_str().unwrap().to_owned()).collect())
            .unwrap_or_default()
    };
    manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| !f["path"].as_str().unwrap().contains("many-pages"))
        .map(|f| {
            (
                f["path"].as_str().unwrap().to_owned(),
                f["password"].as_str().map(str::to_owned),
                strings(&f["expected"]["mustRemove"]),
            )
        })
        .collect()
}

/// What the engine refuses on each corpus fixture (typed, no output); every other one exports.
const REFUSED: &[(&str, ErrorCode)] = &[
    ("hddn/annotation-comment.pdf", ErrorCode::RenderIncomplete), // annotation without appearance
    ("generated/forms-cyclic.pdf", ErrorCode::RenderIncomplete),
    (
        "generated/cjk-predefined-cmaps.pdf",
        ErrorCode::RenderIncomplete,
    ), // non-embedded CJK font drawn with a stand-in
    ("hddn/malformed-truncated.pdf", ErrorCode::RepairedPdf),
    ("hddn/malformed-xref.pdf", ErrorCode::RepairedPdf),
    ("hddn/oversized-image.pdf", ErrorCode::LimitExceeded),
    ("hddn/compressed-stream.pdf", ErrorCode::LimitExceeded),
    ("generated/corrupt-flate.pdf", ErrorCode::DecodeFailed),
];

fn exports(path: &str, pw: Option<&str>) -> Option<Vec<u8>> {
    let refused = REFUSED.iter().find(|r| r.0 == path);
    match (
        raster(&fixture(path), pw, consenting(), Limits::default()),
        refused,
    ) {
        (Ok((out, _)), None) => Some(out),
        (Err(e), Some(r)) => {
            assert_eq!(e.code, r.1, "{path}");
            None
        }
        (Ok(_), Some(_)) => panic!("{path} exported but is pinned as refused"),
        (Err(e), None) => panic!("{path} refused: {:?}", e.code),
    }
}

#[test]
fn every_corpus_secret_is_absent_from_the_raster_bytes_objects_and_text() {
    let mut exported = 0;
    for (path, pw, secrets) in corpus() {
        let Some(out) = exports(&path, pw.as_deref()) else {
            continue;
        };
        exported += 1;
        let forbid: Vec<&str> = secrets.iter().map(String::as_str).collect();
        let (ok, log) = objects_oracle("raster-export", "corpus.pdf", &out, &forbid);
        assert!(ok, "{path}: objects.py: {log}");
        // Negative control: the oracle does see the secrets in a source it can open.
        if pw.is_none() && !forbid.is_empty() {
            let (ok, _) =
                objects_oracle("raster-export", "corpus-in.pdf", &fixture(&path), &forbid);
            assert!(!ok, "{path}: the oracle found nothing in the INPUT");
        }
    }
    assert_eq!(exported, corpus().len() - REFUSED.len());
}

#[test]
fn every_exported_corpus_page_still_looks_like_its_source() {
    for (path, pw, _) in corpus() {
        let Some(out) = exports(&path, pw.as_deref()) else {
            continue;
        };
        // fidelity.py applies one password to both files and the output is not encrypted.
        if pw.is_some() {
            continue;
        }
        let (i, o, spec) = (
            tmp("raster-export", "look.in.pdf"),
            tmp("raster-export", "look.out.pdf"),
            tmp("raster-export", "look.json"),
        );
        std::fs::write(&i, fixture(&path)).unwrap();
        std::fs::write(&o, &out).unwrap();
        // Raster outputs re-sample every page: declared raster, looser pixel limits (oracle README).
        std::fs::write(
            &spec,
            serde_json::json!({"raster": true, "threshold": 64, "max_frac": 0.01}).to_string(),
        )
        .unwrap();
        let (ok, log) = python(&[
            "tests/oracles/fidelity.py",
            i.to_str().unwrap(),
            o.to_str().unwrap(),
            "--spec",
            spec.to_str().unwrap(),
        ]);
        assert!(ok, "{path}: fidelity.py --raster: {log}");
    }
}

/// `(width, height, rotation, images, fonts)` per page, as MuPDF sees the file.
fn geometry(bytes: &[u8], name: &str, password: &str) -> Vec<(f64, f64, u32, usize, usize)> {
    let p = tmp("raster-export", name);
    std::fs::write(&p, bytes).unwrap();
    let (ok, log) = python(&[
        "tests/oracles/raster.py",
        "geometry",
        p.to_str().unwrap(),
        password,
    ]);
    assert!(ok, "{log}");
    log.lines()
        .map(|l| {
            let v: Vec<&str> = l.split_whitespace().collect();
            (
                v[0].parse().unwrap(),
                v[1].parse().unwrap(),
                v[2].parse().unwrap(),
                v[3].parse().unwrap(),
                v[4].parse().unwrap(),
            )
        })
        .collect()
}

#[test]
fn every_page_keeps_the_source_display_size_with_one_image_and_no_rotation_or_fonts() {
    // rotation-crop.pdf: four pages mixing /Rotate and CropBox. MuPDF's display rect of each
    // source page equals the output page's rect, which carries no /Rotate.
    let src = fixture("hddn/rotation-crop.pdf");
    let (out, _) = raster(&src, None, consenting(), Limits::default()).unwrap();
    let (a, b) = (
        geometry(&src, "geo.in.pdf", ""),
        geometry(&out, "geo.out.pdf", ""),
    );
    assert_eq!(a.len(), 4);
    assert!(a.iter().any(|g| g.2 != 0), "the fixture rotates a page");
    for (s, o) in a.iter().zip(&b) {
        assert!(
            (s.0 - o.0).abs() < 0.01 && (s.1 - o.1).abs() < 0.01,
            "{s:?} vs {o:?}"
        );
        assert_eq!(
            (o.2, o.3, o.4),
            (0, 1, 0),
            "no /Rotate, one image, no fonts"
        );
    }
    // A UserUnit scales the display size; the output page is that many points, no UserUnit.
    let unit = {
        let mut pdf = pdf_writer::Pdf::new();
        pdf.catalog(pdf_writer::Ref::new(1))
            .pages(pdf_writer::Ref::new(2));
        pdf.pages(pdf_writer::Ref::new(2))
            .kids([pdf_writer::Ref::new(3)])
            .count(1);
        let mut p = pdf.page(pdf_writer::Ref::new(3));
        p.parent(pdf_writer::Ref::new(2))
            .media_box(pdf_writer::Rect::new(0.0, 0.0, 100.0, 50.0));
        p.pair(pdf_writer::Name(b"UserUnit"), 3.0f32);
        p.finish();
        pdf.finish()
    };
    let (out, _) = raster(&unit, None, consenting(), Limits::default()).unwrap();
    let g = &geometry(&out, "geo.unit.pdf", "")[0];
    assert_eq!((g.0.round(), g.1.round()), (300.0, 150.0));
}

fn count(report: &ExportPlan, code: LossCode) -> u32 {
    report
        .report
        .losses
        .iter()
        .filter(|l| l.code == code)
        .map(|l| l.count)
        .sum()
}

fn oracle_annots(bytes: &[u8], password: &str) -> (u32, u32, u32) {
    let p = tmp("raster-export", "annots.pdf");
    std::fs::write(&p, bytes).unwrap();
    let (ok, log) = python(&[
        "tests/oracles/raster.py",
        "annots",
        p.to_str().unwrap(),
        password,
    ]);
    assert!(ok, "{log}");
    let v: Vec<u32> = log.split_whitespace().map(|v| v.parse().unwrap()).collect();
    (v[0], v[1], v[2])
}

#[test]
fn the_loss_report_matches_what_the_source_held_and_the_policy_did() {
    // MuPDF counts the source's annotations, links and widgets; the report must agree.
    for name in [
        "generated/annot-appearance.pdf",
        "hddn/acroform.pdf",
        "hddn/actions-js.pdf",
    ] {
        let src = fixture(name);
        let (annots, links, widgets) = oracle_annots(&src, "");
        let (out, plan) = raster(&src, None, consenting(), Limits::default()).unwrap();
        let r = &plan.report;
        assert_eq!(r.mode, ExportMode::Raster);
        assert_eq!(r.output_bytes as usize, out.len(), "{name}");
        assert_eq!(count(&plan, LossCode::LinksRemoved), links, "{name} links");
        assert_eq!(
            count(&plan, LossCode::AnnotationsFlattened),
            annots,
            "{name} annots"
        );
        assert_eq!(
            count(&plan, LossCode::FormsFlattened),
            widgets,
            "{name} widgets"
        );
        assert_eq!(
            (r.flattened.annotations, r.flattened.widgets),
            (annots, widgets)
        );
        // Every raster export loses search and live vectors, on every page.
        assert_eq!(count(&plan, LossCode::TextSearchLost), r.pages);
        assert_eq!(count(&plan, LossCode::VectorRasterized), r.pages);
        assert_eq!((r.redaction.clusters_removed, r.edits.approximate), (0, 0));
    }
    // `remove` does not draw annotations: the pixels differ from `flatten`, and the loss says so.
    let src = fixture("generated/annot-appearance.pdf");
    let remove = ExportPolicy {
        annotations: AnnotationPolicy::Remove,
        ..consenting()
    };
    let (gone, plan) = raster(&src, None, remove, Limits::default()).unwrap();
    assert_eq!(count(&plan, LossCode::AnnotationsRemoved), 1);
    assert_eq!(count(&plan, LossCode::AnnotationsFlattened), 0);
    assert_eq!(plan.report.flattened.annotations, 0);
    let (drawn, _) = raster(&src, None, consenting(), Limits::default()).unwrap();
    let pixels = |b: &[u8], n: &str| {
        let (f, raw) = (
            tmp("raster-export", n),
            tmp("raster-export", &format!("{n}.rgb")),
        );
        std::fs::write(&f, b).unwrap();
        let (ok, log) = python(&[
            "tests/oracles/raster.py",
            "image",
            f.to_str().unwrap(),
            "0",
            raw.to_str().unwrap(),
        ]);
        assert!(ok, "{log}");
        std::fs::read(raw).unwrap()
    };
    assert_ne!(pixels(&drawn, "drawn.pdf"), pixels(&gone, "gone.pdf"));
    // The document-level items come through the same scan as the native report.
    let (_, plan) = raster(
        &fixture("generated/leaks-metadata-attachment-js.pdf"),
        None,
        consenting(),
        Limits::default(),
    )
    .unwrap();
    for code in [
        LossCode::MetadataRemoved,
        LossCode::AttachmentsRemoved,
        LossCode::ScriptsRemoved,
    ] {
        assert!(count(&plan, code) > 0, "{code:?}");
    }
}

const PAGE: &str = "BT /F1 10 Tf 1 0 0 1 20 380 Tm (alpha SECRETONE omega) Tj ET\n\
BT /F1 10 Tf 1 0 0 1 20 360 Tm (visible line) Tj ET";

#[test]
fn a_redacted_raster_export_has_no_secret_anywhere_and_keeps_the_public_page() {
    let input = pdf_bytes(&[PAGE], &Font::default(), 0);
    let mut e = Engine::new(Limits::default());
    let (doc, _) = e.open(input.clone(), None, false).unwrap();
    let t = e.extract_text(&doc, 0, Default::default());
    let text = run(&mut e, t).unwrap().text;
    let start = text.find("SECRETONE").unwrap() as u32;
    let t = e.apply_changes(
        &doc,
        ChangeSet {
            base_revision: 0,
            text_edits: None,
            redactions: Some(vec![Redaction::Text {
                id: "r".into(),
                page_index: 0,
                start,
                end: start + 9,
                fill: None,
            }]),
        },
    );
    assert!(run(&mut e, t).unwrap().accepted);
    let t = e.prepare_export(&doc, ExportMode::Raster, consenting());
    let plan = run(&mut e, t).unwrap();
    let out = e.take_export(&doc, &plan.plan_id).unwrap();
    assert_eq!(
        (
            plan.report.redaction.regions,
            plan.report.redaction.clusters_removed
        ),
        (1, 0)
    );

    let (ok, log) = objects_oracle("raster-export", "red.pdf", &out, &["SECRETONE"]);
    assert!(ok, "objects.py: {log}");
    // The oracle sees the secret in the source, and the output has no text layer at all: even
    // public words are pixels only.
    let (ok, _) = objects_oracle("raster-export", "red-in.pdf", &input, &["SECRETONE"]);
    assert!(!ok);
    let (ok, log) = objects_oracle("raster-export", "red-pub.pdf", &out, &["visible line"]);
    assert!(ok, "no text layer, even for public words: {log}");
    // The public line is still drawn (fidelity.py raster mode, changed = the word's line box).
    let (i, o, spec) = (
        tmp("raster-export", "red.in.pdf"),
        tmp("raster-export", "red.out.pdf"),
        tmp("raster-export", "red.json"),
    );
    std::fs::write(&i, &input).unwrap();
    std::fs::write(&o, &out).unwrap();
    std::fs::write(
        &spec,
        serde_json::json!({"raster": true, "threshold": 64, "max_frac": 0.01, "changed": [{"page": 0, "rect": [20.0, 8.0, 280.0, 30.0]}]}).to_string(),
    )
    .unwrap();
    let (ok, log) = python(&[
        "tests/oracles/fidelity.py",
        i.to_str().unwrap(),
        o.to_str().unwrap(),
        "--spec",
        spec.to_str().unwrap(),
    ]);
    assert!(ok, "fidelity.py: {log}");
}

#[test]
fn an_output_over_the_byte_limit_is_refused_with_no_plan() {
    let src = pdf_bytes(&[PAGE, PAGE, PAGE], &Font::default(), 0);
    let tight = Limits {
        max_output_bytes: 3000,
        ..Limits::default()
    };
    let mut e = Engine::new(tight);
    let (doc, _) = e.open(src, None, false).unwrap();
    let t = e.prepare_export(&doc, ExportMode::Raster, consenting());
    let err = run(&mut e, t).unwrap_err();
    assert_eq!(err.code, ErrorCode::LimitExceeded);
    assert_eq!(
        err.details.unwrap().limit.as_deref(),
        Some("maxOutputBytes")
    );
    assert_eq!(e.stats().plan_bytes, 0);
}

/// `FOLIARK_BENCH_PDF=<file> cargo test --release -p foliark-engine --test raster_export -- --ignored --nocapture`
#[test]
#[ignore = "timing probe, not a check"]
fn bench_raster_export_at_scale_2() {
    let Ok(path) = std::env::var("FOLIARK_BENCH_PDF") else {
        return;
    };
    let src = std::fs::read(path).unwrap();
    let t = std::time::Instant::now();
    let (out, plan) = raster(&src, None, consenting(), Limits::default()).unwrap();
    eprintln!(
        "raster bench: {} pages, {} B in -> {} B out, {:.2} s",
        plan.report.pages,
        src.len(),
        out.len(),
        t.elapsed().as_secs_f64()
    );
}
