//! Text and font resources through the fresh native writer (NEW-23, spec §5.6, §6.3).
//! Source PDFs are built here from the OFL Liberation Sans in foliark-text; PyMuPDF and pypdf
//! (tests/oracles) judge every output without knowing foliark.
use foliark_engine::Document;
use foliark_engine::writer::Writer;
use foliark_engine::writer::fonts::{
    Program, UsedGlyphs, embed_registered_font, empty_unused_truetype_glyphs, rebuild_to_unicode,
    touch_font,
};
use foliark_engine::writer::text::emit_plan;
use foliark_model::{FontSource, FontStyle, Limits};
use foliark_text::{Ladder, LayoutParams, Registry, StyleFlags, layout_paragraph, normalize};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::raw::TableProvider;
use skrifa::{FontRef, GlyphId, MetadataProvider};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tmp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("native-text");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn sans() -> Vec<u8> {
    std::fs::read(root().join("crates/foliark-text/tests/data/LiberationSans-Regular.ttf")).unwrap()
}

/// Runs the oracle venv's python (cwd = repo root); `(success, stdout+stderr)`.
fn python(args: &[&str]) -> (bool, String) {
    let py = root().join("tests/oracles/.venv/bin/python");
    assert!(
        py.exists(),
        "oracle venv missing: python3 -m venv tests/oracles/.venv && tests/oracles/.venv/bin/pip install -r tests/oracles/requirements.txt"
    );
    let out = Command::new(py)
        .args(args)
        .current_dir(root())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

// ---- source PDFs, written by hand so the fixtures do not depend on the code under test -------

#[derive(Default)]
struct Raw(Vec<Vec<u8>>);

impl Raw {
    fn add(&mut self, body: &str) -> usize {
        self.0.push(body.as_bytes().to_vec());
        self.0.len()
    }

    fn stream(&mut self, dict: &str, data: &[u8]) -> usize {
        let mut o = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
        o.extend_from_slice(data);
        o.extend_from_slice(b"\nendstream");
        self.0.push(o);
        self.0.len()
    }

    /// Object 1 is the catalog, 2 the page tree, 3 the page; fonts and content come after.
    fn finish(mut self) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in std::mem::take(&mut self.0).iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        let mut table = format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1);
        for o in &offsets {
            let _ = writeln!(table, "{o:010} 00000 n ");
        }
        let _ = write!(
            table,
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len() + 1
        );
        out.extend_from_slice(table.as_bytes());
        out
    }
}

/// One page, font resource `/F1` = object 4, content = the last object.
fn page_with(raw: &mut Raw, font_objects: impl FnOnce(&mut Raw), content: &str) {
    raw.add("<< /Type /Catalog /Pages 2 0 R >>");
    raw.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    let page = raw.add("");
    font_objects(raw);
    let c = raw.stream("", content.as_bytes());
    raw.0[page - 1] = format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {c} 0 R /Resources << /Font << /F1 4 0 R >> >> >>"
    )
    .into_bytes();
}

fn advance_1000(font: &FontRef, gid: u16) -> f64 {
    let m = font.glyph_metrics(Size::unscaled(), LocationRef::default());
    let upem = f64::from(font.head().unwrap().units_per_em());
    f64::from(m.advance_width(GlyphId::new(u32::from(gid))).unwrap()) * 1000.0 / upem
}

fn gid_of(font: &FontRef, c: char) -> u16 {
    font.charmap().map(c).unwrap().to_u32() as u16
}

const DESCRIPTOR: &str = "/Type /FontDescriptor /FontName /LiberationSans /Flags 32 /FontBBox [-200 -300 2000 1000] /ItalicAngle 0 /Ascent 905 /Descent -212 /CapHeight 716 /StemV 80";

