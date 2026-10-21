//! Provenance: for every glyph, path and image the interpreter draws, where in the file the
//! operator is (stream + byte range), which Form instances it is drawn through and which
//! object owns the resources it uses.
use super::{Document, ReaderError, object_id};
use foliark_model::{InstancePath, InstanceStep, ObjectId, Origin, SourceLoc};
use hayro::hayro_interpret::font::GlyphRun;
use hayro::hayro_interpret::hayro_syntax::object::dict::keys::{PARENT, RESOURCES};
use hayro::hayro_interpret::hayro_syntax::object::{Dict, ObjRef, Stream};
use hayro::hayro_interpret::hayro_syntax::xref::XRef;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageDrawProps,
    InterpreterCache, InterpreterSettings, InterpreterWarning, SoftMask, TransformExt,
    interpret_page,
};
use hayro::vello_cpu::peniko::kurbo::{Affine, Rect, Shape};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::{Arc, Mutex};

/// Deepest Form/pattern/Type3/appearance nesting (spec §9 `maxNesting` default).
pub const MAX_NESTING: u32 = 32;

#[derive(Debug, Clone, PartialEq)]
pub enum ItemKind {
    /// `code` is the raw character code, `font` the font dictionary if it is an indirect object.
    Glyph {
        font: Option<ObjectId>,
        code: u32,
    },
    Path,
    /// `xobject` is `None` for an inline image.
    Image {
        xobject: Option<ObjectId>,
    },
}

/// One drawn item of a page.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub kind: ItemKind,
    pub origin: Origin,
    pub instance: InstancePath,
    /// The object whose dictionary holds the `/Resources` in effect (a Form, a page or an
    /// ancestor `/Pages` node for inherited resources).
    pub resource_owner: ObjectId,
    /// False for content that optional content hides (it is interpreted anyway).
    pub oc_visible: bool,
    /// Glyph (font space to display), image (unit square to display) or path base matrix.
    pub transform: [f64; 6],
    /// Display-space bounds of the active clips, intersected.
    pub clip: Option<[f64; 4]>,
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

struct ProvenanceDevice<'x> {
    xref: &'x XRef,
    /// `stack[0]` is the page.
    stack: Vec<Frame>,
    clips: Vec<Rect>,
    origin: Origin,
    oc_visible: bool,
    items: Vec<Item>,
    error: Option<ReaderError>,
    /// Every Form that was entered, to tell a cycle from a damaged object.
    entered: HashSet<ObjectId>,
}

impl ProvenanceDevice<'_> {
    fn push_item(&mut self, kind: ItemKind, transform: Affine) {
        let clip = self.clips.split_first().map(|(first, rest)| {
            let r = rest.iter().fold(*first, |acc, c| acc.intersect(*c));
            [r.x0, r.y0, r.x1, r.y1]
        });
        let frame = self.stack.last().expect("page frame");
        self.items.push(Item {
            kind,
            origin: self.origin,
            instance: InstancePath(self.stack[1..].iter().map(|f| f.step).collect()),
            resource_owner: frame.owner,
            oc_visible: self.oc_visible,
            transform: transform.as_coeffs(),
            clip,
        });
    }
}

impl<'a> Device<'a> for ProvenanceDevice<'_> {
    fn draw_path(
        &mut self,
        _: &hayro::vello_cpu::kurbo::BezPath,
        props: DrawProps<'a>,
        _: &DrawMode,
    ) {
        self.push_item(ItemKind::Path, props.transform);
    }

    fn push_clip_path(&mut self, clip_path: &ClipPath) {
        self.clips.push(clip_path.path.bounding_box());
    }

    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}

    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, _: &DrawMode) {
        for g in run.glyphs() {
            let kind = ItemKind::Glyph {
                font: g.font_ref().map(object_id),
                code: g.char_code(),
            };
            self.push_item(kind, props.transform * g.transform());
        }
    }

    fn draw_image(&mut self, image: Image<'a, '_>, props: ImageDrawProps<'a>) {
        let id = match &image {
            Image::Raster(r) => r.stream().obj_id(),
            Image::Stencil(s) => s.stream().obj_id(),
        };
        let xobject = (id.obj_number != 0).then(|| object_id(id.into()));
        self.push_item(ItemKind::Image { xobject }, props.transform);
    }

    fn pop_clip(&mut self) {
        self.clips.pop();
    }

    fn pop_transparency_group(&mut self) {}

    fn begin_operator(&mut self, stream: ObjRef, range: Range<usize>) {
        self.origin = Origin::Source {
            loc: SourceLoc {
                stream: object_id(stream),
                start: range.start as u32,
                end: range.end as u32,
            },
        };
    }

    fn begin_form(&mut self, form: ObjRef, _: Affine) -> bool {
        if self.error.is_some() {
            return false;
        }
        let id = object_id(form);
        if self.stack[1..].iter().any(|f| f.step.form == id) {
            self.error = Some(ReaderError::FormCycle { obj: id });
            return false;
        }
        if self.stack.len() > MAX_NESTING as usize {
            self.error = Some(ReaderError::NestingLimit { limit: MAX_NESTING });
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
        self.oc_visible = visible;
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
    /// Interpret a page and report every glyph, path and image with its provenance. Hidden
    /// optional content is included (`oc_visible == false`), and any content stream that fails
    /// to decode, a Form cycle or too deep nesting is an `Err`.
    pub fn page_items(&self, index: usize) -> Result<Vec<Item>, ReaderError> {
        self.page_contents(index)?;
        let page = self.page(index)?;
        let warnings = Arc::new(Mutex::new(Vec::new()));
        let sink = warnings.clone();
        let settings = InterpreterSettings {
            ocg_override: true,
            strict_streams: true,
            warning_sink: Arc::new(move |w| sink.lock().expect("warning sink").push(w)),
            ..InterpreterSettings::default()
        };
        let (w, h) = page.render_dimensions();
        let cache = InterpreterCache::new();
        let mut ctx = Context::new(
            page.initial_transform(true).to_kurbo(),
            Rect::new(0.0, 0.0, f64::from(w), f64::from(h)),
            &cache,
            page.xref(),
            settings,
        );
        let mut device = ProvenanceDevice {
            xref: page.xref(),
            stack: vec![Frame {
                step: InstanceStep {
                    form: NO_OBJECT,
                    occurrence: 0,
                },
                owner: resources_owner(page.raw()),
                drawn: HashMap::new(),
            }],
            clips: Vec::new(),
            origin: Origin::Source {
                loc: SourceLoc {
                    stream: NO_OBJECT,
                    start: 0,
                    end: 0,
                },
            },
            oc_visible: true,
            items: Vec::new(),
            error: None,
            entered: HashSet::new(),
        };
        interpret_page(page, &mut ctx, &mut device);
        if let Some(e) = device.error {
            return Err(e);
        }
        // The parser reports a reference cycle as an object that is not there.
        let failed = warnings
            .lock()
            .expect("warning sink")
            .iter()
            .find_map(|w| match *w {
                InterpreterWarning::StreamDecodeFailure(r) => {
                    Some(ReaderError::DecodeFailed { obj: object_id(r) })
                }
                InterpreterWarning::UnresolvedXObject(r) => {
                    let obj = object_id(r);
                    Some(if device.entered.contains(&obj) {
                        ReaderError::FormCycle { obj }
                    } else {
                        ReaderError::ContentUnreadable { obj }
                    })
                }
                _ => None,
            });
        if let Some(e) = failed {
            return Err(e);
        }
        self.check_repaired()?;
        Ok(device.items)
    }
}
