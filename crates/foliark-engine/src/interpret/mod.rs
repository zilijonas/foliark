//! The semantic page model (spec §14 `interp`): one hayro-interpret pass over a page that
//! reports every glyph, image, path and shading in draw order, in user space, with where its
//! operator is in the file (stream + byte range), which Form instances it is drawn through and
//! which object owns the resources it uses. Text extraction, search, redaction, editing and the
//! writer all read this one model.
//!
//! Hidden optional content is interpreted and flagged (`ocg_hidden`). Not descended, each with
//! a [`Diagnostic`] and never silently skipped: Type3 glyph procedures, tiling pattern cells,
//! operators the interpreter does not know.
use crate::limits::{self, Budget};
use crate::reader::{Document, object_id};
use foliark_model::{
    InstancePath, InstanceStep, Limits, Matrix, ObjectId, PageGeometry, PdfError, Point, Quad,
    SourceLoc,
};
use hayro::hayro_interpret::font::{Glyph, GlyphRun};
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_interpret::hayro_syntax::object::dict::keys::{PARENT, RESOURCES};
use hayro::hayro_interpret::hayro_syntax::object::{Dict, ObjRef, Stream};
use hayro::hayro_interpret::hayro_syntax::xref::XRef;
use hayro::hayro_interpret::pattern::Pattern;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageDrawProps,
    InterpreterCache, InterpreterSettings, InterpreterWarning, Paint, SoftMask, interpret_page,
};
use hayro::vello_cpu::peniko::kurbo::{Affine, BezPath, Rect, Shape};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::sync::{Arc, Mutex};

mod fonts;
pub use fonts::{FontInfo, FontKind};
pub use hayro::hayro_interpret::{FillRule, StrokeProps};

#[derive(Debug, Clone)]
pub struct SemanticPage {
    pub index: usize,
    pub geometry: PageGeometry,
    /// Draw order.
    pub items: Vec<Item>,
    /// Every font a glyph of this page uses whose dictionary is an indirect object.
    pub fonts: BTreeMap<ObjectId, FontInfo>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone)]
pub struct Item {
    /// The operator that drew it. One operator can draw many items (a `TJ`, `re f`).
    pub loc: SourceLoc,
    /// The Form instances it is drawn through, outermost first. Empty = drawn by the page.
    pub path: InstancePath,
    /// The object whose dictionary holds the `/Resources` in effect (a Form, a page or an
    /// ancestor `/Pages` node for inherited resources).
    pub resource_owner: ObjectId,
    /// True for content that optional content hides (it is interpreted anyway).
    pub ocg_hidden: bool,
    /// User-space bounds `[x0, y0, x1, y1]` of the active clips intersected; `None` = unclipped.
    /// Decide off-page or clipped-out visibility against it.
    pub clip: Option<[f64; 4]>,
    pub kind: ItemKind,
}

#[derive(Debug, Clone)]
pub enum ItemKind {
    Glyph(GlyphItem),
    Image(ImageItem),
    /// Painted or clip-only vector geometry. A clip-only item is reported for every clip the
    /// interpreter pushes (`W n`, and also a Form's /BBox, with the `Do` operator as `loc`).
    Path(PathItem),
    /// A shading (`sh` or a shading-pattern fill) with the geometry it covers.
    Shading(PathItem),
}

#[derive(Debug, Clone)]
pub struct GlyphItem {
    pub char_code: u32,
    /// Glyph id of an outline font, `None` for Type3.
    pub gid: Option<u32>,
    /// `None` when the font dictionary is a direct object (diagnostic `DirectFontDict`).
    pub font: Option<ObjectId>,
    /// ToUnicode, then encoding / glyph name (multiple code points allowed); `None` = unmapped.
    pub unicode: Option<String>,
    /// `Tr` 0 to 7. Modes 3 and 7 are reported too.
    pub render_mode: u8,
    /// PDF text rendering matrix with the CTM: em space (1 = font size) to user space.
    pub trm: Matrix,
    /// Advance in em (Tc/Tw applied), along y for vertical writing.
    pub advance_em: f64,
    pub vertical: bool,
    /// The §3.1 glyph box in user space: top-left, top-right, bottom-right, bottom-left.
    pub quad_user: Quad,
    /// RGBA8 of the fill (stroke for modes 1/5); `None` for a pattern.
    pub fill: Option<[u8; 4]>,
    /// Index of this code within its show operator, counting a whole `TJ` array and codes that
    /// were not drawn. This is what a `TJ` rewrite addresses.
    pub op_glyph_index: u32,
}