/// A TrueType font with WinAnsi codes, /Widths and a /CharSet (which the writer must drop).
fn simple_pdf(lines: &[&str]) -> Vec<u8> {
    let program = sans();
    let font = FontRef::new(&program).unwrap();
    let widths: Vec<String> = (32u8..=126)
        .map(|c| format!("{:.1}", advance_1000(&font, gid_of(&font, c as char))))
        .collect();
    let mut content = String::new();
    for (i, l) in lines.iter().enumerate() {
        let _ = writeln!(content, "BT /F1 12 Tf 72 {} Td ({l}) Tj ET", 700 - 20 * i);
    }
    let mut raw = Raw::default();
    page_with(
        &mut raw,
        |r| {
            r.add(&format!(
                "<< /Type /Font /Subtype /TrueType /BaseFont /LiberationSans /FirstChar 32 /LastChar 126 /Widths [{}] /Encoding /WinAnsiEncoding /FontDescriptor 5 0 R >>",
                widths.join(" ")
            ));
            r.add(&format!(
                "<< {DESCRIPTOR} /CharSet (/A/B/space) /FontFile2 6 0 R >>"
            ));
            r.stream(&format!("/Length1 {}", program.len()), &program);
        },
        &content,
    );
    raw.finish()
}

/// A Type0 / Identity-H font. `lines` hold `(glyph id, text it stands for)`; `subset` embeds a
/// subsetter output (codes are the remapped glyph ids) under an ABCDEF+ name; a /CIDSet is
/// added (which the writer must drop).
fn cid_pdf(lines: &[Vec<(u16, &str)>], subset: bool) -> Vec<u8> {
    let full = sans();
    let font = FontRef::new(&full).unwrap();
    let gids: BTreeSet<u16> = lines.iter().flatten().map(|(g, _)| *g).collect();
    let mut remap = subsetter::GlyphRemapper::new();
    let program = if subset {
        for g in &gids {
            remap.remap(*g);
        }
        subsetter::subset(&full, 0, &remap).unwrap()
    } else {
        full.clone()
    };
    let code = |g: u16| if subset { remap.get(g).unwrap() } else { g };
    let mut w = String::new();
    let mut to_unicode = BTreeMap::new();
    for l in lines.iter().flatten() {
        to_unicode.insert(u32::from(code(l.0)), l.1);
    }
    let mut sorted: Vec<_> = gids.iter().map(|&g| (code(g), g)).collect();
    sorted.sort();
    for (c, g) in sorted {
        let _ = write!(w, "{c} [{:.1}] ", advance_1000(&font, g));
    }
    let mut content = String::new();
    for (i, l) in lines.iter().enumerate() {
        let hex: String = l.iter().map(|(g, _)| format!("{:04X}", code(*g))).collect();
        let _ = writeln!(content, "BT /F1 12 Tf 72 {} Td <{hex}> Tj ET", 700 - 20 * i);
    }
    let cmap = rebuild_to_unicode(2, to_unicode).unwrap();
    let name = if subset {
        "ABCDEF+LiberationSans"
    } else {
        "LiberationSans"
    };
    let mut raw = Raw::default();
    page_with(
        &mut raw,
        |r| {
            r.add(&format!(
                "<< /Type /Font /Subtype /Type0 /BaseFont /{name} /Encoding /Identity-H /DescendantFonts [5 0 R] /ToUnicode 8 0 R >>"
            ));
            r.add(&format!(
                "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /{name} /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor 6 0 R /DW 0 /W [{w}] /CIDToGIDMap /Identity >>"
            ));
            r.add(&format!(
                "<< {DESCRIPTOR} /CIDSet 9 0 R /FontFile2 7 0 R >>"
            ));
            r.stream(&format!("/Length1 {}", program.len()), &program);
            r.stream("", &cmap);
            r.stream("", &[0xFF, 0xFF]);
        },
        &content,
    );
    raw.finish()
}

fn glyphs(text: &str) -> Vec<(u16, String)> {
    let full = sans();
    let font = FontRef::new(&full).unwrap();
    text.chars()
        .map(|c| (gid_of(&font, c), c.to_string()))
        .collect()
}

fn run_of(text: &str) -> Vec<(u16, &str)> {
    // `&str` borrows need a stable owner; leak the few test strings.
    glyphs(text)
        .into_iter()
        .map(|(g, t)| (g, &*Box::leak(t.into_boxed_str())))
        .collect()
}

