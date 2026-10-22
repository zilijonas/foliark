//! Searchable OCR text and semantic tags at export (spec §5 rules 1 and 6, §8.2; NEW-44). Tiny
//! hand-built pages: an invisible OCR layer (render mode 3 and 7) next to visible text. Outputs
//! are judged by Foliark's text, by the independent oracles (objects.py: payloads in decoded
//! streams and strings, with the input as the negative control; fidelity.py: pixels, words and
//! PyMuPDF/pypdf extraction) and by PyMuPDF's own search.
mod common;
use common::{fidelity_oracle, has, objects_oracle, python, run, tiny_pdf};
use foliark_engine::{Document, Engine};
use foliark_model::{
    CapabilityStatus, ChangeSet, ErrorCode, ExportMode, ExportPlan, ExportPolicy, ExtractOptions,
    InvisibleTextPolicy, Limits, LossCode, PdfError, Rect, Redaction, SanitizedItem,
};

fn export_with(
    bytes: &[u8],
    policy: ExportPolicy,
    redactions: Vec<Redaction>,
) -> Result<(ExportPlan, Vec<u8>), PdfError> {
    let mut e = Engine::new(Limits::default());
    let (doc, _) = e.open(bytes.to_vec(), None, false).unwrap();
    if !redactions.is_empty() {
        let changes = ChangeSet {
            base_revision: 0,
            text_edits: None,
            redactions: Some(redactions),
        };
        let t = e.apply_changes(&doc, changes);
        assert!(run(&mut e, t).unwrap().accepted);
    }
    let t = e.prepare_export(&doc, ExportMode::Native, policy);
    let plan = match run(&mut e, t) {
        Ok(p) => p,
        Err(err) => {
            let t = e.capabilities(&doc, false);
            let caps = run(&mut e, t).unwrap();
            // A refusal known up front is the capability's reason too.
            assert_eq!(caps.export_native.status, CapabilityStatus::Unsupported);
            assert_eq!(caps.export_native.reasons[0].code, err.code);
            return Err(err);
        }
    };
    let out = e.take_export(&doc, &plan.plan_id).unwrap();
    Ok((plan, out))
}

fn words(bytes: &[u8]) -> String {
    Document::open(bytes.to_vec(), None)
        .unwrap()
        .extract_text(0, &ExtractOptions::default())
        .unwrap()
        .text
}

/// Occurrences of `needle` PyMuPDF's own search finds on page 0.
fn fitz_hits(name: &str, bytes: &[u8], needle: &str) -> usize {
    let p = common::tmp("sanitize-semantics", &format!("{name}.search.pdf"));
    std::fs::write(&p, bytes).unwrap();
    let code =
        format!("import fitz,sys;print(len(fitz.open(sys.argv[1])[0].search_for('{needle}')))");
    let (ok, out) = python(&["-c", &code, p.to_str().unwrap()]);
    assert!(ok, "{out}");
    out.trim().parse().unwrap()
}

fn show(mode: u8, x: u32, y: u32, s: &str) -> String {
    // Mode 7 also clips: keep it inside q/Q.
    format!("q BT {mode} Tr /F1 12 Tf {x} {y} Td ({s}) Tj ET Q\n")
}

fn page(content: &str) -> Vec<u8> {
    tiny_pdf("", "/Resources << /Font << /F1 5 0 R >> >>", content, &[])
}

/// Display rect `(x0, y0, x1, y1)` of a 12 pt line whose baseline starts at user `(x, y)`.
fn line(x: f64, y: f64) -> (f64, f64, f64, f64) {
    (x - 2.0, 400.0 - y - 13.0, x + 250.0, 400.0 - y + 4.0)
}

fn region(id: &str, r: (f64, f64, f64, f64)) -> Redaction {
    Redaction::Region {
        id: id.into(),
        page_index: 0,
        rect: Rect {
            x: r.0,
            y: r.1,
            width: r.2 - r.0,
            height: r.3 - r.1,
        },
        fill: None,
    }
}

fn loss(p: &ExportPlan, code: LossCode) -> u32 {
    p.report
        .losses
        .iter()
        .find(|l| l.code == code)
        .map_or(0, |l| l.count)
}

/// The output is free of `forbid` for objects.py, the input is not (negative control), and
/// fidelity.py finds everything outside `changed` the same and `keep` extractable by two parsers.
fn judge(
    name: &str,
    input: &[u8],
    out: &[u8],
    forbid: &[&str],
    keep: &[&str],
    changed: &[(f64, f64, f64, f64)],
) {
    let (ok, log) = objects_oracle("sanitize-semantics", &format!("{name}.pdf"), out, forbid);
    assert!(ok, "{name}: objects.py: {log}");
    let (ok, _) = objects_oracle(
        "sanitize-semantics",
        &format!("{name}.in.pdf"),
        input,
        forbid,
    );
    assert!(
        !ok,
        "{name}: negative control, the payloads are in the input"
    );
    let changed: Vec<_> = changed.iter().map(|c| (0, c.0, c.1, c.2, c.3)).collect();
    let (ok, log) = fidelity_oracle("sanitize-semantics", name, input, out, &changed, keep);
    assert!(ok, "{name}: fidelity.py: {log}");
}