#[derive(Debug, Clone)]
pub struct ImageItem {
    /// `None` for an inline image.
    pub obj: Option<ObjectId>,
    /// Unit square to user space.
    pub ctm: Matrix,
    pub width: u32,
    pub height: u32,
    pub has_smask: bool,
    pub has_mask: bool,
    pub is_stencil: bool,
}

#[derive(Debug, Clone)]
pub struct PathItem {
    /// User space.
    pub geometry: BezPath,
    /// The fill rule, or the clip rule of a clip-only item.
    pub fill: Option<FillRule>,
    pub stroke: Option<StrokeProps>,
    pub clip_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    /// `None` when the interpreter reported it without a position.
    pub loc: Option<SourceLoc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticKind {
    /// An operator the interpreter does not know; it was skipped.
    UnknownOperator(String),
    /// Type3 glyphs are reported as glyphs; their CharProc content is not descended.
    Type3NotDescended(Option<ObjectId>),
    /// Paint from a tiling pattern: the cell content is not descended.
    PatternNotDescended,
    /// Glyphs of a font dictionary that is a direct object have no `font` id.
    DirectFontDict,
    UnsupportedFont,
    ImageDecodeFailure,
    UnresolvedAnnotationAppearance,
    /// A hayro warning kind added after this model was written.
    InterpreterWarning,
}

const NO_OBJECT: ObjectId = ObjectId {
    num: 0,
    generation: 0,
};

struct Frame {
    step: InstanceStep,
    owner: ObjectId,
    /// How often each Form was drawn directly inside this frame.
    drawn: HashMap<ObjectId, u32>,
}

struct Builder<'x> {
    xref: &'x XRef,
    limits: &'x Limits,
    budget: &'x mut Budget,
    /// `stack[0]` is the page.
    stack: Vec<Frame>,
    clips: Vec<Rect>,
    loc: SourceLoc,
    hidden: bool,
    items: Vec<Item>,
    fonts: BTreeMap<ObjectId, FontInfo>,
    diagnostics: Vec<Diagnostic>,
    /// Fonts that already have a `Type3NotDescended` (`NO_OBJECT` = direct dictionary).
    type3: HashSet<ObjectId>,
    direct_font_reported: bool,
    error: Option<PdfError>,
    /// Every Form that was entered, to tell a cycle from a damaged object.
    entered: HashSet<ObjectId>,
}

