//! Style runs through an edit (ADR 0013): the look each character of an edited text is drawn in,
//! carried over from the text the page has.
use std::ops::Range;

/// Above this many token pairs the changed middle is one replaced block.
const MAX_CELLS: usize = 1 << 20;

/// Words (maximal runs of non-whitespace characters) and single whitespace characters.
fn tokens(s: &[char]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let n = if s[i].is_whitespace() {
            1
        } else {
            s[i..].iter().take_while(|c| !c.is_whitespace()).count()
        };
        out.push(i..i + n);
        i += n;
    }
    out
}

/// The look of every character of `new`, given `old` and the look of each of its characters
/// (`looks`, one per `char`). The texts are lined up word by word (common prefix and suffix, then
/// the longest common subsequence of the words between); every word the two share keeps its
/// characters' looks. Whitespace never lines up on its own: a shared space is no evidence that
/// the words around it correspond. What lies between shared words is a replaced block:
/// - with as many words on each side, word k replaces word k (each keeps its own look, below);
/// - otherwise it is one block, and text typed over text takes ONE look;
/// - in a block, equal whitespace at its ends keeps its looks; if each side is a single word
///   (an in-word edit) a common character prefix and suffix keep theirs too;
/// - what remains: text that replaces text takes the look of the first character it replaces;
/// - inserted text touching a word takes that word's look (the character before it first);
/// - an inserted whole word takes the look its neighbouring words share, else `main`;
/// - new whitespace takes `main`.
pub fn carry(old: &str, looks: &[usize], new: &str, main: usize) -> Vec<usize> {
    let (a, b): (Vec<char>, Vec<char>) = (old.chars().collect(), new.chars().collect());
    let (ta, tb) = (tokens(&a), tokens(&b));
    let eq = |i: usize, j: usize| a[ta[i].clone()] == b[tb[j].clone()];
    let word = |t: &[Range<usize>], s: &[char], k: usize| !s[t[k].start].is_whitespace();
    let common = ta.len().min(tb.len());
    let pre = (0..common).take_while(|&k| eq(k, k)).count();
    let suf = (0..common - pre)
        .take_while(|&k| eq(ta.len() - 1 - k, tb.len() - 1 - k))
        .count();
    let (n, m) = (ta.len() - pre - suf, tb.len() - pre - suf);
    let mut pairs: Vec<(usize, usize)> = (0..pre)
        .filter(|&k| word(&ta, &a, k))
        .map(|k| (k, k))
        .collect();
    if n > 0 && m > 0 && n * m <= MAX_CELLS {
        // Longest common subsequence of the middle words, from the back; lengths fit u16
        // because min(n, m) <= 1024 here.
        let at = |i: usize, j: usize| i * (m + 1) + j;
        let mut t = vec![0u16; (n + 1) * (m + 1)];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                t[at(i, j)] = if eq(pre + i, pre + j) && word(&ta, &a, pre + i) {
                    t[at(i + 1, j + 1)] + 1
                } else {
                    t[at(i + 1, j)].max(t[at(i, j + 1)])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if eq(pre + i, pre + j) && word(&ta, &a, pre + i) {
                pairs.push((pre + i, pre + j));
                (i, j) = (i + 1, j + 1);
            } else if t[at(i + 1, j)] >= t[at(i, j + 1)] {
                i += 1;
            } else {
                j += 1;
            }
        }
    }
    pairs.extend(
        (0..suf)
            .map(|k| (ta.len() - suf + k, tb.len() - suf + k))
            .filter(|&(i, _)| word(&ta, &a, i)),
    );

    let mut out = vec![main; b.len()];
    // Character offsets cutting the tokens `r` into one block, or, with `split`, into one block
    // per word (each after the previous word's end, so its own leading whitespace goes along).
    let cuts = |t: &[Range<usize>], s: &[char], r: Range<usize>, split: bool| {
        let at = |k: usize| t.get(k).map_or(s.len(), |x| x.start);
        let mut v = vec![at(r.start)];
        if split {
            let words: Vec<usize> = r.clone().filter(|&k| word(t, s, k)).collect();
            v.extend(words[..words.len() - 1].iter().map(|&k| t[k].end));
        }
        v.push(at(r.end));
        v
    };
    let words =
        |t: &[Range<usize>], s: &[char], r: Range<usize>| r.filter(|&k| word(t, s, k)).count();
    let (mut i0, mut j0) = (0, 0);
    for (i, j) in pairs.into_iter().chain([(ta.len(), tb.len())]) {
        // Word for word when both sides have the same number of them (two or more).
        let n = words(&ta, &a, i0..i);
        let split = n > 1 && n == words(&tb, &b, j0..j);
        let (ca, cb) = (cuts(&ta, &a, i0..i, split), cuts(&tb, &b, j0..j, split));
        for (x, y) in ca.windows(2).zip(cb.windows(2)) {
            block(&a, looks, &b, x[0]..x[1], y[0]..y[1], main, &mut out);
        }
        if i < ta.len() {
            for (o, n) in ta[i].clone().zip(tb[j].clone()) {
                out[n] = looks[o];
            }
        }
        (i0, j0) = (i + 1, j + 1);
    }
    out
}

