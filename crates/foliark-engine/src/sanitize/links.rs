//! Hyperlinks kept by `policy.links: 'keep'` (ADR 0014, native export only). A link survives only
//! when it is a plain jump the output can honour and cannot reveal what a redaction removed:
//!
//! 1. its action is a `URI` (`http:`, `https:`, `mailto:`; printable ASCII, 2048 bytes at most) or a
//!    `GoTo` / `/Dest` that resolves, through the catalog `/Dests` or the `/Names /Dests` tree, to an
//!    explicit destination on a page of the output (named destinations are not written);
//! 2. no `/Next`, no `/AA`, no `/OC`, not hidden: nothing the fresh dictionary cannot carry;
//! 3. neither its `/Rect` nor any `/QuadPoints` quad meets a redaction region (display space,
//!    the overlap test of the redaction planner);
//! 4. a URI, percent-decoded, NFC and lowercased, holds no token of the redacted text.
//!
//! Anything else is dropped and counted by the caller as `LINKS_REMOVED`. The survivors are
//! written fresh by the writer from an allowlist; no input key is copied.
use crate::Document;
use crate::reader::{self, object_id};
use crate::redaction::plan::{bounds_of, overlap, text_coverage};
use crate::writer::{Link, LinkTarget};
use foliark_model::{ExtractOptions, ObjectId, PdfError, Point, Rect, TextEdit};
use hayro::hayro_syntax::object::{Array, Dict, MaybeRef, Name, Object, String as PdfString};
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use unicode_normalization::UnicodeNormalization;

const HIDDEN: u32 = 2;
const NO_VIEW: u32 = 32;
const MAX_URI: usize = 2048;
/// Name tree nodes one lookup may visit (a damaged tree cannot make it loop or crawl).
const TREE_NODES: usize = 4096;
/// Smallest token of redacted text that counts (shorter ones match everything).
const MIN_TOKEN: usize = 3;
/// View name and the number of arguments it takes (PDF 12.3.2.2).
const VIEWS: [(&str, usize); 8] = [
    ("XYZ", 3),
    ("Fit", 0),
    ("FitH", 1),
    ("FitV", 1),
    ("FitR", 4),
    ("FitB", 0),
    ("FitBH", 1),
    ("FitBV", 1),
];

type Pages = HashMap<ObjectId, usize>;

fn finite(a: Option<Array<'_>>) -> Option<Vec<f64>> {
    let a = a?;
    let v: Vec<f64> = a.iter::<f64>().collect();
    // `iter` stops at the first non-number: a count mismatch with the raw length is malformed.
    (v.len() == a.raw_iter().count() && v.iter().all(|x| x.is_finite())).then_some(v)
}

fn uri_ok(b: &[u8]) -> bool {
    let lower = b.to_ascii_lowercase();
    b.len() <= MAX_URI
        && b.iter().all(|c| (0x21..=0x7e).contains(c))
        && [&b"http:"[..], b"https:", b"mailto:"]
            .iter()
            .any(|s| lower.starts_with(s))
}

/// The explicit destination `arr` names: a page of the output, a view and its arguments.
fn explicit(arr: &Array<'_>, pages: &Pages) -> Option<LinkTarget> {
    let mut it = arr.raw_iter();
    let MaybeRef::Ref(r) = it.next()? else {
        return None;
    };
    let page = *pages.get(&object_id(r))?;
    let MaybeRef::NotRef(Object::Name(n)) = it.next()? else {
        return None;
    };
    let (view, want) = VIEWS.iter().find(|(v, _)| *v == n.as_str())?;
    let mut args = Vec::new();
    for a in it {
        args.push(match a {
            MaybeRef::NotRef(Object::Null(_)) => None,
            MaybeRef::NotRef(Object::Number(n)) if n.as_f64().is_finite() => Some(n.as_f64()),
            _ => return None,
        });
    }
    (args.len() == *want).then_some(LinkTarget::Dest { page, view, args })
}

