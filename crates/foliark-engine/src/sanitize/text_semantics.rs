//! Text semantics at export (spec §5 rules 1 and 6, §8.2; NEW-44).
//!
//! - Invisible OCR text (render mode 3 or 7) stays searchable under `invisibleText: keep` and
//!   goes under every region (rule 1: the redaction plan sees it like any glyph). `remove`
//!   drops all of it in the layer walk ([`super::visibility::scan_with`]).
//! - `ActualText`, `Alt` and `E` of marked-content property lists, inline or in `/Properties`,
//!   at any depth, are counted here and never copied (the writer's content re-emitter and its
//!   allowlist).
//! - No false success: a glyph whose place on the page cannot be established (a singular CTM,
//!   font size 0) may be a hidden copy of a redacted word that the plan cannot see, so an export
//!   with regions on that page is refused.
use crate::Document;
use crate::export::preflight::Blocker;
use crate::interpret::ItemKind;
use crate::redaction::plan::RedactionPlan;
use foliark_model::{ErrorCode, PdfError, Quad};
use hayro::hayro_syntax::content::Instruction;
use hayro::hayro_syntax::object::{Dict, Object};
use hayro::hayro_syntax::page::Resources;

/// Marked-content property keys that carry document text.
pub const ACCESSIBILITY_KEYS: [&str; 3] = ["ActualText", "Alt", "E"];

/// Deeper than any real property list; past it nothing is counted (and the writer drops it).
const MAX_DEPTH: u8 = 32;

fn count(o: &Object<'_>, depth: u8) -> u32 {
    if depth > MAX_DEPTH {
        return 0;
    }
    match o {
        Object::Dict(d) => d
            .keys()
            .map(|k| {
                u32::from(ACCESSIBILITY_KEYS.contains(&k.as_str()))
                    + d.get::<Object<'_>>(k).map_or(0, |v| count(&v, depth + 1))
            })
            .sum(),
        Object::Array(a) => a.iter::<Object<'_>>().map(|v| count(&v, depth + 1)).sum(),
        _ => 0,
    }
}

/// Accessibility entries of one `BDC` instruction: its inline property dictionary, or the
/// `/Properties` entry it names.
pub fn accessibility_entries(ins: &Instruction<'_, '_>, res: &Resources<'_>) -> u32 {
    let mut ops = ins.operands();
    ops.next();
    match ops.next() {
        Some(Object::Name(n)) => res
            .properties
            .get::<Dict<'_>>(n)
            .map_or(0, |d| count(&Object::Dict(d), 0)),
        Some(o) => count(o, 0),
        None => 0,
    }
}

/// Whether a glyph quad covers a finite area (shoelace) a region can be tested against.
fn placed(q: &Quad) -> bool {
    let p = &q.0;
    let a = (0..4)
        .map(|i| p[i].x * p[(i + 1) % 4].y - p[(i + 1) % 4].x * p[i].y)
        .sum::<f64>()
        .abs()
        / 2.0;
    a.is_finite() && a > 1e-6
}

/// Pages with regions that draw a glyph without a place on the page: its quad has no area, so
/// no region can be said to cover it or not.
pub fn mapping_blockers(doc: &Document, plan: &RedactionPlan) -> Result<Vec<Blocker>, PdfError> {
    let mut b = Blocker {
        code: ErrorCode::InvalidArgument,
        feature: "textPositionUnknown",
        pages: Vec::new(),
        ids: Vec::new(),
        fields: Vec::new(),
    };
    for p in &plan.pages {
        let lost = doc
            .page_model(p.page_index as usize)?
            .items
            .iter()
            .any(|it| matches!(&it.kind, ItemKind::Glyph(g) if !placed(&g.quad_user)));
        if lost {
            b.pages.push(p.page_index);
            b.ids.extend(p.regions.iter().map(|r| r.redaction.clone()));
        }
    }
    b.ids.sort_unstable();
    b.ids.dedup();
    Ok(if b.pages.is_empty() { vec![] } else { vec![b] })
}
