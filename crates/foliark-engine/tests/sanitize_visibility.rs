//! Hidden layers, clipped-out and off-page content at export (spec §8.2; NEW-43). Inputs are
//! corpus fixtures or tiny hand-built files; outputs are judged by Foliark's own text, by the
//! independent oracles (objects.py: payloads in any stream or string, `/OCProperties`;
//! fidelity.py: PyMuPDF renders the input with its default layer configuration, which is the
//! one flattening applies) and by byte searches for resources that must not survive.
mod common;
use common::{fidelity_oracle, fixture, objects_oracle, run};
use foliark_engine::{Document, Engine};
use foliark_model::{
    ChangeSet, ErrorCode, ExportMode, ExportPlan, ExportPolicy, ExtractOptions, Limits, LossCode,
    PdfError, Rect, Redaction, SanitizedItem,
};

// ---- tiny PDFs -----------------------------------------------------------------------------------

fn stream(dict: &str, data: &str) -> String {
    format!(
        "<< {dict} /Length {} >>\nstream\n{data}\nendstream",
        data.len()
    )
}

/// A Form drawing `body` with Helvetica `/F1` and the layer groups 10 (on) and 11 (off) as
/// `/g1`, `/g2`.
fn form(extra: &str, bbox: &str, body: &str) -> String {
    stream(
        &format!(
            "/Type /XObject /Subtype /Form /BBox [{bbox}] {extra} /Resources << /Font << /F1 5 0 R >> \
             /Properties << /g1 10 0 R /g2 11 0 R >> >>"
        ),
        body,
    )
}

fn text(x: u32, y: u32, s: &str) -> String {
    format!("BT /F1 12 Tf {x} {y} Td ({s}) Tj ET\n")
}

/// Objects 1.. in order (1 = catalog, 2 = pages, 3 = the page, 4 = its content, 5 = Helvetica);
/// `objs` gives objects 6.. (use `null` for a gap).
fn pdf(catalog: &str, page_extra: &str, content: &str, objs: &[String]) -> Vec<u8> {
    let mut all = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {catalog} >>"),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R {page_extra} >>"
        ),
        stream("", content),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
    ];
    all.extend(objs.iter().cloned());
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut at = Vec::new();
    for (i, o) in all.iter().enumerate() {
        at.push(out.len());
        out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", all.len() + 1).bytes());
    for a in at {
        out.extend(format!("{a:010} 00000 n \n").bytes());
    }
    out.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            all.len() + 1
        )
        .bytes(),
    );
    out
}

/// `/Resources` of a page with Helvetica, the properties of groups 10 / 11 and `more`.
fn resources(props: &str, more: &str) -> String {
    format!(
        "/Resources << /Font << /F1 5 0 R >> /Properties << /g1 10 0 R /g2 11 0 R {props} >> {more} >>"
    )
}

const GROUPS: [&str; 2] = [
    "<< /Type /OCG /Name (LAYER_ON) >>",
    "<< /Type /OCG /Name (SECRET_NAME_OFF) >>",
];

// ---- driving ---------------------------------------------------------------------------------------

fn export(bytes: &[u8]) -> Result<(ExportPlan, Vec<u8>), PdfError> {
    let mut e = Engine::new(Limits::default());
    let (doc, _) = e.open(bytes.to_vec(), None, false).unwrap();
    let t = e.prepare_export(&doc, ExportMode::Native, ExportPolicy::default());
    let plan = run(&mut e, t)?;
    let out = e.take_export(&doc, &plan.plan_id).unwrap();
    Ok((plan, out))
}

/// `export` with `redactions` committed first.
fn export_redacted(
    bytes: &[u8],
    redactions: impl FnOnce(&str) -> Vec<Redaction>,
) -> (ExportPlan, Vec<u8>) {
    let mut e = Engine::new(Limits::default());
    let (doc, _) = e.open(bytes.to_vec(), None, false).unwrap();
    let t = e.extract_text(&doc, 0, Default::default());
    let text = run(&mut e, t).unwrap().text;
    let changes = ChangeSet {
        base_revision: 0,
        text_edits: None,
        redactions: Some(redactions(&text)),
    };
    let t = e.apply_changes(&doc, changes);
    assert!(run(&mut e, t).unwrap().accepted);
    let t = e.prepare_export(&doc, ExportMode::Native, ExportPolicy::default());
    let plan = run(&mut e, t).unwrap();
    let out = e.take_export(&doc, &plan.plan_id).unwrap();
    (plan, out)
}

