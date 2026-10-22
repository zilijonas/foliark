//! Form and content-stream occurrences (spec §5.3; NEW-38): an occurrence that needs a change
//! is cloned along its whole chain, every other occurrence of the same Forms keeps the
//! original. Text, images and vectors are removed in one writer, then the OUTPUT is judged by
//! the tests/oracles scripts (pikepdf walk of the drawn occurrences, PyMuPDF renders), never by
//! the engine: forbidden strings must be unreachable from the redacted occurrence and present
//! in the survivors, and no original that nothing draws may stay in the resources.
mod image_pdf;
use foliark_engine::redaction::image::redact_images;
use foliark_engine::redaction::plan::RedactionPlan;
use foliark_engine::redaction::text::redact_text;
use foliark_engine::redaction::vector::redact_vectors;
use foliark_engine::writer::Writer;
use foliark_engine::{Binding, Document};
use foliark_model::{ErrorCode, Limits, Rect, SharedKind};
use image_pdf::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

struct Out {
    plan: RedactionPlan,
    input: PathBuf,
    output: PathBuf,
}

/// Remove text, vectors and images of `per_page` regions in one writer; the output must reopen.
fn run(name: &str, input: &[u8], per_page: &[(u32, &[Rect])]) -> Out {
    let doc = Document::open(input.to_vec(), None).unwrap();
    let plan = make_plan(&doc, per_page);
    let mut w = Writer::new(doc.reader(), Limits::default());
    redact_text(&doc, &plan, &mut w).unwrap();
    redact_vectors(&doc, &plan, &mut w).unwrap();
    redact_images(&doc, &plan, &mut w).unwrap();
    let bytes = w.write().unwrap();
    Document::open(bytes.clone(), None).unwrap();
    let (input_path, output) = (
        tmp(&format!("{name}.in.pdf")),
        tmp(&format!("{name}.out.pdf")),
    );
    std::fs::write(&input_path, input).unwrap();
    std::fs::write(&output, bytes).unwrap();
    Out {
        plan,
        input: input_path,
        output,
    }
}

/// objects.py with `spec`: `(ok, log)`.
fn objects(path: &Path, spec: &Value) -> (bool, String) {
    let spec_path = path.with_extension("spec.json");
    std::fs::write(&spec_path, spec.to_string()).unwrap();
    let (ok, out, err) = python(&[
        "tests/oracles/objects.py",
        path.to_str().unwrap(),
        "--spec",
        spec_path.to_str().unwrap(),
    ]);
    (ok, format!("{out}{err}"))
}

/// The output passes `spec`, and the input fails it (the oracle is not vacuous).
fn judge(o: &Out, spec: &Value) {
    let (ok, log) = objects(&o.output, spec);
    assert!(ok, "objects.py on the output: {log}");
    let (ok, log) = objects(&o.input, spec);
    assert!(!ok, "objects.py found nothing wrong in the INPUT: {log}");
}

/// Everything outside `changed` (page, region) renders and reads the same as in the input.
fn same_outside(o: &Out, changed: &[(u32, Rect)]) {
    let rects: Vec<String> = changed
        .iter()
        .map(|(p, r)| {
            format!(
                "{p}:{},{},{},{}",
                r.x - 0.5,
                r.y - 0.5,
                r.x + r.width + 0.5,
                r.y + r.height + 0.5
            )
        })
        .collect();
    // pypdf cannot place text drawn inside Forms: pixels and PyMuPDF words judge these pages.
    let spec = o.output.with_extension("fidelity.json");
    std::fs::write(&spec, json!({"pypdf_chunks": false}).to_string()).unwrap();
    let mut args = vec![
        "tests/oracles/fidelity.py",
        o.input.to_str().unwrap(),
        o.output.to_str().unwrap(),
        "--spec",
        spec.to_str().unwrap(),
    ];
    for r in &rects {
        args.extend(["--changed", r]);
    }
    let (ok, out, err) = python(&args);
    assert!(ok, "fidelity.py: {out}{err}");
}

fn font() -> Vec<u8> {
    let widths = vec!["500"; 256].join(" ");
    format!("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 0 /LastChar 255 /Widths [{widths}] >>")
        .into_bytes()
}