/// Visible text, then an OCR layer: two secrets (render mode 3 and 7) and two survivors.
fn ocr_page() -> Vec<u8> {
    let mut c = show(0, 20, 380, "PUBLIC_VISIBLE");
    c += &show(3, 20, 300, "SECRET_OCR_THREE");
    c += &show(3, 20, 260, "KEEP_OCR_THREE");
    c += &show(7, 20, 220, "SECRET_OCR_SEVEN");
    c += &show(7, 20, 180, "KEEP_OCR_SEVEN");
    page(&c)
}

#[test]
fn keep_policy_removes_ocr_under_regions_and_keeps_the_rest_searchable() {
    let input = ocr_page();
    let rows = [line(20.0, 300.0), line(20.0, 220.0)];
    let (plan, out) = export_with(
        &input,
        ExportPolicy::default(),
        vec![region("a", rows[0]), region("b", rows[1])],
    )
    .unwrap();
    let t = words(&out);
    assert!(!t.contains("SECRET"), "{t}");
    for k in ["PUBLIC_VISIBLE", "KEEP_OCR_THREE", "KEEP_OCR_SEVEN"] {
        assert!(t.contains(k), "{k} missing: {t}");
        assert!(fitz_hits("keep", &out, k) >= 1, "PyMuPDF cannot find {k}");
    }
    assert_eq!(fitz_hits("keep-secret", &out, "SECRET"), 0);
    // The survivors are reported as kept; nothing was removed by policy.
    assert_eq!(plan.report.kept.invisible_text_clusters, 14 + 14);
    assert_eq!(plan.report.redaction.clusters_removed, 16 + 16);
    assert!(
        !plan
            .report
            .removed
            .contains_key(&SanitizedItem::InvisibleText)
    );
    assert_eq!(loss(&plan, LossCode::TextSearchLost), 0);
    judge(
        "keep",
        &input,
        &out,
        &["SECRET_OCR_THREE", "SECRET_OCR_SEVEN"],
        &["PUBLIC_VISIBLE", "KEEP_OCR_THREE", "KEEP_OCR_SEVEN"],
        &rows,
    );
}

#[test]
fn remove_policy_drops_all_invisible_text_and_leaves_visible_text_where_it_was() {
    // A hidden run inside a text object must not move the visible one after it.
    let mut c = show(0, 20, 380, "PUBLIC_VISIBLE");
    c += &show(3, 20, 300, "OCR_ONE");
    c += &show(7, 20, 260, "OCR_TWO");
    c += "BT /F1 12 Tf 20 220 Td 3 Tr (OCR_RUN  ) Tj 0 Tr (VISIBLE_AFTER) Tj ET\n";
    let input = page(&c);
    let policy = ExportPolicy {
        invisible_text: InvisibleTextPolicy::Remove,
        ..ExportPolicy::default()
    };
    let (plan, out) = export_with(&input, policy, vec![]).unwrap();
    let t = words(&out);
    assert!(!t.contains("OCR_"), "{t}");
    assert!(
        t.contains("PUBLIC_VISIBLE") && t.contains("VISIBLE_AFTER"),
        "{t}"
    );
    assert_eq!(plan.report.kept.invisible_text_clusters, 0);
    let n = 7 + 7 + 9;
    assert_eq!(plan.report.removed[&SanitizedItem::InvisibleText], n);
    assert_eq!(loss(&plan, LossCode::TextSearchLost), n);
    let run = line(20.0, 220.0);
    judge(
        "remove",
        &input,
        &out,
        &["OCR_ONE", "OCR_TWO", "OCR_RUN"],
        &["PUBLIC_VISIBLE", "VISIBLE_AFTER"],
        &[
            line(20.0, 300.0),
            line(20.0, 260.0),
            (run.0, run.1, 20.0 + 9.0 * 6.7, run.3),
        ],
    );
    // Same input under `keep`: the policy is what removes, not the pipeline.
    let (kept, out) = export_with(&input, ExportPolicy::default(), vec![]).unwrap();
    assert!(words(&out).contains("OCR_RUN"));
    // Extraction trims the run's two trailing spaces: 21 clusters of the 23 glyphs removed.
    assert_eq!(kept.report.kept.invisible_text_clusters, n - 2);
}

