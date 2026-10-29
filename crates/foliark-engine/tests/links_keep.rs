//! `policy.links: 'keep'` (ADR 0014, NEW-124). The fixture `generated/links-keep.pdf` has seven links
//! on page 0 and SECRET_LINKTOKEN as the redaction target. Outputs are read by pikepdf (the
//! summary below) and judged by the independent oracle `objects.py` (spec `links`), never by the
//! engine itself. The default policy still removes every link.
mod common;
use common::{fixture, python, run, stream, tiny_pdf, tmp};
use foliark_engine::{Document, Engine};
use foliark_model::{
    ChangeSet, ExportMode, ExportPlan, ExportPolicy, ExtractOptions, FontSource, FontStyle, Limits,
    LinkPolicy, LossCode, Rect, Redaction, SanitizedItem, TextEdit,
};

fn keep() -> ExportPolicy {
    ExportPolicy {
        links: LinkPolicy::Keep,
        ..ExportPolicy::default()
    }
}

fn export(bytes: &[u8], redactions: Vec<Redaction>, policy: ExportPolicy) -> (ExportPlan, Vec<u8>) {
    let mut e = Engine::new(Limits::default());
    let (doc, _) = e.open(bytes.to_vec(), None, false).unwrap();
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
    let plan = run(&mut e, t).unwrap();
    let out = e.take_export(&doc, &plan.plan_id).unwrap();
    (plan, out)
}

fn text_redaction(bytes: &[u8], secret: &str) -> Redaction {
    let doc = Document::open(bytes.to_vec(), None).unwrap();
    let text = doc
        .extract_text(0, &ExtractOptions::default())
        .unwrap()
        .text;
    let start = text.find(secret).expect("secret on the page") as u32;
    Redaction::Text {
        id: "r".into(),
        page_index: 0,
        start,
        end: start + secret.len() as u32,
        fill: None,
    }
}

fn region(x: f64, y: f64, width: f64, height: f64) -> Redaction {
    Redaction::Region {
        id: "g".into(),
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

/// One line per annotation of every page, read by pikepdf: `page subtype target keys`, where
/// target is the URI or `page<n>` of an explicit /Dest, `keys` the sorted annotation keys.
fn annots(bytes: &[u8], name: &str) -> Vec<String> {
    let p = tmp("links-keep", name);
    std::fs::write(&p, bytes).unwrap();
    let code = "import sys, pikepdf\n\
        pdf = pikepdf.open(sys.argv[1])\n\
        gens = [pg.obj.objgen for pg in pdf.pages]\n\
        for i, pg in enumerate(pdf.pages):\n\
        \tfor a in pg.obj.get('/Annots', []):\n\
        \t\tt = str(bytes(a['/A']['/URI']), 'latin-1') if '/A' in a else 'page%d' % gens.index(a['/Dest'][0].objgen)\n\
        \t\tprint('ANNOT', i, a['/Subtype'], t, ','.join(sorted(str(k) for k in a.keys())))\n";
    let (ok, log) = python(&["-c", code, p.to_str().unwrap()]);
    assert!(ok, "{log}");
    log.lines()
        .filter_map(|l| l.strip_prefix("ANNOT "))
        .map(str::to_owned)
        .collect()
}

fn oracle(name: &str, bytes: &[u8], spec: &str, forbid: &[&str]) {
    let (p, s) = (tmp("links-keep", name), tmp("links-keep", "spec.json"));
    std::fs::write(&p, bytes).unwrap();
    std::fs::write(&s, spec).unwrap();
    let mut args = vec![
        "tests/oracles/objects.py",
        p.to_str().unwrap(),
        "--spec",
        s.to_str().unwrap(),
    ];
    for f in forbid {
        args.extend(["--forbid", f]);
    }
    let (ok, log) = python(&args);
    assert!(ok, "{name}: objects.py: {log}");
}

const LINK_KEYS: &str = "/Border,/Dest,/Rect,/Subtype,/Type";

#[test]
fn keep_writes_back_only_the_https_link_and_the_named_goto() {
    let input = fixture("generated/links-keep.pdf");
    let (plan, out) = export(
        &input,
        vec![text_redaction(&input, "SECRET_LINKTOKEN")],
        keep(),
    );
    // https stays, with only the allowlisted keys; the GoTo of the named destination `chapter2`
    // is an explicit /Dest on the second page. The five others are dropped and counted.
    assert_eq!(
        annots(&out, "keep.pdf"),
        [
            "0 /Link https://example.com/public /A,/Border,/Rect,/Subtype,/Type",
            &format!("0 /Link page1 {LINK_KEYS}"),
        ]
    );
    assert_eq!(plan.report.kept.links, 2);
    assert_eq!(loss(&plan, LossCode::LinksRemoved), 5);
    assert_eq!(plan.report.removed[&SanitizedItem::Annotations], 5);
    // Region of SECRET_LINKTOKEN (display space) as the oracle's own input.
    let spec = r#"{"links": {"tokens": ["secret_linktoken"],
        "regions": [{"page": 0, "rect": [20, 88.8, 143.2, 103.5]}]}}"#;
    oracle(
        "keep.pdf",
        &out,
        spec,
        &[
            "SECRET_LINKTOKEN",
            "SECRET_LINK_NOTE",
            "SECRET_LINK_NAME",
            "SECRET_LINK_AUTHOR",
            "SECRET_GOTO_NOTE",
            "calc.exe",
            "javascript",
            "chapter2",
        ],
    );
    // Deterministic, and the default policy is untouched by the option.
    assert_eq!(
        export(
            &input,
            vec![text_redaction(&input, "SECRET_LINKTOKEN")],
            keep()
        )
        .1,
        out
    );
}