/// A destination value: the array itself, or a dictionary holding it as `/D`.
fn dest_value(o: Object<'_>, pages: &Pages) -> Option<LinkTarget> {
    match o {
        Object::Array(a) => explicit(&a, pages),
        Object::Dict(d) => explicit(&d.get::<Array<'_>>("D")?, pages),
        _ => None,
    }
}

/// `key` in the name tree rooted at `node`, with `/Limits` used to skip subtrees.
fn tree_find<'a>(node: &Dict<'a>, key: &[u8], left: &mut usize) -> Option<Object<'a>> {
    *left = left.checked_sub(1)?;
    if let Some(lim) = node.get::<Array<'_>>("Limits") {
        let mut l = lim.iter::<PdfString<'_>>();
        if let (Some(lo), Some(hi)) = (l.next(), l.next())
            && (key < lo.as_bytes() || key > hi.as_bytes())
        {
            return None;
        }
    }
    if let Some(names) = node.get::<Array<'a>>("Names") {
        let mut it = names.flex_iter();
        while let Some(k) = it.next::<PdfString<'a>>() {
            let v = it.next::<Object<'a>>()?;
            if k.as_bytes() == key {
                return Some(v);
            }
        }
    }
    let kids = node.get::<Array<'a>>("Kids")?;
    kids.iter::<Dict<'a>>()
        .find_map(|k| tree_find(&k, key, left))
}

/// The explicit destination a named destination resolves to.
fn named(doc: &reader::Document, key: &[u8], pages: &Pages) -> Option<LinkTarget> {
    let root = doc.pdf().xref().root_id();
    let cat = doc.pdf().xref().get::<Dict<'_>>(root)?;
    if let Some(v) = cat
        .get::<Dict<'_>>("Dests")
        .and_then(|d| d.get::<Object<'_>>(key))
    {
        return dest_value(v, pages);
    }
    let tree = cat.get::<Dict<'_>>("Names")?.get::<Dict<'_>>("Dests")?;
    let mut left = TREE_NODES;
    dest_value(tree_find(&tree, key, &mut left)?, pages)
}

/// The destination of a `GoTo` action (`/D`) or a link's own `/Dest`.
fn goto(doc: &reader::Document, o: Object<'_>, pages: &Pages) -> Option<LinkTarget> {
    match o {
        Object::Array(a) => explicit(&a, pages),
        Object::Name(n) => named(doc, &n, pages),
        Object::String(s) => named(doc, s.as_bytes(), pages),
        _ => None,
    }
}

fn target(doc: &reader::Document, link: &Dict<'_>, pages: &Pages) -> Option<LinkTarget> {
    if link.contains_key("A") {
        let a = link.get::<Dict<'_>>("A")?;
        if a.contains_key("Next") {
            return None;
        }
        return match a.get::<Name<'_>>("S")?.as_str() {
            "URI" => {
                let u = a.get::<PdfString<'_>>("URI")?;
                uri_ok(u.as_bytes()).then(|| LinkTarget::Uri(u.as_bytes().to_vec()))
            }
            "GoTo" => goto(doc, a.get::<Object<'_>>("D")?, pages),
            _ => None,
        };
    }
    goto(doc, link.get::<Object<'_>>("Dest")?, pages)
}

/// Every link of `doc` that rule 1 and 2 allow, in page order. Rects are user space.
fn candidates(doc: &reader::Document) -> Vec<Link> {
    let pages: Pages = doc
        .pdf()
        .pages()
        .iter()
        .enumerate()
        .filter_map(|(i, p)| Some((object_id(p.raw().obj_id()?.into()), i)))
        .collect();
    let mut out = Vec::new();
    for (i, page) in doc.pdf().pages().iter().enumerate() {
        let Some(list) = page.raw().get::<Array<'_>>("Annots") else {
            continue;
        };
        for a in list.iter::<Dict<'_>>() {
            if a.get::<Name<'_>>("Subtype")
                .is_none_or(|n| n.as_str() != "Link")
                || a.get::<u32>("F").unwrap_or(0) & (HIDDEN | NO_VIEW) != 0
                || a.contains_key("AA")
                || a.contains_key("OC")
            {
                continue;
            }
            let Some(rect) =
                finite(a.get::<Array<'_>>("Rect")).and_then(|v| <[f64; 4]>::try_from(v).ok())
            else {
                continue;
            };
            let quad_points = match a.contains_key("QuadPoints") {
                false => Vec::new(),
                true => match finite(a.get::<Array<'_>>("QuadPoints")) {
                    Some(q) if !q.is_empty() && q.len() % 8 == 0 => q,
                    _ => continue,
                },
            };
            let Some(target) = target(doc, &a, &pages) else {
                continue;
            };
            let highlight = a.get::<Name<'_>>("H").and_then(|n| match &*n {
                [h @ (b'N' | b'I' | b'O' | b'P')] => Some(*h),
                _ => None,
            });
            out.push(Link {
                page: i,
                rect,
                quad_points,
                highlight,
                target,
            });
        }
    }
    out
}