fn text(x: f64, y: f64, s: &str) -> String {
    format!("BT /F1 12 Tf {x} {y} Td ({s}) Tj ET\n")
}

/// Display-space region around 12 pt Helvetica text of `w` pt starting at user (x, y) on a
/// 300 x 400 page.
fn over(x: f64, y: f64, w: f64) -> Rect {
    rect(x - 2.0, 400.0 - y - 10.0, x + w + 2.0, 400.0 - y + 4.0)
}

const OFFSET: &str = "q 1 0 0 1 0 -100 cm"; // the second occurrence sits 100 pt lower

/// Catalog 1, page tree 2, the font 3; the caller adds objects 4 and up.
fn catalog(kids: &str, count: usize) -> Vec<Vec<u8>> {
    vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {count} >>").into_bytes(),
        font(),
    ]
}

fn page_obj(contents: usize, xobjects: &str) -> Vec<u8> {
    format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents {contents} 0 R /Resources << /XObject << {xobjects} >> /Font << /F1 3 0 R >> >> >>"
    )
    .into_bytes()
}

fn form(res: &str, content: &str) -> Vec<u8> {
    stream(
        &format!("/Type /XObject /Subtype /Form /BBox [0 0 300 400] /Resources << {res} >>"),
        content.as_bytes(),
    )
}

#[test]
fn only_the_selected_occurrence_of_a_nested_repeated_form_changes() {
    // A (text + B) is drawn twice on page 0 and once on page 1; B holds the secret.
    let mut o = catalog("4 0 R 6 0 R", 2);
    o.push(page_obj(5, "/A 8 0 R")); // 4
    o.push(stream("", format!("/A Do\n{OFFSET}\n/A Do Q").as_bytes())); // 5
    o.push(page_obj(7, "/A 8 0 R")); // 6
    o.push(stream("", b"/A Do")); // 7
    let a = format!("{}/B Do", text(20.0, 350.0, "SECRETA"));
    o.push(form("/XObject << /B 9 0 R >> /Font << /F1 3 0 R >>", &a)); // 8
    let b = format!(
        "{}{}",
        text(20.0, 300.0, "SECRETB"),
        text(120.0, 300.0, "PUBLICB")
    );
    o.push(form("/Font << /F1 3 0 R >>", &b)); // 9
    let input = assemble(&o);

    let region = over(20.0, 300.0, 42.0);
    let out = run("nested", &input, &[(0, &[region])]);

    // Both Forms on the chain are cloned; the untouched occurrences are disclosed.
    assert_eq!(out.plan.report.forms_cloned, 2);
    let forms: Vec<_> = out
        .plan
        .report
        .shared_survivors
        .iter()
        .filter(|s| s.kind == SharedKind::Form)
        .collect();
    assert_eq!(forms.len(), 2);
    for s in forms {
        assert_eq!(
            (&s.redacted_on[..], &s.untouched_on[..]),
            (&[0][..], &[0, 1][..])
        );
    }

    let spec = json!({
        "occurrences": [
            {"page": 0, "path": [0], "forbid": ["SECRETB"], "require": ["SECRETA", "PUBLICB"]},
            {"page": 0, "path": [0, 0], "forbid": ["SECRETB"], "require": ["PUBLICB"]},
            {"page": 0, "path": [1], "require": ["SECRETA", "SECRETB", "PUBLICB"]},
            {"page": 1, "path": [0], "require": ["SECRETA", "SECRETB", "PUBLICB"]},
        ],
        "no_unused_xobjects": true,
    });
    judge(&out, &spec);
    same_outside(&out, &[(0, region)]);
}

