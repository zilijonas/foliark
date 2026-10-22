//! Annotations, form widgets and signatures at export (spec §8.2; NEW-42). Inputs are corpus
//! fixtures or tiny hand-built files; outputs are judged by the independent oracles (PyMuPDF
//! render of input vs output with annotations on, objects.py for payloads and features) and the
//! report counts against a pikepdf count of the input.
mod common;
use common::{fidelity_oracle, fixture, objects_oracle, python, run, tmp};
use foliark_engine::{Document, Engine, RenderOptions};
use foliark_model::{
    AnnotationPolicy, ChangeSet, ErrorCode, ExportMode, ExportPlan, ExportPolicy, Limits, LossCode,
    PdfError, Rect, Redaction, SanitizedItem, SignedInputPolicy,
};

// ---- tiny PDFs -----------------------------------------------------------------------------------

fn stream(dict: &str, data: &str) -> String {
    format!(
        "<< {dict} /Length {} >>\nstream\n{data}\nendstream",
        data.len()
    )
}

/// An appearance Form drawing `text` (Helvetica) at (2, 4) in a `bbox` box.
fn form(bbox: &str, matrix: &str, text: &str) -> String {
    stream(
        &format!(
            "/Type /XObject /Subtype /Form /BBox [{bbox}] /Matrix [{matrix}] /Resources << /Font << /F1 5 0 R >> >>"
        ),
        &format!("0.9 0.9 0.6 rg 0 0 400 400 re f 0 g BT /F1 12 Tf 2 4 Td ({text}) Tj ET"),
    )
}

const ID: &str = "1 0 0 1 0 0";

/// 300 x 400 page showing KEEP_PUBLIC; objects 6.. are `bodies`, `annots` the /Annots array.
fn pdf(catalog: &str, annots: &str, bodies: &[String]) -> Vec<u8> {
    pdf_on(catalog, "", annots, bodies)
}

/// `pdf` with `page` (for example `/Rotate 90`) added to the page dictionary.
fn pdf_on(catalog: &str, page: &str, annots: &str, bodies: &[String]) -> Vec<u8> {
    let content = "BT /F1 12 Tf 20 370 Td (KEEP_PUBLIC) Tj ET";
    let mut objs = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {catalog} >>"),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [{annots}] {page} >>"
        ),
        stream("", content),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
    ];
    objs.extend(bodies.iter().cloned());
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut at = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        at.push(out.len());
        out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).bytes());
    for a in at {
        out.extend(format!("{a:010} 00000 n \n").bytes());
    }
    out.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objs.len() + 1
        )
        .bytes(),
    );
    out
}

// ---- driving ---------------------------------------------------------------------------------------

fn engine(bytes: &[u8]) -> (Engine, String) {
    let mut e = Engine::new(Limits::default());
    let (doc, _) = e.open(bytes.to_vec(), None, false).unwrap();
    (e, doc)
}

fn export(
    bytes: &[u8],
    redactions: Vec<Redaction>,
    policy: ExportPolicy,
) -> Result<(ExportPlan, Vec<u8>), PdfError> {
    let (mut e, doc) = engine(bytes);
    if !redactions.is_empty() {
        let t = e.apply_changes(
            &doc,
            ChangeSet {
                base_revision: 0,
                text_edits: None,
                redactions: Some(redactions),
            },
        );
        assert!(run(&mut e, t).unwrap().accepted);
    }
    let t = e.prepare_export(&doc, ExportMode::Native, policy);
    let plan = run(&mut e, t)?;
    let out = e.take_export(&doc, &plan.plan_id).unwrap();
    Ok((plan, out))
}

