//! Retained fonts, images and colour profiles at export (spec §5.6, §8.2; NEW-45). Tiny
//! hand-built pages with seeded payloads in resources that content never draws. Outputs are
//! judged by objects.py (decoded streams and strings, with the input as the negative control),
//! fidelity.py (PyMuPDF renders both sides; words must stay extractable by PyMuPDF and pypdf), and
//! for font programs by skrifa on the program read back out of the file.
mod common;
use common::{bin_stream, fidelity_oracle, has, objects_oracle, python, run, tmp};
use foliark_engine::{Document, Engine};
use foliark_model::{
    ErrorCode, ExportMode, ExportPlan, ExportPolicy, ExtractOptions, Limits, PdfError,
    SanitizedItem,
};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};

fn export(bytes: &[u8]) -> Result<(ExportPlan, Vec<u8>), PdfError> {
    let mut e = Engine::new(Limits::default());
    let (doc, _) = e.open(bytes.to_vec(), None, false).unwrap();
    let t = e.prepare_export(&doc, ExportMode::Native, ExportPolicy::default());
    let plan = run(&mut e, t)?;
    let out = e.take_export(&doc, &plan.plan_id).unwrap();
    Ok((plan, out))
}

fn words(bytes: &[u8]) -> String {
    Document::open(bytes.to_vec(), None)
        .unwrap()
        .extract_text(0, &ExtractOptions::default())
        .unwrap()
        .text
}

/// objects.py finds `forbid` in the input and not in the output; fidelity.py finds the page the
/// same (no region may change) and `keep` extractable by two parsers.
fn judge(name: &str, input: &[u8], out: &[u8], forbid: &[&str], keep: &[&str]) {
    let (ok, log) = objects_oracle("sanitize-resources", &format!("{name}.pdf"), out, forbid);
    assert!(ok, "{name}: objects.py: {log}");
    let (ok, _) = objects_oracle(
        "sanitize-resources",
        &format!("{name}.in.pdf"),
        input,
        forbid,
    );
    assert!(
        !ok,
        "{name}: negative control, the payloads are in the input"
    );
    let (ok, log) = fidelity_oracle("sanitize-resources", name, input, out, &[], keep);
    assert!(ok, "{name}: fidelity.py: {log}");
}

