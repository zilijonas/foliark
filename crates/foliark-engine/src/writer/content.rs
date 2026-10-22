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
    while let Some((_, span)) = ops.next_with_span() {
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
                let mark = out.len();
                normalize(&data[span], &mut out);
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
}