fn region(id: &str, x: f64, y: f64, width: f64, height: f64) -> Redaction {
    Redaction::Region {
        id: id.into(),
        page_index: 0,
        rect: Rect {
            x,
            y,
            width,
            height,
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

fn clean(dir: &str, name: &str, out: &[u8], forbid: &[&str]) {
    let (ok, log) = objects_oracle(dir, name, out, forbid);
    assert!(ok, "{name}: objects.py: {log}");
}

/// Annotations of `bytes` by /Subtype, counted by pikepdf.
fn count_annots(bytes: &[u8], subtype: &str) -> u32 {
    let p = tmp("sanitize-interactive", "count.pdf");
    std::fs::write(&p, bytes).unwrap();
    let code = "import sys, pikepdf\n\
        pdf = pikepdf.open(sys.argv[1])\n\
        print('COUNT', sum(1 for pg in pdf.pages for a in pg.obj.get('/Annots', []) if a.get('/Subtype') == sys.argv[2]))\n";
    let (ok, log) = python(&["-c", code, p.to_str().unwrap(), &format!("/{subtype}")]);
    assert!(ok, "{log}");
    log.lines()
        .find_map(|l| l.strip_prefix("COUNT "))
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

// ---- cases ---------------------------------------------------------------------------------------------

#[test]
fn appearance_states_alternates_and_the_bbox_mapping() {
    let forms = [
        form("0 0 80 20", ID, "STATE_ON"),
        form("0 0 80 20", ID, "UNSELECTED_OFF_STATE"),
        form("0 0 80 20", ID, "ALT_ROLLOVER"),
        form("0 0 80 20", ID, "ALT_DOWN"),
        form("0 0 80 20", ID, "OFF_BOX_DRAWN"),
        // Scaled 1.5 x 2 and offset; then turned a quarter (transformed box 100 x 20 at -20,0).
        form("0 0 100 20", ID, "SCALED_NOTE"),
        form("0 0 20 100", "0 1 -1 0 0 0", "ROT_NOTE"),
    ];
    let mut bodies: Vec<String> = forms.to_vec(); // objects 6..=12
    bodies.push(
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (cb1) /V /Yes /AS /Yes /Rect [20 300 100 320] /F 4 \
         /AP << /N << /Yes 6 0 R /Off 7 0 R >> /R << /Yes 8 0 R >> /D << /Yes 9 0 R >> >> >>"
            .into(),
    ); // 13
    bodies.push(
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (cb2) /V /Off /AS /Off /Rect [20 260 100 280] /F 4 \
         /AP << /N << /Yes 6 0 R /Off 10 0 R >> /R << /Off 8 0 R >> >> >>"
            .into(),
    ); // 14
    bodies.push(
        "<< /Type /Annot /Subtype /FreeText /Rect [150 200 250 240] /Contents (NOTE_TEXT_SECRET) /F 4 \
         /AP << /N 11 0 R >> >>"
            .into(),
    ); // 15
    bodies.push(
        "<< /Type /Annot /Subtype /Stamp /Rect [30 100 130 140] /F 4 /AP << /N 12 0 R >> >>".into(),
    ); // 16
    let input = pdf(
        "/AcroForm << /Fields [13 0 R 14 0 R] >>",
        "13 0 R 14 0 R 15 0 R 16 0 R",
        &bodies,
    );
    let (plan, out) = export(&input, vec![], ExportPolicy::default()).unwrap();

    let secrets = [
        "UNSELECTED_OFF_STATE",
        "ALT_ROLLOVER",
        "ALT_DOWN",
        "NOTE_TEXT_SECRET",
    ];
    clean("sanitize-interactive", "states.pdf", &out, &secrets);
    let (found, _) = objects_oracle("sanitize-interactive", "states.in.pdf", &input, &secrets);
    assert!(!found, "negative control: the payloads are in the input");
    // Only the selected state of each widget is drawn, and every drawn thing is where the input
    // renders it (annotations on): the 12.5.5 mapping for the scaled and the turned appearance.
    let keep = [
        "STATE_ON",
        "OFF_BOX_DRAWN",
        "SCALED_NOTE",
        "ROT_NOTE",
        "KEEP_PUBLIC",
    ];
    for k in keep {
        let (ok, log) = fidelity_oracle("sanitize-interactive", "states", &input, &out, &[], &[k]);
        assert!(ok, "{k}: {log}");
    }
    assert_eq!(
        (
            plan.report.flattened.annotations,
            plan.report.flattened.widgets
        ),
        (2, 2)
    );
    assert_eq!(
        plan.report.removed[&SanitizedItem::Widgets],
        count_annots(&input, "Widget")
    );
    assert_eq!(loss(&plan, LossCode::FormsFlattened), 2);
    assert_eq!(loss(&plan, LossCode::AnnotationsFlattened), 2);
    for key in ["/Annots", "/AcroForm", "/Widget"] {
        assert!(
            !out.windows(key.len()).any(|w| w == key.as_bytes()),
            "{key} survived"
        );
    }
}

#[test]
fn a_no_rotate_note_keeps_its_upper_left_corner_and_stands_upright_on_a_rotated_page() {
    // HDDN public-cropped-rotated (NEW-94): a /F 28 (NoRotate) note on a page turned 90, 180 or
    // 270 degrees is drawn upright with the rectangle's upper-left corner fixed; the plain note
    // beside it (/F 4) turns with the page. MuPDF renders the input with annotations on; the
    // appearances are asymmetric colour boxes (MuPDF's text extraction ignores NoRotate).
    let boxes = |w: &str, h: &str, small: &str| {
        stream(
            &format!("/Type /XObject /Subtype /Form /BBox [0 0 {w} {h}]"),
            &format!("1 0 0 rg 0 0 {w} {h} re f 0 0 1 rg 0 0 {small} re f"),
        )
    };
    let bodies = [
        boxes("80", "20", "20 10"),
        boxes("60", "30", "10 20"),
        "<< /Type /Annot /Subtype /Text /Rect [150 200 230 220] /F 28 /AP << /N 6 0 R >> >>".into(),
        "<< /Type /Annot /Subtype /Text /Rect [40 100 100 130] /F 4 /AP << /N 7 0 R >> >>".into(),
    ];
    for rotate in [90, 180, 270] {
        let input = pdf_on("", &format!("/Rotate {rotate}"), "8 0 R 9 0 R", &bodies);
        let (_, out) = export(&input, vec![], ExportPolicy::default()).unwrap();
        let name = format!("norotate{rotate}");
        let (ok, log) = fidelity_oracle(
            "sanitize-interactive",
            &name,
            &input,
            &out,
            &[],
            &["KEEP_PUBLIC"],
        );
        assert!(ok, "rotate {rotate}: {log}");
        // The raster renderer (image-only export) draws the input's notes where flattening put
        // them: both pages show the same pixels.
        let draw = |bytes: &[u8]| {
            let d = Document::open(bytes.to_vec(), None).unwrap();
            d.render_page(0, RenderOptions::at(1.0)).unwrap().data
        };
        let (a, b) = (draw(&input), draw(&out));
        let off = a
            .iter()
            .zip(&b)
            .filter(|(x, y)| x.abs_diff(**y) > 32)
            .count();
        assert!(
            off < a.len() / 1000,
            "rotate {rotate}: {off} samples differ"
        );
    }
}

#[test]
fn identical_notes_on_two_pages_stay_separate_objects_when_one_is_redacted() {
    // HDDN public-cropped-rotated (NEW-94): flattening draws each note's appearance as its own
    // Form. Two pages whose appearances are byte-equal must not merge into one object in the
    // staged copy, or a redaction on one page would count as a shared survivor on the other and
    // refuse the native export.
    let note = form("0 0 80 20", ID, "SAME_NOTE");
    let objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R 8 0 R] /Count 2 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [7 0 R] >>".to_owned(),
        stream("", "BT /F1 12 Tf 20 370 Td (KEEP_PUBLIC) Tj ET"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
        note.clone(),
        "<< /Type /Annot /Subtype /FreeText /Rect [20 300 100 320] /F 4 /AP << /N 6 0 R >> >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [10 0 R] >>".to_owned(),
        note,
        "<< /Type /Annot /Subtype /FreeText /Rect [20 300 100 320] /F 4 /AP << /N 9 0 R >> >>".to_owned(),
    ];
    let mut input = b"%PDF-1.7\n".to_vec();
    let mut at = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        at.push(input.len());
        input.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
    }
    let xref = input.len();
    input.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).bytes());
    for a in at {
        input.extend(format!("{a:010} 00000 n \n").bytes());
    }
    input.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objs.len() + 1
        )
        .bytes(),
    );
    let (plan, _) = export(
        &input,
        vec![region("r1", 20.0, 80.0, 80.0, 20.0)],
        ExportPolicy::default(),
    )
    .unwrap();
    assert!(plan.report.redaction.shared_survivors.is_empty());
}

