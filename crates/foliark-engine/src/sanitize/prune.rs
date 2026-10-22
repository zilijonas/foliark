//! Native export keeps only the resources surviving content uses (spec §8.1, ADR 0007; NEW-80).
//! Run on the written output, after every substitution: the output is read back, the names its
//! content streams actually use are collected, and every other `/Resources` entry is dropped by
//! a second write, which also leaves whatever only those entries reached unreachable.
//!
//! Owners of `/Resources`: pages, and Forms, tiling patterns and Type3 fonts that have their own.
//! One without its own inherits the resources of the context that draws it, so what it uses is
//! counted there. Reached from the page: `Do` (Forms), `Tf` (Type3 glyph procedures), `scn`
//! (tiling cells), `gs` (soft-mask groups). Names are collected from `Do Tf gs sh cs CS scn SCN
//! BDC DP` and inline image colour spaces. An owner this walk cannot analyze is left whole.
use crate::limits::Budget;
use crate::reader::{self, decode_content, object_id};
use crate::writer::{Owner, Writer};
use foliark_model::{ObjectId, PdfError};
use hayro::hayro_syntax::content::UntypedIter;
use hayro::hayro_syntax::object::{Dict, MaybeRef, Name, Object, ObjectIdentifier as Oid, Stream};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ops::Deref;

const CATEGORIES: [&str; 7] = [
    "Font",
    "XObject",
    "ExtGState",
    "ColorSpace",
    "Pattern",
    "Shading",
    "Properties",
];

type Used = BTreeSet<(&'static str, Vec<u8>)>;

/// Where names are looked up and counted: the owner of the resources in force (`None`: a direct
/// object that cannot be told apart in the writer, never pruned).
#[derive(Clone)]
struct Frame<'a> {
    owner: Option<Owner>,
    res: Option<Dict<'a>>,
}

struct Walk<'a> {
    doc: &'a reader::Document,
    budget: Budget,
    owners: BTreeMap<Owner, (Dict<'a>, Used)>,
    visited: HashSet<(ObjectId, Option<Owner>)>,
}

fn name_of(o: &Object<'_>) -> Option<Vec<u8>> {
    match o {
        Object::Name(n) => Some(n.deref().to_vec()),
        _ => None,
    }
}

impl<'a> Walk<'a> {
    fn mark(&mut self, f: &Frame<'a>, category: &'static str, name: Vec<u8>) {
        if let (Some(o), Some(res)) = (f.owner, &f.res) {
            self.owners
                .entry(o)
                .or_insert_with(|| (res.clone(), Used::new()))
                .1
                .insert((category, name));
        }
    }

