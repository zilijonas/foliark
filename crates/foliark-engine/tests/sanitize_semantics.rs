//! Searchable OCR text and semantic tags at export (spec §5 rules 1 and 6, §8.2; NEW-44). Tiny
//! hand-built pages: an invisible OCR layer (render mode 3 and 7) next to visible text. Outputs
//! are judged by Foliark's text, by the independent oracles (objects.py: payloads in decoded
//! streams and strings, with the input as the negative control; fidelity.py: pixels, words and
//! PyMuPDF/pypdf extraction) and by PyMuPDF's own search.
mod common;
use common::{fidelity_oracle, has, objects_oracle, python, run, stream, tiny_pdf};
use foliark_engine::{Document, Engine};
use foliark_model::{
    CapabilityStatus, ChangeSet, ErrorCode, ExportMode, ExportPlan, ExportPolicy, ExtractOptions,
    FontSource, FontStyle, InvisibleTextPolicy, Limits, LossCode, PdfError, Rect, Redaction,
    SanitizedItem, TextEdit,
};

fn export_with(
    bytes: &[u8],
    policy: ExportPolicy,
    redactions: Vec<Redaction>,
) -> Result<(ExportPlan, Vec<u8>), PdfError> {
    export_edited(bytes, policy, vec![], redactions)
}

