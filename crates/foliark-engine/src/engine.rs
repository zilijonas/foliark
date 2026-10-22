//! Engine sessions (spec §13): one `Engine` per worker owns the limits, the open documents,
//! the font registry and the long-op `Task`s. One method per public op, `DocId` strings as
//! handles. Ops that are not built yet return the typed refusal in [`not_built`].
use crate::text::TextLayout;
use crate::{Document, RenderOptions, RenderedPage};
use foliark_model::{
    Capabilities, ChangeResult, ChangeSet, DocumentInfo, ErrorCode, ErrorDetails, ExportMode,
    ExportPlan, ExportPolicy, ExtractOptions, FontSource, Hit, Limits, PageGeometry, ParagraphInfo,
    PdfError, Phase, Point, Progress, Rect, RegisteredFont, SearchHit, SearchOptions, TextPage,
    TextRange,
};
use foliark_text::Registry;
use std::collections::HashMap;

pub type DocId = String;

/// Spec §13: a long op advances in bounded steps, so the worker can yield and honour a cancel
/// in between. Dropping a task cancels it; tasks never change engine state until `Done`.
pub enum Step<T> {
    Pending(Progress),
    Done(T),
}

pub trait Task {
    type Out;
    /// Run until about `budget` work units are spent (at least one page), then report.
    fn step(&mut self, e: &mut Engine, budget: u64) -> Result<Step<Self::Out>, PdfError>;
}

pub type BoxTask<T> = Box<dyn Task<Out = T>>;

/// An op the spec defines but this build does not implement. `INVALID_ARGUMENT` with
/// `details.feature` is the refusal: the spec has no dedicated code, `INTERNAL` would make
/// the client reset the worker, and `*_UNSUPPORTED` codes name specific PDF features.
fn not_built(feature: &str) -> PdfError {
    PdfError::with(
        ErrorCode::InvalidArgument,
        ErrorDetails {
            feature: Some(feature.into()),
            ..Default::default()
        },
    )
}

/// A task that does its whole job in one step (its cost is one page).
type Job<T> = Box<dyn FnOnce(&mut Engine) -> Result<T, PdfError>>;

struct Once<T>(Option<Job<T>>);

impl<T> Task for Once<T> {
    type Out = T;
    fn step(&mut self, e: &mut Engine, _budget: u64) -> Result<Step<T>, PdfError> {
        let run = self.0.take().ok_or(PdfError::new(ErrorCode::Internal))?;
        run(e).map(Step::Done)
    }
}

fn once<T: 'static>(f: impl FnOnce(&mut Engine) -> Result<T, PdfError> + 'static) -> BoxTask<T> {
    Box::new(Once(Some(Box::new(f))))
}

fn refused<T: 'static>(feature: &'static str) -> BoxTask<T> {
    once(move |_| Err(not_built(feature)))
}

struct Session {
    doc: Document,
    info: DocumentInfo,
    revision: u32,
    /// The last page layout built (hit testing and selection hit one page repeatedly).
    layout: Option<(usize, bool, TextLayout)>,
}

impl Session {
    fn layout(&mut self, id: &str, page: u32, hidden: bool) -> Result<&TextLayout, PdfError> {
        let page = page as usize;
        if self
            .layout
            .as_ref()
            .is_none_or(|(p, h, _)| (*p, *h) != (page, hidden))
        {
            let mut l = self.doc.text_layout(page, hidden)?;
            l.page.doc_id = id.to_owned();
            l.page.revision = self.revision;
            self.layout = Some((page, hidden, l));
        }
        self.layout
            .as_ref()
            .map(|(_, _, l)| l)
            .ok_or(PdfError::new(ErrorCode::Internal))
    }
}

pub struct Engine {
    limits: Limits,
    generation: u32,
    next_doc: u64,
    docs: HashMap<DocId, Session>,
    fonts: Registry,
}

fn finite(values: &[f64]) -> Result<(), PdfError> {
    values
        .iter()
        .all(|v| v.is_finite())
        .then_some(())
        .ok_or(PdfError::new(ErrorCode::InvalidArgument))
}