fn normal(s: &str) -> String {
    s.nfc().collect::<String>().to_lowercase()
}

/// Tokens of redacted text: each whitespace-separated piece of at least [`MIN_TOKEN`] characters,
/// and the whole text without its whitespace.
fn tokens_of(text: &str, out: &mut BTreeSet<String>) {
    let squashed: String = text.split_whitespace().collect();
    for t in text.split_whitespace().chain(Some(squashed.as_str())) {
        let t = normal(t);
        if t.chars().count() >= MIN_TOKEN {
            out.insert(t);
        }
    }
}

/// The text each region of `regions` (display space, by page) removes, as tokens: the clusters
/// the redaction planner's own coverage test takes, read from `doc`, the document the plan was
/// made on.
fn removed_tokens(
    doc: &Document,
    regions: &BTreeMap<usize, Vec<Rect>>,
) -> Result<BTreeSet<String>, PdfError> {
    let opts = ExtractOptions {
        chars: true,
        include_ocg_hidden: true,
    };
    let mut out = BTreeSet::new();
    for (&page, rects) in regions {
        let text = doc.extract_text(page, &opts)?;
        let units: Vec<u16> = text.text.encode_utf16().collect();
        for r in rects {
            let b = bounds_of(r);
            let mut piece = String::new();
            let mut end = None;
            for c in text.clusters.iter().flatten() {
                if text_coverage(&c.quad, b).is_none() {
                    continue;
                }
                let (s, e) = (c.start as usize, (c.end as usize).min(units.len()));
                if s >= e {
                    continue;
                }
                if end.is_some_and(|p| s > p) {
                    piece.push(' ');
                }
                piece.push_str(&String::from_utf16_lossy(&units[s..e]));
                end = Some(e);
            }
            tokens_of(&piece, &mut out);
        }
    }
    Ok(out)
}

/// `%XX` decoded, repeatedly (a doubly encoded token is still the token).
fn decoded(uri: &[u8]) -> String {
    let mut cur = uri.to_vec();
    for _ in 0..3 {
        let mut next = Vec::with_capacity(cur.len());
        let mut i = 0;
        while i < cur.len() {
            let hex = cur
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
            match (cur[i], hex) {
                (b'%', Some(b)) => {
                    next.push(b);
                    i += 3;
                }
                (c, _) => {
                    next.push(c);
                    i += 1;
                }
            }
        }
        if next == cur {
            break;
        }
        cur = next;
    }
    normal(&String::from_utf8_lossy(&cur))
}

fn display_bounds(pts: impl Iterator<Item = Point>) -> [f64; 4] {
    pts.fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, p| {
        [b[0].min(p.x), b[1].min(p.y), b[2].max(p.x), b[3].max(p.y)]
    })
}

