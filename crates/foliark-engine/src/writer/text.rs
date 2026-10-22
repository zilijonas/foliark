//! Planned text as content operators (spec §6.3): what `append_page_content` draws on top of a
//! page. Rust's `LayoutPlan` is authoritative; this only serializes it.
use super::err;
use super::fonts::EmbeddedFont;
use foliark_model::{ErrorCode, PdfError};
use foliark_text::LayoutPlan;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// How a plan's font is selected: registered font id -> (resource name given to
/// `Writer::add_page_font`, the embedded font).
pub type PlanFonts<'a> = BTreeMap<&'a str, (&'a str, &'a EmbeddedFont)>;

/// Runs set in a font of the source file (`RunSource::Embedded`): the font object's reference
/// as the plan names it -> (resource name given to `Writer::add_page_font`, bytes per code).
/// The plan's glyph ids are the character codes themselves and its advances the font's own
/// widths, so nothing is adjusted.
pub type SourceFonts<'a> = BTreeMap<&'a str, (&'a str, usize)>;

/// Number text that never prints an exponent and trims trailing zeros.
fn num(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" {
        "0".into()
    } else {
        s.into()
    }
}

/// Operators drawing `plan` in fill color `rgb` (0 to 1), wrapped in `q ... Q`, text mode 0.
/// `to_user` is the affine `[a b c d e f]` from plan space (x right, baseline y down, as in
/// the plan) to default user space; glyphs stand upright in plan space. Each line is one
/// `BT` with a `Tm` per run and a `TJ` of 2-byte hex CIDs; the difference between a glyph's
/// planned advance and the font's `/W` width goes into the `TJ` as a number.
/// A run in a font that is neither registered nor a listed source font, or a glyph missing
/// from the subset, is `FONT_UNSUPPORTED`.
pub fn emit_plan(
    plan: &LayoutPlan,
    fonts: &PlanFonts<'_>,
    sources: &SourceFonts<'_>,
    rgb: [f64; 3],
    to_user: [f64; 6],
) -> Result<Vec<u8>, PdfError> {
    let [a, b, c, d, e, f] = to_user;
    let mut s = format!(
        "q\n{} {} {} rg\n0 Tr\n",
        num(rgb[0]),
        num(rgb[1]),
        num(rgb[2])
    );
    for line in &plan.lines {
        s.push_str("BT\n");
        let mut x = line.x;
        for run in &line.runs {
            let (res, font) = match (fonts.get(run.font.as_str()), sources.get(run.font.as_str())) {
                (Some((res, font)), _) => (*res, Some(*font)),
                (None, Some((res, _))) => (*res, None),
                (None, None) => {
                    return Err(err(ErrorCode::FontUnsupported, "run font is not embedded"));
                }
            };
            let (px, py) = (
                a * x + c * line.baseline_y + e,
                b * x + d * line.baseline_y + f,
            );
            // text space is y up, plan space y down: flip the second row
            let _ = write!(
                s,
                "/{res} {} Tf\n{} {} {} {} {} {} Tm\n[",
                num(run.size),
                num(a),
                num(b),
                num(-c),
                num(-d),
                num(px),
                num(py)
            );
            for g in &run.glyphs {
                let Some(font) = font else {
                    let width = sources[run.font.as_str()].1;
                    let _ = write!(s, "<{:0w$X}>", g.gid, w = width * 2);
                    x += g.x_advance;
                    continue;
                };
                let gid = u16::try_from(g.gid)
                    .ok()
                    .and_then(|gid| font.cid.get(&gid))
                    .ok_or_else(|| err(ErrorCode::FontUnsupported, "glyph not in the subset"))?;
                let _ = write!(s, "<{gid:04X}>");
                let nominal = font.widths.get(gid).copied().unwrap_or(0.0);
                let adjust = nominal - g.x_advance * 1000.0 / run.size;
                if adjust.abs() > 0.005 {
                    let _ = write!(s, " {} ", num(adjust));
                }
                x += g.x_advance;
            }
            s.push_str("] TJ\n");
        }
        s.push_str("ET\n");
    }
    s.push_str("Q\n");
    Ok(s.into_bytes())
}