fn words(bytes: &[u8]) -> String {
    Document::open(bytes.to_vec(), None)
        .unwrap()
        .extract_text(0, &ExtractOptions::default())
        .unwrap()
        .text
}

fn has(bytes: &[u8], needle: &str) -> bool {
    bytes.windows(needle.len()).any(|w| w == needle.as_bytes())
}

/// Display rect of a 12 pt line whose baseline starts at user `(x, y)` of the 300 x 400 test page.
fn line(x: f64, y: f64) -> (f64, f64, f64, f64) {
    (x - 2.0, 400.0 - y - 13.0, x + 250.0, 400.0 - y + 4.0)
}

fn loss(p: &ExportPlan, code: LossCode) -> u32 {
    p.report
        .losses
        .iter()
        .find(|l| l.code == code)
        .map_or(0, |l| l.count)
}

/// The output is free of `forbid` for the object oracle, the input is not (the negative control),
/// and every `keep` string is extractable by two parsers.
///
/// `changed` are display-space rects `(x0, y0, x1, y1)` of the removed text: pypdf reads the input
/// without its layer configuration, so only those lines may differ (MuPDF, the pixel check and
/// the word check, already render the input with the default configuration).
fn judge(
    name: &str,
    input: &[u8],
    out: &[u8],
    forbid: &[&str],
    keep: &[&str],
    changed: &[(f64, f64, f64, f64)],
) {
    let (ok, log) = objects_oracle("sanitize-visibility", &format!("{name}.pdf"), out, forbid);
    assert!(ok, "{name}: objects.py: {log}");
    let (ok, _) = objects_oracle(
        "sanitize-visibility",
        &format!("{name}.in.pdf"),
        input,
        forbid,
    );
    assert!(
        !ok,
        "{name}: negative control, the payloads are in the input"
    );
    let changed: Vec<_> = changed.iter().map(|c| (0, c.0, c.1, c.2, c.3)).collect();
    let (ok, log) = fidelity_oracle("sanitize-visibility", name, input, out, &changed, keep);
    assert!(ok, "{name}: fidelity.py: {log}");
}

// ---- cases -----------------------------------------------------------------------------------------

#[test]
fn the_two_layer_fixtures_export_and_the_hidden_layer_is_gone() {
    for (rel, forbid, keep, changed, layers) in [
        (
            "generated/ocg-hidden-text.pdf",
            ["SECRET_HIDDEN_OCG", "SECRET_LAYER_OFF_NAME"],
            ["KEEP_PUBLIC_OCG", "KEEP_PUBLIC_ON_LAYER"],
            (18.0, 106.0, 155.0, 126.0),
            2,
        ),
        (
            "hddn/optional-content-layer.pdf",
            ["SECRET_LAYER_BODY", "SECRET_LAYER_NAME"],
            [
                "KEEP_PUBLIC_OPTIONAL_CONTENT_LAYER",
                "SECRET_OPTIONAL_CONTENT_LAYER",
            ],
            (70.0, 226.0, 300.0, 246.0),
            1,
        ),
    ] {
        let input = fixture(rel);
        let (plan, out) = export(&input).unwrap();
        let name = rel.replace('/', "-");
        // /OCProperties is a FEATURE finding of objects.py, so it is covered by this call too.
        judge(&name, &input, &out, &forbid, &keep, &[changed]);
        let t = words(&out);
        assert!(
            keep.iter().all(|k| t.contains(k)) && !t.contains(forbid[0]),
            "{rel}: {t}"
        );
        assert_eq!(plan.report.flattened.ocg_layers, layers);
        assert!(plan.report.removed[&SanitizedItem::OcgContent] >= 1);
        assert_eq!(loss(&plan, LossCode::LayersFlattened), layers);
    }
}

