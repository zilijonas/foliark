//! Image erasure through the writer (spec §5.2; NEW-34). Every expectation is computed from
//! placement geometry in the test (analytic grids, or a polygon clipper that shares no code
//! with the engine), and every output is read back by pikepdf (decoded samples) and by the
//! tests/oracles objects.py image-region check (PyMuPDF), never by the engine itself.
mod image_pdf;
use foliark_engine::redaction::image::redact_images;
use foliark_engine::writer::Writer;
use foliark_engine::{Document, interpret::ItemKind};
use foliark_model::{ErrorCode, Limits};
use image_pdf::*;
use serde_json::Value;

const W: usize = 8;

fn img8() -> Vec<u8> {
    rgb_image(W, W, "")
}

fn one_image(extra: &str, content: &str, image: Vec<u8>) -> Vec<u8> {
    pdf(&[page(extra, content, &[("Im0", first_obj(1))])], &[image])
}

fn bytes(img: &Value) -> Vec<u8> {
    bytes_of(&img["data"])
}

/// Decoded RGB output of the only image drawn on page 0 must be `px` outside `erased` and
/// black inside; returns how many samples were erased.
fn assert_grid(img: &Value, w: usize, h: usize, erased: &dyn Fn(usize, usize) -> bool) -> usize {
    assert_eq!(img["cs"], "/DeviceRGB");
    assert_eq!(img["bpc"], 8);
    assert_eq!(img["filter"], "/FlateDecode");
    assert_eq!(
        (img["w"].as_u64(), img["h"].as_u64()),
        (Some(w as u64), Some(h as u64))
    );
    let data = bytes(img);
    assert_eq!(data.len(), w * h * 3);
    let mut n = 0;
    for j in 0..h {
        for i in 0..w {
            let got = &data[(j * w + i) * 3..][..3];
            if erased(i, j) {
                n += 1;
                assert_eq!(got, [0, 0, 0], "sample ({i},{j}) must be erased");
            } else {
                assert_eq!(
                    got,
                    px(i, j),
                    "sample ({i},{j}) is public and must be unchanged"
                );
            }
        }
    }
    n
}

fn page_image(out: &Out, page: usize) -> Value {
    let s = samples(&out.path);
    let images = s["pages"][page]["images"].as_object().unwrap();
    assert_eq!(images.len(), 1, "one image resource on page {page}");
    images.values().next().unwrap().clone()
}

#[test]
fn partial_region_erases_exactly_the_samples_it_touches_and_keeps_the_rest() {
    // 8 x 8 samples of 10 pt at x 100..180, display y 120..200.
    let input = one_image("", "q 80 0 0 80 100 200 cm /Im0 Do Q", img8());
    let r = rect(125.0, 133.0, 153.0, 151.0);
    let out = run("partial", &input, &[(0, &[r])]).unwrap();
    let n = assert_grid(&page_image(&out, 0), W, W, &|i, j| {
        (2..=5).contains(&i) && (1..=3).contains(&j)
    });
    assert_eq!(n, 12);
    assert_eq!(
        (
            out.report.images_erased,
            out.report.images_cloned,
            out.report.inline_converted
        ),
        (1, 0, 0)
    );
    let (ok, text) = objects_check(&out.path, &[(0, [125.0, 133.0, 153.0, 151.0])]);
    assert!(ok, "{text}");
    // Control: the same check must catch the unredacted file.
    let leaky = tmp("partial-input.pdf");
    std::fs::write(&leaky, &input).unwrap();
    assert!(
        !objects_check(&leaky, &[(0, [125.0, 133.0, 153.0, 151.0])]).0,
        "the oracle must fail the input"
    );
    // The original (unfiltered) secret samples are in no image object of the output.
    let secret: Vec<u8> = (0..4).flat_map(|i| px(i + 2, 1)).collect();
    for im in samples(&out.path)["all"].as_array().unwrap() {
        assert!(
            !bytes(im).windows(secret.len()).any(|w| w == secret),
            "secret run survived"
        );
    }
}

#[test]
fn a_region_on_sample_edges_erases_the_touching_neighbours_too() {
    let input = one_image("", "q 80 0 0 80 100 200 cm /Im0 Do Q", img8());
    // Exactly on sample edges: x 120..150, display y 130..150. Samples that only touch are
    // erased (conservative): cols 1..=5, rows 0..=3.
    let out = run("edge", &input, &[(0, &[rect(120.0, 130.0, 150.0, 150.0)])]).unwrap();
    assert_grid(&page_image(&out, 0), W, W, &|i, j| {
        (1..=5).contains(&i) && (0..=3).contains(&j)
    });
}

