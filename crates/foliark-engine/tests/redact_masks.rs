//! Alpha, stencil and shared image occurrences (spec §5.2; NEW-35). Same method as
//! redact_images.rs: expectations from placement geometry in the test, outputs read back by
//! pikepdf (decoded samples) and the tests/oracles objects.py scan (PyMuPDF, orphans).
mod image_pdf;
use foliark_engine::Document;
use foliark_engine::redaction::image::redact_images;
use foliark_engine::writer::Writer;
use foliark_model::{Limits, SharedKind};
use image_pdf::*;
use serde_json::Value;

const W: usize = 8;

fn bytes(img: &Value) -> Vec<u8> {
    bytes_of(&img["data"])
}

fn by_num(s: &Value, num: u64) -> &Value {
    s["all"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["num"] == num)
        .expect("object in output")
}

/// Region (125,133,153,151) over an 8x8 image at 10 pt samples (x 100..180, display y 120..200):
/// color cols 2..=5, rows 1..=3; over a 4x4 mask grid of 20 pt samples: cols 1..=2, rows 0..=1.
const REGION: [f64; 4] = [125.0, 133.0, 153.0, 151.0];
fn rg() -> foliark_model::Rect {
    rect(REGION[0], REGION[1], REGION[2], REGION[3])
}
const PLACE: &str = "q 80 0 0 80 100 200 cm /Im0 Do Q";

fn masked_pdf(image_extra: &str, mask: Vec<u8>) -> Vec<u8> {
    let base = first_obj(1);
    pdf(
        &[page("", PLACE, &[("Im0", base)])],
        &[
            rgb_image(W, W, &format!("{image_extra} {} 0 R", base + 1)),
            mask,
        ],
    )
}

fn assert_color(img: &Value, erased: &dyn Fn(usize, usize) -> bool) {
    let data = bytes(img);
    for j in 0..W {
        for i in 0..W {
            let got = &data[(j * W + i) * 3..][..3];
            let want = if erased(i, j) { [0, 0, 0] } else { px(i, j) };
            assert_eq!(got, want, "color ({i},{j})");
        }
    }
}

#[test]
fn a_soft_mask_of_another_resolution_goes_opaque_under_the_region() {
    // 4 x 4 alpha, every sample partly transparent: the secret would hide under it.
    let alpha = |i: usize, j: usize| (10 + i * 20 + j) as u8;
    let mask: Vec<u8> = (0..4)
        .flat_map(|j| (0..4).map(move |i| alpha(i, j)))
        .collect();
    let input = masked_pdf(
        "/SMask",
        stream(
            "/Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceGray /BitsPerComponent 8",
            &mask,
        ),
    );
    let out = run("smask", &input, &[(0, &[rg()])]).unwrap();
    let s = samples(&out.path);
    let img = &s["pages"][0]["images"]["Im0"];
    assert_color(img, &|i, j| (2..=5).contains(&i) && (1..=3).contains(&j));
    let sm = by_num(&s, img["smask"].as_u64().expect("an SMask is written"));
    assert_eq!(
        (
            sm["w"].as_u64(),
            sm["h"].as_u64(),
            sm["cs"].as_str(),
            sm["bpc"].as_u64()
        ),
        (Some(4), Some(4), Some("/DeviceGray"), Some(8))
    );
    let a = bytes(sm);
    for j in 0..4 {
        for i in 0..4 {
            let want = if (1..=2).contains(&i) && (0..=1).contains(&j) {
                255
            } else {
                alpha(i, j)
            };
            assert_eq!(a[j * 4 + i], want, "alpha ({i},{j})");
        }
    }
    // Two images in the file: the replaced one and its new mask. No source mask is left.
    assert_eq!(s["all"].as_array().unwrap().len(), 2);
    let (ok, text) = objects_check(&out.path, &[(0, REGION)]);
    assert!(ok, "{text}");
}

