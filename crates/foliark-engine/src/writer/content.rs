//! Content stream re-emission (ADR 0002). Each instruction is copied from its byte span with
//! operand tokens verbatim (numbers keep their exact text); comments, whitespace runs and
//! junk between instructions are dropped. A substitution replaces every instruction a byte
//! range touches by caller-supplied operators.
use super::err;
use foliark_model::{ErrorCode, ObjectId, PdfError};
use hayro::hayro_syntax::content::UntypedIter;
use std::ops::Range;

const WS: &[u8] = b"\0\t\n\x0c\r ";

/// `subs` are non-overlapping `(range, ops)` pairs; `data` is the decoded stream. A
/// substitution that touches no instruction is an error (it would silently do nothing).
///
/// Without substitutions the output must be a fixed point (emitting it again changes nothing);
/// a stream whose damaged inline images cannot be delimited that way is `MALFORMED_PDF`, so
/// ambiguous bytes are never written (ADR 0002).
pub fn reemit(
    data: &[u8],
    obj: ObjectId,
    subs: &[(Range<usize>, Vec<u8>)],
) -> Result<Vec<u8>, PdfError> {
    let out = emit(data, obj, subs)?;
    if subs.is_empty() && emit(&out, obj, &[])? != out {
        return Err(err(
            ErrorCode::MalformedPdf,
            &format!(
                "content stream {} {} R has an ambiguous inline image",
                obj.num, obj.generation
            ),
        ));
    }
    Ok(out)
}

fn emit(data: &[u8], obj: ObjectId, subs: &[(Range<usize>, Vec<u8>)]) -> Result<Vec<u8>, PdfError> {
    let mut out = Vec::with_capacity(data.len());
    let mut used = vec![false; subs.len()];
    let mut ops = UntypedIter::new(data);
    while let Some((ins, span)) = ops.next_with_span() {
        let hit = subs
            .iter()
            .position(|(r, _)| r.start < span.end && span.start < r.end);
        match hit {
            Some(i) if used[i] => {}
            Some(i) => {
                used[i] = true;
                out.extend_from_slice(&subs[i].1);
                if out.last().is_some_and(|b| *b != b'\n') {
                    out.push(b'\n');
                }
            }
            None => {
                if !OPERATORS.contains(&&**ins.operator) {
                    continue;
                }
                let mark = out.len();
                normalize(&data[span], &mut out);
                // Spec 8.2 (ADR 0006): marked-content property lists keep their tag and structure
                // ids, not the text an inline dictionary can carry.
                if out.ends_with(b"BDC") || out.ends_with(b"DP") {
                    let clean = strip_marked_props(&out[mark..]);
                    out.truncate(mark);
                    out.extend_from_slice(&clean);
                }
                // Never emit tokens that read back as something else (damaged inline images).
                let mut back = UntypedIter::new(&out[mark..]);
                let mut count = 0;
                while back.next_with_span().is_some() {
                    count += 1;
                }
                if count != 1 {
                    return Err(err(
                        ErrorCode::MalformedPdf,
                        &format!(
                            "instruction in {} {} R does not re-emit",
                            obj.num, obj.generation
                        ),
                    ));
                }
                // An unterminated inline image keeps its trailing newline as data; adding
                // another one every pass would make re-emission grow.
                if out.last() != Some(&b'\n') {
                    out.push(b'\n');
                }
            }
        }
    }
    if used.iter().any(|u| !u) {
        return Err(err(
            ErrorCode::InvalidArgument,
            &format!(
                "span substitution in {} {} R matches no instruction",
                obj.num, obj.generation
            ),
        ));
    }
    Ok(out)
}

fn is_delim(b: u8) -> bool {
    b"()<>[]{}/%".contains(&b)
}