#[test]
fn default_state_nesting_and_hidden_runs_are_resolved_where_the_page_looks_the_same() {
    // BaseState OFF with group 10 ON: 11 and 12 are off.
    let catalog = "/OCProperties << /OCGs [10 0 R 11 0 R 12 0 R] \
                   /D << /BaseState /OFF /ON [10 0 R] >> >>";
    let mut objs: Vec<String> = vec!["null".into(); 4];
    objs.extend(GROUPS.map(str::to_owned));
    objs.push("<< /Type /OCG /Name (THIRD_OFF) >>".into()); // 12
    let group = |name: &str, s: &str, y: u32| format!("/OC /{name} BDC {} EMC\n", text(20, y, s));
    let mut content = text(20, 380, "ALWAYS_SHOWN");
    content += &group("g1", "SHOW_G1", 360);
    content += &group("g2", "HIDE_G2", 340);
    content += &group("g3", "HIDE_G3", 320);
    // Nested: an OFF group inside an ON one hides only the inner part.
    content += "/OC /g1 BDC /OC /g2 BDC BT /F1 12 Tf 20 300 Td (HIDE_NESTED) Tj ET EMC ";
    content += &format!("{}EMC\n", text(20, 280, "SHOW_AFTER_NESTED"));
    // A hidden show inside a text object must not move the shown one after it (trailing spaces
    // keep the two apart so the hidden word can be the only changed region).
    content += "BT /F1 12 Tf 20 250 Td /OC /g2 BDC (HIDE_RUN  ) Tj EMC (SHOWN_AFTER) Tj ET\n";
    // Hidden paint and inline image; the shown rectangle keeps its pixels (fidelity.py).
    content += "/OC /g2 BDC 1 0 0 rg 150 100 80 30 re f q 20 0 0 20 150 60 cm \
                BI /W 1 /H 1 /CS /G /BPC 8 /F /AHx ID 00> EI Q EMC\n";
    content += "/OC /g1 BDC 0 0 1 rg 20 40 80 30 re f EMC\n";
    let props = "/g3 12 0 R";
    let input = pdf(catalog, &resources(props, ""), &content, &objs);
    let (plan, out) = export(&input).unwrap();

    let t = words(&out);
    for shown in [
        "ALWAYS_SHOWN",
        "SHOW_G1",
        "SHOW_AFTER_NESTED",
        "SHOWN_AFTER",
    ] {
        assert!(t.contains(shown), "{shown} missing: {t}");
    }
    let hidden = [
        "HIDE_G2",
        "HIDE_G3",
        "HIDE_NESTED",
        "HIDE_RUN",
        "SECRET_NAME_OFF",
        "THIRD_OFF",
    ];
    assert!(hidden.iter().all(|h| !t.contains(h)), "{t}");
    // "BDC" in the forbidden list: every OC marker pair is dropped, shown or hidden.
    let forbid: Vec<&str> = hidden.iter().copied().chain(["BDC"]).collect();
    let run = line(20.0, 250.0);
    judge(
        "states",
        &input,
        &out,
        &forbid,
        &[
            "ALWAYS_SHOWN",
            "SHOW_G1",
            "SHOW_AFTER_NESTED",
            "SHOWN_AFTER",
        ],
        &[
            line(20.0, 340.0),
            line(20.0, 320.0),
            line(20.0, 300.0),
            (run.0, run.1, 84.0, run.3),
        ],
    );
    assert_eq!(plan.report.flattened.ocg_layers, 3);
}