#[test]
fn field_values_stay_visible_and_the_form_is_gone() {
    let input = fixture("generated/acroform-values.pdf");
    let (plan, out) = export(&input, vec![], ExportPolicy::default()).unwrap();
    // The visible value is page content now; the name, the default and the machinery are not.
    clean(
        "sanitize-interactive",
        "values.pdf",
        &out,
        &["SECRET_FORM_NAME", "SECRET_FORM_DEFAULT"],
    );
    let (ok, log) = fidelity_oracle(
        "sanitize-interactive",
        "values",
        &input,
        &out,
        &[],
        &["SECRET_FORM_VALUE", "KEEP_PUBLIC_FORM"],
    );
    assert!(ok, "{log}");
    assert_eq!(
        plan.report.flattened.widgets,
        count_annots(&input, "Widget")
    );
    assert_eq!(loss(&plan, LossCode::FormsFlattened), 1);
}

#[test]
fn redaction_over_a_flattened_widget_or_note_removes_the_appearance_text() {
    let input = fixture("generated/acroform-values.pdf");
    // Display space (top left): the widget rect of the manifest.
    let (plan, out) = export(
        &input,
        vec![region("w", 20.0, 70.0, 200.0, 30.0)],
        ExportPolicy::default(),
    )
    .unwrap();
    clean(
        "sanitize-interactive",
        "values-redacted.pdf",
        &out,
        &[
            "SECRET_FORM_VALUE",
            "SECRET_FORM_NAME",
            "SECRET_FORM_DEFAULT",
        ],
    );
    let (ok, log) = fidelity_oracle(
        "sanitize-interactive",
        "values-redacted",
        &input,
        &out,
        &[(0, 20.0, 70.0, 220.0, 100.0)],
        &["KEEP_PUBLIC_FORM"],
    );
    assert!(ok, "{log}");
    assert!(plan.report.redaction.clusters_removed >= 1);

    // A comment away from the region is flattened and unchanged outside it.
    let input = fixture("generated/annot-appearance.pdf");
    let away = (150.0, 300.0, 100.0, 50.0);
    let (plan, out) = export(
        &input,
        vec![region("blank", away.0, away.1, away.2, away.3)],
        ExportPolicy::default(),
    )
    .unwrap();
    clean(
        "sanitize-interactive",
        "note-away.pdf",
        &out,
        &[
            "SECRET_ANNOT_CONTENTS",
            "SECRET_ANNOT_NAME",
            "SECRET_ANNOT_AUTHOR",
            "SECRET_LINK_URI",
        ],
    );
    let (ok, log) = fidelity_oracle(
        "sanitize-interactive",
        "note-away",
        &input,
        &out,
        &[(0, away.0, away.1, away.0 + away.2, away.1 + away.3)],
        &["SECRET_ANNOT_TEXT", "KEEP_PUBLIC_ANNOT"],
    );
    assert!(ok, "{log}");
    assert_eq!(plan.report.flattened.annotations, 1);

    // Over the note: its text is redacted like page text.
    let (_, out) = export(
        &input,
        vec![region("note", 20.0, 80.0, 180.0, 40.0)],
        ExportPolicy::default(),
    )
    .unwrap();
    clean(
        "sanitize-interactive",
        "note-over.pdf",
        &out,
        &["SECRET_ANNOT_TEXT", "SECRET_ANNOT_CONTENTS"],
    );
}

