//! Integrated native removal (spec §5; NEW-39): text, images, vectors and shared Forms leave one
//! export together, the report counts are what the stages did, and a request that cannot be fully
//! carried out refuses the whole export naming its ids. Outputs are judged by the independent
//! oracles in tests/oracles (pikepdf walk, PyMuPDF text and renders), never by the engine.
mod common;
mod image_pdf;
use common::{python, run, tmp};
use foliark_engine::export::{self, AppliedRedaction};
use foliark_engine::{Binding, Committed, Document, Engine, StoredRedaction};
use foliark_model::{
    ChangeSet, ErrorCode, ExportMode, ExportPlan, ExportPolicy, Limits, LossCode, PdfError, Rect,
    Redaction, SharedKind, SharedSurvivor,
};
use image_pdf::{assemble, rgb_image, stream};
use serde_json::{Value, json};

const HELV: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>";

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Rect {
    Rect {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    }
}

fn text(x: f64, y: f64, s: &str) -> String {
    format!("BT /F1 12 Tf {x} {y} Td ({s}) Tj ET\n")
}

/// Objects 1 catalog, 2 tree, 3 font; pages from 4: page `i` is `4 + 2i` with content `5 + 2i`;
/// `extra` objects follow the pages. Each page is `(contents, /Resources body)`.
fn doc_with(pages: &[(String, String)], extra: Vec<Vec<u8>>) -> Vec<u8> {
    let kids: String = (0..pages.len())
        .map(|i| format!("{} 0 R ", 4 + 2 * i))
        .collect();
    let mut o = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", pages.len()).into_bytes(),
        HELV.as_bytes().to_vec(),
    ];
    for (i, (content, res)) in pages.iter().enumerate() {
        o.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> {res} >> >>",
                5 + 2 * i
            )
            .into_bytes(),
        );
        o.push(stream("", content.as_bytes()));
    }
    o.extend(extra);
    assemble(&o)
}

fn form(content: &str) -> Vec<u8> {
    stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 300 400] /Resources << /Font << /F1 3 0 R >> >>",
        content.as_bytes(),
    )
}

fn region(id: &str, r: Rect, fill: Option<Option<[f64; 3]>>) -> Redaction {
    Redaction::Region {
        id: id.into(),
        page_index: 0,
        rect: r,
        fill,
    }
}

fn engine(bytes: Vec<u8>, redactions: Vec<Redaction>) -> (Engine, String) {
    let mut e = Engine::new(Limits::default());
    let (doc, _) = e.open(bytes, None, false).unwrap();
    let task = e.apply_changes(
        &doc,
        ChangeSet {
            base_revision: 0,
            text_edits: None,
            redactions: Some(redactions),
        },
    );
    let r = run(&mut e, task).unwrap();
    assert!(r.accepted, "{:?}", r.rejected);
    (e, doc)
}

/// An export that must succeed, or the refusal (never prints the bytes).
fn refused(e: &mut Engine, doc: &str) -> PdfError {
    match export(e, doc) {
        Ok((plan, _)) => panic!("exported: {:?}", plan.report.redaction),
        Err(err) => err,
    }
}

fn export(e: &mut Engine, doc: &str) -> Result<(ExportPlan, Vec<u8>), PdfError> {
    let task = e.prepare_export(doc, ExportMode::Native, ExportPolicy::default());
    let plan = run(e, task)?;
    let bytes = e.take_export(doc, &plan.plan_id)?;
    Ok((plan, bytes))
}

/// objects.py with `spec` on `bytes`.
fn judge(name: &str, bytes: &[u8], spec: &Value) -> (bool, String) {
    let (pdf, json_path) = (
        tmp("results", name),
        tmp("results", &format!("{name}.json")),
    );
    std::fs::write(&pdf, bytes).unwrap();
    std::fs::write(&json_path, spec.to_string()).unwrap();
    python(&[
        "tests/oracles/objects.py",
        pdf.to_str().unwrap(),
        "--spec",
        json_path.to_str().unwrap(),
    ])
}