impl Builder<'_> {
    /// Charge work units; the first failure is kept and everything after it is ignored. The
    /// interpreter cannot be stopped from a device, so it still walks the rest of the stream.
    fn charge(&mut self, units: u64) -> bool {
        if self.error.is_none()
            && let Err(e) = self.budget.charge(units)
        {
            self.error = Some(e);
        }
        self.error.is_none()
    }

    fn diagnose(&mut self, kind: DiagnosticKind) {
        let loc = Some(self.loc);
        self.diagnostics.push(Diagnostic { kind, loc });
    }

    fn push(&mut self, kind: ItemKind) {
        let clip = self.clips.split_first().map(|(first, rest)| {
            let r = rest.iter().fold(*first, |acc, c| acc.intersect(*c));
            [r.x0, r.y0, r.x1, r.y1]
        });
        let frame = self.stack.last().expect("page frame");
        self.items.push(Item {
            loc: self.loc,
            path: InstancePath(self.stack[1..].iter().map(|f| f.step).collect()),
            resource_owner: frame.owner,
            ocg_hidden: self.hidden,
            clip,
            kind,
        });
    }

    fn glyph(
        &mut self,
        g: &hayro::hayro_interpret::font::PositionedGlyph<'_>,
        mode: u8,
        fill: Option<[u8; 4]>,
        ctm: Affine,
    ) {
        let font = g.font_ref().map(object_id);
        match font {
            Some(id) => {
                if !self.fonts.contains_key(&id)
                    && let Some(info) = self
                        .xref
                        .get::<Dict<'_>>(id_to_ref(id).into())
                        .and_then(|d| FontInfo::read(&d))
                {
                    self.fonts.insert(id, info);
                }
            }
            None if !self.direct_font_reported => {
                self.direct_font_reported = true;
                self.diagnose(DiagnosticKind::DirectFontDict);
            }
            None => {}
        }
        let gid = match &**g {
            Glyph::Outline(o) => Some(o.glyph_id().to_u32()),
            Glyph::Type3(_) => {
                if self.type3.insert(font.unwrap_or(NO_OBJECT)) {
                    self.diagnose(DiagnosticKind::Type3NotDescended(font));
                }
                None
            }
        };
        let (adv_x, adv_y) = g.advance();
        let vertical = adv_x == 0.0 && adv_y != 0.0;
        let (ascent, descent) = font
            .and_then(|f| self.fonts.get(&f))
            .map_or((0.8, -0.2), |f| (f.ascent, f.descent));
        let local: [(f64, f64); 4] = if vertical {
            [(-0.5, 0.0), (0.5, 0.0), (0.5, adv_y), (-0.5, adv_y)]
        } else {
            [
                (0.0, ascent),
                (adv_x, ascent),
                (adv_x, descent),
                (0.0, descent),
            ]
        };
        let trm = ctm * g.transform() * Affine::scale(1000.0);
        let m = Matrix(trm.as_coeffs());
        let quad_user = Quad(local.map(|(x, y)| m.apply(Point { x, y })));
        self.push(ItemKind::Glyph(GlyphItem {
            char_code: g.char_code(),
            gid,
            font,
            unicode: g.as_unicode().map(|u| match u {
                BfString::Char(c) => c.to_string(),
                BfString::String(s) => s,
            }),
            render_mode: mode,
            trm: m,
            advance_em: if vertical { adv_y } else { adv_x },
            vertical,
            quad_user,
            fill,
            op_glyph_index: g.code_index(),
        }));
    }

    fn path_item(&mut self, path: &BezPath, props: &DrawProps<'_>, mode: &DrawMode) {
        let (fill, stroke) = match mode {
            DrawMode::Fill(r) => (Some(*r), None),
            DrawMode::Stroke(s) => (None, Some(s.clone())),
            DrawMode::FillAndStroke(r, s) => (Some(*r), Some(s.clone())),
            DrawMode::Invisible => (None, None),
        };
        let item = PathItem {
            geometry: props.transform * path.clone(),
            fill,
            stroke,
            clip_only: false,
        };
        match &props.paint {
            Paint::Pattern(p) if matches!(**p, Pattern::Shading(_)) => {
                self.push(ItemKind::Shading(item));
            }
            Paint::Pattern(_) => {
                self.diagnose(DiagnosticKind::PatternNotDescended);
                self.push(ItemKind::Path(item));
            }
            Paint::Color(_) => self.push(ItemKind::Path(item)),
        }
    }
}

fn id_to_ref(id: ObjectId) -> ObjRef {
    ObjRef::new(id.num as i32, i32::from(id.generation))
}

