//! `policy.metadata: 'keep'` (ADR 0016, NEW-134). The input has a full Info dictionary (the eight
//! standard keys, a custom key, `Trapped`) and an XMP packet. Outputs are read by pikepdf and
//! judged by the independent oracle `objects.py --allow info-standard`, never by the engine.
//! The default policy still writes no Info and no XMP.
mod common;
use common::{python, run, stream, tiny_pdf, tmp};
use foliark_engine::{Document, Engine};
use foliark_model::{
    ChangeSet, ExportMode, ExportPlan, ExportPolicy, ExtractOptions, FontSource, FontStyle, Limits,
    LossCode, MetadataPolicy, Rect, Redaction, SanitizedItem, TextEdit,
};

const KEYS: [&str; 8] = [
    "Title",
    "Author",
    "Subject",
    "Keywords",
    "Creator",
    "Producer",
    "CreationDate",
    "ModDate",
];

fn sorted(keys: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = keys.iter().map(|k| (*k).to_owned()).collect();
    v.sort();
    v
}

fn keep() -> ExportPolicy {
    ExportPolicy {
        metadata: MetadataPolicy::Keep,
        ..ExportPolicy::default()
    }
}

/// Info with a UTF-16 title (`Café Ltd`), a custom key and `Trapped`; XMP as object 7.
fn input() -> Vec<u8> {
    let info = "<< /Title <FEFF00430061006600E9004C00740064> /Author (Marta Jones) \
        /Subject (Confidential memo) /Keywords (alpha beta) /Creator (Writer) /Producer (Tool 1.0) \
        /CreationDate (D:20240102030405+02'00') /ModDate (D:20240102030406Z) \
        /Custom (SECRET_CUSTOM_VALUE) /Trapped /False >>";
    let xmp = stream(
        "/Type /Metadata /Subtype /XML",
        "<x:xmpmeta>SECRET_XMP_BODY</x:xmpmeta>",
    );
    let pdf = tiny_pdf(
        "/Metadata 7 0 R",
        "/Resources << /Font << /F1 5 0 R >> >>",
        "BT /F1 12 Tf 20 370 Td (Confidential) Tj ET BT /F1 12 Tf 20 340 Td (Marta Jones) Tj ET",
        &[info.into(), xmp],
    );
    // The trailer follows the xref table: naming /Info there moves no offset.
    String::from_utf8_lossy(&pdf)
        .replacen("/Root 1 0 R", "/Root 1 0 R /Info 6 0 R", 1)
        .into_bytes()
}

fn engine() -> Engine {
    let mut e = Engine::new(Limits::default());
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
    e
}

fn export(
    mode: ExportMode,
    cs: Option<ChangeSet>,
    mut policy: ExportPolicy,
) -> (ExportPlan, Vec<u8>) {
    let mut e = engine();
    let (doc, _) = e.open(input(), None, false).unwrap();
    if let Some(cs) = cs {
        let t = e.apply_changes(&doc, cs);
        let r = run(&mut e, t).unwrap();
        assert!(r.accepted, "{:?}", r.rejected);
    }
    policy.raster.acknowledge_losses = true;
    let t = e.prepare_export(&doc, mode, policy);
    let plan = run(&mut e, t).unwrap();
    let out = e.take_export(&doc, &plan.plan_id).unwrap();
    (plan, out)
}

fn native(cs: Option<ChangeSet>, policy: ExportPolicy) -> (ExportPlan, Vec<u8>) {
    export(ExportMode::Native, cs, policy)
}

fn redact_jones() -> ChangeSet {
    let text = Document::open(input(), None)
        .unwrap()
        .extract_text(0, &ExtractOptions::default())
        .unwrap()
        .text;
    let start = text.find("Jones").unwrap() as u32;
    ChangeSet {
        base_revision: 0,
        text_edits: None,
        redactions: Some(vec![Redaction::Text {
            id: "r".into(),
            page_index: 0,
            start,
            end: start + 5,
            fill: None,
        }]),
    }
}

fn edit_confidential() -> ChangeSet {
    ChangeSet {
        base_revision: 0,
        text_edits: Some(vec![TextEdit {
            id: "e".into(),
            page_index: 0,
            rects: vec![Rect {
                x: 15.0,
                y: 15.0,
                width: 120.0,
                height: 20.0,
            }],
            text: "CHANGED".into(),
            font: None,
        }]),
        redactions: None,
    }
}

