//! Predefined CMaps from the SDK pack (NEW-88, spec 10.3). The fixture `generated/cjk-predefined-cmaps.pdf` has Type0
//! fonts encoded by UniGB-UCS2-H / UniJIS-UCS2-H and no ToUnicode, so every Unicode value comes from the registered
//! pack files. PyMuPDF (tests/oracles venv) is the independent judge of text, pixels and the redacted export.
mod common;
use common::{fixture, objects_oracle, python, repo, run};
use foliark_engine::{Document, Engine, RenderOptions, cmaps};
use foliark_model::{
    ChangeSet, ErrorCode, ExportMode, ExportPolicy, ExtractOptions, Limits, Redaction,
    RenderWarningCode,
};

const FIXTURE: &str = "generated/cjk-predefined-cmaps.pdf";
const NEEDED: [&str; 3] = ["Adobe-GB1-UCS2", "UniGB-UCS2-H", "UniJIS-UCS2-H"];
const WORD: &str = "\u{6587}\u{6d4b}"; // the middle of the Chinese line

fn pack(name: &str) -> Vec<u8> {
    std::fs::read(
        repo()
            .join("packages/sdk/cmaps")
            .join(format!("{name}.cmap")),
    )
    .unwrap()
}

fn load_pack() {
    for name in NEEDED {
        cmaps::register(name, &pack(name)).unwrap();
    }
}

/// PyMuPDF's text and its count of dark pixels in the three CJK rows (display y 70..190), for a PDF file.
fn oracle(bytes: &[u8], name: &str) -> (String, usize) {
    let path = common::tmp("cmaps", name);
    std::fs::write(&path, bytes).unwrap();
    let script = "import fitz,sys,json;p=fitz.open(sys.argv[1])[0];pm=p.get_pixmap(dpi=72);\
        d=sum(1 for y in range(70,190) for x in range(pm.width) if pm.pixel(x,y)[0]<64);\
        print(json.dumps([p.get_text(),d]))";
    let (ok, out) = python(&["-c", script, path.to_str().unwrap()]);
    assert!(ok, "{out}");
    let (text, dark): (String, usize) = serde_json::from_str(out.lines().last().unwrap()).unwrap();
    (text, dark)
}

fn utf16_at(text: &str, needle: &str) -> u32 {
    text.encode_utf16().count() as u32
        - text[text.find(needle).unwrap()..].encode_utf16().count() as u32
}

#[test]
fn text_comes_from_the_pack_and_matches_pymupdf() {
    let bytes = fixture(FIXTURE);
    let (_, info) = Engine::new(Limits::default())
        .open(bytes.clone(), None, false)
        .unwrap();
    assert_eq!(info.required_assets, NEEDED.map(|n| format!("cmap:{n}")));
    load_pack();
    let ours = Document::open(bytes.clone(), None)
        .unwrap()
        .extract_text(0, &ExtractOptions::default())
        .unwrap()
        .text;
    let (theirs, _) = oracle(&bytes, "plain.pdf");
    let squash = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    assert_eq!(squash(&ours), squash(&theirs));
    assert!(
        squash(&ours)
            .contains("\u{4e2d}\u{6587}\u{6d4b}\u{8bd5} \u{65e5}\u{672c}\u{8a9e} \u{4f60}\u{597d}")
    );
}

#[test]
fn the_render_has_no_cmap_warning_and_draws_what_pymupdf_draws() {
    load_pack();
    let bytes = fixture(FIXTURE);
    let page = Document::open(bytes.clone(), None)
        .unwrap()
        .render_page(0, RenderOptions::at(1.0))
        .unwrap();
    let codes: Vec<_> = page.header.warnings.iter().map(|w| w.code).collect();
    assert!(
        !codes.contains(&RenderWarningCode::CmapMissing),
        "{codes:?}"
    );
    let w = page.header.width as usize;
    let dark = |x: usize, y: usize| page.data[(y * w + x) * 4] < 64;
    // seven box glyphs, 24 pt cells from x = 20; the centre of each is ink, the gap before the first is not
    for k in 0..4 {
        assert!(dark(20 + 24 * k + 12, 90), "GB glyph {k}");
    }
    for k in 0..3 {
        assert!(dark(20 + 24 * k + 12, 150), "JP glyph {k}");
    }
    assert!(!dark(10, 90) && !dark(20 + 24 * 4 + 12, 90));
    // the non-embedded line is drawn with a stand-in, and the page says so
    assert!(codes.contains(&RenderWarningCode::FontSubstituted));
    let ours = (70..190usize)
        .map(|y| (0..w).filter(|&x| dark(x, y)).count())
        .sum::<usize>();
    let (_, theirs) = oracle(&bytes, "render.pdf");
    // MuPDF draws the stand-in line with a real CJK face, ours with a std-14 face: compare the embedded rows only
    let rows = |y0: usize, y1: usize| {
        (y0..y1)
            .map(|y| (0..w).filter(|&x| dark(x, y)).count())
            .sum::<usize>()
    };
    assert!(ours >= rows(78, 102) + rows(138, 162));
    assert!(
        theirs >= rows(78, 102) + rows(138, 162) - 40,
        "ours {} theirs {theirs}",
        rows(78, 102) + rows(138, 162)
    );
}