/// One replaced block: `a[ra]` became `b[rb]`.
fn block(
    a: &[char],
    looks: &[usize],
    b: &[char],
    mut ra: Range<usize>,
    mut rb: Range<usize>,
    main: usize,
    out: &mut [usize],
) {
    let ws = |c: &char| c.is_whitespace();
    // Equal whitespace at the ends keeps its looks; then, for an in-word edit only, equal
    // characters do (across several words a shared letter or final '.' is no evidence).
    for chars in [false, true] {
        if chars && a[ra.clone()].iter().chain(&b[rb.clone()]).any(ws) {
            break;
        }
        let keep = |c: &char| chars || ws(c);
        while !ra.is_empty() && !rb.is_empty() && a[ra.start] == b[rb.start] && keep(&a[ra.start]) {
            out[rb.start] = looks[ra.start];
            (ra.start, rb.start) = (ra.start + 1, rb.start + 1);
        }
        while !ra.is_empty()
            && !rb.is_empty()
            && a[ra.end - 1] == b[rb.end - 1]
            && keep(&a[ra.end - 1])
        {
            out[rb.end - 1] = looks[ra.end - 1];
            (ra.end, rb.end) = (ra.end - 1, rb.end - 1);
        }
    }
    if rb.is_empty() {
        return;
    }
    let look = if !ra.is_empty() {
        looks[ra.start]
    } else {
        let p = ra.start;
        let (first, last) = (b[rb.start], b[rb.end - 1]);
        match (p.checked_sub(1), a.get(p)) {
            (Some(l), _) if !ws(&a[l]) && !ws(&first) => looks[l],
            (_, Some(r)) if !ws(r) && !ws(&last) => looks[p],
            _ => {
                let left = a[..p].iter().rposition(|c| !ws(c)).map(|k| looks[k]);
                let right = a[p..].iter().position(|c| !ws(c)).map(|k| looks[p + k]);
                match (left, right) {
                    (Some(x), Some(y)) if x == y => x,
                    (Some(x), None) | (None, Some(x)) => x,
                    _ => main,
                }
            }
        }
    };
    // New whitespace is the paragraph's: it draws nothing, and the run's font may not have it.
    for k in rb {
        out[k] = if ws(&b[k]) { main } else { look };
    }
}

#[cfg(test)]
mod tests {
    use super::carry;

    /// `marked` with `*bold*` (look 1) and `_italic_` (look 2) as text and looks.
    fn parse(marked: &str) -> (String, Vec<usize>) {
        let (mut s, mut l, mut cur) = (String::new(), Vec::new(), 0);
        for c in marked.chars() {
            match c {
                '*' => cur = if cur == 1 { 0 } else { 1 },
                '_' => cur = if cur == 2 { 0 } else { 2 },
                c => {
                    s.push(c);
                    l.push(cur);
                }
            }
        }
        (s, l)
    }

