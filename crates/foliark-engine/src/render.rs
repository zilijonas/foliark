//! Page rendering (spec §1.3): hayro driven with the canonical display-space transform, so
//! pixel (i, j) of a render at `scale` covers the display rect
//! `[clip.x + i/scale, clip.y + j/scale, 1/scale, 1/scale]` whatever the page's CropBox,
//! /Rotate and /UserUnit are.
use crate::{Document, limits};
use foliark_model::{
    ErrorCode, Matrix, PdfError, Rect, RenderWarning, RenderWarningCode, RenderedPageHeader,
};
use hayro::hayro_interpret::font::FontQuery;
use hayro::hayro_interpret::{InterpreterSettings, InterpreterWarning};
use hayro::hayro_syntax::object::{Array, Dict, Name};
use hayro::kurbo::Affine;
use hayro::vello_cpu::color::{AlphaColor, Srgb};
use hayro::vello_cpu::peniko::ImageAlphaType;
use hayro::vello_cpu::{Pixmap, RasterizerSettings, RenderContext, Resources, TargetInit};
use hayro::{RenderCache, RenderSettings, render_into};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Spec §1.3: each rendered side is at most 16384 px.
const MAX_SIDE: f64 = 16384.0;

/// Spec §1.3 `RenderOptions` (the abort signal lives in the SDK).
#[derive(Debug, Clone, PartialEq)]
pub struct RenderOptions {
    /// Pixels per point.
    pub scale: f64,
    /// Display-space rect to render; `None` is the whole page.
    pub clip: Option<Rect>,
    /// Draw annotation appearances.
    pub annotations: bool,
    /// Straight-alpha sRGB, each 0..=1.
    pub background: [f64; 4],
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            scale: 1.0,
            clip: None,
            annotations: true,
            background: [1.0; 4],
        }
    }
}

impl RenderOptions {
    pub fn at(scale: f64) -> Self {
        Self {
            scale,
            ..Self::default()
        }
    }
}

/// Spec §1.3 `RenderedPage`: `data` is RGBA8 straight alpha sRGB, row-major, ImageData-compatible.
#[derive(Debug)]
pub struct RenderedPage {
    pub header: RenderedPageHeader,
    pub data: Vec<u8>,
}

impl Document {
    /// Images are decoded no larger than they are drawn (hayro P17) and the decodes are kept for
    /// the next render of this document, so this is the preview path. Use [`Self::render_page_exact`]
    /// where pixels must come from the full-size image.
    pub fn render_page(
        &self,
        index: usize,
        options: RenderOptions,
    ) -> Result<RenderedPage, PdfError> {
        self.render(index, options, true)
    }

    /// As [`Self::render_page`] with every image decoded at full size and resampled by the
    /// renderer (raster export burns redactions into these pixels).
    pub fn render_page_exact(
        &self,
        index: usize,
        options: RenderOptions,
    ) -> Result<RenderedPage, PdfError> {
        self.render(index, options, false)
    }