/// PyMuPDF: `(text of page 0, every pixel of the display rect is this gray level)`.
fn render_probe(name: &str, bytes: &[u8], r: Rect) -> (String, Option<u8>) {
    let pdf = tmp("results", name);
    std::fs::write(&pdf, bytes).unwrap();
    let script = "import fitz,sys\n\
        d=fitz.open(sys.argv[1]);p=d[0]\n\
        x0,y0,x1,y1=map(float,sys.argv[2:6])\n\
        s=set(p.get_pixmap(clip=fitz.Rect(x0,y0,x1,y1),alpha=False).samples)\n\
        print(p.get_text().replace(chr(10),' '));print(list(s)[0] if len(s)==1 else -1)";
    let (x0, y0, x1, y1) = (r.x, r.y, r.x + r.width, r.y + r.height);
    let args = [x0, y0, x1, y1].map(|v| v.to_string());
    let (ok, out) = python(&[
        "-c",
        script,
        pdf.to_str().unwrap(),
        &args[0],
        &args[1],
        &args[2],
        &args[3],
    ]);
    assert!(ok, "{out}");
    let mut lines = out.lines();
    let text = lines.next().unwrap().to_owned();
    let gray: i32 = lines.next().unwrap().trim().parse().unwrap();
    (text, u8::try_from(gray).ok())
}

fn feature(e: &PdfError) -> Option<&str> {
    e.details.as_ref().and_then(|d| d.feature.as_deref())
}

#[test]
fn removal_does_not_depend_on_the_fill() {
    // SECRETTXT and a red square, both fully inside their regions.
    let content = format!(
        "{}1 0 0 rg 150 250 40 40 re f\n",
        text(20.0, 350.0, "SECRETTXT")
    );
    let bytes = doc_with(&[(content, String::new())], vec![]);
    let (over_text, over_square) = (
        rect(15.0, 38.0, 90.0, 56.0),
        rect(145.0, 105.0, 195.0, 155.0),
    );
    let regions = |fill| vec![region("t", over_text, fill), region("v", over_square, fill)];

    let (mut e, doc) = engine(bytes.clone(), regions(Some(None)));
    let (plan, out) = export(&mut e, &doc).unwrap();
    let r = &plan.report.redaction;
    assert_eq!((r.clusters_removed, r.vector_subpaths_removed), (9, 1));
    let spec = json!({
        "forbidden": ["SECRETTXT"],
        "vector_regions": [{"page": 0, "rect": [145, 105, 195, 155]}],
    });
    let (ok, log) = judge("nofill.pdf", &out, &spec);
    assert!(ok, "content survived a null fill: {log}");
    let (ok, _) = judge("nofill.in.pdf", &bytes, &spec);
    assert!(!ok, "the oracle found nothing wrong in the INPUT");
    for r in [over_text, over_square] {
        let (txt, gray) = render_probe("nofill.render.pdf", &out, r);
        assert!(!txt.contains("SECRET"));
        assert_eq!(
            gray,
            Some(255),
            "a null fill must show the background, not a box"
        );
    }

    // Default fill: same removal plus the black boxes.
    let (mut e, doc) = engine(bytes, regions(None));
    let (plan, black) = export(&mut e, &doc).unwrap();
    assert_eq!(plan.report.redaction, *r);
    assert_eq!(render_probe("black.pdf", &black, over_text).1, Some(0));
}

/// Page 0: text, an image, a red square and the shared Form `F` (secret text); page 1 draws `F` too.
fn mixed() -> (Vec<u8>, Vec<Redaction>) {
    let p0 = format!(
        "{}{}q 60 0 0 60 20 200 cm /Im Do Q\n1 0 0 rg 120 200 60 60 re f\n/Fm Do\n",
        text(20.0, 350.0, "SECRETTXT"),
        text(20.0, 300.0, "PUBLICTXT"),
    );
    // Objects after the two pages: 8 image, 9 Form.
    let res = "/XObject << /Im 8 0 R /Fm 9 0 R >>".to_string();
    let bytes = doc_with(
        &[
            (p0, res),
            ("/Fm Do\n".into(), "/XObject << /Fm 9 0 R >>".into()),
        ],
        vec![rgb_image(8, 8, ""), form(&text(20.0, 100.0, "SECRETFORM"))],
    );
    let regions = vec![
        region("txt", rect(15.0, 38.0, 90.0, 56.0), None),
        region("img", rect(40.0, 150.0, 70.0, 180.0), None),
        region("vec", rect(110.0, 130.0, 190.0, 210.0), None),
        region("frm", rect(15.0, 288.0, 100.0, 305.0), None),
    ];
    (bytes, regions)
}

