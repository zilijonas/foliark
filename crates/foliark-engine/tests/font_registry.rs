//! Source-font reuse and the editor face (spec §6.4 rung 2, `editorFace`), on real PDFs.
//! Needs PDF objects, so it lives in the engine crate; the registry itself is tested in
//! foliark-text.
mod common;

use foliark_engine::Document;
use foliark_engine::interpret::ItemKind;
use foliark_engine::text::SourceFontFace;
use foliark_model::{ErrorCode, FontSource, FontStyle, RunSource};
use foliark_text::{FontFace, Ladder, LayoutParams, LayoutPlan, Registry, StyleFlags};
use pdf_writer::{Finish, Name, Pdf, Rect, Ref};
use skrifa::raw::TableProvider;
use skrifa::raw::types::Tag;
use skrifa::{FontRef, MetadataProvider};

const SANS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../foliark-text/tests/data/LiberationSans-Regular.ttf"
);

fn sans() -> Vec<u8> {
    std::fs::read(SANS).unwrap()
}

/// A Type0 / Identity-H font whose subsetter-made program (no cmap, OS/2, name or post) holds
/// `embedded`; the page shows `shown`. Returns the document and each embedded char's CID.
fn embedded_pdf(embedded: &str, shown: &str, encoding: &[u8]) -> (Document, Vec<(char, u16)>) {
    let font = sans();
    let src = FontRef::new(&font).unwrap();
    let mut remap = subsetter::GlyphRemapper::new();
    let cids: Vec<(char, u16)> = embedded
        .chars()
        .map(|c| {
            (
                c,
                remap.remap(src.charmap().map(c as u32).unwrap().to_u32() as u16),
            )
        })
        .collect();
    let program = subsetter::subset(&font, 0, &remap).unwrap();
    let metrics = src.glyph_metrics(
        skrifa::instance::Size::unscaled(),
        skrifa::instance::LocationRef::default(),
    );
    let upem = f32::from(src.head().unwrap().units_per_em());
    let [cat, tree, page, content, type0, cidf, desc, file, cmap] =
        [1, 2, 3, 4, 5, 6, 7, 8, 9].map(Ref::new);
    let mut pdf = Pdf::new();
    pdf.catalog(cat).pages(tree);
    pdf.pages(tree).kids([page]).count(1);
    let mut p = pdf.page(page);
    p.parent(tree)
        .media_box(Rect::new(0.0, 0.0, 300.0, 400.0))
        .contents(content);
    p.resources().fonts().pair(Name(b"F1"), type0);
    p.finish();
    let hex: String = shown
        .chars()
        .map(|c| format!("{:04X}", cids.iter().find(|x| x.0 == c).unwrap().1))
        .collect();
    pdf.stream(
        content,
        format!("BT /F1 12 Tf 10 100 Td <{hex}> Tj ET").as_bytes(),
    );
    let mut t = pdf.type0_font(type0);
    t.base_font(Name(b"AAAAAA+LiberationSans"))
        .encoding_predefined(Name(encoding))
        .descendant_font(cidf)
        .to_unicode(cmap);
    t.finish();
    let mut c = pdf.cid_font(cidf);
    c.subtype(pdf_writer::types::CidFontType::Type2)
        .base_font(Name(b"AAAAAA+LiberationSans"))
        .system_info(pdf_writer::types::SystemInfo {
            registry: pdf_writer::Str(b"Adobe"),
            ordering: pdf_writer::Str(b"Identity"),
            supplement: 0,
        })
        .font_descriptor(desc)
        .default_width(0.0)
        .cid_to_gid_map_predefined(Name(b"Identity"));
    {
        let mut w = c.widths();
        for &(ch, cid) in &cids {
            let gid = src.charmap().map(ch as u32).unwrap();
            let adv = metrics.advance_width(gid).unwrap() * 1000.0 / upem;
            w.consecutive(cid, [adv]);
        }
    }
    c.finish();
    pdf.font_descriptor(desc)
        .name(Name(b"AAAAAA+LiberationSans"))
        .flags(pdf_writer::types::FontFlags::NON_SYMBOLIC)
        .bbox(Rect::new(-200.0, -300.0, 1200.0, 1000.0))
        .italic_angle(0.0)
        .ascent(905.0)
        .descent(-212.0)
        .cap_height(700.0)
        .stem_v(80.0)
        .font_file2(file);
    pdf.stream(file, &program);
    let entries: String = cids
        .iter()
        .map(|(c, cid)| format!("<{cid:04X}> <{:04X}>\n", *c as u32))
        .collect();
    pdf.stream(
        cmap,
        format!(
            "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /X def \
             /CMapType 2 def 1 begincodespacerange <0000> <FFFF> endcodespacerange\n\
             {} beginbfchar\n{entries}endbfchar endcmap CMapName currentdict /CMap \
             defineresource pop end end",
            cids.len()
        )
        .as_bytes(),
    );
    (Document::open(pdf.finish(), None).unwrap(), cids)
}