/// A 300 x 400 page drawing `content` with `resources`; `objs` are objects 5 onwards.
fn page(resources: &str, content: &str, objs: &[Vec<u8>]) -> Vec<u8> {
    let mut all: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R /Resources << {resources} >> >>"
        )
        .into_bytes(),
        bin_stream("", content.as_bytes()),
    ];
    all.extend(objs.iter().cloned());
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut at = Vec::new();
    for (i, o) in all.iter().enumerate() {
        at.push(out.len());
        out.extend(format!("{} 0 obj\n", i + 1).bytes());
        out.extend_from_slice(o);
        out.extend(b"\nendobj\n");
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

const HELVETICA: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>";

// ---- TrueType -----------------------------------------------------------------------------------

struct Count(usize);
impl OutlinePen for Count {
    fn move_to(&mut self, _: f32, _: f32) {
        self.0 += 1;
    }
    fn line_to(&mut self, _: f32, _: f32) {
        self.0 += 1;
    }
    fn quad_to(&mut self, _: f32, _: f32, _: f32, _: f32) {
        self.0 += 1;
    }
    fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {
        self.0 += 1;
    }
    fn close(&mut self) {}
}

fn outline_ops(font: &FontRef, c: char) -> usize {
    let gid = font.charmap().map(c).unwrap();
    let mut n = Count(0);
    if let Some(o) = font.outline_glyphs().get(GlyphId::new(gid.to_u32())) {
        let _ = o.draw(
            DrawSettings::unhinted(Size::unscaled(), LocationRef::default()),
            &mut n,
        );
    }
    n.0
}

/// The ToUnicode CMap text of the first font that has one, by pikepdf.
fn to_unicode(name: &str, pdf: &[u8]) -> String {
    let src = tmp("sanitize-resources", &format!("{name}.cmap.pdf"));
    std::fs::write(&src, pdf).unwrap();
    let code = "import pikepdf,sys\n\
        p=pikepdf.open(sys.argv[1])\n\
        f=[o for o in p.objects if isinstance(o,pikepdf.Dictionary) and '/ToUnicode' in o][0]\n\
        print(f.ToUnicode.read_bytes().decode('latin1'))";
    let (ok, log) = python(&["-c", code, src.to_str().unwrap()]);
    assert!(ok, "{log}");
    log
}

/// The first embedded `/FontFile2` of `pdf`, decoded, by pikepdf.
fn font_program(name: &str, pdf: &[u8]) -> Vec<u8> {
    let (src, dst) = (
        tmp("sanitize-resources", &format!("{name}.font.pdf")),
        tmp("sanitize-resources", &format!("{name}.ttf")),
    );
    std::fs::write(&src, pdf).unwrap();
    let code = "import pikepdf,sys\n\
        p=pikepdf.open(sys.argv[1])\n\
        f=[o for o in p.objects if isinstance(o,pikepdf.Dictionary) and '/FontFile2' in o][0]\n\
        open(sys.argv[2],'wb').write(f.FontFile2.read_bytes())";
    let (ok, log) = python(&["-c", code, src.to_str().unwrap(), dst.to_str().unwrap()]);
    assert!(ok, "{log}");
    std::fs::read(dst).unwrap()
}

#[test]
fn unused_glyphs_names_and_tounicode_of_a_retained_truetype_font_are_gone() {
    let program = std::fs::read(
        common::repo().join("crates/foliark-text/tests/data/LiberationSans-Regular.ttf"),
    )
    .unwrap();
    let font = FontRef::new(&program).unwrap();
    let widths: Vec<String> = (32u8..=126)
        .map(|c| {
            let m = font.glyph_metrics(Size::unscaled(), LocationRef::default());
            let g = font.charmap().map(c as char).unwrap();
            format!("{:.1}", m.advance_width(g).unwrap_or(0.0) * 1000.0 / 2048.0)
        })
        .collect();
    // ToUnicode that also maps code 0x5A (never drawn) to the payload.
    let cmap = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap \
        /CMapName /X def 1 begincodespacerange <00> <FF> endcodespacerange \
        2 beginbfchar <48> <0048> <5A> <005300450043005200450054> endbfchar endcmap end end";
    let objs = [
        format!(
            "<< /Type /Font /Subtype /TrueType /BaseFont /AAAAAA+Face /FirstChar 32 /LastChar 126 \
             /Widths [{}] /Encoding /WinAnsiEncoding /ToUnicode 7 0 R /FontDescriptor 6 0 R >>",
            widths.join(" ")
        )
        .into_bytes(),
        b"<< /Type /FontDescriptor /FontName /AAAAAA+Face /Flags 32 /FontBBox [-200 -300 2000 1000] \
          /ItalicAngle 0 /Ascent 905 /Descent -212 /CapHeight 716 /StemV 80 /FontFile2 8 0 R >>"
            .to_vec(),
        bin_stream("", cmap.as_bytes()),
        bin_stream(&format!("/Length1 {}", program.len()), &program),
    ];
    let input = page(
        "/Font << /F1 5 0 R >>",
        "BT /F1 14 Tf 20 360 Td (HELLO) Tj ET",
        &objs,
    );
    let (_, out) = export(&input).unwrap();

    let t = words(&out);
    assert!(t.contains("HELLO"), "{t}");
    // The payload is a hex-encoded ToUnicode entry for a code nothing draws.
    let (ok, log) = fidelity_oracle(
        "sanitize-resources",
        "truetype",
        &input,
        &out,
        &[],
        &["HELLO"],
    );
    assert!(ok, "fidelity.py: {log}");
    let (before, after) = (
        to_unicode("truetype.in", &input),
        to_unicode("truetype", &out),
    );
    assert!(before.contains("005300450043005200450054"), "{before}");
    assert!(
        !after.contains("0053004500430052") && after.contains("0048"),
        "{after}"
    );
    let kept = font_program("truetype", &out);
    let f = FontRef::new(&kept).unwrap();
    for c in "HELO".chars() {
        assert!(outline_ops(&f, c) > 0, "{c} must stay drawable");
    }
    for c in ['Z', 'A', '€'] {
        assert_eq!(
            outline_ops(&f, c),
            0,
            "{c} is not drawn: its outline must be gone"
        );
    }
    // The `name` table (copyright, designer) and the glyph name of the unused Euro sign.
    for needle in ["Liberation", "Ascender", "Euro"] {
        assert!(has(&program, needle) && !has(&kept, needle), "{needle}");
    }
}

#[test]
fn unused_type3_charprocs_are_emptied_and_used_ones_still_draw() {
    let objs = [
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] \
          /CharProcs << /a 6 0 R /b 7 0 R >> /Encoding << /Type /Encoding /Differences [97 /a /b] >> \
          /FirstChar 97 /LastChar 98 /Widths [1000 1000] >>"
            .to_vec(),
        bin_stream("", b"1000 0 0 0 750 750 d1 0 0 750 750 re f"),
        bin_stream("", b"1000 0 d0 /SECRET_CHARPROC cs 0 0 750 750 re f"),
    ];
    let input = page(
        "/Font << /F1 5 0 R >>",
        "BT /F1 20 Tf 20 300 Td (aa) Tj ET",
        &objs,
    );
    let (_, out) = export(&input).unwrap();
    judge("type3", &input, &out, &["SECRET_CHARPROC"], &[]);
}