#[test]
fn full_erasure_leaves_no_source_sample() {
    let input = one_image("", "q 80 0 0 80 100 200 cm /Im0 Do Q", img8());
    let out = run("full", &input, &[(0, &[rect(0.0, 0.0, 300.0, 400.0)])]).unwrap();
    assert_eq!(assert_grid(&page_image(&out, 0), W, W, &|_, _| true), 64);
}

#[test]
fn flips_and_half_turns_map_through_the_ctm() {
    // Vertical flip: row 0 sits at the bottom. Region y 133..151 reaches rows 4..=6.
    let flip = one_image("", "q 80 0 0 -80 100 280 cm /Im0 Do Q", img8());
    let out = run("flip", &flip, &[(0, &[rect(125.0, 133.0, 153.0, 151.0)])]).unwrap();
    assert_grid(&page_image(&out, 0), W, W, &|i, j| {
        (2..=5).contains(&i) && (4..=6).contains(&j)
    });
    // 180 degrees: columns run backwards as well, region x 105..133 reaches cols 4..=7.
    let half = one_image("", "q -80 0 0 -80 180 280 cm /Im0 Do Q", img8());
    let out = run("half", &half, &[(0, &[rect(105.0, 133.0, 133.0, 151.0)])]).unwrap();
    assert_grid(&page_image(&out, 0), W, W, &|i, j| {
        (4..=7).contains(&i) && (4..=6).contains(&j)
    });
    let (ok, text) = objects_check(&out.path, &[(0, [105.0, 133.0, 133.0, 151.0])]);
    assert!(ok, "{text}");
}

#[test]
fn page_rotate_and_cropbox_are_part_of_the_mapping() {
    // Display space of /Rotate 90 over CropBox [20 30 280 380]: (x, y) -> (y - 30, x - 20).
    // 10 x 10 samples of 10 pt at user x 60..160, y 100..200: sample (i, j) sits at display
    // x [160 - 10j, 170 - 10j], y [40 + 10i, 50 + 10i].
    let input = pdf(
        &[page(
            "/Rotate 90 /CropBox [20 30 280 380]",
            "q 100 0 0 100 60 100 cm /Im0 Do Q",
            &[("Im0", first_obj(1))],
        )],
        &[rgb_image(10, 10, "")],
    );
    let r = rect(101.0, 71.0, 124.0, 94.0);
    let out = run("rotate", &input, &[(0, &[r])]).unwrap();
    let n = assert_grid(&page_image(&out, 0), 10, 10, &|i, j| {
        (3..=5).contains(&i) && (4..=6).contains(&j)
    });
    assert_eq!(n, 9);
    let (ok, text) = objects_check(&out.path, &[(0, [101.0, 71.0, 124.0, 94.0])]);
    assert!(ok, "{text}");
}

/// Area of a convex polygon clipped to an axis-aligned rectangle (Sutherland-Hodgman).
fn clipped_area(poly: &[(f64, f64)], r: [f64; 4]) -> f64 {
    let mut p: Vec<[f64; 2]> = poly.iter().map(|&(x, y)| [x, y]).collect();
    // (axis, bound, keep the side >= bound)
    for (axis, bound, ge) in [
        (0, r[0], true),
        (0, r[2], false),
        (1, r[1], true),
        (1, r[3], false),
    ] {
        let inside = |q: [f64; 2]| {
            if ge {
                q[axis] >= bound
            } else {
                q[axis] <= bound
            }
        };
        let cut = |a: [f64; 2], b: [f64; 2]| {
            let t = (bound - a[axis]) / (b[axis] - a[axis]);
            let mut c = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
            c[axis] = bound;
            c
        };
        let input = std::mem::take(&mut p);
        for k in 0..input.len() {
            let (a, b) = (input[k], input[(k + 1) % input.len()]);
            match (inside(a), inside(b)) {
                (true, true) => p.push(b),
                (true, false) => p.push(cut(a, b)),
                (false, true) => {
                    p.push(cut(a, b));
                    p.push(b);
                }
                _ => {}
            }
        }
        if p.is_empty() {
            return 0.0;
        }
    }
    (0..p.len())
        .map(|k| p[k][0] * p[(k + 1) % p.len()][1] - p[(k + 1) % p.len()][0] * p[k][1])
        .sum::<f64>()
        .abs()
        / 2.0
}

