//! Size pass over a finished non-preserve graph (NEW-83): a page's content streams become one
//! stream (the spec concatenates them with whitespace between), objects with the same value and
//! bytes become one, and the ids close up again. Pages stay apart (the page tree lists each).
//! The output has no catalog entry that names an object (no optional-content or field list), so
//! nothing else depends on an object being distinct.
use super::graph::{Node, deflate};
use super::{Val, err};
use foliark_model::{ErrorCode, PdfError};
use pdf_writer::Ref;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Read;

fn map(v: &mut Val, to: &impl Fn(i32) -> i32) {
    match v {
        Val::Ref(r) => *r = Ref::new(to(r.get())),
        Val::Arr(items) => items.iter_mut().for_each(|x| map(x, to)),
        Val::Dict(entries) => entries.iter_mut().for_each(|(_, x)| map(x, to)),
        _ => {}
    }
}

/// Follows `merged` (duplicate -> kept) to its end.
fn kept(merged: &HashMap<i32, i32>) -> impl Fn(i32) -> i32 + '_ {
    |mut id| {
        while let Some(&next) = merged.get(&id) {
            id = next;
        }
        id
    }
}

fn flate_only(v: &Val) -> bool {
    matches!(v, Val::Dict(e) if matches!(e.as_slice(),
        [(k, Val::Name(n))] if k == b"Filter" && n == b"FlateDecode"))
}

fn inflate(data: &[u8]) -> Result<Vec<u8>, PdfError> {
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(data)
        .read_to_end(&mut out)
        .map_err(|_| err(ErrorCode::Internal, "own flate stream does not inflate"))?;
    Ok(out)
}

/// `nodes` sorted by id, ids from 3 on; `pages` their page objects.
pub(crate) fn shrink(
    nodes: &mut Vec<Node>,
    pages: &mut [Ref],
    dedupe_objects: bool,
) -> Result<(), PdfError> {
    merge_contents(nodes, pages)?;
    if dedupe_objects {
        dedupe(nodes, pages);
    }
    nodes.sort_by_key(|n| n.id.get());
    let to: HashMap<i32, i32> = nodes
        .iter()
        .zip(3..)
        .map(|(n, new)| (n.id.get(), new))
        .collect();
    for n in nodes.iter_mut() {
        map(&mut n.val, &|id| to.get(&id).copied().unwrap_or(id));
        n.id = Ref::new(to[&n.id.get()]);
    }
    for p in pages.iter_mut() {
        *p = Ref::new(to[&p.get()]);
    }
    Ok(())
}

fn merge_contents(nodes: &mut Vec<Node>, pages: &[Ref]) -> Result<(), PdfError> {
    let at: HashMap<i32, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.get(), i))
        .collect();
    let mut next = nodes.iter().map(|n| n.id.get()).max().unwrap_or(2) + 1;
    let mut merged = Vec::new();
    let mut parts_seen = HashSet::new();
    for p in pages {
        let Val::Dict(e) = &nodes[at[&p.get()]].val else {
            continue;
        };
        let Some((_, Val::Arr(parts))) = e.iter().find(|(k, _)| k == b"Contents") else {
            continue;
        };
        let ids: Vec<i32> = parts
            .iter()
            .filter_map(|v| {
                if let Val::Ref(r) = v {
                    Some(r.get())
                } else {
                    None
                }
            })
            .collect();
        let streams = ids.iter().all(|id| {
            at.get(id)
                .is_some_and(|&i| nodes[i].data.is_some() && flate_only(&nodes[i].val))
        });
        if ids.len() < 2 || ids.len() != parts.len() || !streams {
            continue;
        }
        let mut plain: Vec<u8> = Vec::new();
        for id in &ids {
            if plain.last().is_some_and(|b| !b.is_ascii_whitespace()) {
                plain.push(b'\n');
            }
            plain.extend(inflate(nodes[at[id]].data.as_deref().unwrap_or_default())?);
        }
        parts_seen.extend(ids);
        merged.push((
            p.get(),
            Node {
                id: Ref::new(next),
                val: Val::Dict(vec![(
                    b"Filter".to_vec(),
                    Val::Name(b"FlateDecode".to_vec()),
                )]),
                data: Some(deflate(&plain)),
            },
        ));
        next += 1;
    }
    for (page, node) in merged {
        let Val::Dict(e) = &mut nodes[at[&page]].val else {
            continue;
        };
        if let Some((_, v)) = e.iter_mut().find(|(k, _)| k == b"Contents") {
            *v = Val::Ref(node.id);
        }
        nodes.push(node);
    }
    // Old parts that no page still names (a shared stream may be kept by a page not merged).
    let mut used = HashSet::new();
    for n in nodes.iter() {
        if let Val::Dict(e) = &n.val {
            for (k, v) in e {
                if k == b"Contents" {
                    match v {
                        Val::Ref(r) => _ = used.insert(r.get()),
                        Val::Arr(a) => used.extend(a.iter().filter_map(|x| match x {
                            Val::Ref(r) => Some(r.get()),
                            _ => None,
                        })),
                        _ => {}
                    }
                }
            }
        }
    }
    nodes.retain(|n| !parts_seen.contains(&n.id.get()) || used.contains(&n.id.get()));
    Ok(())
}

fn is_link(v: &Val) -> bool {
    matches!(v, Val::Dict(e) if e.iter().any(|(k, x)| k == b"Subtype" && *x == Val::Name(b"Link".to_vec())))
}

fn dedupe(nodes: &mut Vec<Node>, pages: &[Ref]) {
    let pages: HashSet<i32> = pages.iter().map(|p| p.get()).collect();
    let mut to: HashMap<i32, i32> = HashMap::new();
    loop {
        let mut seen: HashMap<(String, u64), Vec<usize>> = HashMap::new();
        let mut dups = HashSet::new();
        for (i, n) in nodes.iter().enumerate() {
            // A page's own link annotation (ADR 0014) is never shared with another page.
            if pages.contains(&n.id.get()) || is_link(&n.val) {
                continue;
            }
            let mut v = n.val.clone();
            map(&mut v, &kept(&to));
            let mut h = DefaultHasher::new();
            n.data.hash(&mut h);
            let cands = seen.entry((format!("{v:?}"), h.finish())).or_default();
            if let Some(&c) = cands.iter().find(|&&c| nodes[c].data == n.data) {
                to.insert(n.id.get(), nodes[c].id.get());
                dups.insert(i);
            } else {
                cands.push(i);
            }
        }
        if dups.is_empty() {
            break;
        }
        let mut keep = (0..nodes.len()).map(|i| !dups.contains(&i));
        nodes.retain(|_| keep.next().unwrap_or(true));
    }
    for n in nodes.iter_mut() {
        map(&mut n.val, &kept(&to));
    }
}
