//! One-font synthetic PDFs for the text tests: Helvetica F1 (not embedded) whose 256 codes all
//! advance 0.5 em, an optional ToUnicode CMap and, with `unnamed`, codes 1..=9 named by glyph
//! names no encoding knows (so only ToUnicode can map them).
#![allow(dead_code)]
use foliark_engine::Document;
use foliark_model::{Quad, TextPage};
use pdf_writer::{Finish, Name, Pdf, Rect, Ref};

#[derive(Default)]
pub struct Font {
    pub to_unicode: Vec<(u8, &'static str)>,
    pub unnamed: bool,
}

/// Page size is 300 x 400; `pages` are content streams; `rotate` is the /Rotate of every page.
pub fn build(pages: &[&str], font: &Font, rotate: i32) -> Document {
    let mut pdf = Pdf::new();
    let (cat, tree, f1, cmap) = (Ref::new(1), Ref::new(2), Ref::new(3), Ref::new(4));
    pdf.catalog(cat).pages(tree);
    let kids: Vec<Ref> = (0..pages.len() as i32)
        .map(|i| Ref::new(10 + 2 * i))
        .collect();
    pdf.pages(tree)
        .kids(kids.iter().copied())
        .count(pages.len() as i32);
    for (i, content) in pages.iter().enumerate() {
        let mut p = pdf.page(kids[i]);
        p.parent(tree)
            .media_box(Rect::new(0.0, 0.0, 300.0, 400.0))
            .contents(Ref::new(11 + 2 * i as i32));
        if rotate != 0 {
            p.rotate(rotate);
        }
        p.resources().fonts().pair(Name(b"F1"), f1);
        p.finish();
        pdf.stream(Ref::new(11 + 2 * i as i32), content.as_bytes());
    }
    let mut f = pdf.indirect(f1).dict();
    f.pair(Name(b"Type"), Name(b"Font"));
    f.pair(Name(b"Subtype"), Name(b"Type1"));
    f.pair(Name(b"BaseFont"), Name(b"Helvetica"));
    f.pair(Name(b"FirstChar"), 0);
    f.pair(Name(b"LastChar"), 255);
    f.insert(Name(b"Widths")).array().items(vec![500; 256]);
    if font.unnamed {
        let names: Vec<String> = (1..=9).map(|i| format!("g{i}")).collect();
        let mut d = f.insert(Name(b"Encoding")).dict();
        let mut diff = d.insert(Name(b"Differences")).array();
        diff.item(1);
        for n in &names {
            diff.item(Name(n.as_bytes()));
        }
        diff.finish();
        d.finish();
    }
    if !font.to_unicode.is_empty() {
        f.pair(Name(b"ToUnicode"), cmap);
    }
    f.finish();
    if !font.to_unicode.is_empty() {
        let entries: String = font
            .to_unicode
            .iter()
            .map(|(c, s)| {
                let hex: String = s.encode_utf16().map(|u| format!("{u:04X}")).collect();
                format!("<{c:02X}> <{hex}>\n")
            })
            .collect();
        let body = format!(
            "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /X def /CMapType 2 def\n\
             1 begincodespacerange <00> <FF> endcodespacerange\n{} beginbfchar\n{entries}endbfchar\n\
             endcmap CMapName currentdict /CMap defineresource pop end end",
            font.to_unicode.len()
        );
        pdf.stream(cmap, body.as_bytes());
    }
    Document::open(pdf.finish(), None).unwrap()
}

/// `text` sliced by UTF-16 offsets, the way JavaScript's `slice` does.
pub fn slice16(text: &str, start: u32, end: u32) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    String::from_utf16(&units[start as usize..end as usize]).unwrap()
}

pub fn texts(p: &TextPage) -> Vec<&str> {
    p.words.iter().map(|w| w.text.as_str()).collect()
}

pub fn width(q: &Quad) -> f64 {
    (q.0[1].x - q.0[0].x).hypot(q.0[1].y - q.0[0].y)
}

pub fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}