#[test]
fn rotated_and_skewed_placements_erase_by_pixel_square_intersection() {
    // 12 x 9 samples under a 25 degree rotation with a shear: cm = [a b c d e f].
    let (w, h) = (12usize, 9usize);
    let t = 25f64.to_radians();
    let (a, b, c, d, e, f) = (
        90.0 * t.cos(),
        90.0 * t.sin(),
        -60.0 * t.sin() + 12.0,
        60.0 * t.cos(),
        70.0,
        150.0,
    );
    let content = format!("q {a} {b} {c} {d} {e} {f} cm /Im0 Do Q");
    let input = one_image("", &content, rgb_image(w, h, ""));
    // The display space of an unrotated page is y-down: (x, 400 - y).
    let corner = |u: f64, v: f64| (a * u + c * v + e, 400.0 - (b * u + d * v + f));
    let square = |i: usize, j: usize| {
        let (u0, u1) = (i as f64 / w as f64, (i + 1) as f64 / w as f64);
        let (v0, v1) = (1.0 - (j + 1) as f64 / h as f64, 1.0 - j as f64 / h as f64);
        [
            corner(u0, v0),
            corner(u1, v0),
            corner(u1, v1),
            corner(u0, v1),
        ]
    };
    let region = [105.0, 210.0, 160.0, 260.0];
    let out = run(
        "skew",
        &input,
        &[(0, &[rect(region[0], region[1], region[2], region[3])])],
    )
    .unwrap();
    let got = bytes(&page_image(&out, 0));
    let grown = [
        region[0] - 1e-6,
        region[1] - 1e-6,
        region[2] + 1e-6,
        region[3] + 1e-6,
    ];
    let (mut must, mut erased_n) = (0, 0);
    for j in 0..h {
        for i in 0..w {
            let sq = square(i, j);
            let erased = got[(j * w + i) * 3..][..3] == [0, 0, 0];
            if clipped_area(&sq, region) > 1e-9 {
                must += 1;
                assert!(
                    erased,
                    "sample ({i},{j}) overlaps the region and must be erased"
                );
            }
            if erased {
                erased_n += 1;
                assert!(
                    clipped_area(&sq, grown) > 0.0,
                    "sample ({i},{j}) is erased but nowhere near the region"
                );
            } else {
                assert_eq!(
                    got[(j * w + i) * 3..][..3],
                    px(i, j),
                    "public sample ({i},{j}) changed"
                );
            }
        }
    }
    assert!(
        must >= 6 && erased_n < w * h,
        "the region must cut a part of the image ({must}, {erased_n})"
    );
    let (ok, text) = objects_check(&out.path, &[(0, region)]);
    assert!(ok, "{text}");
}

#[test]
fn gray_indexed_and_cmyk_sources_become_rendered_gray_or_rgb_and_are_disclosed() {
    let gray: Vec<u8> = (0..16).map(|k| 40 + k as u8 * 10).collect();
    // 4 x 4 samples, 4 bits each: row = 2 bytes, indices cycle 0..3.
    let idx: Vec<u8> = (0..4)
        .flat_map(|j| {
            [
                (((j % 4) << 4) | ((j + 1) % 4)) as u8,
                ((((j + 2) % 4) << 4) | ((j + 3) % 4)) as u8,
            ]
        })
        .collect();
    let cmyk: Vec<u8> = (0..16).flat_map(|k| [0, 0, 0, 10 + k as u8 * 10]).collect();
    let base = first_obj(1);
    let input = pdf(
        &[page(
            "",
            "q 40 0 0 40 20 300 cm /G Do Q q 40 0 0 40 100 300 cm /I Do Q q 40 0 0 40 180 300 cm /K Do Q",
            &[("G", base), ("I", base + 1), ("K", base + 2)],
        )],
        &[
            stream(
                "/Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceGray /BitsPerComponent 8",
                &gray,
            ),
            stream(
                "/Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace [/Indexed /DeviceRGB 3 <C81E1E 1EC81E 1E1EC8 C8C81E>] /BitsPerComponent 4",
                &idx,
            ),
            stream(
                "/Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceCMYK /BitsPerComponent 8",
                &cmyk,
            ),
        ],
    );
    // Each image is 40 pt: display x 20..60 / 100..140 / 180..220, y 60..100. Erase the top-left
    // sample of each (10 pt) with a region just inside it.
    let regions = [
        rect(21.0, 61.0, 29.0, 69.0),
        rect(101.0, 61.0, 109.0, 69.0),
        rect(181.0, 61.0, 189.0, 69.0),
    ];
    let out = run("colors", &input, &[(0, &regions)]).unwrap();
    assert_eq!(
        (out.report.images_erased, out.report.color_converted),
        (3, 2),
        "gray is already device, the other two are converted"
    );
    let s = samples(&out.path);
    let images = &s["pages"][0]["images"];
    let g = &images["G"];
    assert_eq!(
        (g["cs"].as_str(), g["bpc"].as_u64(), g["filter"].as_str()),
        (Some("/DeviceGray"), Some(8), Some("/FlateDecode"))
    );
    let gd = bytes(g);
    assert_eq!(gd[0], 0);
    assert_eq!(&gd[1..], &gray[1..], "public gray samples unchanged");
    let i = &images["I"];
    assert_eq!(i["cs"], "/DeviceRGB");
    let pal = [
        [0xC8, 0x1E, 0x1E],
        [0x1E, 0xC8, 0x1E],
        [0x1E, 0x1E, 0xC8],
        [0xC8, 0xC8, 0x1E],
    ];
    let id = bytes(i);
    assert_eq!(&id[..3], [0, 0, 0]);
    for k in 1..16 {
        let (j, col) = (k / 4, k % 4);
        let nib = if col % 2 == 0 {
            idx[j * 2 + col / 2] >> 4
        } else {
            idx[j * 2 + col / 2] & 15
        };
        assert_eq!(&id[k * 3..][..3], pal[nib as usize], "indexed sample {k}");
    }
    let k = &images["K"];
    assert_eq!(k["cs"], "/DeviceRGB");
    let kd = bytes(k);
    assert_eq!(&kd[..3], [0, 0, 0]);
    assert!(
        kd[3..].chunks(3).all(|p| p != [0, 0, 0]),
        "public CMYK samples render to non-black RGB"
    );
    let (ok, text) = objects_check(
        &out.path,
        &[
            (0, [21.0, 61.0, 29.0, 69.0]),
            (0, [101.0, 61.0, 109.0, 69.0]),
            (0, [181.0, 61.0, 189.0, 69.0]),
        ],
    );
    assert!(ok, "{text}");
}

