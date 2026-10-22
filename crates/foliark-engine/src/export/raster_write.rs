//! Raster PDF (spec §7; NEW-56): a fresh file of one image per page, built with `pdf-writer`
//! from nothing the source held: no original object, text, font, annotation, metadata, Info or
//! ID, whatever the source carried.
//!
//! **Geometry.** Each page is `MediaBox [0 0 W H]` with W x H the source page's DISPLAY size in
//! points (spec §2: CropBox, /Rotate and /UserUnit already applied), no /Rotate, no CropBox, no
//! UserUnit. The rotation is baked into the pixels (the render draws the page as displayed), so
//! a viewer shows the same size and orientation as the source's display; the cropped-away
//! margin is not in the file at all. The image fills the page: `q W 0 0 H 0 0 cm /Im Do Q`.
//!
//! **Encoding.** 8-bit DeviceRGB, `FlateDecode` (lossless, spec §7). Rows are streamed from the
//! RGBA buffer into the encoder, so no second full-page buffer exists.
use super::raster_pixels::Pixels;
use super::raster_plan::{PageSpec, RasterPlan};
use super::{AppliedRedaction, Prepared, verify_failed};
use crate::Document;
use flate2::Compression;
use flate2::write::ZlibEncoder;
use foliark_model::{
    EditsReport, ExportMode, ExportReport, ExtractOptions, Flattened, KeptReport, Limits, PdfError,
    RedactionReport,
};
use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref};
use std::io::Write;

/// One page's image, ready to place.
pub struct Encoded {
    pub width: u32,
    pub height: u32,
    pub flate: Vec<u8>,
}

fn io(_: std::io::Error) -> PdfError {
    PdfError::new(foliark_model::ErrorCode::Internal)
}

/// Compress `px` as 8-bit RGB (alpha is opaque by construction) one row at a time.
pub fn encode(px: &Pixels) -> Result<Encoded, PdfError> {
    let mut z = ZlibEncoder::new(Vec::new(), Compression::new(6));
    let mut row = Vec::with_capacity(px.width as usize * 3);
    for src in px.rgba.chunks_exact(px.width as usize * 4) {
        row.clear();
        for p in src.as_chunks::<4>().0 {
            row.extend_from_slice(&p[..3]);
        }
        z.write_all(&row).map_err(io)?;
    }
    Ok(Encoded {
        width: px.width,
        height: px.height,
        flate: z.finish().map_err(io)?,
    })
}

/// The file: catalog 1, page tree 2, then page / contents / image per page.
pub fn assemble(pages: &[(PageSpec, Encoded)]) -> Vec<u8> {
    let mut pdf = Pdf::new();
    pdf.catalog(Ref::new(1)).pages(Ref::new(2));
    let page_ref = |i: usize| Ref::new(3 + 3 * i as i32);
    pdf.pages(Ref::new(2))
        .kids((0..pages.len()).map(page_ref))
        .count(pages.len() as i32);
    for (i, (spec, img)) in pages.iter().enumerate() {
        let page = page_ref(i);
        let (contents, image) = (Ref::new(page.get() + 1), Ref::new(page.get() + 2));
        let (w, h) = (spec.width_pt as f32, spec.height_pt as f32);
        let mut p = pdf.page(page);
        p.parent(Ref::new(2))
            .media_box(Rect::new(0.0, 0.0, w, h))
            .contents(contents);
        p.resources().x_objects().pair(Name(b"Im"), image);
        p.finish();
        let mut c = Content::new();
        c.save_state()
            .transform([w, 0.0, 0.0, h, 0.0, 0.0])
            .x_object(Name(b"Im"))
            .restore_state();
        pdf.stream(contents, &c.finish());
        let mut x = pdf.image_xobject(image, &img.flate);
        x.width(img.width as i32).height(img.height as i32);
        x.color_space().device_rgb();
        x.bits_per_component(8);
        x.filter(Filter::FlateDecode);
        x.finish();
    }
    pdf.finish()
}

/// Spec §7 verify: the file reopens strictly, has the source's pages at the display size with no
/// /Rotate, and no page holds a text cluster.
pub fn verify(bytes: &[u8], plan: &RasterPlan, limits: &Limits) -> Result<(), PdfError> {
    let out = Document::open_with_limits(bytes.to_vec(), None, false, limits.clone())
        .map_err(|_| verify_failed())?;
    if out.page_count() != plan.pages.len() {
        return Err(verify_failed());
    }
    for (i, spec) in plan.pages.iter().enumerate() {
        let g = out.page_geometry(i)?;
        let near = |a: f64, b: f64| (a - b).abs() < 0.01;
        if g.rotation != 0 || !near(g.width, spec.width_pt) || !near(g.height, spec.height_pt) {
            return Err(verify_failed());
        }
        let text = out.extract_text(i, &ExtractOptions::default())?;
        if text.clusters.iter().flatten().next().is_some() || !text.text.is_empty() {
            return Err(verify_failed());
        }
    }
    Ok(())
}

pub fn report(plan: &RasterPlan, edits: usize, bytes: &[u8]) -> Result<ExportReport, PdfError> {
    let output_bytes = u32::try_from(bytes.len())
        .map_err(|_| PdfError::limit("maxOutputBytes", bytes.len() as u64, u64::from(u32::MAX)))?;
    let flat = plan.annotations;
    Ok(ExportReport {
        mode: ExportMode::Raster,
        pages: plan.pages.len() as u32,
        output_bytes,
        removed: plan.removed(),
        flattened: Flattened {
            annotations: if flat { plan.annots.visible } else { 0 },
            widgets: if flat { plan.widgets } else { 0 },
            ocg_layers: plan.ocg_layers,
        },
        redaction: RedactionReport {
            regions: plan.regions,
            clusters_removed: 0,
            images_erased: 0,
            images_cloned: 0,
            forms_cloned: 0,
            vector_subpaths_removed: 0,
            vector_rects_cut: 0,
            collateral: Vec::new(),
            shared_survivors: Vec::new(),
        },
        edits: EditsReport {
            applied: edits as u32,
            approximate: 0,
        },
        kept: KeptReport {
            invisible_text_clusters: 0,
            fonts_with_glyph_inventory: Vec::new(),
        },
        losses: plan.losses(),
    })
}

pub fn prepared(
    pages: &[(PageSpec, Encoded)],
    plan: &RasterPlan,
    edits: usize,
    applied: &[AppliedRedaction],
    limits: &Limits,
) -> Result<Prepared, PdfError> {
    let bytes = assemble(pages);
    crate::limits::check_output(bytes.len() as u64, true, limits)?;
    verify(&bytes, plan, limits)?;
    let report = report(plan, edits, &bytes)?;
    Ok(Prepared {
        bytes,
        report,
        applied: applied.to_vec(),
    })
}
