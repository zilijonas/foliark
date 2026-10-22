//! Engine sessions (spec §13): one `Engine` per worker owns the limits, the open documents,
//! the font registry and the long-op `Task`s. One method per public op, `DocId` strings as
//! handles. Ops that are not built yet return the typed refusal in [`not_built`].
use crate::cache::DOC_CACHE_BYTES;
use crate::document::{Binding, Committed, EditApplier, EditContext, Session};
use crate::edit::{ParagraphApplier, layout_edit};
use crate::export::{self, Held};
use crate::{Document, RenderOptions, RenderedPage};
use foliark_model::{
    Capabilities, ChangeResult, ChangeSet, DocumentInfo, EditPreview, ErrorCode, ErrorDetails,
    ExportMode, ExportPlan, ExportPolicy, ExtractOptions, FontMatchRule, FontSource, Hit, Limits,
    PageGeometry, ParagraphInfo, PdfError, Phase, Point, Progress, Rect, RegisteredFont, SearchHit,
    SearchOptions, TextPage, TextRange,
};
use foliark_text::Registry;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

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

/// Diagnostics for tests and leak checks: what the engine currently holds. No content.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Stats {
    pub documents: usize,
    /// Input bytes kept by open documents (each also lives parsed).
    pub source_bytes: usize,
    pub cache_entries: usize,
    /// Estimated heap of the cached page layouts.
    pub cache_bytes: usize,
    pub cache_evictions: u64,
    /// Bytes of the export plan held for `export` (0 when none).
    pub plan_bytes: usize,
}

