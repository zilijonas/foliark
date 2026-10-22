//! Form and content-stream clones (spec §5.3): text, image and vector removal write into the
//! stream of the occurrence they act on, never into one that another occurrence also draws.
//!
//! A Form object drawn more than once in the document (`PlannedForm::shared`) is cloned for
//! every occurrence an affected item sits under, along the whole chain from the page down: the
//! innermost clone carries the rewritten content, and each ancestor on the chain is a clone
//! too, because its `Do` of the child now names the child's clone (a fresh `/FlkFmN` entry in
//! the ancestor's resources; the old entry is dropped when no `Do` of it is left, so an
//! original stays reachable only through occurrences that draw it). Every other occurrence of
//! the same Forms keeps the original, byte for byte. A Form drawn once is rewritten in place.
//! A content stream that several pages draw is cloned per affected page.
//!
//! The clone is a writer alias (`Writer::alias`): same dictionary and decoded content as its
//! source, so source byte ranges stay valid and the three removals only need the target id.
//!
//! Limits, typed refusals: a Form with no `/Resources` of its own that needs a resource entry
//! (`"redactionFormInheritedResources"`), and a `Do` that the model reports for an occurrence
//! but that is not a `Do` in the parent's stream, such as an annotation appearance or a
//! pattern cell (`"redactionFormNotDrawnByDo"`). Content of soft-mask groups is not part of
//! the semantic model, so it is neither planned nor cloned.
use super::plan::RedactionPlan;
use super::text::{at, decode};
use crate::Document;
use crate::reader::object_id;
use crate::writer::{Val, Writer};
use foliark_model::{ErrorCode, ErrorDetails, InstanceStep, ObjectId, PdfError, SourceLoc};
use hayro::hayro_syntax::content::UntypedIter;
use hayro::hayro_syntax::object::{MaybeRef, Object, Stream};
use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Range;

/// Clone ids: generation 65535 marks a free slot, so no input object has one.
const CLONE_BASE: u32 = 0x4000_0000;
const CLONE_GENERATION: u16 = u16::MAX;

pub(super) fn refuse(feature: &str, page: u32) -> PdfError {
    PdfError::with(
        ErrorCode::InvalidArgument,
        ErrorDetails {
            feature: Some(feature.into()),
            pages: Some(vec![page]),
            ..ErrorDetails::default()
        },
    )
}

/// Which object each removal writes into, per (page, occurrence), for one plan.
pub(super) struct Targets {
    forms: HashMap<(u32, Vec<InstanceStep>), ObjectId>,
    content: BTreeMap<(u32, ObjectId), ObjectId>,
}

/// The streams of a page's `/Contents`, without decoding them.
pub(super) fn content_ids(doc: &Document, page: usize) -> Result<Vec<ObjectId>, PdfError> {
    let dict = doc.reader().page(page)?.raw();
    let xref = doc.reader().pdf().xref();
    let refs = |a: &hayro::hayro_syntax::object::Array<'_>| -> Vec<ObjectId> {
        a.raw_iter()
            .filter_map(|m| match m {
                MaybeRef::Ref(r) => Some(object_id(r)),
                MaybeRef::NotRef(_) => None,
            })
            .collect()
    };
    Ok(match dict.get_raw::<Object<'_>>(b"Contents") {
        Some(MaybeRef::Ref(r)) => match xref.get::<Object<'_>>(r.into()) {
            Some(Object::Array(a)) => refs(&a),
            _ => vec![object_id(r)],
        },
        Some(MaybeRef::NotRef(Object::Array(a))) => refs(&a),
        _ => vec![],
    })
}

impl Targets {
    pub fn new(doc: &Document, plan: &RedactionPlan) -> Result<Self, PdfError> {
        let mut next = CLONE_BASE;
        let mut fresh = || {
            next += 1;
            ObjectId {
                num: next - 1,
                generation: CLONE_GENERATION,
            }
        };
        let mut forms = HashMap::new();
        // Page-level streams an edit or a `Do` redirect lands in.
        let mut wanted: BTreeSet<(u32, ObjectId)> = BTreeSet::new();
        for page in &plan.pages {
            let p = page.page_index;
            let items = page
                .clusters
                .iter()
                .map(|c| (&c.id.path, c.id.loc.stream))
                .chain(page.images.iter().map(|i| (&i.path, i.loc.stream)))
                .chain(page.vectors.iter().map(|v| (&v.path, v.loc.stream)));
            wanted.extend(
                items
                    .filter(|(path, _)| path.0.is_empty())
                    .map(|(_, s)| (p, s)),
            );
            for f in page.forms.iter().filter(|f| f.shared) {
                forms.insert((p, f.path.0.clone()), fresh());
                if f.path.0.len() == 1 {
                    wanted.insert((p, f.do_loc.stream));
                }
            }
        }
        let mut content = BTreeMap::new();
        if !wanted.is_empty() {
            let mut users: HashMap<ObjectId, u32> = HashMap::new();
            let streams: BTreeSet<ObjectId> = wanted.iter().map(|w| w.1).collect();
            for p in 0..doc.page_count() {
                let ids = content_ids(doc, p)?;
                for s in ids.into_iter().collect::<BTreeSet<_>>() {
                    if streams.contains(&s) {
                        *users.entry(s).or_default() += 1;
                    }
                }
            }
            for (p, s) in wanted {
                if users.get(&s).is_some_and(|n| *n > 1) {
                    content.insert((p, s), fresh());
                }
            }
        }
        Ok(Self { forms, content })
    }

    /// Clones of Form occurrences that [`Targets::apply`] makes.
    pub fn forms_cloned(&self) -> u32 {
        self.forms.len() as u32
    }