    fn render(
        &self,
        index: usize,
        options: RenderOptions,
        reduce_images: bool,
    ) -> Result<RenderedPage, PdfError> {
        let invalid = || PdfError::new(ErrorCode::InvalidArgument);
        let RenderOptions {
            scale,
            clip,
            annotations,
            background,
        } = options;
        if !(scale.is_finite() && scale > 0.0)
            || !background.iter().all(|c| (0.0..=1.0).contains(c))
        {
            return Err(invalid());
        }
        let geometry = self.0.page_geometry(index)?;
        let clip = clip.unwrap_or(Rect {
            x: 0.0,
            y: 0.0,
            width: geometry.width,
            height: geometry.height,
        });
        let finite = [clip.x, clip.y, clip.width, clip.height]
            .iter()
            .all(|v| v.is_finite());
        if !(finite && clip.width > 0.0 && clip.height > 0.0) {
            return Err(invalid());
        }
        let side = |len: f64| (len * scale).round().max(1.0);
        let (width, height) = (side(clip.width), side(clip.height));
        if width > MAX_SIDE || height > MAX_SIDE {
            return Err(PdfError::limit(
                "maxRenderSide",
                width.max(height) as u64,
                MAX_SIDE as u64,
            ));
        }
        let limits = self.0.limits();
        let pixels = (width * height) as u64;
        limits::check("maxRenderPixels", pixels, limits.max_render_pixels.into())?;
        limits::Budget::new(limits).charge_pixels(pixels)?;
        // The renderer decodes fonts and images without caps: refuse bombs first, and refuse a
        // page whose content fails to decode instead of drawing it blank.
        self.0.ensure_budgets()?;
        self.0.page_contents(index)?;

        let page = self.0.page(index)?;
        let warnings = Arc::new(Mutex::new(Vec::<RenderWarning>::new()));
        // The renderer has no device of ours to charge, so count operators here (one work unit
        // each) and let the interpreter stop on a cancel or when the count passes the budget.
        let ops = Arc::new(AtomicU64::new(0));
        let (cancel, counted, max_ops) =
            (self.0.cancel_handle(), ops.clone(), limits.max_work_units);
        let mut settings = InterpreterSettings {
            render_annotations: annotations,
            report_font_substitution: true,
            should_stop: Some(Arc::new(move || {
                cancel.load(Ordering::Relaxed) || counted.fetch_add(1, Ordering::Relaxed) >= max_ops
            })),
            max_image_pixels: limits.max_image_pixels.into(),
            reduce_images,
            reduced_image_cache: reduce_images.then(|| self.0.images.clone() as _),
            cmap_resolver: Arc::new(crate::cmaps::resolve),
            ..Default::default()
        };
        let sink = warnings.clone();
        let warn = move |code, detail: String| {
            let w = RenderWarning { code, detail };
            let mut all = sink.lock().unwrap();
            if !all.contains(&w) {
                all.push(w);
            }
        };
        // A font the PDF neither embeds nor is one of the 14 standard faces is drawn with a
        // stand-in; a predefined CMap the build does not carry leaves its text undrawable.
        let on_font = warn.clone();
        let standard = settings.font_resolver.clone();
        settings.font_resolver = Arc::new(move |query| {
            if let FontQuery::Fallback(f) = query {
                let name = f.post_script_name.as_deref().or(f.font_name.as_deref());
                on_font(
                    RenderWarningCode::FontSubstituted,
                    name.unwrap_or("unnamed font").to_owned(),
                );
            }
            standard(query)
        });
        let on_cmap = warn.clone();
        let cmaps = settings.cmap_resolver.clone();
        settings.cmap_resolver = Arc::new(move |name| {
            let found = cmaps(name);
            if found.is_none() {
                on_cmap(RenderWarningCode::CmapMissing, format!("{name:?}"));
            }
            found
        });
        let too_large = Arc::new(Mutex::new(None::<u64>));
        let on_too_large = too_large.clone();
        let on_event = warn.clone();
        let substituted = Arc::new(Mutex::new(Vec::new()));
        let on_substituted = {
            let seen = substituted.clone();
            move |obj| seen.lock().unwrap().push(obj)
        };
        settings.warning_sink = Arc::new(move |w| match w {
            InterpreterWarning::ImageDecodeFailure => on_event(
                RenderWarningCode::ImageDecodeFailed,
                "image failed to decode".into(),
            ),
            InterpreterWarning::UnresolvedXObject(obj) => on_event(
                RenderWarningCode::ImageDecodeFailed,
                format!("XObject {} {} unreadable", obj.obj_number, obj.gen_number),
            ),
            InterpreterWarning::UnresolvedAnnotationAppearance => on_event(
                RenderWarningCode::AnnotationAppearanceMissing,
                "appearance stream unreadable".into(),
            ),
            InterpreterWarning::FontSubstituted(obj) => on_substituted(obj),
            InterpreterWarning::ImageTooLarge(w, h) => {
                *on_too_large.lock().unwrap() = Some(u64::from(w) * u64::from(h));
            }
            InterpreterWarning::UnsupportedFont => on_event(
                RenderWarningCode::CmapMissing,
                "CID font with an unsupported encoding".into(),
            ),
            _ => {}
        });
        if annotations {
            for annot in page
                .raw()
                .get::<Array<'_>>("Annots")
                .into_iter()
                .flat_map(|a| a.iter::<Dict<'_>>())
            {
                let silent = matches!(
                    annot
                        .get::<Name<'_>>("Subtype")
                        .map(|n| n.as_str().to_owned())
                        .as_deref(),
                    Some("Link" | "Popup")
                );
                let hidden = annot.get::<u32>("F").unwrap_or(0) & 2 != 0;
                let has_normal = annot
                    .get::<Dict<'_>>("AP")
                    .is_some_and(|ap| ap.contains_key("N"));
                if !(silent || hidden || has_normal) {
                    warn(
                        RenderWarningCode::AnnotationAppearanceMissing,
                        "annotation has no appearance stream".into(),
                    );
                }
            }
        }

        // User space to pixels: the canonical user-to-display matrix (CropBox, /Rotate,
        // /UserUnit), moved so the clip's corner is pixel (0, 0), then scaled.
        let Matrix([a, b, c, d, e, f]) = geometry
            .user_to_display
            .then(Matrix([1.0, 0.0, 0.0, 1.0, -clip.x, -clip.y]))
            .then(Matrix::scale(scale, scale));
        let mut ctx = RenderContext::new(width as u16, height as u16);
        render_into(
            page,
            &RenderCache::new(),
            &settings,
            &RenderSettings::default(),
            &mut ctx,
            Affine::new([a, b, c, d, e, f]),
        );
        if self.0.cancelled() {
            return Err(PdfError::new(ErrorCode::Cancelled));
        }
        let ops = ops.load(Ordering::Relaxed);
        limits::check("maxWorkUnits", ops, max_ops)?;
        // Inline images and images the budget scan did not see are refused at draw time.
        if let Some(pixels) = *too_large.lock().unwrap() {
            return Err(PdfError::limit(
                "maxImagePixels",
                pixels,
                limits.max_image_pixels.into(),
            ));
        }
        ctx.flush();
        let mut pixmap = Pixmap::new(ctx.width(), ctx.height());
        ctx.render_with(
            &mut pixmap,
            &mut Resources::default(),
            RasterizerSettings {
                target_init: TargetInit::Clear(AlphaColor::<Srgb>::new(
                    background.map(|c| c as f32),
                )),
                ..Default::default()
            },
        );
        // The sink only has the font's object; name it for the caller.
        let pdf = self.0.pdf();
        for obj in std::mem::take(&mut *substituted.lock().unwrap()) {
            let name = obj
                .and_then(|o| pdf.xref().get::<Dict<'_>>(o.into()))
                .and_then(|d| d.get::<Name<'_>>("BaseFont").map(|n| n.as_str().to_owned()));
            warn(
                RenderWarningCode::FontSubstituted,
                name.unwrap_or_else(|| "unnamed font".into()),
            );
        }
        let warnings = std::mem::take(&mut *warnings.lock().unwrap());
        Ok(RenderedPage {
            header: RenderedPageHeader {
                width: u32::from(pixmap.width()),
                height: u32::from(pixmap.height()),
                scale,
                clip,
                complete: warnings.is_empty(),
                warnings,
            },
            data: pixmap.take_rgba8(ImageAlphaType::Alpha),
        })
    }
}
