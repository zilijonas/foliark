//! The semantic page model: what the interpreter reports, in what order, in which space.
use foliark_engine::Document;
use foliark_engine::interpret::{
    DiagnosticKind, FontKind, GlyphItem, Item, ItemKind, SemanticPage,
};
use foliark_model::ObjectId;
use pdf_writer::{Finish, Name, Pdf, Rect, Ref, Str};

const fn id(num: u32) -> ObjectId {
    ObjectId { num, generation: 0 }
}

const PAGE: Ref = Ref::new(3);
const CONTENT: Ref = Ref::new(4);
const FONT: Ref = Ref::new(5);

/// Catalog 1, Pages 2, page 3 (100 x 100) with `content` as stream 4. `resources` adds to the
/// page resources; the caller writes the objects they name.
fn build(
    content: &[u8],
    resources: impl FnOnce(&mut pdf_writer::writers::Resources),
    extra: impl FnOnce(&mut Pdf),
) -> SemanticPage {
    let mut pdf = Pdf::new();
    pdf.catalog(Ref::new(1)).pages(Ref::new(2));
    pdf.pages(Ref::new(2)).kids([PAGE]).count(1);
    let mut page = pdf.page(PAGE);
    page.parent(Ref::new(2))
        .media_box(Rect::new(0.0, 0.0, 100.0, 100.0))
        .contents(CONTENT);
    resources(&mut page.resources());
    page.finish();
    pdf.stream(CONTENT, content);
    extra(&mut pdf);
    Document::open(pdf.finish(), None)
        .unwrap()
        .page_model(0)
        .unwrap()
}

fn glyphs(page: &SemanticPage) -> Vec<(&Item, &GlyphItem)> {
    page.items
        .iter()
        .filter_map(|i| match &i.kind {
            ItemKind::Glyph(g) => Some((i, g)),
            _ => None,
        })
        .collect()
}

/// A simple font "Foo" with widths 600/700/800 for A/B/C, a descriptor with ascent 900 and
/// descent -300 (height 1.2, so the quad is normalized to 0.75 / -0.25), and no ToUnicode.
fn foo_font(pdf: &mut Pdf, to_unicode: Option<Ref>) {
    let desc = Ref::new(20);
    pdf.indirect(desc)
        .dict()
        .pair(Name(b"Type"), Name(b"FontDescriptor"))
        .pair(Name(b"FontName"), Name(b"ABCDEF+Foo-Bold"))
        .pair(Name(b"Flags"), 32)
        .pair(Name(b"Ascent"), 900)
        .pair(Name(b"Descent"), -300)
        .pair(Name(b"ItalicAngle"), 0)
        .pair(Name(b"StemV"), 80)
        .insert(Name(b"FontBBox"))
        .array()
        .items([0, -300, 1000, 900]);
    let mut f = pdf.indirect(FONT).dict();
    f.pair(Name(b"Type"), Name(b"Font"))
        .pair(Name(b"Subtype"), Name(b"Type1"))
        .pair(Name(b"BaseFont"), Name(b"ABCDEF+Foo-Bold"))
        .pair(Name(b"FirstChar"), 65)
        .pair(Name(b"LastChar"), 67)
        .pair(Name(b"FontDescriptor"), desc)
        .insert(Name(b"Widths"))
        .array()
        .items([600, 700, 800]);
    if let Some(t) = to_unicode {
        f.pair(Name(b"ToUnicode"), t);
    }
    f.finish();
}

fn cmap(pdf: &mut Pdf, id: Ref) {
    pdf.stream(
        id,
        b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n\
          /CMapName /Adobe-Identity-UCS def /CMapType 2 def\n\
          1 begincodespacerange <00> <FF> endcodespacerange\n\
          2 beginbfchar <41> <00660069> <42> <0058> endbfchar\n\
          endcmap CMapName currentdict /CMap defineresource pop end end",
    );
}