    /// The stream that an edit of `stream`, drawn on `page` through `path`, must go to.
    pub fn stream(&self, page: u32, path: &[InstanceStep], stream: ObjectId) -> ObjectId {
        let found = match path.last() {
            None => self.content.get(&(page, stream)),
            Some(_) => self.forms.get(&(page, path.to_vec())),
        };
        found.copied().unwrap_or(stream)
    }

    /// A resource entry can be added to the Form `path` ends with only if it has its own.
    pub fn check_resources(
        &self,
        doc: &Document,
        page: u32,
        path: &[InstanceStep],
    ) -> Result<(), PdfError> {
        let Some(last) = path.last() else {
            return Ok(());
        };
        let own = doc
            .reader()
            .pdf()
            .xref()
            .get::<Stream<'_>>(at(last.form))
            .is_some_and(|s| s.dict().contains_key("Resources"));
        if own {
            Ok(())
        } else {
            Err(refuse("redactionFormInheritedResources", page))
        }
    }

    /// Create the clones in `w` and point each cloned occurrence's parent at them. Safe to
    /// call once per writer by each removal: a second call finds the clones there and returns.
    pub fn apply(
        &self,
        doc: &Document,
        plan: &RedactionPlan,
        w: &mut Writer<'_>,
    ) -> Result<(), PdfError> {
        let Some(first) = self.forms.values().chain(self.content.values()).next() else {
            return Ok(());
        };
        if w.is_clone(*first) {
            return Ok(());
        }
        let mut streams: HashMap<ObjectId, Vec<u8>> = HashMap::new();
        let mut source = |id: ObjectId| -> Result<Vec<u8>, PdfError> {
            Ok(match streams.entry(id) {
                Entry::Occupied(e) => e.into_mut(),
                Entry::Vacant(e) => e.insert(decode(doc, id)?),
            }
            .clone())
        };
        // Check and plan every redirect before the writer changes.
        let mut redirects = Vec::new();
        for page in &plan.pages {
            for f in page.forms.iter().filter(|f| f.shared) {
                let steps = &f.path.0;
                let parent = &steps[..steps.len() - 1];
                self.check_resources(doc, page.page_index, parent)?;
                let data = source(f.do_loc.stream)?;
                let name = do_op(&data, f.do_loc)
                    .ok_or_else(|| refuse("redactionFormNotDrawnByDo", page.page_index))?
                    .1;
                redirects.push((
                    page.page_index,
                    parent.to_vec(),
                    f.do_loc,
                    name,
                    self.forms[&(page.page_index, steps.clone())],
                ));
            }
        }
        for ((page, stream), clone) in &self.content {
            w.alias(*clone, *stream)?;
            w.redirect_page_content(*page as usize, *stream, *clone)?;
        }
        for ((_, steps), clone) in &self.forms {
            let form = steps.last().expect("a Form chain is not empty").form;
            w.alias(*clone, form)?;
        }
        // Old name -> uses left once the redirected `Do`s are gone, per resource owner.
        let mut gone: HashMap<(u32, Vec<InstanceStep>), BTreeSet<String>> = HashMap::new();
        let mut redirected: Vec<(u32, Vec<InstanceStep>, SourceLoc)> = Vec::new();
        for (n, (page, parent, loc, name, clone)) in redirects.iter().enumerate() {
            let target = self.stream(*page, parent, loc.stream);
            let new = format!("FlkFm{n}");
            let range = loc.start as usize..loc.end as usize;
            w.replace_span(target, range, format!("/{new} Do").as_bytes());
            match parent.last() {
                None => {
                    w.add_page_xobject(*page as usize, &new, Val::Src(*clone))?;
                }
                Some(_) => {
                    w.add_form_xobject(target, &new, Val::Src(*clone));
                }
            }
            gone.entry((*page, parent.clone()))
                .or_default()
                .insert(name.clone());
            redirected.push((*page, parent.clone(), *loc));
        }
        // An original no remaining `Do` names must not stay reachable from the resources.
        for ((page, parent), names) in gone {
            let streams: Vec<ObjectId> = match parent.last() {
                None => content_ids(doc, page as usize)?,
                Some(s) => vec![s.form],
            };
            let mut left: BTreeSet<String> = BTreeSet::new();
            for id in streams {
                let data = source(id)?;
                for (range, name) in do_ops(&data) {
                    let loc_gone = redirected.iter().any(|(p, par, l)| {
                        *p == page
                            && *par == parent
                            && l.stream == id
                            && (l.start as usize) < range.end
                            && range.start < l.end as usize
                    });
                    if !loc_gone {
                        left.insert(name);
                    }
                }
            }
            for name in names.difference(&left) {
                match parent.last() {
                    None => {
                        w.drop_page_xobject(page as usize, name)?;
                    }
                    Some(last) => {
                        w.drop_form_xobject(self.stream(page, &parent, last.form), name);
                    }
                }
            }
        }
        Ok(())
    }
}

/// Every `Do` of a decoded content stream: its byte range and its name operand.
fn do_ops(data: &[u8]) -> Vec<(Range<usize>, String)> {
    let mut out = Vec::new();
    let mut it = UntypedIter::new(data);
    while let Some((ins, span)) = it.next_with_span() {
        if &**ins.operator != b"Do" {
            continue;
        }
        let name = ins.operands().find_map(|o| match o {
            Object::Name(n) => Some(n.as_str().to_owned()),
            _ => None,
        });
        if let Some(name) = name {
            out.push((span, name));
        }
    }
    out
}

/// The `Do` operator at `loc` and its name, if that is what the byte range holds.
fn do_op(data: &[u8], loc: SourceLoc) -> Option<(Range<usize>, String)> {
    let want = loc.start as usize..loc.end as usize;
    do_ops(data)
        .into_iter()
        .find(|(r, _)| r.start < want.end && want.start < r.end)
}