impl<'a> Device<'a> for Builder<'_> {
    fn draw_path(&mut self, path: &BezPath, props: DrawProps<'a>, mode: &DrawMode) {
        if self.charge(1) {
            self.path_item(path, &props, mode);
        }
    }

    fn push_clip_path(&mut self, clip: &ClipPath) {
        self.clips.push(clip.path.bounding_box());
        if self.charge(1) {
            self.push(ItemKind::Path(PathItem {
                geometry: clip.path.clone(),
                fill: Some(clip.fill),
                stroke: None,
                clip_only: true,
            }));
        }
    }

    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}

    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, mode: &DrawMode) {
        // Modes 2 and 6 draw the run twice (fill, stroke): it is one set of glyphs.
        if matches!(mode, DrawMode::Stroke(_)) && matches!(run.render_mode(), 2 | 6) {
            return;
        }
        let fill = match &props.paint {
            Paint::Color(c) => Some(c.to_rgba().to_rgba8()),
            Paint::Pattern(_) => None,
        };
        for g in run.glyphs() {
            if !self.charge(1) {
                return;
            }
            self.glyph(g, run.render_mode(), fill, props.transform);
        }
    }

    fn draw_image(&mut self, image: Image<'a, '_>, props: ImageDrawProps<'a>) {
        if self.error.is_some() {
            return;
        }
        match limits::image_pixels(self.limits, image.width(), image.height()) {
            Ok(pixels) => {
                if !self.charge(pixels.div_ceil(1000)) {
                    return;
                }
            }
            Err(e) => {
                self.error = Some(e);
                return;
            }
        }
        let (stream, is_stencil) = match &image {
            Image::Raster(r) => (r.stream(), false),
            Image::Stencil(s) => (s.stream(), true),
        };
        let (w, h) = (f64::from(image.width()), f64::from(image.height()));
        let id = stream.obj_id();
        let dict = stream.dict();
        self.push(ItemKind::Image(ImageItem {
            obj: (id.obj_number != 0).then(|| object_id(id.into())),
            // hayro draws in pixel space (y down); the unit square is what a CTM means.
            ctm: Matrix((props.transform * Affine::new([w, 0.0, 0.0, -h, 0.0, h])).as_coeffs()),
            width: image.width(),
            height: image.height(),
            has_smask: dict.contains_key("SMask"),
            has_mask: dict.contains_key("Mask"),
            is_stencil,
        }));
    }

    fn pop_clip(&mut self) {
        self.clips.pop();
    }

    fn pop_transparency_group(&mut self) {}

    fn begin_operator(&mut self, stream: ObjRef, range: Range<usize>) {
        self.charge(1);
        self.loc = SourceLoc {
            stream: object_id(stream),
            start: range.start as u32,
            end: range.end as u32,
        };
    }

    fn unknown_operator(&mut self, name: &[u8]) {
        let name = String::from_utf8_lossy(&name[..name.len().min(16)]).into_owned();
        self.diagnose(DiagnosticKind::UnknownOperator(name));
    }

    fn begin_form(&mut self, form: ObjRef, _: Affine) -> bool {
        if self.error.is_some() {
            return false;
        }
        let id = object_id(form);
        if self.stack[1..].iter().any(|f| f.step.form == id) {
            self.error = Some(PdfError::at(foliark_model::ErrorCode::MalformedPdf, id));
            return false;
        }
        // `stack[0]` is the page: with `len() - 1` Forms open, `maxNesting` Forms may be open.
        let depth = self.stack.len() as u64;
        if let Err(e) = limits::check("maxNesting", depth, self.limits.max_nesting.into()) {
            self.error = Some(e);
            return false;
        }
        self.entered.insert(id);
        let parent = self.stack.last_mut().expect("page frame");
        let seen = parent.drawn.entry(id).and_modify(|n| *n += 1).or_insert(0);
        let occurrence = *seen;
        let own_resources = self
            .xref
            .get::<Stream<'_>>(form.into())
            .is_some_and(|s| s.dict().contains_key(RESOURCES));
        let owner = if own_resources { id } else { parent.owner };
        self.stack.push(Frame {
            step: InstanceStep {
                form: id,
                occurrence,
            },
            owner,
            drawn: HashMap::new(),
        });
        true
    }

    fn end_form(&mut self) {
        self.stack.pop();
    }

    fn oc_state(&mut self, visible: bool) {
        self.hidden = !visible;
    }
}