fn foo_resources(r: &mut pdf_writer::writers::Resources) {
    r.fonts().pair(Name(b"F1"), FONT);
}

#[test]
fn text_image_and_path_come_back_in_draw_order() {
    let im = Ref::new(6);
    let page = build(
        b"0 0 10 10 re f q 20 0 0 20 30 30 cm /Im0 Do Q BT /F1 10 Tf 5 50 Td (A) Tj ET 0 0 m 5 5 l S",
        |r| {
            foo_resources(r);
            r.x_objects().pair(Name(b"Im0"), im);
        },
        |pdf| {
            foo_font(pdf, None);
            pdf.image_xobject(im, &[200u8; 12])
                .width(2)
                .height(2)
                .bits_per_component(8)
                .color_space()
                .device_rgb();
        },
    );
    let kinds: Vec<&str> = page
        .items
        .iter()
        .map(|i| match &i.kind {
            ItemKind::Path(_) => "path",
            ItemKind::Image(_) => "image",
            ItemKind::Glyph(_) => "glyph",
            ItemKind::Shading(_) => "shading",
        })
        .collect();
    assert_eq!(kinds, ["path", "image", "glyph", "path"]);
    let ItemKind::Path(fill) = &page.items[0].kind else {
        unreachable!()
    };
    assert!(fill.fill.is_some() && fill.stroke.is_none() && !fill.clip_only);
    let ItemKind::Path(stroke) = &page.items[3].kind else {
        unreachable!()
    };
    assert!(stroke.fill.is_none() && stroke.stroke.is_some());
    let ItemKind::Image(img) = &page.items[1].kind else {
        unreachable!()
    };
    assert_eq!(
        (img.obj, img.width, img.height, img.is_stencil),
        (Some(id(6)), 2, 2, false)
    );
    assert_eq!(img.ctm.0, [20.0, 0.0, 0.0, 20.0, 30.0, 30.0]);
    assert!(page.diagnostics.is_empty(), "{:?}", page.diagnostics);
}

#[test]
fn nested_form_gets_its_matrix_path_and_clip_in_user_space() {
    let fm = Ref::new(6);
    let page = build(
        b"q 0 0 60 60 re W n 1 0 0 1 10 20 cm /Fm Do Q",
        |r| {
            r.x_objects().pair(Name(b"Fm"), fm);
        },
        |pdf| {
            foo_font(pdf, None);
            let mut f = pdf.form_xobject(fm, b"BT /F1 10 Tf 5 5 Td (A) Tj ET");
            f.bbox(Rect::new(0.0, 0.0, 50.0, 50.0))
                .matrix([2.0, 0.0, 0.0, 2.0, 0.0, 0.0]);
            f.resources().fonts().pair(Name(b"F1"), FONT);
        },
    );
    let g = glyphs(&page);
    assert_eq!(g.len(), 1);
    let (item, glyph) = g[0];
    // em -> user: font size 10, then the Form's x2, then the page's translate (10, 20); the text
    // origin (5, 5) becomes (5*2 + 10, 5*2 + 20).
    assert_eq!(glyph.trm.0, [20.0, 0.0, 0.0, 20.0, 20.0, 30.0]);
    // Page clip [0,60]^2 and the Form /BBox [0,50]^2 x2 + (10,20) = [10,110]x[20,120].
    assert_eq!(item.clip, Some([10.0, 20.0, 60.0, 60.0]));
    assert_eq!(item.path.0.len(), 1);
    assert_eq!((item.path.0[0].form, item.path.0[0].occurrence), (id(6), 0));
    assert_eq!(item.resource_owner, id(6));
}