/// Copy one instruction span: tokens verbatim, comments dropped, whitespace runs one space,
/// inline image data (after `ID`) untouched.
fn normalize(src: &[u8], out: &mut Vec<u8>) {
    let start = out.len();
    let mut i = 0;
    let mut gap = false;
    while i < src.len() {
        let b = src[i];
        if WS.contains(&b) {
            gap = true;
            i += 1;
            continue;
        }
        if b == b'%' {
            while i < src.len() && src[i] != b'\n' && src[i] != b'\r' {
                i += 1;
            }
            gap = true;
            continue;
        }
        if gap && out.len() > start {
            out.push(b' ');
        }
        gap = false;
        let end = match b {
            b'(' => literal_end(src, i),
            b'<' if src.get(i + 1) == Some(&b'<') => i + 2,
            b'<' => src[i..]
                .iter()
                .position(|c| *c == b'>')
                .map_or(src.len(), |p| i + p + 1),
            b'>' if src.get(i + 1) == Some(&b'>') => i + 2,
            b'>' | b'[' | b']' | b'{' | b'}' | b')' => i + 1,
            _ => {
                let mut j = i + 1;
                while j < src.len() && !WS.contains(&src[j]) && !is_delim(src[j]) {
                    j += 1;
                }
                j
            }
        };
        let token = &src[i..end];
        out.extend_from_slice(token);
        i = end;
        if token == b"ID" && out[start..].starts_with(b"BI") {
            // One whitespace byte, then the image data up to the final `EI` when only
            // whitespace follows it, else (no real terminator) the rest of the span untouched.
            let data_start = (i + 1).min(src.len());
            let ei = match src.windows(2).rposition(|w| w == b"EI") {
                Some(p) if src[p + 2..].iter().all(|b| WS.contains(b)) => p + 2,
                _ => src.len(),
            };
            out.push(b' ');
            out.extend_from_slice(&src[data_start.min(ei)..ei]);
            return;
        }
    }
}

/// Marked-content property keys that survive: structure ids and artifact classification, never text.
const MARKED_KEYS: [&[u8]; 5] = [b"MCID", b"Type", b"Subtype", b"BBox", b"Attached"];

/// PDF 32000 operators; anything else (inside `BX`/`EX` or not) is dropped with its operands, since
/// a reader ignores it and its operands can carry anything.
const OPERATORS: &[&[u8]] = &[
    b"b", b"B", b"b*", b"B*", b"BDC", b"BI", b"BMC", b"BT", b"BX", b"c", b"cm", b"CS", b"cs", b"d",
    b"d0", b"d1", b"Do", b"DP", b"EI", b"EMC", b"ET", b"EX", b"f", b"F", b"f*", b"G", b"g", b"gs",
    b"h", b"i", b"ID", b"j", b"J", b"K", b"k", b"l", b"m", b"M", b"MP", b"n", b"q", b"Q", b"re",
    b"RG", b"rg", b"ri", b"s", b"S", b"SC", b"sc", b"SCN", b"scn", b"sh", b"T*", b"Tc", b"Td",
    b"TD", b"Tf", b"Tj", b"TJ", b"TL", b"Tm", b"Tr", b"Ts", b"Tw", b"Tz", b"v", b"w", b"W", b"W*",
    b"y", b"'", b"\"",
];

/// `ins` is one normalized `/Tag << ... >> BDC` (or `DP`) instruction with its inline dictionary
/// cut to [`MARKED_KEYS`] whose values hold no string or dictionary. A named property list
/// (`/Tag /Name BDC`) is returned unchanged: `/Properties` is sanitized with the resources.
fn strip_marked_props(ins: &[u8]) -> Vec<u8> {
    let Some(open) = ins.windows(2).position(|w| w == b"<<") else {
        return ins.to_vec();
    };
    let end = value_end(ins, open);
    let skip = |mut i: usize| {
        while ins.get(i).is_some_and(|b| WS.contains(b)) {
            i += 1;
        }
        i
    };
    let mut kept = b"<<".to_vec();
    let mut i = open + 2;
    loop {
        i = skip(i);
        if i >= end || ins[i..].starts_with(b">>") {
            break;
        }
        let key_end = value_end(ins, i);
        let at = skip(key_end);
        let val_end = value_end(ins, at).max(at + 1).min(ins.len());
        let value = &ins[at.min(val_end)..val_end];
        if ins[i] == b'/'
            && MARKED_KEYS.contains(&&ins[i + 1..key_end])
            && !value.iter().any(|b| matches!(b, b'(' | b'<'))
        {
            kept.push(b' ');
            kept.extend_from_slice(&ins[i..val_end]);
        }
        i = val_end.max(i + 1);
    }
    kept.extend_from_slice(b" >>");
    [&ins[..open], &kept, &ins[end.min(ins.len())..]].concat()
}