#[test]
fn memberships_and_visibility_expressions() {
    // 10 on, 11 and 12 off. MuPDF ignores or misreads OCMDs (it shows an AllOn over an OFF group
    // and hides an AnyOff one), so no render oracle here: the expectations are the spec's.
    let catalog = "/OCProperties << /OCGs [10 0 R 11 0 R 12 0 R] /D << /OFF [11 0 R 12 0 R] >> >>";
    let mut objs: Vec<String> = vec!["null".into(); 4];
    objs.extend(GROUPS.map(str::to_owned));
    objs.push("<< /Type /OCG /Name (THIRD_OFF) >>".into()); // 12
    for m in [
        "/OCGs [10 0 R 11 0 R] /P /AnyOff",                    // 13 shown
        "/OCGs [10 0 R 11 0 R] /P /AllOn",                     // 14 hidden
        "/VE [/Not 11 0 R]",                                   // 15 shown
        "/VE [/And 10 0 R 11 0 R]",                            // 16 hidden
        "/OCGs [11 0 R 12 0 R] /P /AllOff",                    // 17 shown
        "/OCGs [10 0 R 11 0 R]",                               // 18 AnyOn by default: shown
        "/OCGs 11 0 R",                                        // 19 one group, off: hidden
        "/VE [/Or [/Not 10 0 R] [/And 10 0 R [/Not 12 0 R]]]", // 20 nested: shown
    ] {
        objs.push(format!("<< /Type /OCMD {m} >>"));
    }
    let props: String = (13..=20).map(|n| format!("/m{n} {n} 0 R ")).collect();
    let cases = [
        ("SHOW_ANYOFF", true),
        ("HIDE_ALLON", false),
        ("SHOW_VE_NOT", true),
        ("HIDE_VE_AND", false),
        ("SHOW_ALLOFF", true),
        ("SHOW_ANYON_DEFAULT", true),
        ("HIDE_SINGLE_OFF", false),
        ("SHOW_VE_NESTED", true),
    ];
    let content: String = cases
        .iter()
        .enumerate()
        .map(|(i, (s, _))| {
            format!(
                "/OC /m{} BDC {} EMC\n",
                13 + i,
                text(20, 380 - 20 * i as u32, s)
            )
        })
        .collect();
    let input = pdf(catalog, &resources(&props, ""), &content, &objs);
    let (_, out) = export(&input).unwrap();
    let t = words(&out);
    for (s, shown) in cases {
        assert_eq!(t.contains(s), shown, "{s}: {t}");
    }
}

#[test]
fn layers_in_forms_and_xobjects_and_annotations() {
    let catalog = "/OCProperties << /OCGs [10 0 R 11 0 R] /D << /OFF [11 0 R] >> >>";
    let mut objs: Vec<String> = vec!["null".into(); 4];
    objs.extend(GROUPS.map(str::to_owned)); // 10, 11
    objs.extend(vec!["null".into(); 9]); // 12..=20
    // 21: a Form with a hidden layer inside. 22: a Form that is OFF as a whole. 23: one that
    // is ON. 24: an OFF image. 25..28: two annotations and their appearances.
    objs.push(form(
        "",
        "0 0 200 50",
        &format!(
            "{}/OC /g2 BDC {} EMC",
            text(0, 30, "FORM_VISIBLE"),
            text(0, 10, "SECRET_IN_FORM")
        ),
    ));
    objs.push(form(
        "/OC 11 0 R",
        "0 0 200 20",
        &text(0, 5, "SECRET_OFF_XOBJECT"),
    ));
    objs.push(form("/OC 10 0 R", "0 0 200 20", &text(0, 5, "XOBJECT_ON")));
    objs.push(stream(
        "/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /OC 11 0 R",
        "A",
    ));
    objs.push("<< /Type /Annot /Subtype /FreeText /Rect [20 100 200 130] /OC 11 0 R /F 4 /AP << /N 26 0 R >> >>".into()); // 25
    objs.push(form("", "0 0 180 30", &text(2, 4, "SECRET_OC_ANNOT"))); // 26
    objs.push("<< /Type /Annot /Subtype /FreeText /Rect [20 60 200 90] /OC 10 0 R /F 4 /AP << /N 28 0 R >> >>".into()); // 27
    objs.push(form("", "0 0 180 30", &text(2, 4, "KEEP_ANNOT"))); // 28
    let x = "/Fm1 21 0 R /Fm2 22 0 R /Fm3 23 0 R /Im1 24 0 R";
    let content = format!(
        "q 1 0 0 1 20 300 cm /Fm1 Do Q q 1 0 0 1 20 250 cm /Fm2 Do Q q 1 0 0 1 20 200 cm /Fm3 Do Q \
         q 30 0 0 30 150 250 cm /Im1 Do Q {}",
        text(20, 380, "PAGE_TEXT")
    );
    let input = pdf(
        catalog,
        &format!(
            "{} /Annots [25 0 R 27 0 R]",
            resources("", &format!("/XObject << {x} >>"))
        ),
        &content,
        &objs,
    );
    let (plan, out) = export(&input).unwrap();

    let forbid = [
        "SECRET_IN_FORM",
        "SECRET_OFF_XOBJECT",
        "SECRET_OC_ANNOT",
        "SECRET_NAME_OFF",
    ];
    let t = words(&out);
    for shown in ["PAGE_TEXT", "FORM_VISIBLE", "XOBJECT_ON", "KEEP_ANNOT"] {
        assert!(t.contains(shown), "{shown} missing: {t}");
    }
    judge(
        "forms",
        &input,
        &out,
        &forbid,
        &["FORM_VISIBLE", "KEEP_ANNOT"],
        // pypdf places Form text without the Form's cm: the same words also sit at (0, 10) there.
        &[
            line(20.0, 310.0),
            line(20.0, 300.0),
            line(0.0, 10.0),
            line(0.0, 5.0),
            line(20.0, 255.0),
            line(22.0, 104.0),
        ],
    );
    // The OFF image and the OFF Form are not in the file any more: nothing draws them, so they
    // were replaced, not merely left unreferenced.
    assert!(!has(&out, "/Image"), "the OFF image survived");
    assert_eq!(plan.report.flattened.annotations, 1);
    assert!(plan.report.removed[&SanitizedItem::OcgContent] >= 4);
}