#[test]
fn remove_policy_drops_notes_but_widgets_are_still_flattened() {
    let remove = ExportPolicy {
        annotations: AnnotationPolicy::Remove,
        ..ExportPolicy::default()
    };
    let input = fixture("generated/annot-appearance.pdf");
    let (plan, out) = export(&input, vec![], remove).unwrap();
    clean(
        "sanitize-interactive",
        "remove.pdf",
        &out,
        &["SECRET_ANNOT_TEXT"],
    );
    assert_eq!(plan.report.flattened.annotations, 0);
    assert_eq!(loss(&plan, LossCode::AnnotationsRemoved), 1);
    let (plan, _) = export(&fixture("generated/acroform-values.pdf"), vec![], remove).unwrap();
    assert_eq!(plan.report.flattened.widgets, 1);
}

#[test]
fn a_value_nothing_shows_refuses_and_names_the_fields() {
    let widget = |extra: &str| {
        format!(
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) {extra} /Rect [20 300 120 320] /F 4 >>"
        )
    };
    let refused = |catalog: &str, bodies: &[String]| {
        let input = pdf(catalog, "6 0 R", bodies);
        let err = export(&input, vec![], ExportPolicy::default()).unwrap_err();
        let d = err.details.clone().unwrap();
        (err.code, d.fields, d.pages)
    };
    let acro = "/AcroForm << /Fields [6 0 R] >>";
    // A value and no appearance, in either policy.
    let got = refused(acro, &[widget("/V (typed)")]);
    assert_eq!(
        got,
        (
            ErrorCode::FormAppearanceMissing,
            Some(vec!["Name".into()]),
            Some(vec![0])
        )
    );
    // An appearance that cannot be placed (a selected state it lacks) is no appearance.
    let ap = "/V /Yes /AS /Yes /AP << /N << /No 7 0 R >> >>";
    assert_eq!(
        refused(acro, &[widget(ap), form("0 0 80 20", ID, "x")]).0,
        ErrorCode::FormAppearanceMissing
    );
    // NeedAppearances: the stored appearance of a valued field may be stale.
    let stale = "/AcroForm << /Fields [6 0 R] /NeedAppearances true >>";
    let with_ap = widget("/V (typed) /AP << /N 7 0 R >>");
    assert_eq!(
        refused(stale, &[with_ap, form("0 0 100 20", ID, "typed")]).0,
        ErrorCode::FormAppearanceMissing
    );
    // Nothing hidden: empty fields, an off checkbox and a hidden widget export.
    for body in [
        widget(""),
        widget("/V /Off /AS /Off"),
        widget("/V (hidden)").replace("/F 4", "/F 2"),
    ] {
        let input = pdf(acro, "6 0 R", &[body]);
        let (plan, out) = export(&input, vec![], ExportPolicy::default()).unwrap();
        assert_eq!(plan.report.flattened.widgets, 0);
        clean("sanitize-interactive", "empty.pdf", &out, &["hidden"]);
    }
}