#[test]
fn the_default_policy_removes_every_link_and_says_so() {
    let input = fixture("generated/links-keep.pdf");
    let (plan, out) = export(
        &input,
        vec![text_redaction(&input, "SECRET_LINKTOKEN")],
        ExportPolicy::default(),
    );
    assert!(annots(&out, "default.pdf").is_empty());
    assert_eq!(plan.report.kept.links, 0);
    assert_eq!(loss(&plan, LossCode::LinksRemoved), 7);
    assert_eq!(plan.report.removed[&SanitizedItem::Annotations], 7);
    // Same bytes as an explicit `remove`.
    let explicit = ExportPolicy {
        links: LinkPolicy::Remove,
        ..ExportPolicy::default()
    };
    let redaction = vec![text_redaction(&input, "SECRET_LINKTOKEN")];
    assert_eq!(export(&input, redaction, explicit).1, out);
}

#[test]
fn with_nothing_to_drop_there_is_no_links_removed_loss() {
    let input = fixture("generated/links-keep.pdf");
    // No redaction: https, GoTo, the mailto and the percent-encoded URI stay; the region-less
    // export still refuses nothing and drops only javascript: and Launch.
    let (plan, out) = export(&input, vec![], keep());
    assert_eq!(plan.report.kept.links, 5);
    assert_eq!(loss(&plan, LossCode::LinksRemoved), 2);
    assert_eq!(annots(&out, "noredact.pdf").len(), 5);
}

