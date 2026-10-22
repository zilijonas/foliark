use foliark_model::{
    ErrorCode, FontMatchRule, FontRule, FontSource, FontStyle, PdfError, RunSource,
};
use foliark_text::{
    FaceMetrics, FontFace, Glyph, Ladder, LayoutParams, LayoutPlan, Registry, StyleFlags,
    layout_paragraph, normalize,
};

const SIZE: f64 = 12.0;

fn load(file: &str, family: &str, weight: u16) -> FontSource {
    let path = format!("{}/tests/data/{file}", env!("CARGO_MANIFEST_DIR"));
    FontSource {
        bytes: std::fs::read(path).unwrap(),
        family: family.into(),
        weight,
        style: FontStyle::Normal,
    }
}

fn registry() -> Registry {
    let mut r = Registry::new();
    for (f, fam, w) in [
        ("LiberationSans-Regular.ttf", "Liberation Sans", 400),
        ("LiberationSans-Bold.ttf", "Liberation Sans", 700),
        ("LiberationSerif-Regular.ttf", "Liberation Serif", 400),
        ("LiberationMono-Regular.ttf", "Liberation Mono", 400),
        ("Carlito-Regular.ttf", "Carlito", 400),
    ] {
        r.register(load(f, fam, w)).unwrap();
    }
    r
}

fn id(r: &Registry, family: &str, bold: bool) -> String {
    r.lookup(family, bold, false).unwrap().info().id.clone()
}

fn params(wrap_width: f64, max_lines: u32) -> LayoutParams {
    LayoutParams {
        font_size: SIZE,
        wrap_width,
        max_lines,
        line_step: 14.0,
        first_baseline: 100.0,
        origin_x: 50.0,
    }
}

fn ladder<'a>(r: &'a Registry, name: &'a str, flags: StyleFlags) -> Ladder<'a> {
    Ladder {
        registry: r,
        caller: None,
        source: None,
        source_name: name,
        flags,
        font_match: &[],
    }
}

fn advance(r: &Registry, family: &str, text: &str) -> f64 {
    let g = r
        .lookup(family, false, false)
        .unwrap()
        .shape(text, SIZE)
        .unwrap();
    g.iter().map(|g| g.x_advance).sum()
}

fn line_width(plan: &LayoutPlan, k: usize) -> f64 {
    plan.lines[k]
        .runs
        .iter()
        .flat_map(|r| &r.glyphs)
        .map(|g| g.x_advance)
        .sum()
}

/// The input text each line stands for, read back through the glyph cluster ranges.
fn line_texts(plan: &LayoutPlan, input: &str) -> Vec<String> {
    let units: Vec<u16> = normalize(input).unwrap().encode_utf16().collect();
    plan.lines
        .iter()
        .map(|l| {
            let mut seen = 0;
            let mut s = String::new();
            for g in l.runs.iter().flat_map(|r| &r.glyphs) {
                if g.cluster.start >= seen {
                    s += &String::from_utf16(
                        &units[g.cluster.start as usize..g.cluster.end as usize],
                    )
                    .unwrap();
                    seen = g.cluster.end;
                }
            }
            s
        })
        .collect()
}

fn refusal(r: Result<LayoutPlan, PdfError>) -> PdfError {
    r.expect_err("expected a typed refusal")
}

#[test]
fn hard_breaks_indent_and_whitespace_rules() {
    let r = registry();
    let l = ladder(&r, "Arial", StyleFlags::default());
    let input = "  ab   cd \r\n\r\nef\rg";
    let plan = layout_paragraph(input, &params(400.0, 10), &l).unwrap();
    assert_eq!(line_texts(&plan, input), ["ab cd", "", "ef", "g"]);
    // The indent is not drawn: it shifts x by exactly two space advances.
    let sp = advance(&r, "Liberation Sans", "  ");
    assert!((plan.lines[0].x - (50.0 + sp)).abs() < 1e-9);
    assert_eq!(plan.lines[1].x, 50.0);
    assert!(plan.lines[1].runs.is_empty());
    for (k, line) in plan.lines.iter().enumerate() {
        assert_eq!(line.baseline_y, 100.0 + 14.0 * k as f64);
    }
    // Interior runs of spaces collapse to the width of one.
    let one = advance(&r, "Liberation Sans", "ab cd");
    assert!((line_width(&plan, 0) - one).abs() < 1e-9);
    // An indent wider than the wrap width leaves no room for the first word.
    assert_eq!(
        refusal(layout_paragraph("          ab", &params(30.0, 5), &l)).code,
        ErrorCode::EditNoFit
    );
    // A trailing LF is a real empty last line; empty text is zero lines.
    assert_eq!(
        layout_paragraph("a\n", &params(400.0, 5), &l)
            .unwrap()
            .lines
            .len(),
        2
    );
    assert!(
        layout_paragraph("", &params(400.0, 0), &l)
            .unwrap()
            .lines
            .is_empty()
    );
}