#[test]
fn a_mixed_page_loses_text_image_vector_and_shared_form_text_in_one_export() {
    let (bytes, regions) = mixed();
    let (mut e, doc) = engine(bytes.clone(), regions);
    let (plan, out) = export(&mut e, &doc).unwrap();

    // Counts are what the stages did: 9 + 10 letters, the image, the square, one Form clone.
    let r = &plan.report.redaction;
    assert_eq!(
        (
            r.regions,
            r.clusters_removed,
            r.images_erased,
            r.images_cloned,
            r.forms_cloned,
            r.vector_subpaths_removed,
            r.vector_rects_cut
        ),
        (4, 19, 1, 0, 1, 1, 0)
    );
    assert!(r.collateral.is_empty());
    assert_eq!(
        r.shared_survivors,
        vec![SharedSurvivor {
            kind: SharedKind::Form,
            redacted_on: vec![0],
            untouched_on: vec![1]
        }]
    );
    assert!(
        plan.report
            .losses
            .iter()
            .any(|l| l.code == LossCode::ImageReencoded && l.count == 1)
    );

    let spec = json!({
        "forbidden": ["SECRETTXT"],
        "image_regions": [{"page": 0, "rect": [40, 150, 70, 180], "color": [0, 0, 0]}],
        "vector_regions": [{"page": 0, "rect": [110, 130, 190, 210], "fill": [0, 0, 0]}],
        "occurrences": [
            {"page": 0, "path": [0], "forbid": ["SECRETFORM"]},
            {"page": 1, "path": [0], "require": ["SECRETFORM"]},
        ],
        "no_unused_xobjects": true,
    });
    let (ok, log) = judge("mixed.pdf", &out, &spec);
    assert!(ok, "{log}");
    let (ok, _) = judge("mixed.in.pdf", &bytes, &spec);
    assert!(!ok, "the oracle found nothing wrong in the INPUT");
    let (txt, _) = render_probe("mixed.render.pdf", &out, rect(0.0, 0.0, 1.0, 1.0));
    assert!(txt.contains("PUBLICTXT") && !txt.contains("SECRET"));
}

#[test]
fn one_unsupported_redaction_refuses_the_whole_export_and_names_it() {
    // A tiling-pattern square under region "pat", next to supported text and image redactions.
    let (mixed_bytes, mut regions) = mixed();
    let pattern = stream(
        "/PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 /Resources << >>",
        b"0 0 5 5 re f",
    );
    let p0 = format!(
        "{}/Pattern cs /P1 scn 200 100 80 80 re f\n",
        text(20.0, 350.0, "SECRETTXT")
    );
    let bytes = doc_with(&[(p0, "/Pattern << /P1 6 0 R >>".into())], vec![pattern]);
    regions.truncate(1);
    regions.push(region("pat", rect(210.0, 230.0, 260.0, 290.0), None));
    let (mut e, doc) = engine(bytes, regions);
    let err = refused(&mut e, &doc);
    assert_eq!(err.code, ErrorCode::RedactPatternUnsupported);
    let d = err.details.unwrap();
    assert_eq!(d.ids, Some(vec!["pat".to_string()]));
    assert_eq!(d.pages, Some(vec![0]));
    // No plan, so no bytes: the supported "txt" redaction was not exported on its own.
    assert_eq!(
        e.take_export(&doc, "plan-1-1").unwrap_err().code,
        ErrorCode::StalePlan
    );

    // The same supported work alone exports (the refusal came from "pat", not from the rest).
    let (mut e, doc) = engine(mixed_bytes, mixed().1);
    assert!(export(&mut e, &doc).is_ok());
}

#[test]
fn a_partly_covered_cluster_and_collateral_vector_are_reported_in_the_effective_regions() {
    // The region cuts 2.5 pt into the 6 pt wide "S" at x 20..26; a stroked line crosses region "line".
    let content = format!(
        "{}2 w 10 330 m 100 330 l S\n",
        text(20.0, 350.0, "SECRETTXT")
    );
    let bytes = doc_with(&[(content, String::new())], vec![]);
    let committed = Committed {
        edits: vec![],
        redactions: vec![
            ("cut", rect(10.0, 38.0, 22.5, 56.0)),
            ("line", rect(40.0, 65.0, 60.0, 75.0)),
        ]
        .into_iter()
        .map(|(id, r)| StoredRedaction {
            id: id.into(),
            page_index: 0,
            regions: vec![r],
            fill: None,
            must_remove: vec![],
        })
        .collect(),
    };
    let doc = Document::open(bytes, None).unwrap();
    let binding = Binding {
        doc_id: "d".into(),
        generation: 1,
        revision: 1,
    };
    let done = export::prepare(
        &doc,
        &committed,
        binding,
        ExportMode::Native,
        &ExportPolicy::default(),
        &Limits::default(),
    )
    .unwrap();

    let r = &done.report.redaction;
    assert_eq!(r.clusters_removed, 1, "only the cut glyph goes");
    assert_eq!(r.vector_subpaths_removed, 1);
    let line = &r.collateral[0];
    assert_eq!(line.page_index, 0);
    assert!(line.bounds.x <= 10.0 && line.bounds.x + line.bounds.width >= 100.0);
    assert!(
        done.report
            .losses
            .iter()
            .any(|l| l.code == LossCode::VectorCollateral && l.count == 1)
    );

    let effective =
        |id: &str| -> &AppliedRedaction { done.applied.iter().find(|a| a.id == id).unwrap() };
    let cut = effective("cut").effective[0];
    assert!(
        cut.x + cut.width >= 26.0 - 0.01,
        "the whole glyph is disclosed: {cut:?}"
    );
    assert!(cut.x + cut.width > 22.5 + 3.0, "grown past what was asked");
    let line = effective("line").effective[0];
    assert!(line.x <= 10.0 && line.x + line.width >= 100.0, "{line:?}");

    // Survivors stay: PyMuPDF still reads everything after the removed "S".
    let (txt, _) = render_probe("cut.pdf", &done.bytes, rect(0.0, 0.0, 1.0, 1.0));
    assert!(
        txt.contains("ECRETTXT") && !txt.contains("SECRETTXT"),
        "{txt}"
    );
}

