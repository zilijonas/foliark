//! What a file may contain and who may open it (spec §0, §8.2, §12): passwords, damage,
//! signatures and the feature inventory. Everything here is typed and nothing is mutated.
use foliark_engine::reader::Document;
use foliark_model::{ErrorCode, FoliarkError, PdfError};
use pdf_writer::{Finish, Name, Pdf, Rect, Ref, Str};

fn fixture(path: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/../../tests/fixtures/{path}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn open(path: &str, password: Option<&str>) -> Result<Document, PdfError> {
    Document::open(fixture(path), password, false)
}

#[test]
fn password_challenge_wrong_user_and_owner_across_the_encryption_matrix() {
    for scheme in ["rc4-40", "rc4-128", "aes-128", "aes-256"] {
        let path = format!("generated/encrypted-{scheme}.pdf");
        assert_eq!(
            open(&path, None).err(),
            Some(PdfError::new(ErrorCode::PasswordRequired)),
            "{scheme}"
        );
        assert_eq!(
            open(&path, Some("nope")).err(),
            Some(PdfError::new(ErrorCode::PasswordIncorrect)),
            "{scheme}"
        );
        for password in ["user-pw", "owner-pw"] {
            let doc = open(&path, Some(password))
                .unwrap_or_else(|e| panic!("{scheme}/{password}: {e:?}"));
            assert!(doc.info().unwrap().encrypted, "{scheme}/{password}");
            // Decrypting really worked: the content stream decodes and parses.
            assert!(
                !doc.page_contents(0).unwrap().is_empty(),
                "{scheme}/{password}"
            );
        }
    }
    let doc = open("hddn/encrypted.pdf", Some("hddn-fixture-user")).unwrap();
    assert!(doc.info().unwrap().encrypted);
}

#[test]
fn unknown_security_handler_is_a_typed_refusal() {
    let mut bytes = fixture("generated/encrypted-rc4-40.pdf");
    let at = bytes.windows(9).position(|w| w == b"/Standard").unwrap();
    bytes[at + 8] = b'x';
    let err = Document::open(bytes, Some("user-pw"), false).err().unwrap();
    assert_eq!(err, PdfError::new(ErrorCode::EncryptionUnsupported));
    assert_eq!(err.code, ErrorCode::EncryptionUnsupported);
}

#[test]
fn damaged_files_are_typed_and_repair_is_reported_not_hidden() {
    for name in ["hddn/malformed-xref.pdf", "hddn/malformed-truncated.pdf"] {
        assert_eq!(
            open(name, None).err(),
            Some(PdfError::new(ErrorCode::RepairedPdf)),
            "{name}"
        );
        let doc = Document::open(fixture(name), None, true).unwrap();
        let info = doc.info().unwrap();
        assert!(info.repaired, "{name}");
        assert_eq!(info.page_count, 1);
    }
    let err = FoliarkError::from(open("hddn/malformed-xref.pdf", None).err().unwrap());
    assert_eq!(err.code, ErrorCode::RepairedPdf);

    // Cut off before the first object ends: nothing to repair.
    let cut = fixture("hddn/shared-vector.pdf")[..40].to_vec();
    let err = Document::open(cut, None, true).err().unwrap();
    assert_eq!(err.code, ErrorCode::MalformedPdf);
}

/// A one-page file whose catalog and AcroForm come from the closure.
fn signed(sig_value: bool, doc_mdp: bool) -> Vec<u8> {
    let (cat, tree, page, form, field, sig) = (
        Ref::new(1),
        Ref::new(2),
        Ref::new(3),
        Ref::new(4),
        Ref::new(5),
        Ref::new(6),
    );
    let mut pdf = Pdf::new();
    let mut c = pdf.catalog(cat);
    c.pages(tree).pair(Name(b"AcroForm"), form);
    if doc_mdp {
        c.insert(Name(b"Perms")).dict().pair(Name(b"DocMDP"), sig);
    }
    c.finish();
    pdf.pages(tree).kids([page]).count(1);
    pdf.page(page)
        .parent(tree)
        .media_box(Rect::new(0.0, 0.0, 10.0, 10.0));
    pdf.indirect(form)
        .dict()
        .insert(Name(b"Fields"))
        .array()
        .item(field);
    let mut f = pdf.indirect(field).dict();
    f.pair(Name(b"FT"), Name(b"Sig"))
        .pair(Name(b"T"), Str(b"Sig1"));
    if sig_value {
        f.pair(Name(b"V"), sig);
    }
    f.finish();
    let mut s = pdf.indirect(sig).dict();
    s.pair(Name(b"Type"), Name(b"Sig"));
    if sig_value {
        s.pair(Name(b"Contents"), Str(&[0; 8]));
        s.insert(Name(b"ByteRange")).array().items([0, 10, 20, 30]);
    }
    s.finish();
    pdf.finish()
}

#[test]
fn signatures_are_reported_only_when_populated_or_certifying() {
    let info = |b| Document::open(b, None, false).unwrap().info().unwrap();
    let populated = info(signed(true, false)).features;
    assert!(populated.signatures && populated.forms);
    // An empty signature field is a form field, not a signed document.
    assert!(!info(signed(false, false)).features.signatures);
    // Only the Perms entry (certification) is present on the field-less variant.
    assert!(info(signed(false, true)).features.signatures);
}

#[test]
fn feature_inventory_matches_the_fixtures() {
    let features = |p: &str| open(p, None).unwrap().info().unwrap().features;
    let f = features("hddn/shared-vector.pdf");
    assert!(!(f.forms || f.javascript || f.embedded_files || f.optional_content || f.outlines));
    assert_eq!(f.annotations, 0);
    assert!(features("hddn/metadata-xmp.pdf").metadata);
    assert!(features("hddn/actions-js.pdf").javascript);
    assert!(features("hddn/attachment.pdf").embedded_files);
    assert!(features("hddn/acroform.pdf").forms);
    assert!(features("hddn/optional-content-layer.pdf").optional_content);
    assert!(features("hddn/outline-bookmark.pdf").outlines);
    assert!(features("hddn/thumbnail.pdf").thumbnails);
    assert!(features("hddn/actualtext.pdf").struct_tree);
    assert_eq!(features("generated/annot-appearance.pdf").annotations, 2);
    let leaks = features("generated/leaks-metadata-attachment-js.pdf");
    assert!(leaks.javascript && leaks.embedded_files && leaks.metadata && !leaks.forms);
}

#[test]
fn predefined_cmaps_are_listed_as_assets_and_identity_is_not() {
    let (cat, tree, page, a, b) = (
        Ref::new(1),
        Ref::new(2),
        Ref::new(3),
        Ref::new(4),
        Ref::new(5),
    );
    let mut pdf = Pdf::new();
    pdf.catalog(cat).pages(tree);
    pdf.pages(tree).kids([page]).count(1);
    pdf.page(page)
        .parent(tree)
        .media_box(Rect::new(0.0, 0.0, 10.0, 10.0));
    for (id, enc) in [(a, &b"UniJIS-UCS2-H"[..]), (b, b"Identity-H")] {
        pdf.indirect(id)
            .dict()
            .pair(Name(b"Type"), Name(b"Font"))
            .pair(Name(b"Subtype"), Name(b"Type0"))
            .pair(Name(b"Encoding"), Name(enc));
    }
    let info = Document::open(pdf.finish(), None, false)
        .unwrap()
        .info()
        .unwrap();
    assert_eq!(info.required_assets, ["cmap:UniJIS-UCS2-H"]);
}