/// The page-0 font of the first glyph and its reuse face (evidence = all glyphs of page 0).
fn face(doc: &Document, name: Option<&str>) -> Option<SourceFontFace> {
    let page = doc.page_model(0).unwrap();
    let (id, _) = page
        .fonts
        .iter()
        .find(|(_, f)| name.is_none_or(|n| f.name == n))
        .unwrap();
    let glyphs = page.items.iter().filter_map(|i| match &i.kind {
        ItemKind::Glyph(g) => Some(g),
        _ => None,
    });
    SourceFontFace::new(doc, *id, glyphs).unwrap()
}

fn registry() -> Registry {
    let mut r = Registry::new();
    r.register(FontSource {
        bytes: sans(),
        family: "Liberation Sans".into(),
        weight: 400,
        style: FontStyle::Normal,
    })
    .unwrap();
    r
}

fn plan(
    text: &str,
    reg: &Registry,
    src: &SourceFontFace,
) -> Result<LayoutPlan, foliark_model::PdfError> {
    foliark_text::layout_paragraph(
        text,
        &LayoutParams {
            font_size: 12.0,
            wrap_width: 400.0,
            max_lines: 3,
            line_step: 14.0,
            first_baseline: 100.0,
            origin_x: 10.0,
        },
        &Ladder {
            registry: reg,
            caller: None,
            source: Some(src),
            source_name: "AAAAAA+LiberationSans",
            flags: StyleFlags::default(),
            font_match: &[],
        },
    )
}

fn table(font: &FontRef<'_>, tag: &[u8; 4]) -> bool {
    font.table_data(Tag::new(tag)).is_some()
}

/// A program as the writer and most producers leave it: glyphs and metrics, no cmap, OS/2,
/// name or post. The editor face adds them and maps exactly the characters the page showed.
#[test]
fn editor_face_repairs_a_subset_without_cmap_and_os2() {
    let (doc, cids) = embedded_pdf("Hello World \u{17e}q", "Hello World", b"Identity-H");
    let f = face(&doc, None).unwrap();
    assert!(f.covers("H") && f.covers(" ") && !f.covers("\u{17e}") && !f.covers("q"));
    assert!(!f.covers_ascii());
    let e = f.editor_face().unwrap();
    let font = FontRef::new(&e.bytes).unwrap();
    for t in [b"cmap", b"OS/2", b"name", b"post"] {
        assert!(table(&font, t), "{}", String::from_utf8_lossy(t));
    }
    let map = font.charmap();
    for (c, cid) in cids.iter().filter(|(c, _)| "Helo Wrd".contains(*c)) {
        assert_eq!(map.map(*c as u32).unwrap().to_u32(), u32::from(*cid), "{c}");
    }
    // Present in the program but never shown: not in the editor cmap, so not offered.
    assert!(map.map('q' as u32).is_none());
    assert!(font.os2().is_ok() && font.name().is_ok());
    // The checksum of a valid sfnt is the magic number once checkSumAdjustment is in.
    let sum = e.bytes.chunks(4).fold(0u32, |s, c| {
        let mut w = [0u8; 4];
        w[..c.len()].copy_from_slice(c);
        s.wrapping_add(u32::from_be_bytes(w))
    });
    assert_eq!(sum, 0xB1B0_AFBA);
    // Same evidence, same key; more evidence, new key.
    let again = face(
        &embedded_pdf("Hello World \u{17e}q", "Hello World", b"Identity-H").0,
        None,
    );
    assert_eq!(again.unwrap().editor_face().unwrap().font_key, e.font_key);
    let more = face(
        &embedded_pdf("Hello World \u{17e}q", "Hello World q", b"Identity-H").0,
        None,
    );
    assert_ne!(more.unwrap().editor_face().unwrap().font_key, e.font_key);
}

/// HDDN's unicode-glyph-widths: a simple TrueType DejaVu subset with code points above U+FFFF
/// and ligatures. The non-BMP character is shown with the font yet is not reusable (a format-4
/// cmap and the editable alphabet are BMP); every reusable character maps to the glyph the
/// interpreter drew.
#[test]
fn dejavu_fixture_is_bmp_only_and_cmap_matches_drawn_glyphs() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/hddn/unicode-glyph-widths.pdf"
    );
    let doc = Document::open(std::fs::read(path).unwrap(), None).unwrap();
    let f = face(&doc, Some("DejaVuSans")).unwrap();
    assert!(!f.covers("\u{1F600}"));
    assert!(f.covers("W") && f.covers("\u{fb01}") && f.covers("\u{152}"));
    let e = f.editor_face().unwrap();
    let font = FontRef::new(&e.bytes).unwrap();
    let model = doc.page_model(0).unwrap();
    let mut checked = 0;
    for g in model.items.iter().filter_map(|i| match &i.kind {
        ItemKind::Glyph(g)
            if g.font
                .is_some_and(|id| model.fonts[&id].name == "DejaVuSans") =>
        {
            Some(g)
        }
        _ => None,
    }) {
        let mut cs = g.unicode.as_deref().unwrap_or("").chars();
        if let (Some(c), None) = (cs.next(), cs.next())
            && (c as u32) < 0xFFFF
            && f.covers(&c.to_string())
        {
            assert_eq!(
                font.charmap().map(c as u32).map(|x| x.to_u32()),
                g.gid,
                "{c}"
            );
            checked += 1;
        }
    }
    assert!(checked >= 10, "{checked}");
    assert!(font.charmap().map(0x1F600u32).is_none());
}