/// A gradient JPEG with a comment and an Exif description, plus bytes after the end marker.
fn jpeg_with_payloads() -> Vec<u8> {
    let p = tmp("sanitize-resources", "payload.jpg");
    let code = "from PIL import Image\nimport sys\n\
        ex=Image.Exif(); ex[270]='SECRET_JPEG_EXIF'\n\
        Image.linear_gradient('L').resize((16,16)).save(sys.argv[1],'JPEG',quality=90,comment=b'SECRET_JPEG_COM',exif=ex)";
    let (ok, log) = python(&["-c", code, p.to_str().unwrap()]);
    assert!(ok, "{log}");
    let mut j = std::fs::read(p).unwrap();
    assert!(has(&j, "SECRET_JPEG_COM") && has(&j, "SECRET_JPEG_EXIF"));
    j.extend_from_slice(b"SECRET_JPEG_TAIL");
    j
}

/// Raw (still DCT-encoded) bytes of the first JPEG image of `pdf`, by pikepdf.
fn raw_jpeg(name: &str, pdf: &[u8]) -> Vec<u8> {
    let (src, dst) = (
        tmp("sanitize-resources", &format!("{name}.jpg.pdf")),
        tmp("sanitize-resources", &format!("{name}.out.jpg")),
    );
    std::fs::write(&src, pdf).unwrap();
    let code = "import pikepdf,sys\n\
        p=pikepdf.open(sys.argv[1])\n\
        f=[o for o in p.objects if isinstance(o,pikepdf.Stream) and o.get('/Filter')=='/DCTDecode'][0]\n\
        open(sys.argv[2],'wb').write(f.read_raw_bytes())";
    let (ok, log) = python(&["-c", code, src.to_str().unwrap(), dst.to_str().unwrap()]);
    assert!(ok, "{log}");
    std::fs::read(dst).unwrap()
}

#[test]
fn image_metadata_opi_and_alternates_do_not_survive() {
    let objs = [
        HELVETICA.as_bytes().to_vec(),
        bin_stream(
            "/Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8 \
             /Metadata 7 0 R /OPI << /1.3 << /Type /OPI /Version 1.3 /F (SECRET_OPI) >> >> \
             /Alternates [<< /Image 8 0 R /DefaultForPrinting true >>]",
            &[0x00, 0x80, 0xC0, 0xFF],
        ),
        bin_stream(
            "/Type /Metadata /Subtype /XML",
            b"<x:xmpmeta>SECRET_IMAGE_XMP</x:xmpmeta>",
        ),
        bin_stream(
            "/Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceGray /BitsPerComponent 8",
            b"SECRET_ALT_IMAGE",
        ),
    ];
    let input = page(
        "/Font << /F1 5 0 R >> /XObject << /Im1 6 0 R >>",
        "q 100 0 0 100 20 250 cm /Im1 Do Q BT /F1 14 Tf 20 360 Td (PUBLIC_IMAGES) Tj ET",
        &objs,
    );
    let (_, out) = export(&input).unwrap();
    judge(
        "images",
        &input,
        &out,
        &["SECRET_OPI", "SECRET_IMAGE_XMP", "SECRET_ALT_IMAGE"],
        &["PUBLIC_IMAGES"],
    );
}