#[test]
fn text_that_cannot_be_placed_refuses_native_when_regions_exist() {
    // A hidden copy of a word under a singular CTM has no place on the page: no region can be
    // said to cover it, so dropping the region's content would leave it searchable.
    let mut c = show(0, 20, 380, "PUBLIC_VISIBLE");
    c += "q 0 0 0 0 0 0 cm BT 3 Tr /F1 12 Tf 20 300 Td (SECRET_UNPLACED) Tj ET Q\n";
    let input = page(&c);
    let err = export_with(
        &input,
        ExportPolicy::default(),
        vec![region("r", line(20.0, 380.0))],
    )
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    let d = err.details.unwrap();
    assert_eq!(d.feature.as_deref(), Some("textPositionUnknown"));
    assert_eq!((d.pages, d.ids), (Some(vec![0]), Some(vec!["r".into()])));
    // No region: nothing to get wrong, the OCR text is exported like any other.
    let (_, out) = export_with(&input, ExportPolicy::default(), vec![]).unwrap();
    assert!(words(&out).contains("PUBLIC_VISIBLE"));
    // `remove` takes it out whatever its place.
    let policy = ExportPolicy {
        invisible_text: InvisibleTextPolicy::Remove,
        ..ExportPolicy::default()
    };
    let (_, out) = export_with(&input, policy, vec![region("r", line(20.0, 380.0))]).unwrap();
    assert!(!has(&out, "SECRET_UNPLACED") && !words(&out).contains("UNPLACED"));
}

#[test]
fn nested_actualtext_alt_and_e_are_stripped_and_counted() {
    // Inline: nested in an array and a dictionary. Resource: a /Properties entry, one level down.
    let content = "/Span << /MCID 0 /K [<< /ActualText (SECRET_ACTUAL) /Keep 1 >>] \
                   /X << /Alt (SECRET_ALT) >> >> BDC BT /F1 12 Tf 20 380 Td (PUBLIC_VISIBLE) Tj ET EMC\n\
                   /Span /P1 BDC BT /F1 12 Tf 20 340 Td (PUBLIC_TWO) Tj ET EMC\n";
    let input = tiny_pdf(
        "",
        "/Resources << /Font << /F1 5 0 R >> \
         /Properties << /P1 << /MCID 1 /Alt (SECRET_PROP_ALT) /Z << /E (SECRET_PROP_E) >> >> >> >>",
        content,
        &[],
    );
    let (plan, out) = export_with(&input, ExportPolicy::default(), vec![]).unwrap();
    assert_eq!(plan.report.removed[&SanitizedItem::Structure], 4);
    assert_eq!(loss(&plan, LossCode::AccessibilityRemoved), 4);
    let secrets = [
        "SECRET_ACTUAL",
        "SECRET_ALT",
        "SECRET_PROP_ALT",
        "SECRET_PROP_E",
    ];
    judge(
        "actualtext",
        &input,
        &out,
        &secrets,
        &["PUBLIC_VISIBLE", "PUBLIC_TWO"],
        &[],
    );
    // The tag and the harmless siblings stay.
    let (ok, c) = python(&[
        "-c",
        "import fitz,sys;print(fitz.open(sys.argv[1])[0].read_contents().decode())",
        common::tmp("sanitize-semantics", "actualtext.pdf")
            .to_str()
            .unwrap(),
    ]);
    assert!(
        ok && c.contains("/MCID 0") && c.contains("/Keep 1") && c.contains("/P1 BDC"),
        "{c}"
    );
}

#[test]
fn structure_tree_and_outlines_are_gone_with_their_secrets() {
    // Objects 6..9: StructTreeRoot, StructElem, Outlines, one outline item.
    let objs = [
        "<< /Type /StructTreeRoot /K 7 0 R >>".to_owned(),
        "<< /Type /StructElem /S /P /P 6 0 R /Alt (SECRET_STRUCT_ALT) /ActualText (SECRET_STRUCT_ACTUAL) >>"
            .to_owned(),
        "<< /Type /Outlines /First 9 0 R /Last 9 0 R /Count 1 >>".to_owned(),
        "<< /Title (SECRET_OUTLINE_TITLE) /Parent 8 0 R /Dest [3 0 R /Fit] >>".to_owned(),
    ];
    let input = tiny_pdf(
        "/StructTreeRoot 6 0 R /MarkInfo << /Marked true >> /Outlines 8 0 R",
        "/Resources << /Font << /F1 5 0 R >> >>",
        &show(0, 20, 380, "PUBLIC_VISIBLE"),
        &objs,
    );
    let (plan, out) = export_with(&input, ExportPolicy::default(), vec![]).unwrap();
    assert!(plan.report.removed[&SanitizedItem::Structure] >= 1);
    assert!(plan.report.removed[&SanitizedItem::Outlines] >= 1);
    judge(
        "structure",
        &input,
        &out,
        &[
            "SECRET_STRUCT_ALT",
            "SECRET_STRUCT_ACTUAL",
            "SECRET_OUTLINE_TITLE",
        ],
        &["PUBLIC_VISIBLE"],
        &[],
    );
    assert!(!has(&out, "StructTreeRoot") && !has(&out, "/Outlines"));
}