#[test]
fn a_form_clipped_out_of_view_is_still_cleaned_and_its_other_occurrence_keeps_it() {
    // The secret sits right of a clip that hides it; a region over where it would be still
    // takes it out of the selected occurrence.
    let content = format!(
        "q 0 0 100 400 re W n\n{}Q\n{}",
        text(150.0, 300.0, "SECRETH"),
        text(20.0, 200.0, "VISIBLEH")
    );
    let mut o = catalog("4 0 R", 1);
    o.push(page_obj(5, "/H 6 0 R")); // 4
    o.push(stream("", format!("/H Do\n{OFFSET}\n/H Do Q").as_bytes())); // 5
    o.push(form("/Font << /F1 3 0 R >>", &content)); // 6
    let input = assemble(&o);
    let region = over(150.0, 300.0, 42.0);
    let out = run("clipped", &input, &[(0, &[region])]);
    let spec = json!({
        "occurrences": [
            {"page": 0, "path": [0], "forbid": ["SECRETH"], "require": ["VISIBLEH"]},
            {"page": 0, "path": [1], "require": ["SECRETH", "VISIBLEH"]},
        ],
    });
    judge(&out, &spec);
    same_outside(&out, &[(0, region)]);
}

#[test]
fn an_image_in_a_shared_form_is_cloned_with_the_form() {
    let base = first_obj(1);
    let form = stream(
        &format!(
            "/Type /XObject /Subtype /Form /BBox [0 0 300 400] /Resources << /XObject << /Im0 {} 0 R >> >>",
            base + 1
        ),
        b"q 80 0 0 80 100 200 cm /Im0 Do Q",
    );
    let input = pdf(
        &[page(
            "",
            "/F Do q 1 0 0 1 0 -150 cm /F Do Q",
            &[("F", base)],
        )],
        &[form, rgb_image(8, 8, "")],
    );
    // The image of the first occurrence is at display x 100..180, y 120..200.
    let first = rect(100.0, 100.0, 200.0, 220.0);
    let out = run("form-image", &input, &[(0, &[first])]);
    assert_eq!(out.plan.report.images_cloned, 1);
    let img = |r: Rect| json!({"page": 0, "rect": [r.x, r.y, r.x + r.width, r.y + r.height]});
    let spec = json!({"image_regions": [img(first)], "no_unused_xobjects": true});
    judge(&out, &spec);
    same_outside(&out, &[(0, first)]);

    // Both occurrences erased: no original Form or image stays reachable from the page.
    let second = rect(100.0, 250.0, 200.0, 370.0);
    let both = run("form-image-both", &input, &[(0, &[first, second])]);
    assert!(both.plan.report.shared_survivors.is_empty());
    let spec = json!({"image_regions": [img(first), img(second)], "no_unused_xobjects": true});
    judge(&both, &spec);
    let (ok, log) = objects(&both.output, &spec);
    assert!(ok, "{log}");
    let (_, listing, _) = python(&["tests/oracles/samples.py", both.output.to_str().unwrap()]);
    let all: Value = serde_json::from_str(&listing).unwrap();
    let originals = all["all"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| bytes_of(&i["data"]).get(..3) == Some(&px(0, 0)[..]))
        .count();
    assert_eq!(
        originals, 0,
        "an original image object is still in the file"
    );
}

#[test]
fn a_vector_in_a_shared_form_is_removed_from_the_selected_occurrence_only() {
    let content = "1 0 0 rg 50 300 60 40 re f\n0 0 1 rg 200 300 30 30 re f";
    let base = first_obj(1);
    let input = pdf(
        &[page(
            "",
            "/V Do q 1 0 0 1 0 -150 cm /V Do Q",
            &[("V", base)],
        )],
        &[form("", content)],
    );
    // Covers the red rectangle of the first occurrence only (display y 60..100).
    let region = rect(40.0, 50.0, 120.0, 110.0);
    let out = run("form-vector", &input, &[(0, &[region])]);
    assert_eq!(out.plan.report.forms_cloned, 1);
    let spec = json!({
        "vector_regions": [{"page": 0, "rect": [region.x, region.y, region.x + region.width, region.y + region.height]}],
        "no_unused_xobjects": true,
    });
    judge(&out, &spec);
    same_outside(&out, &[(0, region)]);
}