#[test]
fn tj_kerning_and_char_spacing_give_the_manual_positions_and_indices() {
    // Font size 10, Tc 2, widths A 600 B 700 C 800. A at x=100; B at 100 + 6 + 2 = 108;
    // C at 108 + 7 + 2 + 5 (the -500 kern) = 122. The second string continues the index.
    let page = build(
        b"BT /F1 10 Tf 2 Tc 100 200 Td [(AB) -500 (C)] TJ ET",
        foo_resources,
        |pdf| foo_font(pdf, None),
    );
    let g = glyphs(&page);
    let got: Vec<(u32, u32, f64, f64, f64)> = g
        .iter()
        .map(|(_, g)| {
            (
                g.op_glyph_index,
                g.char_code,
                g.trm.0[4],
                g.trm.0[5],
                g.advance_em,
            )
        })
        .collect();
    let want = [
        (0, 65, 100.0, 200.0, 0.8),
        (1, 66, 108.0, 200.0, 0.9),
        (2, 67, 122.0, 200.0, 1.0),
    ];
    for (a, b) in got.iter().zip(want) {
        assert!(
            (a.0, a.1) == (b.0, b.1)
                && (a.2 - b.2).abs() < 1e-6
                && (a.3 - b.3).abs() < 1e-6
                && (a.4 - b.4).abs() < 1e-6,
            "{a:?} vs {b:?}"
        );
    }
    // One operator, one span.
    assert!(g.iter().all(|(i, _)| i.loc == g[0].0.loc));
    // Quad: x from the origin to origin + advance (8 for A), y from descent to ascent after the
    // height-1.2 descriptor is normalized (0.75 / -0.25 em of 10): top-left (100, 207.5),
    // bottom-right (108, 197.5).
    let q = g[0].1.quad_user.0;
    for (got, want) in [
        (q[0], (100.0, 207.5)),
        (q[1], (108.0, 207.5)),
        (q[2], (108.0, 197.5)),
        (q[3], (100.0, 197.5)),
    ] {
        assert!(
            (got.x - want.0).abs() < 1e-6 && (got.y - want.1).abs() < 1e-6,
            "{got:?} vs {want:?}"
        );
    }
    // Font facts from the descriptor and name.
    let f = &page.fonts[&id(5)];
    assert_eq!(
        (f.name.as_str(), f.kind, f.bold, f.italic, f.has_to_unicode),
        ("Foo-Bold", FontKind::Type1, true, false, false)
    );
    assert_eq!(
        (f.first_char, f.widths.clone()),
        (65, vec![600.0, 700.0, 800.0])
    );
}

#[test]
fn unicode_comes_from_to_unicode_else_the_encoding_and_never_invented() {
    let to_unicode = Ref::new(21);
    let with = build(b"BT /F1 10 Tf 10 10 Td (AB) Tj ET", foo_resources, |pdf| {
        foo_font(pdf, Some(to_unicode));
        cmap(pdf, to_unicode);
    });
    let u: Vec<_> = glyphs(&with)
        .iter()
        .map(|(_, g)| g.unicode.clone())
        .collect();
    // A is a two code point ligature mapping and stays one glyph.
    assert_eq!(u, [Some("fi".to_owned()), Some("X".to_owned())]);
    assert!(with.fonts[&id(5)].has_to_unicode);

    // No ToUnicode: the standard encoding names the glyph. C is not in the cmap above.
    let without = build(b"BT /F1 10 Tf 10 10 Td (C) Tj ET", foo_resources, |pdf| {
        foo_font(pdf, None)
    });
    let g = glyphs(&without);
    assert_eq!(g[0].1.unicode.as_deref(), Some("C"));
    assert_eq!(g[0].1.font, Some(id(5)));
}

