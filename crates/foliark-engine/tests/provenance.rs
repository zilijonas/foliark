//! Every drawn item knows its operator, its Form instance path and its resource owner.
use foliark_engine::interpret::{Item, ItemKind, SemanticPage};
use foliark_engine::reader::Document;
use foliark_model::{ErrorCode, InstanceStep, ObjectId, PdfError, SourceLoc};
use pdf_writer::{Finish, Name, Pdf, Rect, Ref, Str};

const fn id(num: u32) -> ObjectId {
    ObjectId { num, generation: 0 }
}

fn loc(item: &Item) -> SourceLoc {
    item.loc
}

/// What the page paints, without the clip-only items every clip and Form /BBox adds.
fn drawn(page: SemanticPage) -> Vec<Item> {
    page.items
        .into_iter()
        .filter(|i| !matches!(&i.kind, ItemKind::Path(p) if p.clip_only))
        .collect()
}

fn items(pdf: Pdf, page: usize) -> Result<Vec<Item>, PdfError> {
    Ok(drawn(
        Document::open(pdf.finish(), None, false)?.page_model(page)?,
    ))
}

/// Catalog 1, Pages 2. Returns the writer so a test can add pages and objects.
fn skeleton(kids: &[Ref]) -> Pdf {
    let mut pdf = Pdf::new();
    pdf.catalog(Ref::new(1)).pages(Ref::new(2));
    pdf.pages(Ref::new(2))
        .kids(kids.iter().copied())
        .count(kids.len() as i32);
    pdf
}

fn page(
    pdf: &mut Pdf,
    id: Ref,
    contents: &[Ref],
    res: impl FnOnce(&mut pdf_writer::writers::Resources),
) {
    let mut p = pdf.page(id);
    p.parent(Ref::new(2))
        .media_box(Rect::new(0.0, 0.0, 100.0, 100.0));
    if contents.len() == 1 {
        p.contents(contents[0]);
    } else {
        p.insert(Name(b"Contents"))
            .array()
            .items(contents.iter().copied());
    }
    res(&mut p.resources());
}

fn image(pdf: &mut Pdf, id: Ref) {
    let mut img = pdf.image_xobject(id, &[200u8; 12]);
    img.width(2).height(2).bits_per_component(8);
    img.color_space().device_rgb();
}

fn form(pdf: &mut Pdf, id: Ref, content: &[u8], inner: Option<(&[u8], Ref)>) {
    let mut f = pdf.form_xobject(id, content);
    f.bbox(Rect::new(0.0, 0.0, 100.0, 100.0));
    let mut res = f.resources();
    if let Some((name, target)) = inner {
        res.x_objects().pair(Name(name), target);
    }
}

#[test]
fn one_image_drawn_twice_is_two_occurrences_of_one_object() {
    let (p, s1, s2, im) = (Ref::new(3), Ref::new(4), Ref::new(5), Ref::new(6));
    let mut pdf = skeleton(&[p]);
    // Two content streams: the operators must map to their own stream, not the merged one.
    let (a, b) = (
        &b"q 20 0 0 20 10 10 cm /Im0 Do Q"[..],
        &b"q 30 0 0 30 50 50 cm /Im0 Do Q"[..],
    );
    pdf.stream(s1, a);
    pdf.stream(s2, b);
    image(&mut pdf, im);
    page(&mut pdf, p, &[s1, s2], |r| {
        r.x_objects().pair(Name(b"Im0"), im);
    });
    let doc = Document::open(pdf.finish(), None, false).unwrap();
    let items = drawn(doc.page_model(0).unwrap());
    assert_eq!(items.len(), 2);
    for (item, (stream, text)) in items.iter().zip([(id(4), a), (id(5), b)]) {
        assert!(matches!(&item.kind, ItemKind::Image(i) if i.obj == Some(id(6)) && !i.is_stencil));
        let l = loc(item);
        assert_eq!(l.stream, stream);
        assert_eq!(&text[l.start as usize..l.end as usize], b"/Im0 Do");
    }
    let ctm = |i: &Item| match &i.kind {
        ItemKind::Image(i) => i.ctm,
        _ => unreachable!(),
    };
    assert_eq!(ctm(&items[0]).0, [20.0, 0.0, 0.0, 20.0, 10.0, 10.0]);
    assert_eq!(ctm(&items[1]).0, [30.0, 0.0, 0.0, 30.0, 50.0, 50.0]);
    assert_eq!(items[0].resource_owner, id(3));
}

#[test]
fn one_form_on_two_pages_and_twice_on_one_page() {
    let (p1, p2, c1, c2, fm) = (
        Ref::new(3),
        Ref::new(4),
        Ref::new(5),
        Ref::new(6),
        Ref::new(7),
    );
    let mut pdf = skeleton(&[p1, p2]);
    pdf.stream(c1, b"q 0 0 20 20 re W n /Fm Do Q");
    pdf.stream(c2, b"q 0 0 40 40 re W n /Fm Do /Fm Do Q");
    form(&mut pdf, fm, b"0 0 10 10 re f", None);
    for (p, c) in [(p1, c1), (p2, c2)] {
        page(&mut pdf, p, &[c], |r| {
            r.x_objects().pair(Name(b"Fm"), fm);
        });
    }
    let doc = Document::open(pdf.finish(), None, false).unwrap();
    let a = drawn(doc.page_model(0).unwrap());
    let b = drawn(doc.page_model(1).unwrap());
    let step = |occurrence| {
        vec![InstanceStep {
            form: id(7),
            occurrence,
        }]
    };
    assert_eq!((a.len(), b.len()), (1, 2));
    assert_eq!(a[0].path.0, step(0));
    assert_eq!(
        (b[0].path.0.clone(), b[1].path.0.clone()),
        (step(0), step(1))
    );
    // Same operator in the same Form, different clip per page (user space).
    assert_eq!(loc(&a[0]), loc(&b[0]));
    assert_eq!(a[0].clip, Some([0.0, 0.0, 20.0, 20.0]));
    assert_eq!(b[0].clip, Some([0.0, 0.0, 40.0, 40.0]));
    // The Form carries its own /Resources, so it owns them.
    assert_eq!(a[0].resource_owner, id(7));
}

