//! Foliark cohort spike: proves hayro + pdf-writer build on wasm32 and probes callback shapes.
use hayro::hayro_syntax::{object::dict::keys::*, Pdf};
use hayro::hayro_interpret::font::{Glyph, GlyphRun};
use hayro::hayro_interpret::{
    interpret_page, BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image,
    ImageDrawProps, InterpreterCache, InterpreterSettings, SoftMask,
};
use hayro::kurbo::{Affine, BezPath, Rect as KRect};
use hayro::{render, PixmapSettings, RenderCache, RenderSettings};
use pdf_writer::{Content, Filter, Finish, Name, Pdf as PdfOut, Rect, Ref, Str};

/// Build a one-page PDF: Helvetica text (one visible, one render-mode-3), a filled rect, a 2x2 RGB image.
pub fn build_pdf(corrupt_stream: bool) -> Vec<u8> {
    let (cat, tree, page, font, content, img) =
        (Ref::new(1), Ref::new(2), Ref::new(3), Ref::new(4), Ref::new(5), Ref::new(6));
    let mut pdf = PdfOut::new();
    pdf.catalog(cat).pages(tree);
    pdf.pages(tree).kids([page]).count(1);
    let mut p = pdf.page(page);
    p.parent(tree).media_box(Rect::new(0.0, 0.0, 200.0, 100.0)).contents(content);
    let mut res = p.resources();
    res.fonts().pair(Name(b"F1"), font);
    res.x_objects().pair(Name(b"Im1"), img);
    res.finish();
    p.finish();
    pdf.type1_font(font).base_font(Name(b"Helvetica"));
    let mut c = Content::new();
    c.begin_marked_content(Name(b"P"));
    c.begin_text().set_font(Name(b"F1"), 18.0).next_line(10.0, 60.0).show(Str(b"Hello Foliark")).end_text();
    c.begin_text().set_font(Name(b"F1"), 10.0).set_text_rendering_mode(pdf_writer::types::TextRenderingMode::Invisible)
        .next_line(10.0, 40.0).show(Str(b"hidden ocr")).end_text();
    c.end_marked_content();
    c.set_fill_rgb(1.0, 0.0, 0.0).rect(120.0, 10.0, 40.0, 30.0).fill_nonzero();
    c.save_state().transform([30.0, 0.0, 0.0, 30.0, 160.0, 60.0]).x_object(Name(b"Im1")).restore_state();
    let bytes = c.finish();
    if corrupt_stream {
        pdf.stream(content, b"garbage not zlib").filter(Filter::FlateDecode);
    } else {
        pdf.stream(content, &bytes);
    }
    let mut im = pdf.image_xobject(img, &[255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0]);
    im.width(2).height(2).color_space().device_rgb();
    im.bits_per_component(8);
    im.finish();
    pdf.finish()
}

#[derive(Debug, Default)]
pub struct Report {
    pub glyphs: Vec<String>,
    pub invisible_runs: usize,
    pub paths: usize,
    pub images: Vec<String>,
    pub marked: Vec<String>,
    pub nonwhite_px: u32,
    pub objects: Vec<String>,
}

#[derive(Default)]
struct Probe(Report);

impl<'a> Device<'a> for Probe {
    fn draw_path(&mut self, _p: &BezPath, _props: DrawProps<'a>, _m: &DrawMode) { self.0.paths += 1; }
    fn push_clip_path(&mut self, _c: &ClipPath) {}
    fn push_transparency_group(&mut self, _o: f32, _m: Option<SoftMask<'a>>, _b: BlendMode) {}
    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, mode: &DrawMode) {
        if matches!(mode, DrawMode::Invisible) { self.0.invisible_runs += 1; }
        for g in run.glyphs() {
            if let Glyph::Outline(o) = &**g {
                let t = g.transform().as_coeffs();
                self.0.glyphs.push(format!(
                    "{:?} gid={} adv={:?} mode={:?} gt={:?} ctm={:?} fontkey={:x}",
                    o.as_unicode(), o.glyph_id().to_u32(), o.advance_width(), mode,
                    t.map(|v| (v * 100.0).round() / 100.0),
                    props.transform.as_coeffs().map(|v| (v * 100.0).round() / 100.0),
                    o.font_cache_key() & 0xffff));
            }
        }
    }
    fn draw_image(&mut self, image: Image<'a, '_>, props: ImageDrawProps<'a>) {
        if let Image::Raster(r) = &image {
            let id = r.stream().obj_id();
            let mut px = 0;
            r.with_rgba(|d, a| { px = d.width() * d.height(); let _ = a; }, None);
            self.0.images.push(format!("raster ref={:?} px={} tf={:?}", id, px, props.transform.as_coeffs()));
        }
    }
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}
    fn begin_marked_content(&mut self, tag: &[u8], mcid: Option<i32>) {
        self.0.marked.push(format!("{} {:?}", String::from_utf8_lossy(tag), mcid));
    }
}