/// Rewrite `input` through the writer and judge it: objects, fidelity (words and positions
/// within 1 pt, pixels) and each `preserve` string extractable by both oracles.
fn round_trip(name: &str, input: &[u8], preserve: &[&str]) -> Vec<u8> {
    let doc = Document::open(input.to_vec(), None).unwrap();
    let out = Writer::new(doc.reader(), Limits::default())
        .write()
        .unwrap();
    for banned in [&b"/CharSet"[..], b"/CIDSet"] {
        assert!(
            !out.windows(banned.len()).any(|w| w == banned),
            "{name}: {} survived",
            String::from_utf8_lossy(banned)
        );
    }
    let (inp, outp) = (
        tmp(&format!("{name}.in.pdf")),
        tmp(&format!("{name}.out.pdf")),
    );
    std::fs::write(&inp, input).unwrap();
    std::fs::write(&outp, &out).unwrap();
    let (ok, log) = python(&["tests/oracles/objects.py", outp.to_str().unwrap()]);
    assert!(ok, "{name}: objects.py: {log}");
    let mut args = vec![
        "tests/oracles/fidelity.py",
        inp.to_str().unwrap(),
        outp.to_str().unwrap(),
    ];
    for p in preserve {
        args.extend(["--preserve", p]);
    }
    let (ok, log) = python(&args);
    assert!(ok, "{name}: fidelity.py: {log}");
    out
}

#[test]
fn text_survives_the_writer_for_every_font_shape() {
    let diacritics = [
        ("lt", "ąčęėįšųūž ĄČĘĖĮŠŲŪŽ"),
        ("lv", "āēīūčģķļņšž ĀĒĪŪČĢĶĻŅŠŽ"),
        ("et", "õäöüšž ÕÄÖÜŠŽ"),
        ("pl", "żółć łąkę świeży ŻÓŁĆ"),
    ];
    // Proportional simple TrueType (also carries /CharSet).
    round_trip(
        "proportional",
        &simple_pdf(&[
            "Proportional widths: Wi ill mm",
            "The quick brown fox jumps",
        ]),
        &["Proportionalwidths:Wiillmm", "Thequickbrownfoxjumps"],
    );
    // CID Identity-H with the full program, and with a subset program (remapped codes).
    let lines = [
        run_of("Identity-H CID text 2026"),
        run_of("second line, same font"),
    ];
    round_trip(
        "cid-full",
        &cid_pdf(&lines, false),
        &["Identity-HCIDtext2026", "secondline,samefont"],
    );
    round_trip(
        "cid-subset",
        &cid_pdf(&lines, true),
        &["Identity-HCIDtext2026", "secondline,samefont"],
    );
    // Ligature glyphs: one glyph, ToUnicode "fi" / "fl".
    let full = sans();
    let font = FontRef::new(&full).unwrap();
    let mut lig = run_of("nd the of");
    lig.insert(0, (gid_of(&font, '\u{FB01}'), "fi"));
    lig.push((gid_of(&font, '\u{FB01}'), "fi"));
    lig.extend(run_of("ce "));
    lig.push((gid_of(&font, '\u{FB02}'), "fl"));
    lig.extend(run_of("ow"));
    round_trip("ligature", &cid_pdf(&[lig], true), &["findtheofficeflow"]);
    // lt / lv / et / pl diacritics, full and subset.
    for (lang, text) in diacritics {
        let compact = text.replace(' ', "");
        round_trip(
            &format!("{lang}-full"),
            &cid_pdf(&[run_of(text)], false),
            &[&compact],
        );
        round_trip(
            &format!("{lang}-subset"),
            &cid_pdf(&[run_of(text)], true),
            &[&compact],
        );
    }
}

// ---- fonts touched by removal ----------------------------------------------------------------

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

fn path_len(font: &FontRef, gid: u16) -> usize {
    let mut c = Count(0);
    if let Some(o) = font.outline_glyphs().get(GlyphId::new(u32::from(gid))) {
        let _ = o.draw(
            DrawSettings::unhinted(Size::unscaled(), LocationRef::default()),
            &mut c,
        );
    }
    c.0
}