#[test]
fn a_dct_source_is_re_encoded_losslessly_without_its_compressed_bytes() {
    let jpeg = std::fs::read(root().join("crates/foliark-engine/tests/data/image16.jpg")).unwrap();
    let input = pdf(
        &[page(
            "",
            "q 160 0 0 160 50 100 cm /Im0 Do Q",
            &[("Im0", first_obj(1))],
        )],
        &[stream(
            "/Type /XObject /Subtype /Image /Width 16 /Height 16 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode",
            &jpeg,
        )],
    );
    // 10 pt samples at display x 50..210, y 140..300. Erase cols 4..=7, rows 6..=9.
    let out = run("dct", &input, &[(0, &[rect(91.0, 201.0, 129.0, 239.0)])]).unwrap();
    let img = page_image(&out, 0);
    assert_eq!(
        (img["cs"].as_str(), img["filter"].as_str()),
        (Some("/DeviceRGB"), Some("/FlateDecode"))
    );
    let want = jpeg_samples(&root().join("crates/foliark-engine/tests/data/image16.jpg"));
    let got = bytes(&img);
    for j in 0..16 {
        for i in 0..16 {
            let (g, w) = (
                &got[(j * 16 + i) * 3..][..3],
                &want[(j * 16 + i) * 3..][..3],
            );
            if (4..=7).contains(&i) && (6..=9).contains(&j) {
                assert_eq!(g, [0, 0, 0], "({i},{j})");
            } else {
                // Two JPEG decoders may differ in the last bits of the IDCT.
                assert!(
                    g.iter().zip(w).all(|(a, b)| a.abs_diff(*b) <= 3),
                    "({i},{j}): {g:?} vs {w:?}"
                );
            }
        }
    }
    assert!(
        !out.bytes.windows(4).any(|w| *w == jpeg[..4]),
        "JPEG magic bytes survived"
    );
}