#[test]
fn xfa_alone_is_unsupported_and_xfa_beside_appearances_is_dropped() {
    let xfa_only = pdf(
        "/AcroForm << /Fields [] /XFA [(template) 6 0 R] >>",
        "",
        &[stream("", "<xdp/>")],
    );
    let err = export(&xfa_only, vec![], ExportPolicy::default()).unwrap_err();
    assert_eq!(err.code, ErrorCode::XfaUnsupported);

    let widget = "<< /Type /Annot /Subtype /Widget /FT /Tx /T (A) /V (xfa shown) /Rect [20 300 120 320] /F 4 /AP << /N 8 0 R >> >>";
    let both = pdf(
        "/AcroForm << /Fields [7 0 R] /XFA [(template) 6 0 R] >>",
        "7 0 R",
        &[
            stream("", "<xdp>XFA_PAYLOAD</xdp>"),
            widget.into(),
            form("0 0 100 20", ID, "xfa shown"),
        ],
    );
    let (_, out) = export(&both, vec![], ExportPolicy::default()).unwrap();
    // objects.py also flags a surviving /XFA or /AcroForm as a feature.
    clean("sanitize-interactive", "xfa.pdf", &out, &["XFA_PAYLOAD"]);
}

#[test]
fn signatures_are_stripped_with_the_appearance_kept_or_refused() {
    let sig = "<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached /ByteRange [0 10 20 30] \
               /Contents <00AA00AA> /Name (SIGNER_NAME_SECRET) >>";
    let field = "<< /Type /Annot /Subtype /Widget /FT /Sig /T (Sig1) /V 6 0 R /Rect [20 100 140 140] /F 4 /AP << /N 7 0 R >> >>";
    let input = pdf(
        "/AcroForm << /Fields [8 0 R] /SigFlags 3 >>",
        "8 0 R",
        &[
            sig.into(),
            form("0 0 120 40", ID, "SIGNED_BY_ALICE"),
            field.into(),
        ],
    );
    let (plan, out) = export(&input, vec![], ExportPolicy::default()).unwrap();
    clean(
        "sanitize-interactive",
        "signed.pdf",
        &out,
        &["SIGNER_NAME_SECRET"],
    );
    assert!(
        !out.windows(9).any(|w| w == b"ByteRange"),
        "signature dictionary survived"
    );
    let (ok, log) = fidelity_oracle(
        "sanitize-interactive",
        "signed",
        &input,
        &out,
        &[],
        &["SIGNED_BY_ALICE"],
    );
    assert!(ok, "{log}");
    assert_eq!(loss(&plan, LossCode::SignaturesInvalidated), 1);
    assert_eq!(plan.report.flattened.widgets, 1);

    let refuse = ExportPolicy {
        signed_input: SignedInputPolicy::Refuse,
        ..ExportPolicy::default()
    };
    assert_eq!(
        export(&input, vec![], refuse).unwrap_err().code,
        ErrorCode::SignedInput
    );
}