#[test]
fn emptied_glyphs_keep_ids_widths_and_composite_parts() {
    let full = sans();
    let font = FontRef::new(&full).unwrap();
    // U+0105 is built from a base letter plus an ogonek component.
    let (a_ogonek, a, b) = (gid_of(&font, 'ą'), gid_of(&font, 'a'), gid_of(&font, 'b'));
    let out = empty_unused_truetype_glyphs(&full, &BTreeSet::from([u32::from(a_ogonek)])).unwrap();
    let after = FontRef::new(&out).unwrap();
    assert!(out.len() < full.len() / 2, "outlines were not removed");
    assert_eq!(
        after.head().unwrap().units_per_em(),
        font.head().unwrap().units_per_em()
    );
    assert_eq!(
        after.maxp().unwrap().num_glyphs(),
        font.maxp().unwrap().num_glyphs(),
        "glyph ids must stay"
    );
    assert!(
        path_len(&after, a_ogonek) > 0 && path_len(&after, a) > 0,
        "kept glyph or its component lost"
    );
    assert_eq!(path_len(&after, b), 0, "unused glyph still has an outline");
    assert_eq!(
        advance_1000(&after, b),
        advance_1000(&font, b),
        "hmtx must stay"
    );
    // A valid sfnt sums to 0xB1B0AFBA once head.checkSumAdjustment is in place.
    let sum = out.chunks(4).fold(0u32, |s, c| {
        let mut w = [0u8; 4];
        w[..c.len()].copy_from_slice(c);
        s.wrapping_add(u32::from_be_bytes(w))
    });
    assert_eq!(sum, 0xB1B0_AFBA);
}

const PROBE_FONT: &str = "
import sys, fitz
d = fitz.open(sys.argv[1])
for x in range(1, d.xref_length()):
    if d.xref_get_key(x, 'Length1')[0] != 'null':
        open(sys.argv[2], 'wb').write(d.xref_stream(x))
        break
else:
    sys.exit('no font program')
";

const PROBE_TEXT: &str = "
import sys, json, fitz
from pypdf import PdfReader
want, banned = json.loads(sys.argv[2]), json.loads(sys.argv[3])
fz = ''.join(fitz.open(sys.argv[1])[0].get_text().split())
py = ''.join((PdfReader(sys.argv[1]).pages[0].extract_text() or '').split())
bad = [(n, w) for n, t in (('fitz', fz), ('pypdf', py)) for w in want if w not in t]
bad += [(n, 'present: ' + w) for n, t in (('fitz', fz), ('pypdf', py)) for w in banned if w in t]
if bad:
    sys.exit('%s fitz=%r pypdf=%r' % (bad, fz, py))
";

#[test]
fn removed_codes_lose_their_unicode_and_their_outline() {
    let lines = [run_of("Alpha Beta Gamma")];
    let input = cid_pdf(&lines, false);
    let doc = Document::open(input, None).unwrap();
    let model = doc.page_model(0).unwrap();
    let font_id = *model.fonts.keys().next().expect("an indirect font");
    let glyph_items: Vec<_> = model
        .items
        .iter()
        .filter_map(|i| match &i.kind {
            foliark_engine::interpret::ItemKind::Glyph(g) => Some(g),
            _ => None,
        })
        .collect();
    assert_eq!(glyph_items.len(), "Alpha Beta Gamma".len());
    let survivors = &glyph_items[..5]; // "Alpha"
    let mut w = Writer::new(doc.reader(), Limits::default());
    let outcome = touch_font(&mut w, font_id, survivors.iter().copied()).unwrap();
    let Program::Emptied { kept, emptied } = outcome else {
        panic!("expected an emptied TrueType program, got {outcome:?}");
    };
    assert!(
        kept >= 4 && emptied > 1000,
        "{kept} kept, {emptied} emptied"
    );
    let out = w.write().unwrap();
    let outp = tmp("touched.out.pdf");
    std::fs::write(&outp, &out).unwrap();
    // The page still draws "Beta Gamma" codes, but no oracle can read them any more.
    let (ok, log) = python(&[
        "-c",
        PROBE_TEXT,
        outp.to_str().unwrap(),
        "[\"Alpha\"]",
        "[\"Beta\", \"Gamma\"]",
    ]);
    assert!(ok, "{log}");
    let program = tmp("touched.font.ttf");
    let (ok, log) = python(&[
        "-c",
        PROBE_FONT,
        outp.to_str().unwrap(),
        program.to_str().unwrap(),
    ]);
    assert!(ok, "{log}");
    let bytes = std::fs::read(&program).unwrap();
    let emptied_font = FontRef::new(&bytes).unwrap();
    let original = sans();
    let original = FontRef::new(&original).unwrap();
    for c in "Alph".chars() {
        assert!(
            path_len(&emptied_font, gid_of(&original, c)) > 0,
            "{c} lost its outline"
        );
    }
    for c in "BetGm".chars() {
        assert_eq!(
            path_len(&emptied_font, gid_of(&original, c)),
            0,
            "{c} kept its outline"
        );
    }
    // The untouched original has all of them (the check above is not vacuous).
    assert!(path_len(&original, gid_of(&original, 'B')) > 0);
}