/// A red square under a luminosity soft mask whose group has its own BBox.
fn masked(content_rect: &str, bbox: &str) -> Vec<u8> {
    let page = format!(
        "{}q /GS gs 1 0 0 rg {content_rect} re f Q\n",
        text(20.0, 90.0, "PUBLICTXT")
    );
    let res = "/ExtGState << /GS << /SMask << /Type /Mask /S /Luminosity /G 6 0 R >> >> >>";
    let group = stream(
        &format!(
            "/Type /XObject /Subtype /Form /BBox [{bbox}] /Group << /S /Transparency /CS /DeviceGray >> /Resources << >>"
        ),
        b"0.5 g 0 0 300 400 re f",
    );
    doc_with(&[(page, res.into())], vec![group])
}

#[test]
fn a_region_that_meets_soft_masked_content_or_its_group_is_refused_never_skipped() {
    let over_square = region("sm", rect(30.0, 110.0, 60.0, 140.0), None);
    let elsewhere = region("txt", rect(15.0, 300.0, 90.0, 320.0), None);

    // Content drawn under the mask.
    let bytes = masked("20 260 100 100", "0 0 300 400");
    let (mut e, doc) = engine(bytes, vec![over_square.clone()]);
    let err = refused(&mut e, &doc);
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(feature(&err), Some("redactionSoftMaskGroup"));
    assert_eq!(err.details.unwrap().ids, Some(vec!["sm".to_string()]));

    // Only the group's own BBox reaches the region (the masked square is far away).
    let bytes = masked("200 20 50 50", "20 260 100 360");
    let (mut e, doc) = engine(bytes, vec![over_square]);
    let err = refused(&mut e, &doc);
    assert_eq!(feature(&err), Some("redactionSoftMaskGroup"));

    // Control: a region away from the content and the group exports.
    let bytes = masked("20 260 100 100", "20 260 120 360");
    let (mut e, doc) = engine(bytes, vec![elsewhere]);
    let (plan, _) = export(&mut e, &doc).unwrap();
    assert_eq!(plan.report.redaction.clusters_removed, 9);
}

#[test]
fn a_shared_image_redacted_on_one_page_is_cloned_and_its_survivors_listed() {
    let content = |_: usize| "q 60 0 0 60 20 200 cm /Im Do Q\n".to_string();
    let res = "/XObject << /Im 8 0 R >>".to_string();
    let bytes = doc_with(
        &[(content(0), res.clone()), (content(1), res)],
        vec![rgb_image(8, 8, "")],
    );
    let (mut e, doc) = engine(
        bytes.clone(),
        vec![region("img", rect(20.0, 140.0, 80.0, 200.0), None)],
    );
    let (plan, out) = export(&mut e, &doc).unwrap();
    let r = &plan.report.redaction;
    assert_eq!((r.images_erased, r.images_cloned), (1, 1));
    assert_eq!(
        r.shared_survivors,
        vec![SharedSurvivor {
            kind: SharedKind::Image,
            redacted_on: vec![0],
            untouched_on: vec![1]
        }]
    );
    let spec = json!({
        "image_regions": [{"page": 0, "rect": [20, 140, 80, 200], "color": [0, 0, 0]}],
        "no_unused_xobjects": true,
    });
    let (ok, log) = judge("shared.pdf", &out, &spec);
    assert!(ok, "{log}");
    // Page 1 still holds the original pixels: the same check on it must fail.
    let page1 =
        json!({"image_regions": [{"page": 1, "rect": [20, 140, 80, 200], "color": [0, 0, 0]}]});
    assert!(!judge("shared.p1.pdf", &out, &page1).0);
}