#[test]
fn greedy_wrap_matches_measured_advances() {
    let r = registry();
    let l = ladder(&r, "Arial", StyleFlags::default());
    let input = "alpha beta gamma";
    let fits = advance(&r, "Liberation Sans", "alpha beta");
    let plan = layout_paragraph(input, &params(fits, 10), &l).unwrap();
    assert_eq!(line_texts(&plan, input), ["alpha beta", "gamma"]);
    assert!((line_width(&plan, 0) - fits).abs() < 1e-9);
    // Consumed spaces are not drawn at a line end.
    assert!(!line_texts(&plan, input)[0].ends_with(' '));
    // One hair narrower and the second word no longer fits.
    let plan = layout_paragraph(input, &params(fits - 0.01, 10), &l).unwrap();
    assert_eq!(line_texts(&plan, input)[..2], ["alpha", "beta"]);
    for k in 0..plan.lines.len() {
        assert!(line_width(&plan, k) <= fits - 0.01 + 1e-9);
    }
    // A hyphen is a break opportunity that keeps the hyphen on the first line, with no space consumed.
    let input = "ab well-known";
    let w = advance(&r, "Liberation Sans", "ab well-");
    let plan = layout_paragraph(input, &params(w, 10), &l).unwrap();
    assert_eq!(line_texts(&plan, input), ["ab well-", "known"]);
    let plan = layout_paragraph(input, &params(w - 0.01, 10), &l).unwrap();
    assert_eq!(line_texts(&plan, input), ["ab", "well-", "known"]);
}

#[test]
fn ligatures_keep_cluster_mapping() {
    let r = registry();
    let carlito = r.lookup("Carlito", false, false).unwrap();
    let g = carlito.shape("fi", SIZE).unwrap();
    assert_eq!(g.len(), 1, "Carlito has an fi ligature");
    assert_eq!(g[0].cluster, 0..2);
    let l = ladder(&r, "Calibri", StyleFlags::default());
    let input = "office fl";
    let plan = layout_paragraph(input, &params(400.0, 2), &l).unwrap();
    let glyphs: Vec<&Glyph> = plan.lines[0].runs.iter().flat_map(|r| &r.glyphs).collect();
    assert!(glyphs.len() < input.chars().count());
    // Ranges tile the text exactly: no gaps, no overlap, offsets are paragraph-relative.
    let mut at = 0;
    for g in &glyphs {
        assert_eq!(g.cluster.start, at);
        at = g.cluster.end;
    }
    assert_eq!(at, input.len() as u32);
    // `clusters` counts characters, not glyphs.
    assert_eq!(
        plan.lines[0].runs.iter().map(|r| r.clusters).sum::<u32>(),
        9
    );
}