fn export_edited(
    bytes: &[u8],
    policy: ExportPolicy,
    edits: Vec<TextEdit>,
    redactions: Vec<Redaction>,
) -> Result<(ExportPlan, Vec<u8>), PdfError> {
    let mut e = Engine::new(Limits::default());
    if !edits.is_empty() {
        e.register_fonts(vec![FontSource {
            bytes: std::fs::read(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../foliark-text/tests/data/LiberationSans-Regular.ttf"
            ))
            .unwrap(),
            family: "Liberation Sans".into(),
            weight: 400,
            style: FontStyle::Normal,
        }])
        .unwrap();
    }
    let (doc, _) = e.open(bytes.to_vec(), None, false).unwrap();
    if !redactions.is_empty() || !edits.is_empty() {
        let changes = ChangeSet {
            base_revision: 0,
            text_edits: (!edits.is_empty()).then_some(edits),
            redactions: (!redactions.is_empty()).then_some(redactions),
        };
        let t = e.apply_changes(&doc, changes);
        let r = run(&mut e, t).unwrap();
        assert!(r.accepted, "{:?}", r.rejected);
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
fn a_zero_width_combining_mark_belongs_to_its_base_and_never_blocks_a_region() {
    // iOS Quartz writes "a" + U+0328 as two glyphs, the mark with advance 0: its box has no area, which
    // used to refuse every region on the page (textPositionUnknown, NEW-102). The mark sits where its
    // base sits, so redacting the base takes the mark, and an untouched word keeps its own.
    let input = common::fixture("generated/quartz-type0-combining-marks.pdf");
    let (secret, keep) = ("SECRET_A\u{328}_E\u{301}X", "KEEP_E\u{301}_MARK");
    let doc = Document::open(input.clone(), None).unwrap();
    let before = doc.extract_text(0, &ExtractOptions::default()).unwrap();
    assert!(
        before.text.contains(secret) && before.text.contains(keep),
        "{:?}",
        before.text
    );
    let (_, out) = export_with(
        &input,
        ExportPolicy::default(),
        vec![region("r", (19.0, 90.0, 135.0, 104.0))],
    )
    .unwrap();
    let t = words(&out);
    assert!(t.contains("KEEP_PUBLIC_MARKS") && t.contains(keep), "{t}");
    assert!(!t.contains("SECRET") && !t.contains('\u{328}'), "{t}");
    assert_eq!(t.matches('\u{301}').count(), 1, "{t}");
    let (ok, log) = objects_oracle("sanitize-semantics", "marks.pdf", &out, &[secret]);
    assert!(ok, "marks: objects.py: {log}");
    let (ok, _) = objects_oracle("sanitize-semantics", "marks.in.pdf", &input, &[secret]);
    assert!(!ok, "marks: negative control, the secret is in the input");
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
    // The tag, structure id and layer name stay; other inline keys go (ADR 0006).
    let (ok, c) = python(&[
        "-c",
        "import fitz,sys;print(fitz.open(sys.argv[1])[0].read_contents().decode())",
        common::tmp("sanitize-semantics", "actualtext.pdf")
            .to_str()
            .unwrap(),
    ]);
    assert!(
        ok && c.contains("/MCID 0") && !c.contains("/Keep") && c.contains("/P1 BDC"),
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

// ---- keepOcr (ADR 0008): OCR is invisible text over a drawn image --------------------------------

fn keep_ocr() -> ExportPolicy {
    ExportPolicy {
        invisible_text: InvisibleTextPolicy::KeepOcr,
        ..ExportPolicy::default()
    }
}

/// Object 6 is a 150 x 100 gray image (`/Im1`, 2 pt a pixel once drawn on 300 x 200); `more` follow as objects 7.. (`/Fm1` is 7), `extra`
/// adds page entries.
fn scan(extra: &str, content: &str, more: &[String]) -> Vec<u8> {
    let pixels: String = (0..150 * 100)
        .map(|i| format!("{:02X}", 0xB0 + i % 64))
        .collect();
    let mut objs = vec![stream(
        "/Type /XObject /Subtype /Image /Width 150 /Height 100 /ColorSpace /DeviceGray \
         /BitsPerComponent 8 /Filter /ASCIIHexDecode",
        &format!("{pixels}>"),
    )];
    objs.extend(more.iter().cloned());
    let form = if more.is_empty() { "" } else { "/Fm1 7 0 R" };
    tiny_pdf(
        "",
        &format!("/Resources << /Font << /F1 5 0 R >> /XObject << /Im1 6 0 R {form} >> >> {extra}"),
        content,
        &objs,
    )
}

fn form_object(content: &str) -> String {
    stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 300 400] /Resources << /Font << /F1 5 0 R >> >>",
        content,
    )
}

/// The image covers the upper half of the 300 x 400 page: user y 200 to 400.
const UPPER: &str = "q 300 0 0 200 0 200 cm /Im1 Do Q\n";

fn kept_words(out: &[u8], name: &str, keep: &[&str], gone: &[&str]) {
    let t = words(out);
    for k in keep {
        assert!(t.contains(k), "{name}: {k} missing: {t}");
        assert!(
            fitz_hits(name, out, k) >= 1,
            "{name}: PyMuPDF cannot find {k}"
        );
    }
    for g in gone {
        assert!(!t.contains(g), "{name}: {g} left: {t}");
        assert_eq!(fitz_hits(name, out, g), 0, "{name}: PyMuPDF finds {g}");
    }
}

#[test]
fn keep_ocr_keeps_scan_text_outside_regions_and_removes_the_redacted_word() {
    let mut c = String::from(UPPER);
    c += &show(3, 20, 360, "KEEP_OCR_TOP");
    c += &show(3, 20, 300, "SECRET_OCR_REGION");
    c += &show(7, 20, 240, "KEEP_OCR_SEVEN");
    let input = scan("", &c, &[]);
    let row = line(20.0, 300.0);
    let (plan, out) = export_with(&input, keep_ocr(), vec![region("a", row)]).unwrap();
    kept_words(
        &out,
        "scan",
        &["KEEP_OCR_TOP", "KEEP_OCR_SEVEN"],
        &["SECRET"],
    );
    assert_eq!(plan.report.kept.invisible_text_clusters, 12 + 14);
    assert!(
        !plan
            .report
            .removed
            .contains_key(&SanitizedItem::InvisibleText)
    );
    assert_eq!(loss(&plan, LossCode::TextSearchLost), 0);
    judge(
        "scan",
        &input,
        &out,
        &["SECRET_OCR_REGION"],
        &["KEEP_OCR_TOP", "KEEP_OCR_SEVEN"],
        // The image loses whole 2 pt pixels under the region.
        &[(row.0 - 3.0, row.1 - 3.0, row.2 + 3.0, row.3 + 3.0)],
    );
}

#[test]
fn keep_ocr_removes_invisible_payload_that_is_not_over_the_image() {
    let mut c = String::from(UPPER);
    c += &show(3, 20, 300, "KEEP_OCR_SCAN");
    c += &show(3, 20, 100, "SECRET_BLANK_AREA");
    // Straddles the image's lower edge (user y 200): half of the glyph box is over it.
    c += &show(7, 20, 197, "SECRET_STRADDLE");
    // Inside the image on the page, but painted off the page: removed as off-page content.
    c += &show(3, 20, 500, "SECRET_OFF_PAGE");
    let input = scan("", &c, &[]);
    let (plan, out) = export_with(&input, keep_ocr(), vec![]).unwrap();
    kept_words(
        &out,
        "blank",
        &["KEEP_OCR_SCAN"],
        &["SECRET_BLANK", "SECRET_STRADDLE", "SECRET_OFF"],
    );
    let n = 17 + 15;
    assert_eq!(plan.report.removed[&SanitizedItem::InvisibleText], n);
    assert_eq!(loss(&plan, LossCode::TextSearchLost), n);
    assert_eq!(plan.report.kept.invisible_text_clusters, 13);
    judge(
        "blank",
        &input,
        &out,
        &["SECRET_BLANK_AREA", "SECRET_STRADDLE", "SECRET_OFF_PAGE"],
        &["KEEP_OCR_SCAN"],
        &[line(20.0, 100.0), line(20.0, 197.0), line(20.0, 500.0)],
    );
    // `remove` loses the scan's own layer too: the difference the new value makes.
    let remove = ExportPolicy {
        invisible_text: InvisibleTextPolicy::Remove,
        ..ExportPolicy::default()
    };
    let (_, all_gone) = export_with(&input, remove, vec![]).unwrap();
    assert!(!words(&all_gone).contains("KEEP_OCR_SCAN"));
}

#[test]
fn keep_ocr_text_without_a_place_on_the_page_is_not_kept() {
    // Font size 0 and a singular matrix put a glyph nowhere: it may be anything.
    let mut c = String::from(UPPER);
    c += &show(3, 20, 300, "KEEP_OCR_SCAN");
    c += "q 0 0 0 0 20 260 cm BT 3 Tr /F1 12 Tf (SECRET_NOWHERE) Tj ET Q\n";
    c += "BT 3 Tr /F1 0 Tf 20 240 Td (SECRET_SIZE_ZERO) Tj ET\n";
    let input = scan("", &c, &[]);
    let (_, out) = export_with(&input, keep_ocr(), vec![region("a", line(20.0, 300.0))]).unwrap();
    kept_words(
        &out,
        "nowhere",
        &[],
        &["SECRET_NOWHERE", "SECRET_SIZE", "KEEP_OCR_SCAN"],
    );
    let (_, out) = export_with(&input, keep_ocr(), vec![]).unwrap();
    kept_words(
        &out,
        "nowhere2",
        &["KEEP_OCR_SCAN"],
        &["SECRET_NOWHERE", "SECRET_SIZE"],
    );
}

#[test]
fn keep_ocr_holds_on_a_rotated_cropped_scan() {
    // /Rotate 90 and a CropBox that cuts the page: user (x, y) shows at (y - 100, x). The image
    // covers user y 200 to 400 (display x 100 to 300).
    let mut c = String::from(UPPER);
    c += &show(3, 20, 300, "KEEP_OCR_ROT");
    c += &show(3, 20, 250, "SECRET_ROT_REGION");
    c += &show(3, 20, 150, "SECRET_ROT_BLANK");
    c += &show(7, 20, 50, "SECRET_ROT_CROPPED");
    let input = scan("/Rotate 90 /CropBox [0 100 300 400]", &c, &[]);
    let r = (96.0, 10.0, 160.0, 140.0);
    let (_, out) = export_with(&input, keep_ocr(), vec![region("a", r)]).unwrap();
    kept_words(
        &out,
        "rot",
        &["KEEP_OCR_ROT"],
        &[
            "SECRET_ROT_REGION",
            "SECRET_ROT_BLANK",
            "SECRET_ROT_CROPPED",
        ],
    );
    let (ok, log) = objects_oracle(
        "sanitize-semantics",
        "rot.pdf",
        &out,
        &[
            "SECRET_ROT_REGION",
            "SECRET_ROT_BLANK",
            "SECRET_ROT_CROPPED",
        ],
    );
    assert!(ok, "rot: objects.py: {log}");
}

#[test]
fn keep_ocr_decides_a_shared_form_for_every_place_it_is_drawn() {
    // One Form (object 7) with an OCR word, drawn over the image and over blank paper: the
    // operator is one object, so what removes it in one place removes it in both.
    let form = form_object("BT 3 Tr /F1 12 Tf 20 10 Td (SHARED_OCR_WORD) Tj ET");
    let both = format!(
        "{UPPER}q 1 0 0 1 0 300 cm /Fm1 Do Q\nq 1 0 0 1 0 40 cm /Fm1 Do Q\n{}",
        show(3, 20, 360, "KEEP_OCR_PAGE")
    );
    let (plan, out) = export_with(
        &scan("", &both, std::slice::from_ref(&form)),
        keep_ocr(),
        vec![],
    )
    .unwrap();
    kept_words(&out, "form-both", &["KEEP_OCR_PAGE"], &["SHARED_OCR"]);
    assert_eq!(plan.report.removed[&SanitizedItem::InvisibleText], 15);
    // Over the image in every place: kept.
    let over = format!("{UPPER}q 1 0 0 1 0 300 cm /Fm1 Do Q\nq 1 0 0 1 0 220 cm /Fm1 Do Q\n");
    let (_, out) = export_with(&scan("", &over, &[form]), keep_ocr(), vec![]).unwrap();
    kept_words(&out, "form-over", &["SHARED_OCR_WORD"], &[]);
}

#[test]
fn keep_ocr_survives_a_paragraph_edit_and_still_honors_regions() {
    // Native text below a scan that carries OCR is edited; one OCR row is redacted.
    let mut c = String::from(UPPER);
    c += "BT /F1 10 Tf 20 100 Td (OLD_VISIBLE_WORDS_IN_A_LONG_ROW) Tj ET\nBT /F1 10 Tf 20 86 Td (second line of it) Tj ET\n";
    c += &show(3, 20, 320, "KEEP_OCR_BESIDE");
    c += &show(3, 20, 280, "SECRET_OCR_EDIT");
    let input = scan("", &c, &[]);
    let edit = TextEdit {
        id: "e".into(),
        page_index: 0,
        rects: vec![Rect {
            x: 10.0,
            y: 285.0,
            width: 150.0,
            height: 37.0,
        }],
        text: "NEW_VISIBLE_WORDS".into(),
        font: None,
    };
    let row = line(20.0, 280.0);
    let (_, out) = export_edited(&input, keep_ocr(), vec![edit], vec![region("a", row)]).unwrap();
    kept_words(
        &out,
        "edit",
        &["NEW_VISIBLE_WORDS", "KEEP_OCR_BESIDE"],
        &["OLD_VISIBLE", "SECRET_OCR_EDIT"],
    );
}