#[test]
fn resources_inherited_from_the_pages_node_belong_to_it() {
    let (p, c, im) = (Ref::new(3), Ref::new(4), Ref::new(5));
    let mut pdf = Pdf::new();
    pdf.catalog(Ref::new(1)).pages(Ref::new(2));
    let mut tree = pdf.pages(Ref::new(2));
    tree.kids([p]).count(1);
    tree.resources().x_objects().pair(Name(b"Im0"), im);
    tree.finish();
    pdf.stream(c, b"/Im0 Do");
    image(&mut pdf, im);
    pdf.page(p)
        .parent(Ref::new(2))
        .media_box(Rect::new(0.0, 0.0, 100.0, 100.0))
        .contents(c);
    let items = items(pdf, 0).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].resource_owner, id(2));
}

#[test]
fn form_cycles_and_runaway_nesting_are_errors() {
    let (p, c, a, b) = (Ref::new(3), Ref::new(4), Ref::new(5), Ref::new(6));
    let mut pdf = skeleton(&[p]);
    pdf.stream(c, b"/A Do");
    form(&mut pdf, a, b"0 0 5 5 re f /B Do", Some((b"B", b)));
    form(&mut pdf, b, b"0 0 7 7 re f /A Do", Some((b"A", a)));
    page(&mut pdf, p, &[c], |r| {
        r.x_objects().pair(Name(b"A"), a);
    });
    assert_eq!(
        items(pdf, 0).unwrap_err(),
        PdfError::at(ErrorCode::MalformedPdf, id(5))
    );

    // 40 distinct Forms nested in each other: deeper than MAX_NESTING, no cycle.
    let mut pdf = skeleton(&[p]);
    pdf.stream(c, b"/F Do");
    let first = 10;
    for n in 0..40 {
        let next = (n < 39).then(|| Ref::new(first + n + 1));
        form(
            &mut pdf,
            Ref::new(first + n),
            b"/F Do",
            next.map(|r| (&b"F"[..], r)),
        );
    }
    page(&mut pdf, p, &[c], |r| {
        r.x_objects().pair(Name(b"F"), Ref::new(first));
    });
    assert_eq!(
        items(pdf, 0).unwrap_err(),
        PdfError::limit("maxNesting", 33, 32)
    );
}

#[test]
fn glyphs_report_code_and_font_and_hidden_layers_are_marked() {
    let (p, c, font, ocg) = (Ref::new(3), Ref::new(4), Ref::new(5), Ref::new(6));
    let mut pdf = Pdf::new();
    {
        let mut cat = pdf.catalog(Ref::new(1));
        cat.pages(Ref::new(2));
        let mut props = cat.insert(Name(b"OCProperties")).dict();
        props.insert(Name(b"OCGs")).array().item(ocg);
        let mut d = props.insert(Name(b"D")).dict();
        d.insert(Name(b"OFF")).array().item(ocg);
    }
    pdf.pages(Ref::new(2)).kids([p]).count(1);
    pdf.type1_font(font).base_font(Name(b"Helvetica"));
    pdf.indirect(ocg)
        .dict()
        .pair(Name(b"Type"), Name(b"OCG"))
        .pair(Name(b"Name"), Str(b"Hidden"));
    pdf.stream(
        c,
        b"BT /F1 12 Tf 10 50 Td (Hi) Tj ET /OC /MC0 BDC 0 0 10 10 re f EMC 20 20 5 5 re f",
    );
    page(&mut pdf, p, &[c], |r| {
        r.fonts().pair(Name(b"F1"), font);
        r.insert(Name(b"Properties")).dict().pair(Name(b"MC0"), ocg);
    });
    let items = items(pdf, 0).unwrap();
    let kinds: Vec<_> = items
        .iter()
        .map(|i| match &i.kind {
            ItemKind::Glyph(g) => (Some((g.font, g.char_code)), i.ocg_hidden),
            ItemKind::Path(_) => (None, i.ocg_hidden),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        kinds,
        [
            (Some((Some(id(5)), u32::from(b'H'))), false),
            (Some((Some(id(5)), u32::from(b'i'))), false),
            (None, true),
            (None, false),
        ]
    );
    assert!(
        items
            .iter()
            .all(|i| i.loc.stream == id(4) && i.loc.end > i.loc.start)
    );
}

#[test]
fn a_form_that_fails_to_decode_is_an_error_not_a_skipped_form() {
    let (p, c, fm) = (Ref::new(3), Ref::new(4), Ref::new(5));
    let mut pdf = skeleton(&[p]);
    pdf.stream(c, b"/Fm Do 0 0 5 5 re f");
    pdf.form_xobject(fm, b"not zlib")
        .bbox(Rect::new(0.0, 0.0, 10.0, 10.0))
        .filter(pdf_writer::Filter::FlateDecode);
    page(&mut pdf, p, &[c], |r| {
        r.x_objects().pair(Name(b"Fm"), fm);
    });
    assert_eq!(
        items(pdf, 0).unwrap_err(),
        PdfError::at(ErrorCode::DecodeFailed, id(5))
    );
}