/// An op the spec defines but this build does not implement. `INVALID_ARGUMENT` with
/// `details.feature` is the refusal: the spec has no dedicated code, `INTERNAL` would make
/// the client reset the worker, and `*_UNSUPPORTED` codes name specific PDF features.
pub(crate) fn not_built(feature: &str) -> PdfError {
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

pub struct Engine {
    limits: Limits,
    generation: u32,
    next_doc: u64,
    docs: HashMap<DocId, Session>,
    fonts: Registry,
    font_match: Vec<FontMatchRule>,
    applier: Box<dyn EditApplier>,
    cancel: Arc<AtomicBool>,
    cache_bytes: usize,
    /// The one export plan held (spec §4: any newer plan, change, font or close frees it).
    plan: Option<Held>,
    next_plan: u64,
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
            font_match: Vec::new(),
            applier: Box::new(ParagraphApplier),
            cancel: Arc::default(),
            cache_bytes: DOC_CACHE_BYTES,
            plan: None,
            next_plan: 0,
        }
    }

    /// Cap the layout cache of each document opened from now on (default 16 MiB).
    pub fn with_cache_bytes(mut self, bytes: usize) -> Self {
        self.cache_bytes = bytes;
        self
    }

    /// Setting this flag from another thread stops the page being interpreted at the next
    /// operator, and that operation fails `CANCELLED` with nothing changed. The host clears it
    /// afterwards. A single-threaded wasm worker cannot set it mid-step: there a cancel is
    /// honoured between steps, and a step that does not return is ended by the client's reset.
    pub fn cancel_handle(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    /// `fontMatch` of the SDK's `init` (spec §6.4): replaces the default name-to-family table.
    pub fn with_font_match(mut self, rules: Vec<FontMatchRule>) -> Self {
        self.font_match = rules;
        self
    }

    /// Replace the text-edit implementation (default: [`ParagraphApplier`]).
    pub fn with_edit_applier(mut self, applier: Box<dyn EditApplier>) -> Self {
        self.applier = applier;
        self
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

    pub fn stats(&self) -> Stats {
        let seed = Stats {
            plan_bytes: self
                .plan
                .as_ref()
                .and_then(|p| p.bytes.as_ref())
                .map_or(0, Vec::len),
            ..Stats::default()
        };
        self.docs.values().fold(seed, |mut s, d| {
            s.documents += 1;
            s.source_bytes += d.source_len();
            let (image_bytes, image_entries, image_evictions) = d.image_usage();
            s.cache_entries += d.index.len() + image_entries;
            s.cache_bytes += d.index.bytes() + image_bytes;
            s.cache_evictions += d.index.evictions() + image_evictions;
            s
        })
    }

    /// Spec §4: a task started at one revision must finish at it. A commit in between makes it
    /// `STALE_REVISION` instead of mixing two revisions in one result. `at` is the revision when
    /// the task was created (`None`: the document was not open then, which fails as it always did).
    fn still_at(&self, doc: &str, at: Option<u32>) -> Result<(), PdfError> {
        let now = self.get(doc)?.revision;
        match at {
            Some(r) if r != now => Err(PdfError::new(ErrorCode::StaleRevision)),
            _ => Ok(()),
        }
    }

    fn get(&self, doc: &str) -> Result<&Session, PdfError> {
        self.docs
            .get(doc)
            .ok_or(PdfError::new(ErrorCode::DocumentGone))
    }

    /// The revision of an open document (0 at open, +1 per accepted `apply_changes`).
    pub fn revision(&self, doc: &str) -> Result<u32, PdfError> {
        Ok(self.get(doc)?.revision)
    }

    /// What a derived result (an export plan, a cached layout) must be checked against.
    pub fn binding(&self, doc: &str) -> Result<Binding, PdfError> {
        Ok(Binding {
            doc_id: doc.to_owned(),
            generation: self.generation,
            revision: self.get(doc)?.revision,
        })
    }

    /// True while `binding` names an open document at its current revision.
    pub fn is_current(&self, binding: &Binding) -> bool {
        binding.generation == self.generation
            && self
                .get(&binding.doc_id)
                .is_ok_and(|s| s.revision == binding.revision)
    }

    /// The working copy at the current revision (the source until an edit is committed).
    pub fn working_copy(&self, doc: &str) -> Result<&Document, PdfError> {
        Ok(self.get(doc)?.doc())
    }

    /// The committed edit and redaction lists of the current revision.
    pub fn committed(&self, doc: &str) -> Result<&Committed, PdfError> {
        Ok(&self.get(doc)?.committed)
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
        let source = bytes.clone();
        let doc = Document::open_with_limits(bytes, password, allow_repaired, self.limits.clone())?
            .with_cancel(self.cancel.clone());
        let info = doc.reader().info()?;
        self.next_doc += 1;
        let id = format!("{}-{}", self.generation, self.next_doc);
        self.docs.insert(
            id.clone(),
            Session::new(id.clone(), source, doc, info.clone(), self.cache_bytes),
        );
        Ok((id, info))
    }

    pub fn info(&mut self, doc: &str) -> Result<DocumentInfo, PdfError> {
        Ok(self.session(doc)?.info.clone())
    }

    pub fn pages(&mut self, doc: &str) -> Result<Vec<PageGeometry>, PdfError> {
        let d = self.session(doc)?.doc();
        (0..d.page_count()).map(|i| d.page_geometry(i)).collect()
    }

    /// Idempotent: an unknown or already closed id is fine. Frees the document and its caches.
    pub fn close(&mut self, doc: &str) {
        self.plan = None;
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
        Ok(s.layout(page, false)?.hit_test(p, tolerance))
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
        Ok(s.layout(page, false)?.select(from, to))
    }

    pub fn quads_for(
        &mut self,
        doc: &str,
        page: u32,
        start: u32,
        end: u32,
    ) -> Result<TextRange, PdfError> {
        let s = self.session(doc)?;
        s.layout(page, false)?.quads_for(start, end)
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
        self.plan = None;
        fonts.into_iter().map(|f| self.fonts.register(f)).collect()
    }

    pub fn extract_text(&self, doc: &str, page: u32, opts: ExtractOptions) -> BoxTask<TextPage> {
        let at = self.revision(doc).ok();
        let doc = doc.to_owned();
        once(move |e| {
            e.still_at(&doc, at)?;
            let s = e.session(&doc)?;
            let mut text = s.layout(page, opts.include_ocg_hidden)?.page.clone();
            if !opts.chars {
                text.clusters = None;
            }
            Ok(text)
        })
    }

    /// Searches `opts.pages` (default all) in page order, one batch of pages per step.
    pub fn search(&self, doc: &str, query: &str, opts: SearchOptions) -> BoxTask<Vec<SearchHit>> {
        Box::new(SearchTask {
            at: self.revision(doc).ok(),
            doc: doc.to_owned(),
            query: query.to_owned(),
            opts,
            pages: None,
            next: 0,
            hits: Vec::new(),
        })
    }

    pub fn render(&self, doc: &str, page: u32, opts: RenderOptions) -> BoxTask<RenderedPage> {
        let at = self.revision(doc).ok();
        let doc = doc.to_owned();
        once(move |e| {
            e.still_at(&doc, at)?;
            e.session(&doc)?.doc().render_page(page as usize, opts)
        })
    }

    /// Spec §11. Extract and render are supported once the document opened; export is
    /// `unknown` until `prepare_export` succeeds (then `supported` for that revision and mode)
    /// or preflight already knows a stage refuses (`unsupported`). `deep` adds nothing yet:
    /// the redaction preflight already interprets every page it needs.
    pub fn capabilities(&self, doc: &str, _deep: bool) -> BoxTask<Capabilities> {
        let at = self.revision(doc).ok();
        let doc = doc.to_owned();
        once(move |e| {
            e.still_at(&doc, at)?;
            let held = |m| e.held(&doc, m);
            let ready = (held(ExportMode::Native), held(ExportMode::Raster));
            let s = e.get(&doc)?;
            export::capabilities(s.doc(), &s.committed, e.binding(&doc)?, ready)
        })
    }

    /// A plan of `mode` is held for `doc` at its current revision and font registry.
    fn held(&self, doc: &str, mode: ExportMode) -> bool {
        self.plan.as_ref().is_some_and(|p| {
            p.mode == mode
                && p.bytes.is_some()
                && p.binding.doc_id == doc
                && p.fonts == self.fonts.version()
                && self.is_current(&p.binding)
        })
    }

    /// Spec §1.5, §6: the selection `rects` make on the working copy and the budget around it.
    pub fn prepare_paragraph(
        &mut self,
        doc: &str,
        page: u32,
        rects: Vec<Rect>,
        font: Option<String>,
        editor_face: bool,
    ) -> Result<ParagraphInfo, PdfError> {
        let flat: Vec<f64> = rects
            .iter()
            .flat_map(|r| [r.x, r.y, r.width, r.height])
            .collect();
        finite(&flat)?;
        let s = self.get(doc)?;
        let mut info = s.doc().prepare_paragraph(
            page as usize,
            &rects,
            font.as_deref(),
            editor_face,
            &self.fonts,
            &self.font_match,
        )?;
        info.doc_id = doc.to_owned();
        info.revision = s.revision;
        Ok(info)
    }

    /// ADR 0004: the layout `apply_changes` would commit for one edit of the selection `rects`
    /// make, with nothing committed or changed. Refuses as `apply_changes` would, except that
    /// text that does not fit comes back with `fits: false`.
    pub fn layout_edit(
        &self,
        doc: &str,
        page: u32,
        rects: Vec<Rect>,
        text: &str,
        font: Option<String>,
        glyphs: bool,
    ) -> Result<EditPreview, PdfError> {
        let flat: Vec<f64> = rects
            .iter()
            .flat_map(|r| [r.x, r.y, r.width, r.height])
            .collect();
        finite(&flat)?;
        let s = self.get(doc)?;
        let ctx = EditContext {
            limits: &self.limits,
            fonts: &self.fonts,
            font_match: &self.font_match,
        };
        let mut p = layout_edit(
            s.doc(),
            page as usize,
            &rects,
            font.as_deref(),
            text,
            glyphs,
            &ctx,
        )?;
        p.doc_id = doc.to_owned();
        p.revision = s.revision;
        Ok(p)
    }

    /// Spec §4: atomic at `changes.base_revision`. A stale base is `STALE_REVISION`; item
    /// rejections come back in the result with `accepted: false` and change nothing.
    pub fn apply_changes(&self, doc: &str, changes: ChangeSet) -> BoxTask<ChangeResult> {
        let doc = doc.to_owned();
        once(move |e| {
            let s = e
                .docs
                .get_mut(&doc)
                .ok_or(PdfError::new(ErrorCode::DocumentGone))?;
            let ctx = EditContext {
                limits: &e.limits,
                fonts: &e.fonts,
                font_match: &e.font_match,
            };
            let result = s.apply(changes, &ctx, &*e.applier)?;
            if result.accepted {
                e.plan = None;
            }
            Ok(result)
        })
    }

    /// Spec §4: runs the whole export now and holds the bytes (a newer plan, `apply_changes`,
    /// `register_fonts` or `close` frees them). The older plan is freed first, so a failure
    /// leaves none and two outputs are never held at once.
    pub fn prepare_export(
        &self,
        doc: &str,
        mode: ExportMode,
        policy: ExportPolicy,
    ) -> BoxTask<ExportPlan> {
        let at = self.revision(doc).ok();
        let doc = doc.to_owned();
        if mode == ExportMode::Raster {
            // Raster export draws page by page: a real stepping task, so a cancel lands between pages.
            return Box::new(RasterTask {
                doc,
                at,
                policy,
                job: None,
            });
        }
        once(move |e| {
            e.still_at(&doc, at)?;
            e.plan = None;
            crate::cmaps::require(&e.session(&doc)?.info.required_assets)?;
            let binding = e.binding(&doc)?;
            let s = e.get(&doc)?;
            let mut out = export::prepare(
                s.doc(),
                &s.committed,
                binding.clone(),
                mode,
                &policy,
                &e.limits,
            )?;
            out.report.edits.approximate = s.approximate;
            Ok(e.hold(doc, binding, mode, &policy, out))
        })
    }

    /// Keep `out` as the one held plan of `doc` and describe it.
    fn hold(
        &mut self,
        doc: String,
        binding: Binding,
        mode: ExportMode,
        policy: &ExportPolicy,
        out: export::Prepared,
    ) -> ExportPlan {
        self.next_plan += 1;
        let plan = ExportPlan {
            plan_id: format!("plan-{}-{}", self.generation, self.next_plan),
            doc_id: doc,
            generation: binding.generation,
            revision: binding.revision,
            mode,
            policy_hash: export::preflight::policy_hash(mode, policy),
            font_registry_version: self.fonts.version(),
            byte_length: out.bytes.len() as u32,
            report: out.report,
        };
        self.plan = Some(Held {
            id: plan.plan_id.clone(),
            binding,
            mode,
            fonts: plan.font_registry_version,
            bytes: Some(out.bytes),
        });
        plan
    }

    /// Single use. `STALE_PLAN`: not the held plan, or its revision, document or fonts moved on
    /// (the plan is freed). `PLAN_CONSUMED`: already exported.
    pub fn take_export(&mut self, doc: &str, plan_id: &str) -> Result<Vec<u8>, PdfError> {
        self.get(doc)?;
        let stale = || PdfError::new(ErrorCode::StalePlan);
        let held = self
            .plan
            .as_ref()
            .filter(|p| p.id == plan_id && p.binding.doc_id == doc)
            .ok_or_else(stale)?;
        if held.bytes.is_none() {
            return Err(PdfError::new(ErrorCode::PlanConsumed));
        }
        if held.fonts != self.fonts.version() || !self.is_current(&held.binding) {
            self.plan = None;
            return Err(stale());
        }
        self.plan
            .as_mut()
            .and_then(|p| p.bytes.take())
            .ok_or(PdfError::new(ErrorCode::Internal))
    }
}

struct SearchTask {
    doc: DocId,
    at: Option<u32>,
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
        e.still_at(&self.doc, self.at)?;
        let s = e.session(&self.doc)?;
        let pages = self.pages.get_or_insert_with(|| {
            let mut p: Vec<u32> = self
                .opts
                .pages
                .clone()
                .unwrap_or_else(|| (0..s.doc().page_count() as u32).collect());
            p.sort_unstable();
            p.dedup();
            p
        });
        let max_hits = self.opts.max_hits as usize;
        let mut spent = 0u64;
        while self.next < pages.len() && self.hits.len() < max_hits && spent < budget {
            let layout = s.layout(pages[self.next], false)?;
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

/// Spec §7 raster export as a stepping task. The older plan is freed on the first step; a cancel
/// or failure between pages drops the job and leaves no plan.
struct RasterTask {
    doc: DocId,
    at: Option<u32>,
    policy: ExportPolicy,
    job: Option<export::raster::Job>,
}

impl Task for RasterTask {
    type Out = ExportPlan;

    fn step(&mut self, e: &mut Engine, budget: u64) -> Result<Step<ExportPlan>, PdfError> {
        e.still_at(&self.doc, self.at)?;
        let binding = e.binding(&self.doc)?;
        if self.job.is_none() {
            e.plan = None;
            let s = e.get(&self.doc)?;
            let job = export::raster::Job::new(
                s.doc(),
                &s.committed,
                binding.clone(),
                &self.policy,
                &e.limits,
            )?;
            self.job = Some(job);
        }
        let job = self
            .job
            .as_mut()
            .ok_or(PdfError::new(ErrorCode::Internal))?;
        let out = match job.step(e.get(&self.doc)?.doc(), &e.limits, budget)? {
            Some(mut out) => {
                out.report.edits.approximate = e.get(&self.doc)?.approximate;
                out
            }
            None => {
                let (done, total) = job.progress();
                return Ok(Step::Pending(Progress {
                    phase: Phase::Export,
                    done,
                    total,
                }));
            }
        };
        let plan = e.hold(
            self.doc.clone(),
            binding,
            ExportMode::Raster,
            &self.policy,
            out,
        );
        Ok(Step::Done(plan))
    }
}