#[test]
fn a_content_stream_two_pages_draw_is_cloned_for_the_page_that_changes() {
    let mut o = catalog("4 0 R 6 0 R", 2);
    o.push(page_obj(5, "")); // 4
    o.push(stream("", text(20.0, 300.0, "SECRETP").as_bytes())); // 5, drawn by both pages
    o.push(page_obj(5, "")); // 6
    let input = assemble(&o);
    let out = run("shared-content", &input, &[(0, &[over(20.0, 300.0, 42.0)])]);
    let words = |path: &Path| -> Vec<String> {
        let code = "import fitz,json,sys;print(json.dumps([p.get_text() for p in fitz.open(sys.argv[1])]))";
        let (ok, out, err) = python(&["-c", code, path.to_str().unwrap()]);
        assert!(ok, "{err}");
        serde_json::from_str(&out).unwrap()
    };
    let before = words(&out.input);
    assert!(before.iter().all(|t| t.contains("SECRETP")));
    let after = words(&out.output);
    assert!(
        !after[0].contains("SECRETP"),
        "page 0 still shows the secret"
    );
    assert!(after[1].contains("SECRETP"), "page 1 lost its text");
}

#[test]
fn an_unshared_parent_of_a_shared_form_is_edited_in_place_and_the_cycle_is_refused() {
    // P is drawn once and draws C twice: C is cloned, P is edited in place (no clone of P).
    let mut o = catalog("4 0 R", 1);
    o.push(page_obj(5, "/P 6 0 R")); // 4
    o.push(stream("", b"/P Do")); // 5
    let p = format!("/C Do\n{OFFSET}\n/C Do Q");
    o.push(form("/XObject << /C 7 0 R >>", &p)); // 6
    o.push(form("/Font << /F1 3 0 R >>", &text(20.0, 300.0, "SECRETC"))); // 7
    let input = assemble(&o);
    let region = over(20.0, 300.0, 42.0);
    let out = run("parent-in-place", &input, &[(0, &[region])]);
    assert_eq!(out.plan.report.forms_cloned, 1);
    let spec = json!({
        "occurrences": [
            {"page": 0, "path": [0, 0], "forbid": ["SECRETC"]},
            {"page": 0, "path": [0, 1], "require": ["SECRETC"]},
        ],
        "no_unused_xobjects": true,
    });
    judge(&out, &spec);
    same_outside(&out, &[(0, region)]);

    // A -> B -> A: the reader refuses the page, so planning never reaches a clone.
    let mut o = catalog("4 0 R", 1);
    o.push(page_obj(5, "/A 6 0 R")); // 4
    o.push(stream("", b"/A Do")); // 5
    o.push(form("/XObject << /B 7 0 R >>", "0 0 5 5 re f /B Do")); // 6
    o.push(form("/XObject << /A 6 0 R >>", "0 0 7 7 re f /A Do")); // 7
    let doc = Document::open(assemble(&o), None).unwrap();
    let c = committed(0, &[rect(0.0, 300.0, 50.0, 400.0)]);
    let binding = Binding {
        doc_id: "d".into(),
        generation: 1,
        revision: 1,
    };
    let err = RedactionPlan::build(&doc, &c, binding).unwrap_err();
    assert_eq!(err.code, ErrorCode::MalformedPdf);
}

#[test]
fn a_parent_form_without_resources_of_its_own_cannot_take_a_clone_entry_and_is_refused() {
    // P has no /Resources (it inherits the page's), so the clone of C has no place to be named.
    let mut o = catalog("4 0 R", 1);
    o.push(page_obj(5, "/P 6 0 R /C 7 0 R")); // 4
    o.push(stream("", b"/P Do")); // 5
    let p = format!("/C Do\n{OFFSET}\n/C Do Q");
    o.push(stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 300 400]",
        p.as_bytes(),
    )); // 6
    o.push(form("/Font << /F1 3 0 R >>", &text(20.0, 300.0, "SECRETC"))); // 7
    let input = assemble(&o);
    let doc = Document::open(input, None).unwrap();
    let plan = make_plan(&doc, &[(0, &[over(20.0, 300.0, 42.0)])]);
    let mut w = Writer::new(doc.reader(), Limits::default());
    let before = w.write().unwrap();
    let err = redact_text(&doc, &plan, &mut w).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(
        err.details.and_then(|d| d.feature).as_deref(),
        Some("redactionFormInheritedResources")
    );
    assert_eq!(
        w.write().unwrap(),
        before,
        "a refusal leaves the writer untouched"
    );
}