#[test]
fn clipped_out_and_off_page_content_is_removed_and_partly_visible_content_stays() {
    // CropBox 200 x 200 inside a 300 x 400 media box.
    let mut objs: Vec<String> = vec!["null".into(); 4];
    // 10, 11: groups the page lists in /Properties although no /OCProperties names them.
    objs.extend(GROUPS.map(str::to_owned));
    objs.extend(vec!["null".into(); 9]);
    objs.push(form("", "0 0 100 20", &text(0, 5, "SHARED_FORM"))); // 21
    objs.push(stream(
        "/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8",
        "A",
    )); // 22
    let content = [
        text(20, 100, "VISIBLE_IN_CROP"),
        text(250, 100, "OFFPAGE_RIGHT"),
        text(20, 300, "OFFPAGE_TOP"),
        // The inner clip does not meet the outer one: nothing of the text can show.
        format!(
            "q 10 10 100 50 re W n q 150 10 40 40 re W n {}Q Q\n",
            text(20, 20, "NESTED_CLIPPED_OUT")
        ),
        format!(
            "q 10 150 100 40 re W n {}Q\n",
            text(15, 160, "CLIP_VISIBLE")
        ),
        // Clip 10..50 wide, text from 20: the first letters show, so the whole run stays.
        format!(
            "q 10 120 40 30 re W n {}Q\n",
            text(20, 130, "PARTLY_CLIPPED_STAYS")
        ),
        "0 1 0 rg 250 150 20 20 re f 0 0 1 rg 100 10 20 20 re f\n".to_owned(),
        "q 40 0 0 40 260 50 cm /Im1 Do Q\n".to_owned(),
        // One Form, drawn once on the page and once beside it: seen once, so it stays.
        "q 1 0 0 1 20 60 cm /Fm1 Do Q q 1 0 0 1 260 60 cm /Fm1 Do Q\n".to_owned(),
    ]
    .concat();
    let input = pdf(
        "",
        &format!(
            "/CropBox [0 0 200 200] {}",
            resources("", "/XObject << /Fm1 21 0 R /Im1 22 0 R >>")
        ),
        &content,
        &objs,
    );
    let (plan, out) = export(&input).unwrap();

    let forbid = ["OFFPAGE_RIGHT", "OFFPAGE_TOP", "NESTED_CLIPPED_OUT"];
    let t = words(&out);
    for shown in [
        "VISIBLE_IN_CROP",
        "CLIP_VISIBLE",
        "PARTLY_CLIPPED_STAYS",
        "SHARED_FORM",
    ] {
        assert!(t.contains(shown), "{shown} missing: {t}");
    }
    assert!(forbid.iter().all(|f| !t.contains(f)), "{t}");
    let (ok, log) = objects_oracle("sanitize-visibility", "offpage.pdf", &out, &forbid);
    assert!(ok, "objects.py: {log}");
    let (ok, _) = objects_oracle("sanitize-visibility", "offpage.in.pdf", &input, &forbid);
    assert!(!ok, "negative control");
    // Display space of the cropped page; the words outside it are the declared change. The page
    // looks the same everywhere inside the crop (pixels, and the shown words).
    let (ok, log) = fidelity_oracle(
        "sanitize-visibility",
        "offpage",
        &input,
        &out,
        &[
            (0, 200.0, 0.0, 300.0, 200.0),
            (0, 0.0, -300.0, 300.0, 0.0),
            // pypdf's own positions of the two lines it reads from the input.
            (0, -5.0, 190.0, 5.0, 210.0),
            (0, 18.0, 165.0, 150.0, 186.0),
        ],
        &[
            "VISIBLE_IN_CROP",
            "CLIP_VISIBLE",
            "PARTLY_CLIPPED_STAYS",
            "SHARED_FORM",
        ],
    );
    assert!(ok, "fidelity.py: {log}");
    assert!(!has(&out, "/Image"), "the off-page image survived");
    assert_eq!(plan.report.removed[&SanitizedItem::OffPage], 5);
}