#[test]
fn invisible_and_hidden_optional_content_text_is_present_and_flagged() {
    let ocg = Ref::new(6);
    let mut pdf = Pdf::new();
    {
        let mut cat = pdf.catalog(Ref::new(1));
        cat.pages(Ref::new(2));
        let mut props = cat.insert(Name(b"OCProperties")).dict();
        props.insert(Name(b"OCGs")).array().item(ocg);
        props
            .insert(Name(b"D"))
            .dict()
            .insert(Name(b"OFF"))
            .array()
            .item(ocg);
    }
    pdf.pages(Ref::new(2)).kids([PAGE]).count(1);
    let mut page = pdf.page(PAGE);
    page.parent(Ref::new(2))
        .media_box(Rect::new(0.0, 0.0, 100.0, 100.0))
        .contents(CONTENT);
    let mut res = page.resources();
    res.fonts().pair(Name(b"F1"), FONT);
    res.insert(Name(b"Properties"))
        .dict()
        .pair(Name(b"MC0"), ocg);
    res.finish();
    page.finish();
    pdf.stream(
        CONTENT,
        b"BT /F1 10 Tf 10 10 Td 3 Tr (A) Tj 0 Tr ET \
          BT /F1 10 Tf 10 30 Td 7 Tr (B) Tj 0 Tr ET \
          /OC /MC0 BDC BT /F1 10 Tf 10 50 Td (C) Tj ET EMC \
          BT /F1 10 Tf 10 70 Td 2 Tr (A) Tj ET",
    );
    pdf.indirect(ocg)
        .dict()
        .pair(Name(b"Type"), Name(b"OCG"))
        .pair(Name(b"Name"), Str(b"Hidden"));
    foo_font(&mut pdf, None);
    let page = Document::open(pdf.finish(), None)
        .unwrap()
        .page_model(0)
        .unwrap();
    let got: Vec<(u32, u8, bool)> = glyphs(&page)
        .iter()
        .map(|(i, g)| (g.char_code, g.render_mode, i.ocg_hidden))
        .collect();
    // Mode 3 and 7 are extracted; mode 2 (fill + stroke) is one glyph, not two.
    assert_eq!(
        got,
        [
            (65, 3, false),
            (66, 7, false),
            (67, 0, true),
            (65, 2, false)
        ]
    );
}

#[test]
fn type3_text_and_unknown_operators_produce_diagnostics_not_silence() {
    let (t3, proc_a) = (Ref::new(5), Ref::new(7));
    let page = build(
        b"BT /T3 10 Tf 10 10 Td (a) Tj ET 1 2 foo",
        |r| {
            r.fonts().pair(Name(b"T3"), t3);
        },
        |pdf| {
            let mut f = pdf.indirect(t3).dict();
            f.pair(Name(b"Type"), Name(b"Font"))
                .pair(Name(b"Subtype"), Name(b"Type3"))
                .pair(Name(b"FirstChar"), 97)
                .pair(Name(b"LastChar"), 97)
                .insert(Name(b"FontBBox"))
                .array()
                .items([0, 0, 1000, 1000]);
            f.insert(Name(b"FontMatrix"))
                .array()
                .items([0.001, 0.0, 0.0, 0.001, 0.0, 0.0]);
            f.insert(Name(b"Widths")).array().items([1000]);
            f.insert(Name(b"CharProcs")).dict().pair(Name(b"a"), proc_a);
            f.insert(Name(b"Encoding"))
                .dict()
                .insert(Name(b"Differences"))
                .array()
                .item(97)
                .item(Name(b"a"));
            f.finish();
            pdf.stream(proc_a, b"1000 0 d0 0 0 500 500 re f");
        },
    );
    let g = glyphs(&page);
    assert_eq!(g.len(), 1);
    assert_eq!(
        (g[0].1.char_code, g[0].1.gid, g[0].1.font),
        (97, None, Some(id(5)))
    );
    // The CharProc's `re f` is not an item: it was not descended.
    assert_eq!(page.items.len(), 1);
    let kinds: Vec<_> = page.diagnostics.iter().map(|d| d.kind.clone()).collect();
    assert_eq!(
        kinds,
        [
            DiagnosticKind::Type3NotDescended(Some(id(5))),
            DiagnosticKind::UnknownOperator("foo".into())
        ]
    );
    assert_eq!(page.fonts[&id(5)].kind, FontKind::Type3);
    assert_eq!(page.diagnostics[1].loc.unwrap().stream, id(4));
}