#[test]
fn baltic_polish_diacritics_and_combining_input() {
    let r = registry();
    let l = ladder(&r, "Calibri", StyleFlags::default());
    let text = "ąčęėįšųūž āēīūļķņģ õäöü łśźżń";
    let plan = layout_paragraph(text, &params(1000.0, 1), &l).unwrap();
    let glyphs: Vec<&Glyph> = plan.lines[0].runs.iter().flat_map(|r| &r.glyphs).collect();
    assert_eq!(glyphs.len(), text.chars().count());
    assert!(glyphs.iter().all(|g| g.gid != 0 && g.x_advance > 0.0));
    // Decomposed input (base + combining mark) is NFC-normalized to the same plan.
    let decomposed = "a\u{328}c\u{30c}e\u{328}e\u{307}i\u{328}s\u{30c}u\u{328}u\u{304}z\u{30c} \
                      a\u{304}e\u{304}i\u{304}u\u{304}l\u{327}k\u{327}n\u{327}g\u{327} \
                      o\u{303}a\u{308}o\u{308}u\u{308} łs\u{301}z\u{301}z\u{307}n\u{301}";
    let composed = "ąčęėįšųūž āēīūļķņģ õäöü łśźżń";
    assert!(normalize(decomposed).unwrap().chars().count() < decomposed.chars().count());
    let a = layout_paragraph(composed, &params(1000.0, 1), &l).unwrap();
    let b = layout_paragraph(decomposed, &params(1000.0, 1), &l).unwrap();
    assert_eq!(a.lines, b.lines);
    assert_eq!(a.fingerprint, b.fingerprint);
}

#[test]
fn combining_mark_stays_with_its_base_cluster() {
    // No precomposed form: base and mark must come out as one cluster in one font.
    let r = registry();
    let l = ladder(&r, "Arial", StyleFlags::default());
    let plan = layout_paragraph("q\u{307}x", &params(400.0, 1), &l).unwrap();
    assert_eq!(
        plan.lines[0].runs.iter().map(|r| r.clusters).sum::<u32>(),
        2
    );
}

/// A source font stand-in: ASCII only, 500/1000 em advances, gid = character code.
struct AsciiSource;

impl FontFace for AsciiSource {
    fn id(&self) -> &str {
        "src-F1"
    }
    fn source(&self) -> RunSource {
        RunSource::Embedded
    }
    fn covers(&self, cluster: &str) -> bool {
        cluster.is_ascii()
    }
    fn shape(&self, text: &str, size: f64) -> Result<Vec<Glyph>, PdfError> {
        let mut at = 0;
        Ok(text
            .chars()
            .map(|c| {
                let g = Glyph {
                    gid: c as u32,
                    x_advance: 0.5 * size,
                    cluster: at..at + 1,
                };
                at += 1;
                g
            })
            .collect())
    }
    fn metrics(&self) -> FaceMetrics {
        FaceMetrics {
            ascent: 0.9,
            descent: 0.2,
        }
    }
}

#[test]
fn ladder_rungs_per_cluster() {
    let r = registry();
    let src = AsciiSource;
    let mut l = ladder(&r, "ABCDEF+Calibri-Bold", StyleFlags::default());

    // Rung 2: a fully covered paragraph stays in the source font and is exact.
    l.source = Some(&src);
    let plan = layout_paragraph("ab cd", &params(400.0, 1), &l).unwrap();
    assert!(!plan.approximate);
    assert_eq!(plan.lines[0].runs.len(), 1);
    assert_eq!(plan.lines[0].runs[0].source, RunSource::Embedded);
    assert_eq!(line_width(&plan, 0), 5.0 * 0.5 * SIZE);

    // One uncovered cluster moves just that cluster to the name-matched font (subset prefix ignored).
    let plan = layout_paragraph("ab\u{105}cd", &params(400.0, 1), &l).unwrap();
    assert!(plan.approximate);
    let runs = &plan.lines[0].runs;
    assert_eq!(
        runs.iter()
            .map(|r| (r.font.as_str(), r.clusters))
            .collect::<Vec<_>>(),
        [
            ("src-F1", 2),
            (id(&r, "Carlito", false).as_str(), 1),
            ("src-F1", 2)
        ]
    );
    assert_eq!(runs[1].rule, Some(FontRule::NameMatch));
    assert_eq!(plan.edit_layout("e1").runs.len(), 2);

    // Rung 1 beats everything for the clusters it covers.
    let serif = id(&r, "Liberation Serif", false);
    l.caller = Some(&serif);
    let plan = layout_paragraph("ab", &params(400.0, 1), &l).unwrap();
    assert_eq!(
        (
            plan.lines[0].runs[0].font.as_str(),
            plan.lines[0].runs[0].rule
        ),
        (serif.as_str(), Some(FontRule::Caller))
    );
    l.caller = Some("font-missing");
    assert_eq!(
        refusal(layout_paragraph("ab", &params(400.0, 1), &l)).code,
        ErrorCode::InvalidArgument
    );
}