#[test]
fn layers_that_cannot_be_evaluated_refuse_by_name() {
    let catalog = "/OCProperties << /OCGs [10 0 R] /D << >> >>";
    let mut objs: Vec<String> = vec!["null".into(); 4];
    objs.push(GROUPS[0].into());
    objs.push("null".into());
    objs.push("<< /Type /OCMD /VE [/Xor 10 0 R 10 0 R] >>".into()); // 12
    for (props, marked) in [("/m1 12 0 R", "m1"), ("", "missing")] {
        let content = format!("/OC /{marked} BDC {} EMC", text(20, 300, "X"));
        let input = pdf(catalog, &resources(props, ""), &content, &objs);
        let err = export(&input).unwrap_err();
        assert_eq!(err.code, ErrorCode::OptionalContentUnsupported, "{marked}");
        assert_eq!(
            err.details.and_then(|d| d.feature).as_deref(),
            Some("flattenOptionalContent")
        );
    }
}

/// Layer removal and redaction used to rewrite the same operator spans of one Writer ("overlapping
/// span"): a hidden layer and a hidden run in a shown text object, both under the region, next to
/// a shown secret and survivors. Pass 2 plans the page as pass 1 left it, for a region and for a
/// text range (whose cluster ids pass 1 moved).
#[test]
fn hidden_layers_and_redaction_over_the_same_text_export_in_two_passes() {
    let catalog = "/OCProperties << /OCGs [10 0 R 11 0 R] /D << /OFF [11 0 R] >> >>";
    let mut objs: Vec<String> = vec!["null".into(); 4];
    objs.extend(GROUPS.map(str::to_owned));
    let mut content = text(20, 380, "KEEP_SURVIVOR");
    content += &format!("/OC /g1 BDC {}EMC\n", text(20, 200, "SHOW_G1"));
    content += "BT /F1 12 Tf 20 250 Td /OC /g2 BDC (HIDE_RUN  ) Tj EMC (SHOWN_SECRET) Tj ET\n";
    content += &format!("/OC /g2 BDC {}EMC\n", text(20, 250, "HIDDEN_UNDER_REGION"));
    let input = pdf(catalog, &resources("", ""), &content, &objs);
    let forbid = [
        "SHOWN_SECRET",
        "HIDDEN_UNDER_REGION",
        "HIDE_RUN",
        "SECRET_NAME_OFF",
    ];
    let keep = ["KEEP_SURVIVOR", "SHOW_G1"];
    let row = line(20.0, 250.0);
    for (name, redact) in [
        (
            "two-pass-region",
            Box::new(|_: &str| {
                vec![Redaction::Region {
                    id: "r".into(),
                    page_index: 0,
                    rect: Rect {
                        x: row.0,
                        y: row.1,
                        width: row.2 - row.0,
                        height: row.3 - row.1,
                    },
                    fill: None,
                }]
            }) as Box<dyn FnOnce(&str) -> Vec<Redaction>>,
        ),
        (
            "two-pass-text",
            Box::new(|t: &str| {
                let start = t.find("SHOWN_SECRET").unwrap() as u32;
                vec![Redaction::Text {
                    id: "t".into(),
                    page_index: 0,
                    start,
                    end: start + 12,
                    fill: None,
                }]
            }),
        ),
    ] {
        let (plan, out) = export_redacted(&input, redact);
        let t = words(&out);
        assert!(forbid.iter().all(|f| !t.contains(f)), "{name}: {t}");
        assert!(keep.iter().all(|k| t.contains(k)), "{name}: {t}");
        assert!(plan.report.redaction.clusters_removed >= 12, "{name}");
        assert_eq!(plan.report.flattened.ocg_layers, 2);
        // The hidden run is the declared change next to the redacted line.
        judge(name, &input, &out, &forbid, &keep, &[row]);
    }
}