/// Mixed coverage in one paragraph: the source font keeps every cluster it can draw, with the
/// character codes and the advances its own /W lists; only the missing cluster goes to the
/// registered font, and the plan says it is approximate.
#[test]
fn mixed_coverage_keeps_source_glyphs_and_falls_back_per_cluster() {
    let (doc, cids) = embedded_pdf("Hello World", "Hello World", b"Identity-H");
    let f = face(&doc, None).unwrap();
    let reg = registry();
    let p = plan("Hello \u{17e} World", &reg, &f).unwrap();
    assert!(p.approximate);
    let l = p.edit_layout("e1");
    let runs: Vec<_> = l.runs.iter().map(|r| (r.source, r.clusters)).collect();
    assert_eq!(
        runs,
        [(RunSource::Embedded, 12), (RunSource::Registered, 1)]
    );
    let first = &p.lines[0].runs[0];
    assert_eq!(first.font, f.id());
    // Code = the CID the page itself wrote; advance = /W * size.
    assert_eq!(first.glyphs[0].gid, u32::from(cids[0].1));
    let src = FontRef::new(Box::leak(sans().into_boxed_slice())).unwrap();
    let h = src.charmap().map('H' as u32).unwrap();
    let adv = src
        .glyph_metrics(
            skrifa::instance::Size::unscaled(),
            skrifa::instance::LocationRef::default(),
        )
        .advance_width(h)
        .unwrap();
    let want = f64::from(adv) / f64::from(src.head().unwrap().units_per_em()) * 12.0;
    assert!(
        (first.glyphs[0].x_advance - want).abs() < 0.01,
        "{} {want}",
        first.glyphs[0].x_advance
    );
}

/// A base character the source font has plus a combining mark only the registered font has:
/// the whole cluster goes to the registered font (no mark floating on a source glyph).
#[test]
fn combining_cluster_falls_back_whole() {
    let (doc, _) = embedded_pdf("Hello World q", "Hello World q", b"Identity-H");
    let f = face(&doc, None).unwrap();
    assert!(f.covers("q") && !f.covers("\u{301}") && !f.covers("q\u{301}"));
    let p = plan("q\u{301} q", &registry(), &f).unwrap();
    let l = p.edit_layout("e1");
    let runs: Vec<_> = l.runs.iter().map(|r| (r.source, r.clusters)).collect();
    assert_eq!(runs, [(RunSource::Registered, 1), (RunSource::Embedded, 2)]);
    let reg_run = &p.lines[0].runs[0];
    assert!(reg_run.glyphs.iter().all(|g| g.cluster.end <= 2));
}

/// No rung covers the character: a refusal naming it, never a silent .notdef.
#[test]
fn missing_glyph_is_refused_explicitly() {
    let (doc, _) = embedded_pdf("Hello World", "Hello World", b"Identity-H");
    let f = face(&doc, None).unwrap();
    let e = plan("Hello \u{17e}", &Registry::new(), &f).unwrap_err();
    assert_eq!(e.code, ErrorCode::EditNoFont);
    assert_eq!(e.details.unwrap().chars.as_deref(), Some("\u{17e}"));
    assert_eq!(
        f.shape("\u{17e}", 12.0).unwrap_err().code,
        ErrorCode::EditNoFont
    );
}

/// Equal inputs give an equal plan fingerprint; a reopened copy of the file too.
#[test]
fn resolution_is_deterministic() {
    let reg = registry();
    let fp = || {
        let (doc, _) = embedded_pdf("Hello World", "Hello World", b"Identity-H");
        plan("Hello \u{17e}", &reg, &face(&doc, None).unwrap())
            .unwrap()
            .fingerprint
    };
    assert_eq!(fp(), fp());
}

/// Fonts that cannot be reused as-is: Identity-V (the layout is horizontal).
#[test]
fn vertical_cid_font_is_not_reusable() {
    let (doc, _) = embedded_pdf("Hello", "Hello", b"Identity-V");
    assert!(face(&doc, None).is_none());
}

/// A font with no program (Base-14 Helvetica) is reused for the characters the page drew, with
/// its /Widths; it is flagged as not embedded and has no editor face.
#[test]
fn non_embedded_font_is_reused_but_flagged() {
    let doc = common::build(
        &["BT /F1 12 Tf 10 100 Td (Hello) Tj ET"],
        &common::Font::default(),
        0,
    );
    let f = face(&doc, None).unwrap();
    assert!(f.covers("Hel") && !f.covers("W"));
    assert!(!f.approx().embedded && !f.approx().to_unicode);
    assert!(f.editor_face().is_none());
    let g = f.shape("He", 12.0).unwrap();
    assert_eq!(g.iter().map(|g| g.gid).collect::<Vec<_>>(), [72, 101]);
    assert!((g[0].x_advance - 6.0).abs() < 1e-9);
}