// ---- registered font embedding ---------------------------------------------------------------

const PROBE_PLACED: &str = "
import sys, json, fitz
from pypdf import PdfReader
lines, tol = json.loads(sys.argv[2]), 1.0
page = fitz.open(sys.argv[1])[0]
spans = [s for b in page.get_text('dict')['blocks'] for l in b.get('lines', []) for s in l['spans']]
py = ''.join((PdfReader(sys.argv[1]).pages[0].extract_text() or '').split())
bad = []
for text, x, y in lines:
    hit = [s for s in spans if ''.join(s['text'].split()) == ''.join(text.split())]
    if not hit:
        bad.append('fitz has no span %r in %r' % (text, [s['text'] for s in spans]))
    elif abs(hit[0]['origin'][0] - x) > tol or abs(hit[0]['origin'][1] - y) > tol:
        bad.append('%r at %r, want %r' % (text, hit[0]['origin'], (x, y)))
    if ''.join(text.split()) not in py:
        bad.append('pypdf misses %r in %r' % (text, py))
    if not page.search_for(text.split()[0]):
        bad.append('fitz search misses %r' % text.split()[0])
if bad:
    sys.exit('\\n'.join(bad))
";

#[test]
fn registered_liberation_sans_run_is_searchable_in_both_oracles() {
    let mut raw = Raw::default();
    page_with(
        &mut raw,
        |r| {
            r.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
        },
        "",
    );
    let doc = Document::open(raw.finish(), None).unwrap();

    let mut reg = Registry::new();
    let info = reg
        .register(FontSource {
            bytes: sans(),
            family: "Liberation Sans".into(),
            weight: 400,
            style: FontStyle::Normal,
        })
        .unwrap();
    let text = "ąčęėįšųūž ĄČĘĖĮŠŲŪŽ\nfind the office żółć āēīū õäöü";
    let params = LayoutParams {
        font_size: 12.0,
        wrap_width: 400.0,
        max_lines: 10,
        line_step: 14.0,
        first_baseline: 100.0,
        origin_x: 50.0,
    };
    let ladder = Ladder {
        registry: &reg,
        caller: None,
        source: None,
        source_name: "Arial",
        flags: StyleFlags::default(),
        font_match: &[],
    };
    let plan = layout_paragraph(text, &params, &ladder).unwrap();
    assert_eq!(plan.lines.len(), 2);

    let mut used = UsedGlyphs::new();
    used.add_plan(&plan, &info.id, &normalize(text).unwrap());
    let mut w = Writer::new(doc.reader(), Limits::default());
    let embedded = embed_registered_font(&mut w, reg.get(&info.id).unwrap(), &used).unwrap();
    assert!(
        embed_registered_font(&mut w, reg.get(&info.id).unwrap(), &used).is_err(),
        "one font object per (font, document)"
    );
    w.add_page_font(0, "FoliarkF1", embedded.font.clone())
        .unwrap();
    let fonts = BTreeMap::from([(info.id.as_str(), ("FoliarkF1", &embedded))]);
    let ops = emit_plan(
        &plan,
        &fonts,
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0, -1.0, 0.0, 792.0],
    )
    .unwrap();
    w.append_page_content(0, &ops).unwrap();
    let out = w.write().unwrap();
    assert!(out.len() < 60_000, "subset too large: {} bytes", out.len());
    let outp = tmp("registered.out.pdf");
    std::fs::write(&outp, &out).unwrap();

    let expect: Vec<_> = plan
        .lines
        .iter()
        .zip(["ąčęėįšųūž ĄČĘĖĮŠŲŪŽ", "find the office żółć āēīū õäöü"])
        .map(|(l, t)| format!("[\"{t}\", {}, {}]", l.x, l.baseline_y))
        .collect();
    let (ok, log) = python(&[
        "-c",
        PROBE_PLACED,
        outp.to_str().unwrap(),
        &format!("[{}]", expect.join(",")),
    ]);
    assert!(ok, "{log}");
    let (ok, log) = python(&["tests/oracles/objects.py", outp.to_str().unwrap()]);
    assert!(ok, "objects.py: {log}");
}