impl Engine {
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            generation: 1,
            next_doc: 0,
            docs: HashMap::new(),
            fonts: Registry::new(),
        }
    }

    /// The worker generation stamped into doc ids and (later) export plans.
    pub fn with_generation(mut self, generation: u32) -> Self {
        self.generation = generation;
        self
    }

    pub fn generation(&self) -> u32 {
        self.generation
    }

    pub fn font_registry_version(&self) -> u32 {
        self.fonts.version()
    }

    pub fn open_documents(&self) -> usize {
        self.docs.len()
    }

    fn session(&mut self, doc: &str) -> Result<&mut Session, PdfError> {
        self.docs
            .get_mut(doc)
            .ok_or(PdfError::new(ErrorCode::DocumentGone))
    }

    /// Parse, preflight (budgets, features) and register a document at revision 0.
    pub fn open(
        &mut self,
        bytes: Vec<u8>,
        password: Option<&str>,
        allow_repaired: bool,
    ) -> Result<(DocId, DocumentInfo), PdfError> {
        let max = u64::from(self.limits.max_open_documents);
        let open = self.docs.len() as u64;
        if open >= max {
            return Err(PdfError::limit("maxOpenDocuments", open + 1, max));
        }
        let doc = Document::open_with_limits(bytes, password, allow_repaired, self.limits.clone())?;
        let info = doc.reader().info()?;
        self.next_doc += 1;
        let id = format!("{}-{}", self.generation, self.next_doc);
        self.docs.insert(
            id.clone(),
            Session {
                doc,
                info: info.clone(),
                revision: 0,
                layout: None,
            },
        );
        Ok((id, info))
    }

    pub fn info(&mut self, doc: &str) -> Result<DocumentInfo, PdfError> {
        Ok(self.session(doc)?.info.clone())
    }

    pub fn pages(&mut self, doc: &str) -> Result<Vec<PageGeometry>, PdfError> {
        let d = &self.session(doc)?.doc;
        (0..d.page_count()).map(|i| d.page_geometry(i)).collect()
    }

    /// Idempotent: an unknown or already closed id is fine. Frees the document and its caches.
    pub fn close(&mut self, doc: &str) {
        self.docs.remove(doc);
    }

    /// Spec §1: `tolerance` defaults to 2 pt.
    pub fn hit_test(
        &mut self,
        doc: &str,
        page: u32,
        p: Point,
        tolerance: Option<f64>,
    ) -> Result<Option<Hit>, PdfError> {
        let tolerance = tolerance.unwrap_or(2.0);
        finite(&[p.x, p.y, tolerance])?;
        if tolerance < 0.0 {
            return Err(PdfError::new(ErrorCode::InvalidArgument));
        }
        let s = self.session(doc)?;
        Ok(s.layout(doc, page, false)?.hit_test(p, tolerance))
    }

    pub fn select(
        &mut self,
        doc: &str,
        page: u32,
        from: Point,
        to: Point,
    ) -> Result<Option<TextRange>, PdfError> {
        finite(&[from.x, from.y, to.x, to.y])?;
        let s = self.session(doc)?;
        Ok(s.layout(doc, page, false)?.select(from, to))
    }

    pub fn quads_for(
        &mut self,
        doc: &str,
        page: u32,
        start: u32,
        end: u32,
    ) -> Result<TextRange, PdfError> {
        let s = self.session(doc)?;
        s.layout(doc, page, false)?.quads_for(start, end)
    }

    /// All or nothing: every face is validated before the registry changes. Registered fonts
    /// are shared by all documents; `font_registry_version` goes up for each face added.
    pub fn register_fonts(
        &mut self,
        fonts: Vec<FontSource>,
    ) -> Result<Vec<RegisteredFont>, PdfError> {
        let incoming: u64 = fonts.iter().map(|f| f.bytes.len() as u64).sum();
        let max = u64::from(self.limits.max_registered_font_bytes);
        let total = self.fonts.total_bytes().saturating_add(incoming);
        if total > max {
            return Err(PdfError::limit("maxRegisteredFontBytes", total, max));
        }
        let mut scratch = Registry::new();
        for f in &fonts {
            scratch.register(f.clone())?;
        }
        fonts.into_iter().map(|f| self.fonts.register(f)).collect()
    }

    pub fn extract_text(&self, doc: &str, page: u32, opts: ExtractOptions) -> BoxTask<TextPage> {
        let doc = doc.to_owned();
        once(move |e| {
            let s = e.session(&doc)?;
            let mut text = s.layout(&doc, page, opts.include_ocg_hidden)?.page.clone();
            if !opts.chars {
                text.clusters = None;
            }
            Ok(text)
        })
    }

    /// Searches `opts.pages` (default all) in page order, one batch of pages per step.
    pub fn search(&self, doc: &str, query: &str, opts: SearchOptions) -> BoxTask<Vec<SearchHit>> {
        Box::new(SearchTask {
            doc: doc.to_owned(),
            query: query.to_owned(),
            opts,
            pages: None,
            next: 0,
            hits: Vec::new(),
        })
    }

    pub fn render(&self, doc: &str, page: u32, opts: RenderOptions) -> BoxTask<RenderedPage> {
        let doc = doc.to_owned();
        once(move |e| e.session(&doc)?.doc.render_page(page as usize, opts))
    }

    pub fn capabilities(&self, _doc: &str, _deep: bool) -> BoxTask<Capabilities> {
        refused("capabilities")
    }

    pub fn prepare_paragraph(
        &mut self,
        _doc: &str,
        _page: u32,
        _rects: Vec<Rect>,
        _font: Option<String>,
        _editor_face: bool,
    ) -> Result<ParagraphInfo, PdfError> {
        Err(not_built("prepareParagraph"))
    }

    pub fn apply_changes(&self, _doc: &str, _changes: ChangeSet) -> BoxTask<ChangeResult> {
        refused("applyChanges")
    }

    pub fn prepare_export(
        &self,
        _doc: &str,
        _mode: ExportMode,
        _policy: ExportPolicy,
    ) -> BoxTask<ExportPlan> {
        refused("prepareExport")
    }

    pub fn take_export(&mut self, _doc: &str, _plan_id: &str) -> Result<Vec<u8>, PdfError> {
        Err(not_built("export"))
    }
}