/// Where each edited paragraph was and now is, per page (display space): its old rectangles and
/// the clusters of the new text found in `doc`. A new text that cannot be found there (it
/// wrapped or joined differently) blocks the whole page: over-dropping is accepted.
fn edited_areas(
    doc: &Document,
    edits: &[TextEdit],
) -> Result<BTreeMap<usize, Vec<Rect>>, PdfError> {
    let opts = ExtractOptions {
        chars: true,
        include_ocg_hidden: true,
    };
    let mut out: BTreeMap<usize, Vec<Rect>> = BTreeMap::new();
    for e in edits {
        let page = e.page_index as usize;
        let areas = out.entry(page).or_default();
        areas.extend(e.rects.iter().copied());
        let text = doc.extract_text(page, &opts)?;
        let found = text
            .text
            .find(&e.text)
            .filter(|_| !e.text.is_empty())
            .map(|at| {
                let start = text.text[..at].encode_utf16().count() as u32;
                (start, start + e.text.encode_utf16().count() as u32)
            });
        let Some((start, end)) = found else {
            areas.push(Rect {
                x: -1e9,
                y: -1e9,
                width: 2e9,
                height: 2e9,
            });
            continue;
        };
        for c in text.clusters.iter().flatten() {
            if c.start >= start && c.end <= end {
                areas.push(c.quad.bounds());
            }
        }
    }
    Ok(out)
}

/// The links of `source` that `policy.links: 'keep'` writes back. `doc` is the working copy the
/// redactions were planned on; `regions` are the display-space rectangles they remove, per page
/// (requested and effective); `edits` are the committed paragraph edits, whose old and new area
/// no link may overlap either (a link to an address the user edited away must not survive).
pub fn keep(
    source: &reader::Document,
    doc: &Document,
    regions: &BTreeMap<usize, Vec<Rect>>,
    edits: &[TextEdit],
) -> Result<Vec<Link>, PdfError> {
    let mut links = candidates(source);
    if links.is_empty() {
        return Ok(links);
    }
    let mut geometry = BTreeMap::new();
    let mut kept = Vec::with_capacity(links.len());
    let tokens = if links.iter().any(|l| matches!(l.target, LinkTarget::Uri(_))) {
        removed_tokens(doc, regions)?
    } else {
        BTreeSet::new()
    };
    let mut blocked = regions.clone();
    for (page, areas) in edited_areas(doc, edits)? {
        blocked.entry(page).or_default().extend(areas);
    }
    for l in links.drain(..) {
        let Some(rects) = blocked.get(&l.page) else {
            if leaks(&l, &tokens) {
                continue;
            }
            kept.push(l);
            continue;
        };
        let g = match geometry.entry(l.page) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => e.insert(source.page_geometry(l.page)?),
        };
        let to_display = |x, y| g.user_to_display.apply(Point { x, y });
        let boxed =
            |p: &[f64]| display_bounds(p.as_chunks::<2>().0.iter().map(|c| to_display(c[0], c[1])));
        let [x0, y0, x1, y1] = l.rect;
        let mut shapes = vec![boxed(&[x0, y0, x1, y0, x1, y1, x0, y1])];
        shapes.extend(l.quad_points.as_chunks::<8>().0.iter().map(|q| boxed(q)));
        let hit = rects
            .iter()
            .any(|r| shapes.iter().any(|s| overlap(*s, bounds_of(r))));
        if !hit && !leaks(&l, &tokens) {
            kept.push(l);
        }
    }
    Ok(kept)
}

fn leaks(l: &Link, tokens: &BTreeSet<String>) -> bool {
    match &l.target {
        LinkTarget::Uri(u) => {
            let u = decoded(u);
            tokens.iter().any(|t| u.contains(t.as_str()))
        }
        LinkTarget::Dest { .. } => false,
    }
}