    fn show(text: &str, looks: &[usize]) -> String {
        let mut out = String::new();
        let mut cur = 0;
        for (c, &l) in text.chars().zip(looks) {
            if l != cur {
                out.push_str([None, Some("*"), Some("_")][cur].unwrap_or(""));
                out.push_str([None, Some("*"), Some("_")][l].unwrap_or(""));
                cur = l;
            }
            out.push(c);
        }
        out.push_str([None, Some("*"), Some("_")][cur].unwrap_or(""));
        out
    }

    fn edit(old: &str, new: &str) -> String {
        let (o, l) = parse(old);
        show(new, &carry(&o, &l, new, 0))
    }

    #[test]
    fn unchanged_characters_keep_their_look_and_new_ones_take_the_run_typed_into() {
        let old = "a *Owner* is one, see _Annex II_ and *Term* here";
        // Unchanged.
        assert_eq!(edit(old, "a Owner is one, see Annex II and Term here"), old);
        // Inside a bold word, at its end, and replacing a regular word.
        assert_eq!(
            edit(old, "a Ownr is one, see Annex II and Terms here"),
            "a *Ownr* is one, see _Annex II_ and *Terms* here"
        );
        assert_eq!(
            edit(old, "a Owner is two, see Annex II and Term here"),
            "a *Owner* is two, see _Annex II_ and *Term* here"
        );
        // Two edits far apart: everything between keeps its looks.
        assert_eq!(
            edit(old, "the Owner is one, see Annex II and Term now"),
            "the *Owner* is one, see _Annex II_ and *Term* now"
        );
        // Across a run boundary: each replaced word keeps its own look.
        assert_eq!(
            edit(old, "a Owners be one, see Annex II and Term here"),
            "a *Owners* be one, see _Annex II_ and *Term* here"
        );
    }

    #[test]
    fn a_new_whole_word_takes_its_neighbours_shared_look_or_the_main_one() {
        let old = "a *Owner* is one, see _Annex II_";
        assert_eq!(
            edit(old, "a Owner now is one, see Annex II"),
            "a *Owner* now is one, see _Annex II_"
        );
        assert_eq!(
            edit(old, "a new Owner is one, see Annex II"),
            "a new *Owner* is one, see _Annex II_"
        );
        assert_eq!(
            edit(old, "a Owner is one, see Annex IV II"),
            "a *Owner* is one, see _Annex IV_ _II_"
        );
        // Prefixing a word joins it.
        assert_eq!(
            edit(old, "a coOwner is one, see Annex II"),
            "a *coOwner* is one, see _Annex II_"
        );
        // Nothing left to inherit from: the main look.
        assert_eq!(edit("*Owner*", ""), "");
        assert_eq!(edit("", "new"), "new");
    }

    #[test]
    fn text_retyped_over_a_selection_takes_one_look_whatever_spaces_and_dots_it_shares() {
        // Regular start, an inline bold term, an italic tail ending in a full stop.
        let old = "In law the *Owner* holds it all as set out in _Annex II and the Annex IV._";
        // Select all and type three words and a full stop: the spaces and the '.' match the
        // italic tail's, but they are no evidence the words correspond. One look: the first's.
        assert_eq!(edit(old, "Quick test now."), "Quick test now.");
        // From the bold term to the end: one look, the bold term's (new spaces are the main look).
        assert_eq!(
            edit(old, "In law the Quick test now."),
            "In law the *Quick* *test* *now.*"
        );
        // Word for word (as many words each side): each keeps the look of the word it replaces.
        assert_eq!(
            edit("*Owner* is _one_", "Owners be two"),
            "*Owners* be _two_"
        );
    }
}