/// `(keys, values, has XMP)`: the trailer's Info keys (sorted, pikepdf does not keep file order), each value as text.
fn info(bytes: &[u8], name: &str) -> (Vec<String>, Vec<String>, bool) {
    let p = tmp("metadata-keep", name);
    std::fs::write(&p, bytes).unwrap();
    let code = "import sys, pikepdf\n\
        pdf = pikepdf.open(sys.argv[1])\n\
        info = pdf.trailer.get('/Info')\n\
        for k in ([] if info is None else info.keys()):\n\
        \tprint('INFO', str(k)[1:], str(info[k]))\n\
        print('XMP', '/Metadata' in pdf.Root)\n";
    let (ok, log) = python(&["-c", code, p.to_str().unwrap()]);
    assert!(ok, "{log}");
    let mut rows: Vec<_> = log
        .lines()
        .filter_map(|l| l.strip_prefix("INFO "))
        .map(|l| l.split_once(' ').unwrap())
        .collect();
    rows.sort();
    (
        rows.iter().map(|r| r.0.to_owned()).collect(),
        rows.iter().map(|r| r.1.to_owned()).collect(),
        log.contains("XMP True"),
    )
}

fn oracle(name: &str, bytes: &[u8], forbid: &[&str]) {
    let p = tmp("metadata-keep", name);
    std::fs::write(&p, bytes).unwrap();
    let mut args = vec![
        "tests/oracles/objects.py",
        p.to_str().unwrap(),
        "--allow",
        "info-standard",
    ];
    for f in forbid {
        args.extend(["--forbid", f]);
    }
    let (ok, log) = python(&args);
    assert!(ok, "{name}: objects.py: {log}");
}

fn dropped(p: &ExportPlan) -> u32 {
    p.report
        .removed
        .get(&SanitizedItem::Metadata)
        .copied()
        .unwrap_or(0)
}

#[test]
fn keep_writes_the_eight_standard_fields_and_nothing_else() {
    let (plan, out) = native(None, keep());
    let (keys, values, xmp) = info(&out, "keep.pdf");
    assert_eq!(keys, sorted(&KEYS));
    // In key order: Author, CreationDate, Creator, Keywords, ModDate, Producer, Subject, Title.
    assert_eq!(
        values,
        [
            "Marta Jones",
            "D:20240102030405+02'00'",
            "Writer",
            "alpha beta",
            "D:20240102030406Z",
            "Tool 1.0",
            "Confidential memo",
            "Caf\u{e9}Ltd",
        ]
    );
    assert!(!xmp);
    assert_eq!(plan.report.kept.metadata, KEYS);
    // XMP packet + Custom + Trapped.
    assert_eq!(dropped(&plan), 3);
    oracle(
        "keep.pdf",
        &out,
        &["SECRET_CUSTOM_VALUE", "SECRET_XMP_BODY"],
    );
    // Deterministic.
    assert_eq!(native(None, keep()).1, out);
}

#[test]
fn the_default_policy_writes_no_info_and_no_xmp() {
    let (plan, out) = native(None, ExportPolicy::default());
    assert_eq!(info(&out, "default.pdf"), (vec![], vec![], false));
    assert!(plan.report.kept.metadata.is_empty());
    assert_eq!(dropped(&plan), 2);
    assert!(
        plan.report
            .losses
            .iter()
            .any(|l| l.code == LossCode::MetadataRemoved && l.count == 2)
    );
    let explicit = ExportPolicy {
        metadata: MetadataPolicy::Remove,
        ..ExportPolicy::default()
    };
    assert_eq!(native(None, explicit).1, out);
}

#[test]
fn a_field_spelling_redacted_text_is_dropped() {
    let (plan, out) = native(Some(redact_jones()), keep());
    let (keys, _, _) = info(&out, "redacted.pdf");
    // Author is "Marta Jones"; the other seven stay.
    let left = [
        "Title",
        "Subject",
        "Keywords",
        "Creator",
        "Producer",
        "CreationDate",
        "ModDate",
    ];
    assert_eq!(keys, sorted(&left));
    assert_eq!(plan.report.kept.metadata, left);
    assert_eq!(dropped(&plan), 4);
    oracle("redacted.pdf", &out, &["Jones", "SECRET_XMP_BODY"]);
}

#[test]
fn a_field_spelling_edited_away_text_is_dropped() {
    let (plan, out) = native(Some(edit_confidential()), keep());
    let (keys, _, _) = info(&out, "edited.pdf");
    // Subject is "Confidential memo"; the word is gone from the page.
    let left = [
        "Title",
        "Author",
        "Keywords",
        "Creator",
        "Producer",
        "CreationDate",
        "ModDate",
    ];
    assert_eq!(keys, sorted(&left));
    assert_eq!(plan.report.kept.metadata, left);
    oracle("edited.pdf", &out, &["Confidential", "SECRET_XMP_BODY"]);
}

#[test]
fn raster_export_never_writes_info() {
    let (plan, out) = export(ExportMode::Raster, None, keep());
    assert_eq!(info(&out, "raster.pdf"), (vec![], vec![], false));
    assert!(plan.report.kept.metadata.is_empty());
}
