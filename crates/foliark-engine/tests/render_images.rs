//! hayro P17: the preview render decodes an image no larger than it is drawn and keeps that
//! decode for the next render of the document (from the second draw of an image on); `render_page_exact` (raster export) never does.
use foliark_engine::{Engine, RenderOptions};
use foliark_model::Limits;
use pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref};

const W: usize = 600;
const H: usize = 400;

/// A 600x400 gray image of 1 px stripes over a ramp (worst case for a resampler) drawn over
/// the whole 300x200 pt page, so scale 0.5 asks for 150x100 pixels and scale 2 for the native size.
fn pdf() -> Vec<u8> {
    let pixels: Vec<u8> = (0..H)
        .flat_map(|y| {
            (0..W).map(move |x| {
                if x % 5 == 0 {
                    0
                } else {
                    ((x + 2 * y) % 256) as u8
                }
            })
        })
        .collect();
    let mut pdf = Pdf::new();
    pdf.catalog(Ref::new(1)).pages(Ref::new(2));
    pdf.pages(Ref::new(2)).kids([Ref::new(3)]).count(1);
    let mut page = pdf.page(Ref::new(3));
    page.parent(Ref::new(2))
        .media_box(Rect::new(0.0, 0.0, 300.0, 200.0))
        .contents(Ref::new(4));
    page.resources().x_objects().pair(Name(b"Im0"), Ref::new(5));
    page.finish();
    let mut c = Content::new();
    c.save_state()
        .transform([300.0, 0.0, 0.0, 200.0, 0.0, 0.0])
        .x_object(Name(b"Im0"))
        .restore_state();
    pdf.stream(Ref::new(4), &c.finish());
    let mut image = pdf.image_xobject(Ref::new(5), &pixels);
    image.width(W as i32).height(H as i32).bits_per_component(8);
    image.color_space().device_gray();
    image.finish();
    pdf.finish()
}

#[test]
fn reduced_render_matches_the_exact_one_and_is_kept_for_the_next() {
    let mut e = Engine::new(Limits::default());
    let (id, _) = e.open(pdf(), None, false).unwrap();
    let doc = e.working_copy(&id).unwrap();
    let at = RenderOptions::at(0.5);

    let exact = doc.render_page_exact(0, at.clone()).unwrap();
    assert_eq!(e.stats().cache_entries, 0, "an exact render keeps nothing");

    // the first draw of an image is left as it was and only remembered
    let first = doc.render_page(0, at.clone()).unwrap();
    assert_eq!(first.data, exact.data);
    assert_eq!(e.stats().cache_entries, 0);

    // the second reduces it and keeps the result
    let doc = e.working_copy(&id).unwrap();
    let second = doc.render_page(0, at.clone()).unwrap();
    assert_eq!((second.header.width, second.header.height), (150, 100));
    let worst = exact
        .data
        .iter()
        .zip(&second.data)
        .map(|(a, b)| a.abs_diff(*b))
        .max();
    assert!(
        worst <= Some(2),
        "reduced decode drifted from the full-size path: {worst:?}"
    );
    let stats = e.stats();
    assert_eq!((stats.cache_entries, stats.cache_bytes), (1, 150 * 100));

    let third = e.working_copy(&id).unwrap().render_page(0, at).unwrap();
    assert_eq!(third.data, second.data);
    assert_eq!(e.stats().cache_entries, 1, "same key, no second entry");
}

#[test]
fn an_image_drawn_at_its_native_size_is_left_alone() {
    let mut e = Engine::new(Limits::default());
    let (id, _) = e.open(pdf(), None, false).unwrap();
    let doc = e.working_copy(&id).unwrap();
    let exact = doc.render_page_exact(0, RenderOptions::at(2.0)).unwrap();
    for _ in 0..2 {
        let fast = doc.render_page(0, RenderOptions::at(2.0)).unwrap();
        assert_eq!(fast.data, exact.data);
    }
    assert_eq!(e.stats().cache_entries, 0);
}