    /// The object `/name` of the frame's `category`: its id (if indirect) and value.
    fn lookup(
        &self,
        f: &Frame<'a>,
        category: &str,
        name: &[u8],
    ) -> Option<(Option<ObjectId>, Object<'a>)> {
        let map = f.res.as_ref()?.get::<Dict<'a>>(category)?;
        match map.get_raw::<Object<'a>>(name)? {
            MaybeRef::Ref(r) => {
                let o = self.doc.pdf().xref().get::<Object<'a>>(Oid::from(r))?;
                Some((Some(object_id(r)), o))
            }
            MaybeRef::NotRef(o) => Some((None, o)),
        }
    }

    /// A content stream with its own `/Resources` (or none) drawn from `caller`.
    fn stream(
        &mut self,
        id: Option<ObjectId>,
        s: &Stream<'a>,
        caller: &Frame<'a>,
        depth: u32,
    ) -> Result<(), PdfError> {
        let f = self.frame(id, s.dict(), caller);
        if let Some(id) = id
            && !self.visited.insert((id, f.owner))
        {
            return Ok(());
        }
        let data = decode_content(s, self.doc.limits(), &mut self.budget)?.data;
        self.scan(&data, &f, depth)
    }

    /// The frame of an object whose dictionary is `d`: its own resources, else the caller's.
    fn frame(&mut self, id: Option<ObjectId>, d: &Dict<'a>, caller: &Frame<'a>) -> Frame<'a> {
        match d.get::<Dict<'a>>("Resources") {
            Some(res) => {
                let owner = id.map(Owner::Object);
                if let Some(o) = owner {
                    self.owners
                        .entry(o)
                        .or_insert_with(|| (res.clone(), Used::new()));
                }
                Frame {
                    owner,
                    res: Some(res),
                }
            }
            None => caller.clone(),
        }
    }

    fn scan(&mut self, data: &[u8], f: &Frame<'a>, depth: u32) -> Result<(), PdfError> {
        let max = self.doc.limits().max_nesting;
        if depth > max {
            return Err(PdfError::limit(
                "maxNesting",
                u64::from(depth),
                u64::from(max),
            ));
        }
        let mut it = UntypedIter::new(data);
        while let Some(ins) = it.next() {
            self.budget.charge(1)?;
            let ops: Vec<&Object<'_>> = ins.operands().collect();
            let first = ops.first().and_then(|o| name_of(o));
            let last = ops.last().and_then(|o| name_of(o));
            let op: &[u8] = ins.operator;
            match (op, first, last) {
                (b"Do", Some(n), _) => {
                    self.mark(f, "XObject", n.clone());
                    if let Some((id, Object::Stream(s))) = self.lookup(f, "XObject", &n)
                        && s.dict()
                            .get::<Name<'_>>("Subtype")
                            .is_some_and(|t| &*t == b"Form")
                    {
                        self.stream(id, &s, f, depth + 1)?;
                    }
                }
                (b"Tf", Some(n), _) => {
                    self.mark(f, "Font", n.clone());
                    if let Some((id, Object::Dict(font))) = self.lookup(f, "Font", &n)
                        && let Some(procs) = font.get::<Dict<'_>>("CharProcs")
                        && font
                            .get::<Name<'_>>("Subtype")
                            .is_some_and(|t| &*t == b"Type3")
                    {
                        let ff = self.frame(id, &font, f);
                        for k in procs.keys() {
                            if let Some(Object::Stream(p)) = procs.get::<Object<'_>>(k.deref()) {
                                let pid = p.obj_id();
                                self.stream(Some(object_id(pid.into())), &p, &ff, depth + 1)?;
                            }
                        }
                    }
                }
                (b"gs", Some(n), _) => {
                    self.mark(f, "ExtGState", n.clone());
                    if let Some((_, Object::Dict(g))) = self.lookup(f, "ExtGState", &n)
                        && let Some(mask) = g.get::<Dict<'_>>("SMask")
                        && let Some(MaybeRef::Ref(r)) = mask.get_raw::<Object<'_>>("G")
                        && let Some(Object::Stream(s)) =
                            self.doc.pdf().xref().get::<Object<'a>>(Oid::from(r))
                    {
                        self.stream(Some(object_id(r)), &s, f, depth + 1)?;
                    }
                }
                (b"sh", Some(n), _) => self.mark(f, "Shading", n),
                (b"cs" | b"CS", Some(n), _) => self.mark(f, "ColorSpace", n),
                (b"scn" | b"SCN", _, Some(n)) => {
                    self.mark(f, "Pattern", n.clone());
                    if let Some((id, Object::Stream(s))) = self.lookup(f, "Pattern", &n)
                        && s.dict().get::<i32>("PatternType") == Some(1)
                    {
                        self.stream(id, &s, f, depth + 1)?;
                    }
                }
                (b"BDC" | b"DP", _, _) => {
                    if let Some(n) = ops.get(1).and_then(|o| name_of(o)) {
                        self.mark(f, "Properties", n);
                    }
                }
                (b"BI", _, _) => {
                    if let Some(Object::Stream(s)) = ops.first().copied() {
                        for k in ["CS", "ColorSpace"] {
                            match s.dict().get::<Object<'_>>(k) {
                                Some(Object::Array(a)) => {
                                    for o in a.iter::<Object<'_>>() {
                                        if let Some(n) = name_of(&o) {
                                            self.mark(f, "ColorSpace", n);
                                        }
                                    }
                                }
                                Some(o) => {
                                    if let Some(n) = name_of(&o) {
                                        self.mark(f, "ColorSpace", n);
                                    }
                                }
                                None => {}
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// Queue on `w` (the writer of `doc`, the output just written) the removal of every resource
/// entry no surviving content uses; returns how many entries that is.
pub fn unused(doc: &reader::Document, w: &mut Writer<'_>) -> Result<u32, PdfError> {
    let mut walk = Walk {
        doc,
        budget: Budget::new(doc.limits()),
        owners: BTreeMap::new(),
        visited: HashSet::new(),
    };
    for i in 0..doc.page_count() {
        let res = doc.page(i)?.raw().get::<Dict<'_>>("Resources");
        let frame = Frame {
            owner: Some(Owner::Page(i)),
            res,
        };
        if let Some(r) = &frame.res {
            walk.owners
                .entry(Owner::Page(i))
                .or_insert_with(|| (r.clone(), Used::new()));
        }
        for c in doc.page_contents(i)? {
            walk.scan(&c.data, &frame, 0)?;
        }
    }
    let mut dropped = 0;
    for (owner, (res, used)) in &walk.owners {
        for category in CATEGORIES {
            let Some(map) = res.get::<Dict<'_>>(category) else {
                continue;
            };
            let live =
                |n: &[u8]| used.contains(&(category, n.to_vec())) || keeps(res, category, n, used);
            for k in map.keys() {
                if !live(k.deref()) {
                    w.prune_resource(*owner, category, k.deref());
                    dropped += 1;
                }
            }
        }
    }
    Ok(dropped)
}

/// A colour space that a used colour space names as its base (`[/Indexed /Cs1 ..]`).
fn keeps(res: &Dict<'_>, category: &str, name: &[u8], used: &Used) -> bool {
    category == "ColorSpace"
        && used
            .iter()
            .filter(|(c, _)| *c == "ColorSpace")
            .any(|(_, u)| {
                res.get::<Dict<'_>>("ColorSpace")
                    .and_then(|m| m.get::<hayro::hayro_syntax::object::Array<'_>>(u.as_slice()))
                    .is_some_and(|a| {
                        a.iter::<Object<'_>>()
                            .any(|o| name_of(&o).is_some_and(|n| n == name))
                    })
            })
}