#[test]
fn ladder_name_match_rules_and_flags_fallback() {
    let r = registry();
    let first = |l: &Ladder| {
        let p = layout_paragraph("ab", &params(400.0, 1), l).unwrap();
        (p.lines[0].runs[0].font.clone(), p.lines[0].runs[0].rule)
    };
    let flags = |mono, serif, bold| StyleFlags {
        mono,
        serif,
        bold,
        italic: false,
    };
    // Name match, DEFAULT_FONT_MATCH order, case-insensitive.
    assert_eq!(
        first(&ladder(&r, "XYZABC+CALIBRI", flags(false, false, false))),
        (id(&r, "Carlito", false), Some(FontRule::NameMatch))
    );
    assert_eq!(
        first(&ladder(&r, "TimesNewRomanPSMT", flags(false, false, false))),
        (id(&r, "Liberation Serif", false), Some(FontRule::NameMatch))
    );
    // Caller rules come first; a rule naming an unregistered family is skipped.
    let rules = [
        FontMatchRule {
            pattern: "fancy".into(),
            family: "Not Registered".into(),
        },
        FontMatchRule {
            pattern: "fancy".into(),
            family: "Liberation Mono".into(),
        },
    ];
    let mut l = ladder(&r, "FancySans", flags(false, false, false));
    l.font_match = &rules;
    assert_eq!(
        first(&l),
        (id(&r, "Liberation Mono", false), Some(FontRule::NameMatch))
    );
    // No name match: flags pick the face, bold uses the registered bold weight.
    for (f, fam, bold) in [
        (flags(true, false, false), "Liberation Mono", false),
        (flags(false, true, false), "Liberation Serif", false),
        (flags(false, false, true), "Liberation Sans", true),
        (flags(false, false, false), "Liberation Sans", false),
    ] {
        assert_eq!(
            first(&ladder(&r, "Unknown", f)),
            (id(&r, fam, bold), Some(FontRule::Flags)),
            "{fam}"
        );
    }
}

#[test]
fn missing_glyph_is_a_refusal_not_a_notdef() {
    let r = registry();
    let l = ladder(&r, "Arial", StyleFlags::default());
    // U+A7FF is allowed by §6.1 but no test font has it.
    let e = refusal(layout_paragraph(
        "a\u{a7ff}b\u{a7ff}",
        &params(400.0, 1),
        &l,
    ));
    assert_eq!(e.code, ErrorCode::EditNoFont);
    assert_eq!(e.details.unwrap().chars.as_deref(), Some("\u{a7ff}"));
    // With no registered fonts at all every character is missing.
    let empty = Registry::new();
    let e = refusal(layout_paragraph(
        "ab",
        &params(400.0, 1),
        &ladder(&empty, "Arial", StyleFlags::default()),
    ));
    assert_eq!(
        (
            e.code,
            e.details.unwrap().chars.as_deref().map(str::to_owned)
        ),
        (ErrorCode::EditNoFont, Some("ab".into()))
    );
}

#[test]
fn overflow_refuses_with_numbers_and_never_shrinks() {
    let r = registry();
    let l = ladder(&r, "Arial", StyleFlags::default());
    let input = "aaaa bbbb cccc dddd";
    let one_word = advance(&r, "Liberation Sans", "aaaa");
    // Wide enough for one word per line only: four lines needed.
    let wrap = one_word * 1.5;
    let e = refusal(layout_paragraph(input, &params(wrap, 2), &l));
    let d = e.details.unwrap();
    assert_eq!(
        (e.code, d.needed_lines, d.max_lines),
        (ErrorCode::EditNoFit, Some(4), Some(2))
    );
    let plan = layout_paragraph(input, &params(wrap, 4), &l).unwrap();
    assert_eq!(plan.lines.len(), 4);
    assert!(
        plan.lines
            .iter()
            .flat_map(|l| &l.runs)
            .all(|r| r.size == SIZE)
    );
    // A single word wider than the wrap width is refused even when the line count would fit.
    let e = refusal(layout_paragraph("aaaa", &params(one_word - 0.5, 9), &l));
    let d = e.details.unwrap();
    assert_eq!(
        (e.code, d.needed_lines, d.max_lines),
        (ErrorCode::EditNoFit, Some(10), Some(9))
    );
}