#[test]
fn jpeg_comment_exif_and_trailing_bytes_are_cut_and_the_pixels_stay() {
    let jpeg = jpeg_with_payloads();
    let objs = [
        HELVETICA.as_bytes().to_vec(),
        bin_stream(
            "/Type /XObject /Subtype /Image /Width 16 /Height 16 /ColorSpace /DeviceGray \
             /BitsPerComponent 8 /Filter /DCTDecode",
            &jpeg,
        ),
    ];
    let input = page(
        "/Font << /F1 5 0 R >> /XObject << /Im1 6 0 R >>",
        "q 100 0 0 100 20 250 cm /Im1 Do Q BT /F1 14 Tf 20 360 Td (PUBLIC_JPEG) Tj ET",
        &objs,
    );
    let (_, out) = export(&input).unwrap();
    let kept = raw_jpeg("jpeg", &out);
    assert!(has(&raw_jpeg("jpeg.in", &input), "SECRET_JPEG_TAIL"));
    for s in ["SECRET_JPEG_COM", "SECRET_JPEG_EXIF", "SECRET_JPEG_TAIL"] {
        assert!(!has(&kept, s), "{s}");
    }
    assert!(kept.len() < jpeg.len() && kept.ends_with(&[0xFF, 0xD9]));
    // MuPDF draws the stripped file as it drew the original (fidelity.py compares renders).
    let (ok, log) = fidelity_oracle(
        "sanitize-resources",
        "jpeg",
        &input,
        &out,
        &[],
        &["PUBLIC_JPEG"],
    );
    assert!(ok, "fidelity.py: {log}");
}

fn icc(tail: &[u8]) -> Vec<u8> {
    let mut p = vec![0u8; 132];
    p[0..4].copy_from_slice(&132u32.to_be_bytes());
    p[16..20].copy_from_slice(b"RGB ");
    p[36..40].copy_from_slice(b"acsp");
    p.extend_from_slice(tail);
    p
}

fn icc_page(profile: &[u8]) -> Vec<u8> {
    page(
        "/Font << /F1 5 0 R >> /ColorSpace << /CS1 [/ICCBased 6 0 R] >>",
        "/CS1 cs 0.2 0.4 0.6 sc 20 20 200 200 re f BT /F1 14 Tf 20 360 Td (PUBLIC_ICC) Tj ET",
        &[HELVETICA.as_bytes().to_vec(), bin_stream("/N 3", profile)],
    )
}

#[test]
fn icc_bytes_past_the_profile_are_cut_and_a_stream_that_is_no_profile_refuses() {
    let input = icc_page(&icc(b"SECRET_ICC_TAIL"));
    let (_, out) = export(&input).unwrap();
    judge("icc", &input, &out, &["SECRET_ICC_TAIL"], &["PUBLIC_ICC"]);
    let p = tmp("sanitize-resources", "icc.len.pdf");
    std::fs::write(&p, &out).unwrap();
    let (ok, len) = python(&[
        "-c",
        "import pikepdf,sys;p=pikepdf.open(sys.argv[1]);print(len(p.pages[0].Resources.ColorSpace.CS1[1].read_bytes()))",
        p.to_str().unwrap(),
    ]);
    assert!(ok && len.trim() == "132", "{len}");

    let fake = icc_page(&b"SECRET_NOT_ICC".repeat(20));
    let err = export(&fake).unwrap_err();
    assert_eq!(err.code, ErrorCode::MalformedPdf);
}

// ---- unused resources (ADR 0007, NEW-80) --------------------------------------------------------

const FORM: &str = "/Type /XObject /Subtype /Form /BBox [0 0 300 400]";