#[test]
fn an_explicit_mask_stream_and_a_color_key_fold_into_one_opaque_soft_mask() {
    // /Mask stream: 1-bit, sample 1 = masked out. Transparent where i and j are both even.
    let bits: Vec<u8> = (0..W)
        .map(|j| {
            (0..W).fold(0u8, |b, i| {
                b | (u8::from(i % 2 == 0 && j % 2 == 0) << (7 - i))
            })
        })
        .collect();
    let input = masked_pdf(
        "/Mask",
        stream(
            "/Type /XObject /Subtype /Image /Width 8 /Height 8 /ImageMask true /BitsPerComponent 1",
            &bits,
        ),
    );
    let out = run("maskstream", &input, &[(0, &[rg()])]).unwrap();
    let s = samples(&out.path);
    let img = &s["pages"][0]["images"]["Im0"];
    assert!(
        !img["has_mask"].as_bool().unwrap(),
        "the explicit /Mask is folded into /SMask"
    );
    assert_color(img, &|i, j| (2..=5).contains(&i) && (1..=3).contains(&j));
    let a = bytes(by_num(&s, img["smask"].as_u64().unwrap()));
    for j in 0..W {
        for i in 0..W {
            let erased = (2..=5).contains(&i) && (1..=3).contains(&j);
            let want = if erased || !(i % 2 == 0 && j % 2 == 0) {
                255
            } else {
                0
            };
            assert_eq!(a[j * W + i], want, "alpha ({i},{j})");
        }
    }

    // Color key: samples equal to px(3,3) are transparent; (7,7) is given that color too.
    // Erasing (3,3) turns it into opaque black: the key must not hide it.
    let key = px(3, 3);
    let mut data: Vec<u8> = (0..W)
        .flat_map(|j| (0..W).flat_map(move |i| px(i, j)))
        .collect();
    data[(7 * W + 7) * 3..][..3].copy_from_slice(&key);
    let keyed = pdf(
        &[page("", PLACE, &[("Im0", first_obj(1))])],
        &[stream(
            &format!(
                "/Type /XObject /Subtype /Image /Width 8 /Height 8 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Mask [{} {} {} {} {} {}]",
                key[0], key[0], key[1], key[1], key[2], key[2]
            ),
            &data,
        )],
    );
    let out = run(
        "colorkey",
        &keyed,
        &[(0, &[rect(131.0, 151.0, 139.0, 159.0)])],
    )
    .unwrap();
    let s = samples(&out.path);
    let img = &s["pages"][0]["images"]["Im0"];
    let color = bytes(img);
    assert_eq!(&color[(3 * W + 3) * 3..][..3], [0, 0, 0]);
    assert_eq!(
        &color[(7 * W + 7) * 3..][..3],
        key,
        "the other keyed sample keeps its color"
    );
    let a = bytes(by_num(&s, img["smask"].as_u64().unwrap()));
    for (k, v) in a.iter().enumerate() {
        assert_eq!(*v, if k == 7 * W + 7 { 0 } else { 255 }, "alpha {k}");
    }
}