pub fn run(bytes: Vec<u8>) -> Result<Report, String> {
    let pdf = Pdf::new(bytes).map_err(|e| format!("{e:?}"))?;
    let page = pdf.pages().get(0).ok_or("no page")?;
    let mut probe = Probe::default();
    let (w, h) = page.render_dimensions();
    let cache = InterpreterCache::new();
    let mut ctx = Context::new(
        page.initial_transform(true).to_kurbo_compat(),
        KRect::new(0.0, 0.0, w as f64, h as f64),
        &cache, page.xref(), InterpreterSettings::default());
    interpret_page(&page, &mut ctx, &mut probe);
    let pm = render(&page, &RenderCache::new(), &InterpreterSettings::default(),
        &RenderSettings::default(), &PixmapSettings { x_scale: 2.0, y_scale: 2.0, bg_color: hayro::vello_cpu::color::palette::css::WHITE });
    let mut rep = probe.0;
    rep.nonwhite_px = pm.data().iter().filter(|p| !(p.r == 255 && p.g == 255 && p.b == 255)).count() as u32;
    for o in pdf.objects() {
        rep.objects.push(match &o {
            hayro::hayro_syntax::object::Object::Stream(s) => format!("stream {:?} filters={:?}", s.obj_id(), s.filters()),
            hayro::hayro_syntax::object::Object::Dict(d) => format!("dict {:?} keys={}", d.obj_id(), d.len()),
            _ => "other".into(),
        });
    }
    let _ = (TYPE, Affine::IDENTITY);
    Ok(rep)
}

trait Compat { fn to_kurbo_compat(self) -> Affine; }
impl Compat for hayro::hayro_syntax::transform::Transform {
    fn to_kurbo_compat(self) -> Affine { use hayro::hayro_interpret::TransformExt; self.to_kurbo() }
}

/// wasm entry (no wasm-bindgen): returns count of non-white pixels at 2x, or 0 on failure.
#[no_mangle]
pub extern "C" fn foliark_spike() -> u32 {
    run(build_pdf(false)).map(|r| r.nonwhite_px).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn probe() {
        let r = run(build_pdf(false)).unwrap();
        println!("{r:#?}");
        assert!(r.nonwhite_px > 0);
    }
    #[test]
    fn xref_repair_and_bad_stream() {
        // 1) break startxref offset -> repair
        let mut b = build_pdf(false);
        let i = b.windows(9).rposition(|w| w == b"startxref").unwrap();
        for ch in b[i + 10..].iter_mut().take_while(|c| **c != b'\n') { *ch = b'9'; }
        let r = run(b);
        println!("repair => {:?}", r.as_ref().map(|r| r.nonwhite_px));
        // 2) truncated tail (no xref/trailer at all)
        let mut b = build_pdf(false);
        let i = b.windows(6).position(|w| w == b"xref\n0").unwrap();
        b.truncate(i);
        println!("truncated => {:?}", run(b).as_ref().map(|r| r.nonwhite_px));
        // 3) bad flate content stream: error or silent empty?
        let pdf = Pdf::new(build_pdf(true)).unwrap();
        let page = pdf.pages().get(0).unwrap();
        println!("bad stream: page_stream={:?}", page.page_stream().map(|s| s.len()));
        let r = run(build_pdf(true));
        println!("bad stream render => {:?}", r.as_ref().map(|r| (r.nonwhite_px, r.glyphs.len())));
        // 4) password
        println!("garbage => {:?}", Pdf::new(b"%PDF-1.4 nope".to_vec()).err());
    }
}