#[test]
fn resources_no_content_draws_are_gone_and_the_used_ones_stay() {
    let objs = [
        HELVETICA.as_bytes().to_vec(), // 5
        b"<< /Type /Font /Subtype /Type1 /BaseFont /SECRETUNUSEDFONT >>".to_vec(), // 6
        bin_stream(
            &format!("{FORM} /Resources << /Font << /F1 5 0 R >> >>"),
            b"BT /F1 12 Tf 20 100 Td (FORMWORDS) Tj ET",
        ), // 7 used
        bin_stream(
            &format!("{FORM} /Resources << /Font << /F1 5 0 R >> >>"),
            b"BT /F1 12 Tf 20 50 Td (SECRET_UNUSED_FORM) Tj ET",
        ), // 8
        bin_stream(
            "/Type /XObject /Subtype /Image /Width 19 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8",
            b"SECRET_UNUSED_IMAGE",
        ), // 9
        bin_stream(
            "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 20 20] /XStep 20 /YStep 20 \
             /Resources << /Font << /F1 5 0 R >> >>",
            b"BT /F1 8 Tf (SECRET_UNUSED_PATTERN) Tj ET",
        ), // 10
        b"<< /ShadingType 2 /ColorSpace /DeviceGray /Coords [0 0 1 0] /Function 12 0 R >>".to_vec(), // 11
        bin_stream(
            "/FunctionType 0 /Domain [0 1] /Range [0 1] /Size [21] /BitsPerSample 8",
            b"SECRET_UNUSED_SHADING",
        ), // 12
    ];
    let input = page(
        "/Font << /F1 5 0 R /F2 6 0 R >> /XObject << /Fm1 7 0 R /Dead 8 0 R /Img 9 0 R >> \
         /Pattern << /P1 10 0 R >> /Shading << /Sh1 11 0 R >> \
         /ColorSpace << /CsDead [/Indexed /DeviceGray 17 (SECRET_UNUSED_CS_X)] >> \
         /ExtGState << /GSUsed << /CA 0.629 >> /GSDead << /CA 0.371 >> >>",
        "/GSUsed gs BT /F1 20 Tf 20 300 Td (PAGEWORDS) Tj ET /Fm1 Do",
        &objs,
    );
    let (plan, out) = export(&input).unwrap();
    judge(
        "unused-resources",
        &input,
        &out,
        &[
            "SECRET_UNUSED_FORM",
            "SECRET_UNUSED_IMAGE",
            "SECRET_UNUSED_PATTERN",
            "SECRET_UNUSED_SHADING",
            "SECRET_UNUSED_CS",
            "SECRETUNUSEDFONT",
        ],
        &["PAGEWORDS", "FORMWORDS"],
    );
    assert!(
        has(&out, "0.629") && !has(&out, "0.371"),
        "ExtGState entries"
    );
    // F2, Dead, Img, P1, Sh1, CsDead, GSDead
    assert_eq!(plan.report.removed[&SanitizedItem::UnusedResources], 7);
}

#[test]
fn a_form_without_resources_keeps_what_it_uses_from_the_page() {
    let objs = [
        HELVETICA.as_bytes().to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /SECRETUNUSEDFONT >>".to_vec(),
        bin_stream(
            FORM,
            b"/GS1 gs BT /F1 20 Tf 20 200 Td (INHERITED_WORDS) Tj ET",
        ),
    ];
    let input = page(
        "/Font << /F1 5 0 R /F9 6 0 R >> /XObject << /Fm1 7 0 R >> \
         /ExtGState << /GS1 << /CA 0.629 >> >>",
        "/Fm1 Do",
        &objs,
    );
    let (_, out) = export(&input).unwrap();
    judge(
        "inherited-resources",
        &input,
        &out,
        &["SECRETUNUSEDFONT"],
        &[], // pypdf does not inherit page resources into a Form either: the render must match
    );
    assert!(words(&out).contains("INHERITED_WORDS"));
    assert!(has(&out, "0.629"), "the inherited /GS1 stays");
}

#[test]
fn only_the_inner_form_and_the_pattern_a_form_paints_survive_a_chain() {
    let objs = [
        HELVETICA.as_bytes().to_vec(), // 5
        bin_stream(
            &format!(
                "{FORM} /Resources << /XObject << /Inner 7 0 R /Dead 8 0 R >> \
                 /Font << /FX 9 0 R >> /Pattern << /P1 10 0 R /P2 11 0 R >> >>"
            ),
            b"/Pattern cs /P1 scn 20 20 100 100 re f /Inner Do",
        ), // 6 outer
        bin_stream(
            &format!("{FORM} /Resources << /Font << /F1 5 0 R >> >>"),
            b"BT /F1 12 Tf 20 300 Td (INNER_WORDS) Tj ET",
        ), // 7
        bin_stream(
            &format!("{FORM} /Resources << /Font << /F1 5 0 R >> >>"),
            b"BT /F1 12 Tf 20 250 Td (SECRET_DEAD_FORM) Tj ET",
        ), // 8
        b"<< /Type /Font /Subtype /Type1 /BaseFont /SECRETUNUSEDFONT >>".to_vec(), // 9
        bin_stream(
            "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 \
             /Resources << >>",
            b"1 0 0 rg 0 0 5 5 re f",
        ), // 10 painted by the outer form
        bin_stream(
            "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 20 20] /XStep 20 /YStep 20 \
             /Resources << /Font << /F1 5 0 R >> >>",
            b"BT /F1 8 Tf (SECRET_DEAD_PATTERN) Tj ET",
        ), // 11
    ];
    let input = page("/XObject << /Outer 6 0 R >>", "/Outer Do", &objs);
    let (_, out) = export(&input).unwrap();
    judge(
        "nested-forms",
        &input,
        &out,
        &[
            "SECRET_DEAD_FORM",
            "SECRET_DEAD_PATTERN",
            "SECRETUNUSEDFONT",
        ],
        &["INNER_WORDS"],
    );
}