#[test]
fn an_inline_image_becomes_an_xobject_drawn_by_do() {
    let raw: Vec<u8> = (0..W)
        .flat_map(|j| (0..W).flat_map(move |i| px(i, j)))
        .collect();
    let mut content =
        b"0.5 g 10 10 20 20 re f q 80 0 0 80 100 200 cm\nBI /W 8 /H 8 /CS /RGB /BPC 8 ID ".to_vec();
    content.extend_from_slice(&raw);
    content.extend_from_slice(b"\nEI Q\n0 0 1 rg 200 20 30 30 re f");
    let input = pdf(&[page("", &content, &[])], &[]);
    let out = run(
        "inline",
        &input,
        &[(0, &[rect(125.0, 133.0, 153.0, 151.0)])],
    )
    .unwrap();
    assert_eq!(
        (
            out.report.images_erased,
            out.report.inline_converted,
            out.report.images_cloned
        ),
        (1, 1, 0)
    );
    let s = samples(&out.path);
    let text = s["pages"][0]["content"].as_str().unwrap();
    assert!(
        !text.contains("BI") && !text.contains("EI") && !text.contains(" ID"),
        "inline image left in content: {text}"
    );
    assert!(text.contains(" Do"), "{text}");
    assert!(
        text.contains("200 20 30 30 re"),
        "neighbouring operators must stay"
    );
    assert_grid(&page_image(&out, 0), W, W, &|i, j| {
        (2..=5).contains(&i) && (1..=3).contains(&j)
    });
    let secret: Vec<u8> = (0..4).flat_map(|i| px(i + 2, 1)).collect();
    assert!(
        !out.bytes.windows(secret.len()).any(|w| w == secret),
        "raw secret samples in the file"
    );
    // Control for the byte search: the unredacted file does hold them.
    assert!(input.windows(secret.len()).any(|w| w == secret));
    // The oracle skips inline images by design, but the erased output is an XObject now.
    let (ok, text) = objects_check(&out.path, &[(0, [125.0, 133.0, 153.0, 151.0])]);
    assert!(ok, "{text}");
}

#[test]
fn undecodable_oversized_and_formed_images_are_refused_with_the_writer_untouched() {
    let regions: &[(u32, &[foliark_model::Rect])] = &[(0, &[rect(100.0, 100.0, 200.0, 300.0)])];
    let untouched = |doc: &Document| {
        Writer::new(doc.reader(), Limits::default())
            .write()
            .unwrap()
    };
    // Plain data shorter than the dimensions say: hayro would pad it, the strict path refuses.
    let short = pdf(
        &[page(
            "",
            "q 80 0 0 80 100 200 cm /Im0 Do Q",
            &[("Im0", first_obj(1))],
        )],
        &[stream(
            "/Type /XObject /Subtype /Image /Width 8 /Height 8 /ColorSpace /DeviceRGB /BitsPerComponent 8",
            &[9; 20],
        )],
    );
    let doc = Document::open(short, None).unwrap();
    let plan = make_plan(&doc, regions);
    let mut w = Writer::new(doc.reader(), Limits::default());
    let err = redact_images(&doc, &plan, &mut w).unwrap_err();
    assert_eq!(err.code, ErrorCode::DecodeFailed);
    assert_eq!(
        w.write().unwrap(),
        untouched(&doc),
        "a refused run must leave the writer as it was"
    );
    // Corrupt Flate.
    let bad = pdf(
        &[page(
            "",
            "q 80 0 0 80 100 200 cm /Im0 Do Q",
            &[("Im0", first_obj(1))],
        )],
        &[stream(
            "/Type /XObject /Subtype /Image /Width 8 /Height 8 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode",
            &[0x78, 0x9c, 1, 2, 3, 4, 5, 6],
        )],
    );
    let doc = Document::open(bad, None).unwrap();
    let plan = make_plan(&doc, regions);
    let mut w = Writer::new(doc.reader(), Limits::default());
    assert_eq!(
        redact_images(&doc, &plan, &mut w).unwrap_err().code,
        ErrorCode::DecodeFailed
    );
    // An image the Form draws twice would need the Form cloned: NEW-38.
    let base = first_obj(1);
    let form = stream(
        &format!(
            "/Type /XObject /Subtype /Form /BBox [0 0 300 400] /Resources << /XObject << /Im0 {} 0 R >> >>",
            base + 1
        ),
        b"q 80 0 0 80 100 200 cm /Im0 Do Q",
    );
    let input = pdf(
        &[page(
            "",
            "/F Do q 1 0 0 1 0 -150 cm /F Do Q",
            &[("F", base)],
        )],
        &[form, img8()],
    );
    let doc = Document::open(input, None).unwrap();
    assert!(
        doc.page_model(0)
            .unwrap()
            .items
            .iter()
            .filter(|i| matches!(i.kind, ItemKind::Image(_)))
            .count()
            == 2
    );
    let plan = make_plan(&doc, &[(0, &[rect(100.0, 100.0, 200.0, 220.0)])]);
    let mut w = Writer::new(doc.reader(), Limits::default());
    let err = redact_images(&doc, &plan, &mut w).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert_eq!(
        err.details.as_ref().and_then(|d| d.feature.as_deref()),
        Some("redactionFormImageClone")
    );
    assert_eq!(w.write().unwrap(), untouched(&doc));
}