struct SearchTask {
    doc: DocId,
    query: String,
    opts: SearchOptions,
    /// Sorted, deduplicated; resolved on the first step so a gone document fails there.
    pages: Option<Vec<u32>>,
    next: usize,
    hits: Vec<SearchHit>,
}

impl Task for SearchTask {
    type Out = Vec<SearchHit>;

    fn step(&mut self, e: &mut Engine, budget: u64) -> Result<Step<Self::Out>, PdfError> {
        let s = e.session(&self.doc)?;
        let pages = self.pages.get_or_insert_with(|| {
            let mut p: Vec<u32> = self
                .opts
                .pages
                .clone()
                .unwrap_or_else(|| (0..s.doc.page_count() as u32).collect());
            p.sort_unstable();
            p.dedup();
            p
        });
        let max_hits = self.opts.max_hits as usize;
        let mut spent = 0u64;
        while self.next < pages.len() && self.hits.len() < max_hits && spent < budget {
            let layout = s.layout(&self.doc, pages[self.next], false)?;
            // A page costs its clusters (work units, spec §9), and at least one.
            spent += layout.page.clusters.as_ref().map_or(0, Vec::len) as u64 + 1;
            let room = max_hits - self.hits.len();
            self.hits
                .extend(layout.search(&self.query, &self.opts, room));
            self.next += 1;
        }
        if self.next < pages.len() && self.hits.len() < max_hits {
            return Ok(Step::Pending(Progress {
                phase: Phase::Extract,
                done: self.next as u32,
                total: pages.len() as u32,
            }));
        }
        Ok(Step::Done(std::mem::take(&mut self.hits)))
    }
}
