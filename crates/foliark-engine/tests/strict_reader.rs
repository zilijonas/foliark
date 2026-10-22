//! The reader never turns damage into an empty page (spec §0.1, §12).
use foliark_engine::reader::{ContentStream, Document, ReaderError};
use foliark_engine::{Document as Engine, EngineError};
use foliark_model::{ErrorCode, FoliarkError, ObjectId};
use pdf_writer::{Content, Filter, Finish, Pdf, Rect, Ref};

const CONTENT: Ref = Ref::new(4);
const OBJ4: ObjectId = ObjectId {
    num: 4,
    generation: 0,
};

/// One 100x100 page. `stream` is the raw (already encoded) content stream, `None` = no /Contents.
fn pdf_with(stream: Option<(&[u8], Option<Filter>)>) -> Vec<u8> {
    let (cat, tree, page) = (Ref::new(1), Ref::new(2), Ref::new(3));
    let mut pdf = Pdf::new();
    pdf.catalog(cat).pages(tree);
    pdf.pages(tree).kids([page]).count(1);
    let mut p = pdf.page(page);
    p.parent(tree).media_box(Rect::new(0.0, 0.0, 100.0, 100.0));
    if stream.is_some() {
        p.contents(CONTENT);
    }
    p.finish();
    if let Some((data, filter)) = stream {
        let mut s = pdf.stream(CONTENT, data);
        if let Some(f) = filter {
            s.filter(f);
        }
    }
    pdf.finish()
}

fn drawing() -> Vec<u8> {
    let mut c = Content::new();
    c.set_fill_rgb(1.0, 0.0, 0.0)
        .rect(10.0, 10.0, 50.0, 50.0)
        .fill_nonzero();
    c.finish().to_vec()
}

/// A zlib stream of stored (uncompressed) blocks: valid Flate without needing a compressor.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + u32::from(x)) % 65521;
        b = (b + a) % 65521;
    }
    let len = data.len() as u16;
    let mut out = vec![0x78, 0x01, 1];
    out.extend(len.to_le_bytes());
    out.extend((!len).to_le_bytes());
    out.extend(data);
    out.extend(((b << 16) | a).to_be_bytes());
    out
}

fn open(bytes: Vec<u8>) -> Result<Document, ReaderError> {
    Document::open(bytes, None, false)
}

#[test]
fn genuinely_empty_pages_are_ok() {
    let no_contents = open(pdf_with(None)).unwrap();
    assert_eq!(no_contents.page_contents(0), Ok(vec![]));
    let empty_stream = open(pdf_with(Some((b"", None)))).unwrap();
    assert_eq!(
        empty_stream.page_contents(0),
        Ok(vec![ContentStream {
            obj: OBJ4,
            data: vec![]
        }])
    );
}

#[test]
fn valid_flate_decodes_and_garbage_or_truncated_flate_names_the_object() {
    let plain = drawing();
    let good = zlib_stored(&plain);
    let doc = open(pdf_with(Some((&good, Some(Filter::FlateDecode))))).unwrap();
    assert_eq!(doc.page_contents(0).unwrap()[0].data, plain);

    let garbage = open(pdf_with(Some((
        b"this is not zlib data at all",
        Some(Filter::FlateDecode),
    ))))
    .unwrap();
    let err = garbage.page_contents(0).unwrap_err();
    assert_eq!(err, ReaderError::DecodeFailed { obj: OBJ4 });
    assert_eq!(err.code(), ErrorCode::DecodeFailed);
    let wire = FoliarkError::from(&err);
    assert_eq!(wire.details.unwrap().obj_ref.as_deref(), Some("4 0 R"));

    // The block header promises more bytes than the stream has.
    let cut = &good[..good.len() - 12];
    let truncated = open(pdf_with(Some((cut, Some(Filter::FlateDecode))))).unwrap();
    assert_eq!(
        truncated.page_contents(0),
        Err(ReaderError::DecodeFailed { obj: OBJ4 })
    );
}

#[test]
fn render_refuses_a_page_it_would_draw_blank() {
    let bytes = pdf_with(Some((b"garbage", Some(Filter::FlateDecode))));
    let doc = Engine::open(bytes, None).unwrap();
    assert_eq!(
        doc.render_page(0, 1.0).err().unwrap(),
        EngineError::Read(ReaderError::DecodeFailed { obj: OBJ4 })
    );
}

#[test]
fn unknown_filter_and_broken_syntax_are_distinct_errors() {
    let mut bytes = pdf_with(Some((b"0 0 m 1 1 l S", None)));
    let at = bytes.windows(8).position(|w| w == b"/Length ").unwrap();
    bytes.splice(at..at, *b"/Filter /NoSuchDecode ");
    // Splicing moved the xref offsets, so this file is repaired.
    let doc = Document::open(bytes, None, true).unwrap();
    assert_eq!(
        doc.page_contents(0),
        Err(ReaderError::UnsupportedFilter { obj: OBJ4 })
    );

    // `re` needs four operands: the parser stops here, and what follows would be dropped.
    let doc = open(pdf_with(Some((b"1 2 re 0 0 m 1 1 l S", None)))).unwrap();
    assert!(matches!(
        doc.page_contents(0),
        Err(ReaderError::ContentSyntax { obj: OBJ4, .. })
    ));
}

#[test]
fn corrupt_xref_is_repaired_only_when_allowed() {
    let plain = drawing();
    let mut bytes = pdf_with(Some((&plain, None)));
    let at = bytes.windows(9).rposition(|w| w == b"startxref").unwrap();
    bytes[at + 10] = b'9';
    let err = open(bytes.clone()).err().unwrap();
    assert_eq!(err, ReaderError::RepairedPdf);
    assert_eq!(err.code(), ErrorCode::RepairedPdf);

    let doc = Document::open(bytes, None, true).unwrap();
    assert!(doc.was_repaired());
    assert_eq!(doc.page_contents(0).unwrap()[0].data, plain);
}

#[test]
fn file_cut_inside_the_content_stream_is_an_error_not_an_empty_page() {
    let plain = drawing();
    let bytes = pdf_with(Some((&plain, None)));
    let at = bytes.windows(6).position(|w| w == b"stream").unwrap();
    let cut = bytes[..at + 6 + 20].to_vec();
    assert_eq!(open(cut.clone()).err(), Some(ReaderError::RepairedPdf));
    let outcome = Document::open(cut, None, true).and_then(|d| d.page_contents(0));
    assert!(outcome.is_err(), "got {outcome:?}");
}

#[test]
fn password_errors_are_typed_and_the_right_password_decrypts() {
    let bytes = include_bytes!("data/encrypted-aes256.pdf").to_vec();
    assert_eq!(
        Document::open(bytes.clone(), None, false).err(),
        Some(ReaderError::PasswordRequired)
    );
    assert_eq!(
        Document::open(bytes.clone(), Some("nope"), false).err(),
        Some(ReaderError::PasswordIncorrect)
    );
    let doc = Document::open(bytes, Some("user"), false).unwrap();
    let content = &doc.page_contents(0).unwrap()[0].data;
    assert!(content.windows(14).any(|w| w == b"[<536563726574"));
}

#[test]
fn input_without_a_pdf_header_is_malformed() {
    assert_eq!(
        Document::open(b"hello".to_vec(), None, false).err(),
        Some(ReaderError::MalformedPdf)
    );
}