#[test]
fn a_stencil_mask_is_unpainted_under_the_region_whatever_its_decode_array() {
    // Sample 0 paints. Painted where (i + j) is even.
    let row = |j: usize| (0..W).fold(0u8, |b, i| b | (u8::from((i + j) % 2 == 1) << (7 - i)));
    let bits: Vec<u8> = (0..W).map(row).collect();
    let inverted: Vec<u8> = bits.iter().map(|b| !b).collect();
    let base = first_obj(1);
    let input = pdf(
        &[page(
            "",
            "0 0 1 rg q 80 0 0 80 100 200 cm /S1 Do Q q 40 0 0 40 20 20 cm /S2 Do Q",
            &[("S1", base), ("S2", base + 1)],
        )],
        &[
            stream(
                "/Type /XObject /Subtype /Image /Width 8 /Height 8 /ImageMask true /BitsPerComponent 1",
                &bits,
            ),
            stream(
                "/Type /XObject /Subtype /Image /Width 8 /Height 8 /ImageMask true /BitsPerComponent 1 /Decode [1 0]",
                &inverted,
            ),
        ],
    );
    // S1: the usual region. S2 (40 pt, display x 20..60, y 340..380, 5 pt samples): cols 2..=5, rows 1..=3.
    let r2 = rect(31.0, 346.0, 49.0, 359.0);
    let out = run("stencil", &input, &[(0, &[rg(), r2])]).unwrap();
    let s = samples(&out.path);
    let expect = |erased: &dyn Fn(usize, usize) -> bool| -> Vec<u8> {
        (0..W)
            .map(|j| {
                (0..W).fold(
                    row(j),
                    |b, i| if erased(i, j) { b | (1 << (7 - i)) } else { b },
                )
            })
            .collect()
    };
    let one = &s["pages"][0]["images"]["S1"];
    assert_eq!(
        (
            one["mask"].as_bool(),
            one["bpc"].as_u64(),
            one["filter"].as_str()
        ),
        (Some(true), Some(1), Some("/FlateDecode"))
    );
    assert_eq!(
        bytes(one),
        expect(&|i, j| (2..=5).contains(&i) && (1..=3).contains(&j))
    );
    // The second, with /Decode [1 0], comes out normalized: same bits as the first rule gives.
    let two = &s["pages"][0]["images"]["S2"];
    assert_eq!(
        two["decode"], "None",
        "decode array is gone, samples carry the meaning"
    );
    assert_eq!(
        bytes(two),
        expect(&|i, j| (2..=5).contains(&i) && (1..=3).contains(&j))
    );
    let (ok, text) = objects_check(&out.path, &[(0, REGION)]);
    assert!(ok, "{text}");
}

#[test]
fn a_jpx_with_alpha_in_the_codestream_loses_both_planes_under_the_region() {
    let file = root().join("crates/foliark-engine/tests/data/rgba8.jp2");
    let jp2 = std::fs::read(&file).unwrap();
    let input = pdf(
        &[page("", PLACE, &[("Im0", first_obj(1))])],
        &[stream(
            "/Type /XObject /Subtype /Image /Width 8 /Height 8 /Filter /JPXDecode /SMaskInData 1",
            &jp2,
        )],
    );
    let out = run("jpx", &input, &[(0, &[rg()])]).unwrap();
    let s = samples(&out.path);
    let img = &s["pages"][0]["images"]["Im0"];
    assert_eq!(
        (img["smask_in_data"].as_u64(), img["filter"].as_str()),
        (Some(0), Some("/FlateDecode"))
    );
    let want = rgba_samples(&file);
    let (c, a) = (
        bytes(img),
        bytes(by_num(
            &s,
            img["smask"].as_u64().expect("alpha becomes an SMask"),
        )),
    );
    for j in 0..W {
        for i in 0..W {
            let erased = (2..=5).contains(&i) && (1..=3).contains(&j);
            let src = &want[(j * W + i) * 4..][..4];
            let (gc, ga) = (&c[(j * W + i) * 3..][..3], a[j * W + i]);
            if erased {
                assert_eq!((gc, ga), (&[0u8, 0, 0][..], 255), "({i},{j})");
            } else {
                assert_eq!((gc, ga), (&src[..3], src[3]), "({i},{j})");
            }
        }
    }
    assert!(
        !out.bytes
            .windows(4)
            .any(|w| w == b"jP  " || w == [0xFF, 0x4F, 0xFF, 0x51]),
        "JPX codestream survived"
    );
}

/// 8 x 8 image drawn by every `(page, cm)`, one page each unless `pages` repeats.
fn shared(pages: &[&str]) -> Vec<u8> {
    let base = first_obj(pages.len());
    let ps: Vec<Page> = pages
        .iter()
        .map(|c| page("", *c, &[("Im0", base)]))
        .collect();
    pdf(&ps, &[rgb_image(W, W, "")])
}