/// End (exclusive) of the PDF value starting at `i`: string, hex string, name, number,
/// array or (nested) dictionary.
fn value_end(s: &[u8], i: usize) -> usize {
    let pair = |j: usize, c: u8| s.get(j) == Some(&c) && s.get(j + 1) == Some(&c);
    match s.get(i) {
        Some(b'(') => literal_end(s, i),
        Some(b'<') if !pair(i, b'<') => s[i..]
            .iter()
            .position(|c| *c == b'>')
            .map_or(s.len(), |p| i + p + 1),
        Some(b'<' | b'[') => {
            let (mut j, mut depth) = (i, 0usize);
            while j < s.len() {
                match s[j] {
                    b'(' => {
                        j = literal_end(s, j);
                        continue;
                    }
                    b'[' => depth += 1,
                    b']' => depth = depth.saturating_sub(1),
                    b'<' if pair(j, b'<') => {
                        depth += 1;
                        j += 1;
                    }
                    b'>' if pair(j, b'>') => {
                        depth = depth.saturating_sub(1);
                        j += 1;
                    }
                    b'<' => {
                        j = s[j..]
                            .iter()
                            .position(|c| *c == b'>')
                            .map_or(s.len(), |p| j + p);
                    }
                    _ => {}
                }
                j += 1;
                if depth == 0 {
                    return j;
                }
            }
            s.len()
        }
        Some(_) => {
            let mut j = i + 1;
            while j < s.len() && !WS.contains(&s[j]) && !is_delim(s[j]) {
                j += 1;
            }
            j
        }
        None => s.len(),
    }
}

/// End (exclusive) of the literal string starting at `i`: balanced parentheses, `\` escapes.
fn literal_end(src: &[u8], mut i: usize) -> usize {
    let mut depth = 0;
    while i < src.len() {
        match src[i] {
            b'\\' => i += 1,
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    src.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: ObjectId = ObjectId {
        num: 1,
        generation: 0,
    };

    #[test]
    fn drops_comments_keeps_strings_numbers_and_inline_image_bytes() {
        let img: &[u8] = b"BI /W 2 /H 1 /BPC 8 /CS /G ID \x00%(\nEI\x01 EI Q";
        let mut data =
            b"q % SECRET_IN_COMMENT\n1.50   0 0 1 -.0 5 cm\n(a %not (a) comment\\)) Tj\n".to_vec();
        data.extend_from_slice(img);
        let out = reemit(&data, ID, &[]).unwrap();
        assert_eq!(
            out,
            [
                &b"q\n1.50 0 0 1 -.0 5 cm\n(a %not (a) comment\\)) Tj\nBI /W 2 /H 1 /BPC 8 /CS /G ID \x00%(\nEI\x01 EI\nQ\n"[..]
            ]
            .concat()
        );
        // Substitution: the `cm` instruction is replaced; a range touching nothing is refused.
        let sub = [(10..30, b"1 0 0 1 0 0 cm".to_vec())];
        let out = reemit(&data, ID, &sub).unwrap();
        assert!(out.starts_with(b"q\n1 0 0 1 0 0 cm\n(a "));
        assert!(reemit(&data, ID, &[(9999..10000, vec![])]).is_err());
    }

    #[test]
    fn marked_content_keeps_tag_and_ids_but_no_text_and_unknown_operators_go() {
        let data = b"/Span << /MCID 3 /ActualText <FEFF0053> /Alt (a (b) c) /E (x) /K [1 2] /Lang (en) /Type /Pagination /BBox [0 0 1 1] /Subtype (s) >> BDC\n\
/Span << /ActualText (only) >> BDC /P /MC0 BDC /OC /Ly BDC EMC EMC EMC EMC\n\
/Artifact << /X << /Y (n) >> /Contents (n3) >> BDC EMC /T << /K (dp) >> DP BX /Weird (bx) << /K (bd) >> unknownop EX 1 0 0 1 0 0 cm";
        let out = String::from_utf8(reemit(data, ID, &[]).unwrap()).unwrap();
        assert_eq!(
            out,
            "/Span << /MCID 3 /Type /Pagination /BBox [0 0 1 1] >> BDC\n/Span << >> BDC\n/P /MC0 BDC\n/OC /Ly BDC\nEMC\nEMC\nEMC\nEMC\n/Artifact << >> BDC\nEMC\n/T << >> DP\nBX\nEX\n1 0 0 1 0 0 cm\n"
        );
    }
}