#[test]
fn unsupported_scripts_and_controls_are_refused_with_the_characters() {
    let r = registry();
    let l = ladder(&r, "Arial", StyleFlags::default());
    for (input, chars) in [
        (
            "Hello \u{645}\u{631}\u{62d}\u{628}\u{627}",
            "\u{645}\u{631}\u{62d}\u{628}\u{627}",
        ),
        ("日本日", "日本"),
        ("a\tb\u{202e}", "\t\u{202e}"),
        ("emoji \u{1f600}", "\u{1f600}"),
    ] {
        let e = refusal(layout_paragraph(input, &params(400.0, 1), &l));
        assert_eq!(e.code, ErrorCode::EditUnsupportedScript, "{input}");
        assert_eq!(e.details.unwrap().chars.as_deref(), Some(chars));
    }
}

#[test]
fn plans_are_deterministic_and_fingerprint_every_input() {
    let r = registry();
    let l = ladder(&r, "Arial", StyleFlags::default());
    let p = params(200.0, 3);
    let a = layout_paragraph("same text here", &p, &l).unwrap();
    assert_eq!(a, layout_paragraph("same text here", &p, &l).unwrap());
    let fp = |t: &str, p: &LayoutParams, l: &Ladder| layout_paragraph(t, p, l).unwrap().fingerprint;
    assert_eq!(
        a.fingerprint,
        fp("same  text here ", &p, &l),
        "normalized input, same plan"
    );
    assert_ne!(a.fingerprint, fp("same text there", &p, &l));
    assert_ne!(a.fingerprint, fp("same text here", &params(201.0, 3), &l));
    assert_ne!(a.fingerprint, fp("same text here", &params(200.0, 4), &l));
    assert_ne!(
        a.fingerprint,
        fp(
            "same text here",
            &p,
            &ladder(
                &r,
                "Arial",
                StyleFlags {
                    bold: true,
                    ..Default::default()
                }
            )
        )
    );
    // Registering any font invalidates plans (fontRegistryVersion).
    let mut r2 = registry();
    r2.register(load("LiberationSans-Regular.ttf", "Liberation Sans", 400))
        .unwrap();
    assert_ne!(
        a.fingerprint,
        fp(
            "same text here",
            &p,
            &ladder(&r2, "Arial", StyleFlags::default())
        )
    );
}

#[test]
fn registry_rejects_what_it_cannot_lay_out() {
    let mut r = Registry::new();
    let good = load("LiberationSans-Regular.ttf", "Liberation Sans", 400);
    let reg = r.register(good.clone()).unwrap();
    assert_eq!(
        (r.version(), reg.postscript_name.as_str()),
        (1, "LiberationSans")
    );
    assert!(reg.glyph_count > 1000);
    // Ids depend only on the bytes and declared face: stable across registries.
    assert_eq!(Registry::new().register(good.clone()).unwrap().id, reg.id);
    assert_eq!(r.register(good.clone()).unwrap().id, reg.id);
    assert_eq!(r.version(), 2);

    let bad = |bytes: Vec<u8>| FontSource {
        bytes,
        ..good.clone()
    };
    let mut ttc = b"ttcf\x00\x01\x00\x00\x00\x00\x00\x01\x00\x00\x00\x10".to_vec();
    ttc.extend_from_slice(&good.bytes[..64]);
    let mut truncated = good.bytes.clone();
    truncated.truncate(300);
    for (name, src) in [
        ("collection", bad(ttc)),
        ("garbage", bad(b"not a font at all".to_vec())),
        ("truncated", bad(truncated)),
        ("empty", bad(Vec::new())),
    ] {
        let e = r.register(src).expect_err(name);
        assert_eq!(e.code, ErrorCode::FontUnsupported, "{name}");
    }
    assert_eq!(r.version(), 2, "failures do not bump the version");
    assert_eq!(
        r.register(FontSource {
            weight: 500,
            ..good
        })
        .unwrap_err()
        .code,
        ErrorCode::InvalidArgument
    );
}