#[test]
fn the_erased_occurrence_gets_a_clone_and_the_others_keep_the_original() {
    // Page 0 draws the image twice (A at display x 100..180 y 120..200, B scaled and rotated
    // elsewhere); page 1 once. The region sits on A only.
    let input = shared(&[
        "q 80 0 0 80 100 200 cm /Im0 Do Q q 0 30 -30 0 260 20 cm /Im0 Do Q",
        "q 100 0 0 100 50 50 cm /Im0 Do Q",
    ]);
    let out = run("shared", &input, &[(0, &[rg()])]).unwrap();
    assert_eq!((out.report.images_erased, out.report.images_cloned), (1, 1));
    let sv = &out.plan.report.shared_survivors;
    assert_eq!(sv.len(), 1);
    assert_eq!(
        (
            sv[0].kind,
            sv[0].redacted_on.as_slice(),
            sv[0].untouched_on.as_slice()
        ),
        (SharedKind::Image, &[0][..], &[0, 1][..])
    );
    let s = samples(&out.path);
    let p0 = s["pages"][0]["images"].as_object().unwrap();
    assert_eq!(p0.len(), 2, "original for B plus the clone for A");
    let (orig, clone): (Vec<_>, Vec<_>) = p0.iter().partition(|(k, _)| k.as_str() == "Im0");
    let (orig, clone) = (orig[0].1, clone[0].1);
    assert_ne!(orig["num"], clone["num"]);
    assert_color(orig, &|_, _| false);
    assert_color(clone, &|i, j| (2..=5).contains(&i) && (1..=3).contains(&j));
    assert_eq!(
        s["pages"][1]["images"]["Im0"]["num"], orig["num"],
        "page 1 keeps the shared original"
    );
    assert_color(&s["pages"][1]["images"]["Im0"], &|_, _| false);
    // Exactly the two image objects: no orphan of the source, no third copy.
    assert_eq!(s["all"].as_array().unwrap().len(), 2);
    let content = s["pages"][0]["content"].as_str().unwrap();
    assert!(content.contains("/FlkImg0 Do"), "{content}");
    assert_eq!(
        content.matches("/Im0 Do").count(),
        1,
        "only B still draws the original: {content}"
    );
    let (ok, text) = objects_check(&out.path, &[(0, REGION)]);
    assert!(ok, "{text}");
    // Control: without the clone (erase in place by hand) the oracle names the survivor.
    let leaky = tmp("shared-input.pdf");
    std::fs::write(&leaky, &input).unwrap();
    assert!(!objects_check(&leaky, &[(0, REGION)]).0);
}

#[test]
fn every_occurrence_erased_leaves_only_clones_and_no_original() {
    let input = shared(&[
        "q 80 0 0 80 100 200 cm /Im0 Do Q q 40 0 0 40 20 20 cm /Im0 Do Q",
        "q 80 0 0 80 100 200 cm /Im0 Do Q",
    ]);
    let r2 = rect(31.0, 346.0, 49.0, 359.0);
    let out = run("allclones", &input, &[(0, &[rg(), r2]), (1, &[rg()])]).unwrap();
    assert_eq!(out.report.images_cloned, 3);
    assert!(out.plan.report.shared_survivors.is_empty());
    let s = samples(&out.path);
    let all = s["all"].as_array().unwrap();
    assert_eq!(
        all.len(),
        3,
        "three clones, the original is not in the file"
    );
    for im in all {
        assert!(
            bytes(im).chunks(3).any(|p| p == [0, 0, 0]),
            "an unerased copy of the image is in the file"
        );
    }
    for (p, name) in [(0, "Im0"), (1, "Im0")] {
        assert!(
            s["pages"][p]["images"].get(name).is_none(),
            "page {p} still names the original"
        );
    }
    let (ok, text) = objects_check(&out.path, &[(0, REGION), (1, REGION)]);
    assert!(ok, "{text}");
}

#[test]
fn an_unreadable_mask_is_refused_instead_of_dropped() {
    // The SMask has no usable samples (Flate garbage): dropping it would make the image opaque.
    let input = masked_pdf(
        "/SMask",
        stream(
            "/Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode",
            &[0x78, 0x9c, 9, 9, 9, 9],
        ),
    );
    let doc = Document::open(input, None).unwrap();
    let plan = make_plan(&doc, &[(0, &[rg()])]);
    let mut w = Writer::new(doc.reader(), Limits::default());
    let before = Writer::new(doc.reader(), Limits::default())
        .write()
        .unwrap();
    assert!(redact_images(&doc, &plan, &mut w).is_err());
    assert_eq!(w.write().unwrap(), before);
}