#[test]
fn a_redacted_cjk_word_is_gone_from_the_native_export_and_the_rest_stays_searchable() {
    load_pack();
    let input = fixture(FIXTURE);
    let mut e = Engine::new(Limits::default());
    let (doc, _) = e.open(input.clone(), None, false).unwrap();
    let t = e.extract_text(&doc, 0, ExtractOptions::default());
    let text = run(&mut e, t).unwrap().text;
    let start = utf16_at(&text, WORD);
    let t = e.apply_changes(
        &doc,
        ChangeSet {
            base_revision: 0,
            text_edits: None,
            redactions: Some(vec![Redaction::Text {
                id: "cjk".into(),
                page_index: 0,
                start,
                end: start + WORD.encode_utf16().count() as u32,
                fill: None,
            }]),
        },
    );
    let r = run(&mut e, t).unwrap();
    assert!(r.accepted, "{:?}", r.rejected);
    let t = e.prepare_export(&doc, ExportMode::Native, ExportPolicy::default());
    let plan = run(&mut e, t).unwrap();
    let out = e.take_export(&doc, &plan.plan_id).unwrap();

    let (ok, log) = objects_oracle(
        "cmaps",
        "redacted.pdf",
        &out,
        &[WORD, "\u{6587}", "\u{6d4b}"],
    );
    assert!(ok, "{log}");
    let (theirs, _) = oracle(&out, "redacted-text.pdf");
    assert!(
        !theirs.contains('\u{6587}') && !theirs.contains('\u{6d4b}'),
        "{theirs:?}"
    );
    for kept in [
        "KEEP_PUBLIC_CJK",
        "\u{4e2d}",
        "\u{8bd5}",
        "\u{65e5}\u{672c}\u{8a9e}",
        "\u{4f60}\u{597d}",
    ] {
        assert!(theirs.contains(kept), "PyMuPDF lost {kept:?} in {theirs:?}");
    }
    let ours = Document::open(out, None)
        .unwrap()
        .extract_text(0, &ExtractOptions::default())
        .unwrap()
        .text;
    assert!(ours.contains("\u{65e5}\u{672c}\u{8a9e}") && !ours.contains('\u{6587}'));
}

#[test]
fn a_file_that_is_not_the_named_cmap_is_a_typed_failure_never_a_panic() {
    let fail = |name: &str, bytes: &[u8]| {
        let e = cmaps::register(name, bytes).unwrap_err();
        assert_eq!(e.code, ErrorCode::AssetLoadFailed, "{name}");
        assert_eq!(
            e.details.unwrap().feature.as_deref(),
            Some(format!("cmap:{name}").as_str())
        );
    };
    let gb = pack("UniGB-UCS2-H");
    fail("UniGB-UCS2-H", &gb[..gb.len() / 2]); // truncated
    fail("UniGB-UCS2-H", &pack("UniJIS-UCS2-H")); // another map under this name
    fail("UniGB-UCS2-H", b"%%EndResource garbage \xff\xfe");
    fail("UniGB-UCS2-H", &[]);
    fail("NotACMap", &gb);
    fail("Identity-H", &gb);
    fail("UniGB-UCS2-H", &vec![b' '; cmaps::MAX_CMAP_BYTES + 1]);
    // a map that `usecmap`s another needs the base first
    fail("90ms-RKSJ-V", &pack("90ms-RKSJ-V"));
    cmaps::register("90ms-RKSJ-H", &pack("90ms-RKSJ-H")).unwrap();
    cmaps::register("90ms-RKSJ-V", &pack("90ms-RKSJ-V")).unwrap();
    cmaps::register("90ms-RKSJ-V", &pack("90ms-RKSJ-V")).unwrap(); // idempotent
}