/// Links the fixture does not have: dictionary destinations, `/Next`, other actions, bad URIs,
/// flags, `/AA`, and a quad (not the rect) under a region.
#[test]
fn every_other_way_a_link_can_be_unsafe_is_dropped() {
    let long = format!("https://example.com/{}", "a".repeat(2040));
    let link = |extra: &str| {
        format!("<< /Type /Annot /Subtype /Link /Rect [20 355 160 375] /Border [0 0 0] {extra} >>")
    };
    let uri = |u: &str| link(&format!("/A << /S /URI /URI ({u}) >>"));
    let objs = [
        // 6: kept through the catalog /Dests dictionary (an entry that is a dictionary with /D).
        link("/Dest /sec"),
        // 7: kept, scheme in capitals.
        uri("HTTPS://example.com/Caps"),
        // 8..: dropped.
        link("/Dest /bad"),                                  // destination page is not a page
        link("/Dest /missing"),                              // resolves nowhere
        link("/Dest [99 0 R /Fit]"),                         // not a page of the file
        link("/Dest [3 0 R /XYZ 0 400]"),                    // XYZ takes three arguments
        link("/A << /S /GoTo /D [3 0 R /Fit] /Next << /S /Named /N /NextPage >> >>"),
        link("/A << /S /SubmitForm /F (https://example.com/post) >>"),
        link("/A << /S /Named /N /NextPage >>"),
        link("/A << /S /GoToR /F (other.pdf) /D [0 /Fit] >>"),
        uri("ftp://example.com/file"),
        uri("https://example.com/with space"),
        uri(&long),
        link("/A << /S /URI /URI (https://example.com/aa) >> /AA << /E << /S /JavaScript /JS (1) >> >>"),
        link("/F 2 /A << /S /URI /URI (https://example.com/hidden) >>"),
        // The rect is clear of the region, one quad is not.
        link("/QuadPoints [20 300 150 300 20 290 150 290] /A << /S /URI /URI (https://example.com/quad) >>"),
        "<< /Type /Annot /Subtype /Link /Rect [20 355 160 375] /QuadPoints [1 2 3] /A << /S /URI /URI (https://example.com/q3) >> >>".to_owned(),
        // Dest dictionary targets.
        stream("", "x"), // 23: a stream where a page ref is expected (target of /bad)
    ];
    let annots_ref: String = (6..6 + 17).map(|n| format!("{n} 0 R ")).collect();
    let catalog = "/Dests << /sec << /D [3 0 R /FitH 300] >> /bad [23 0 R /Fit] >>";
    let bytes = tiny_pdf(
        catalog,
        &format!("/Annots [{annots_ref}] /Resources << /Font << /F1 5 0 R >> >>"),
        "BT /F1 12 Tf 20 360 Td (KEEP_PUBLIC) Tj ET",
        &objs,
    );
    // The region (display space) covers the quad's y range 290..300 user = 100..110 display.
    let (plan, out) = export(&bytes, vec![region(10.0, 98.0, 200.0, 14.0)], keep());
    assert_eq!(
        annots(&out, "unsafe.pdf"),
        [
            "0 /Link page0 /Border,/Dest,/Rect,/Subtype,/Type".to_owned(),
            "0 /Link HTTPS://example.com/Caps /A,/Border,/Rect,/Subtype,/Type".to_owned(),
        ]
    );
    assert_eq!(plan.report.kept.links, 2);
    assert_eq!(loss(&plan, LossCode::LinksRemoved), 15);
    oracle(
        "unsafe.pdf",
        &out,
        r#"{"links": {"tokens": [], "regions": [{"page": 0, "rect": [10, 98, 210, 112]}]}}"#,
        &["example.com/aa", "post", "other.pdf", "ftp:"],
    );
}

/// A link over an edited paragraph is dropped (its old box and the new text's box), so a mailto
/// to an address the user edited away does not survive; a link elsewhere stays.
#[test]
fn a_link_over_an_edited_paragraph_is_dropped() {
    let link = |rect: &str, uri: &str| {
        format!(
            "<< /Type /Annot /Subtype /Link /Rect [{rect}] /Border [0 0 0] /A << /S /URI /URI ({uri}) >> >>"
        )
    };
    let bytes = tiny_pdf(
        "",
        "/Annots [6 0 R 7 0 R] /Resources << /Font << /F1 5 0 R >> >>",
        "BT /F1 12 Tf 20 370 Td (KEEP_PUBLIC) Tj ET",
        &[
            link("15 365 120 385", "mailto:old@example.org"),
            link("10 10 100 50", "https://example.com/elsewhere"),
        ],
    );
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
    let (doc, _) = e.open(bytes, None, false).unwrap();
    let edit = TextEdit {
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
    };
    let t = e.apply_changes(
        &doc,
        ChangeSet {
            base_revision: 0,
            text_edits: Some(vec![edit]),
            redactions: None,
        },
    );
    let r = run(&mut e, t).unwrap();
    assert!(r.accepted, "{:?}", r.rejected);
    let t = e.prepare_export(&doc, ExportMode::Native, keep());
    let plan = run(&mut e, t).unwrap();
    let out = e.take_export(&doc, &plan.plan_id).unwrap();
    assert_eq!(
        annots(&out, "edited.pdf"),
        ["0 /Link https://example.com/elsewhere /A,/Border,/Rect,/Subtype,/Type"]
    );
    assert_eq!(plan.report.kept.links, 1);
    assert_eq!(loss(&plan, LossCode::LinksRemoved), 1);
}