/// The object that carries the `/Resources` a page inherits.
fn resources_owner(page: &Dict<'_>) -> ObjectId {
    let mut node = page.clone();
    // Bounded: a damaged /Parent chain may loop.
    for _ in 0..64 {
        if node.contains_key(RESOURCES) {
            break;
        }
        match node.get::<Dict<'_>>(PARENT) {
            Some(parent) => node = parent,
            None => break,
        }
    }
    node.obj_id().map_or(NO_OBJECT, |id| object_id(id.into()))
}

impl Document {
    /// Interpret a page into its semantic model. Hidden optional content is included
    /// (`ocg_hidden`); a content stream that fails to decode, a Form cycle, too deep nesting or
    /// an exceeded budget is an `Err`, never a partial page.
    pub fn page_model(&self, index: usize) -> Result<SemanticPage, PdfError> {
        let geometry = self.page_geometry(index)?;
        let mut budget = Budget::new(self.limits());
        self.page_contents_with(index, &mut budget)?;
        let page = self.page(index)?;
        let warnings = Arc::new(Mutex::new(Vec::new()));
        let sink = warnings.clone();
        let settings = InterpreterSettings {
            ocg_override: true,
            strict_streams: true,
            warning_sink: Arc::new(move |w| sink.lock().expect("warning sink").push(w)),
            ..InterpreterSettings::default()
        };
        let [x0, y0, x1, y1] = geometry.crop_box;
        let cache = InterpreterCache::new();
        // Identity: everything is reported in user space.
        let mut ctx = Context::new(
            Affine::IDENTITY,
            Rect::new(x0, y0, x1, y1),
            &cache,
            page.xref(),
            settings,
        );
        let mut device = Builder {
            xref: page.xref(),
            limits: self.limits(),
            budget: &mut budget,
            stack: vec![Frame {
                step: InstanceStep {
                    form: NO_OBJECT,
                    occurrence: 0,
                },
                owner: resources_owner(page.raw()),
                drawn: HashMap::new(),
            }],
            clips: Vec::new(),
            loc: SourceLoc {
                stream: NO_OBJECT,
                start: 0,
                end: 0,
            },
            hidden: false,
            items: Vec::new(),
            fonts: BTreeMap::new(),
            diagnostics: Vec::new(),
            type3: HashSet::new(),
            direct_font_reported: false,
            error: None,
            entered: HashSet::new(),
        };
        interpret_page(page, &mut ctx, &mut device);
        if let Some(e) = device.error {
            return Err(e);
        }
        let mut diagnostics = device.diagnostics;
        for w in warnings.lock().expect("warning sink").iter() {
            let kind = match *w {
                // The parser reports a reference cycle as an object that is not there.
                InterpreterWarning::StreamDecodeFailure(r) => {
                    return Err(PdfError::at(
                        foliark_model::ErrorCode::DecodeFailed,
                        object_id(r),
                    ));
                }
                InterpreterWarning::UnresolvedXObject(r) => {
                    return Err(PdfError::at(
                        foliark_model::ErrorCode::MalformedPdf,
                        object_id(r),
                    ));
                }
                InterpreterWarning::UnsupportedFont => DiagnosticKind::UnsupportedFont,
                InterpreterWarning::ImageDecodeFailure => DiagnosticKind::ImageDecodeFailure,
                InterpreterWarning::UnresolvedAnnotationAppearance => {
                    DiagnosticKind::UnresolvedAnnotationAppearance
                }
                // A warning kind this model does not know yet must not pass silently.
                _ => DiagnosticKind::InterpreterWarning,
            };
            diagnostics.push(Diagnostic { kind, loc: None });
        }
        self.check_repaired()?;
        Ok(SemanticPage {
            index,
            geometry,
            items: device.items,
            fonts: device.fonts,
            diagnostics,
        })
    }
}
