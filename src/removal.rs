//! Running text that states a mention's removal, such as `Delete X` or `X no longer exists`,
//! which names it as gone and stays true once it is.

use std::ops::Range;

use crate::Kind;
use crate::text::{Spelled, code_shaped};

/// A stretch of running text, by offsets into the text it was read from.
#[derive(Debug)]
pub(crate) enum Segment {
    /// Words and punctuation, where a backtick-delimited span is code.
    Text(Range<usize>),
    /// An inline code span.
    Code(Range<usize>),
    /// A block boundary, which ends every clause.
    Stop,
}

/// Verbs that remove what follows them, as in `Delete X` or `deleting X`.
const REMOVING: [&str; 12] = [
    "delete", "deletes", "deleted", "deleting", "remove", "removes", "removed", "removing",
    "retire", "retires", "retired", "retiring",
];

/// Nouns whose `of` names what is removed, as in `the removal of X`.
const REMOVAL: [&str; 3] = ["deletion", "removal", "retirement"];

/// Words that may stand between a removing verb and what it removes.
const DETERMINERS: [&str; 6] = ["the", "a", "an", "both", "all", "each"];

/// Words that may stand between a name and what is said of it, as in `X has now been deleted`.
const AUXILIARIES: [&str; 38] = [
    "is",
    "are",
    "was",
    "were",
    "be",
    "been",
    "being",
    "has",
    "have",
    "had",
    "will",
    "would",
    "shall",
    "should",
    "must",
    "can",
    "could",
    "may",
    "might",
    "get",
    "gets",
    "got",
    "gotten",
    "does",
    "do",
    "did",
    "to",
    "now",
    "already",
    "also",
    "then",
    "finally",
    "entirely",
    "completely",
    "fully",
    "simply",
    "therefore",
    "eventually",
];

/// What a name is once removed, as in `X is deleted` or `with X gone`.
const GONE: [&str; 4] = ["deleted", "removed", "retired", "gone"];

/// Existence, which a negation turns into absence, as in `X does not exist`.
const EXISTING: [&str; 3] = ["exist", "exists", "existing"];

/// Drops the running text mentions of `found[from..]` whose clause states their removal.
/// `segments` locate the text in `text`, whose first byte sits at `base`, the offset space of
/// the mentions.
pub(crate) fn drop_records(
    text: &str,
    base: usize,
    segments: &[Segment],
    found: &mut Vec<Spelled>,
    from: usize,
) {
    let prose = |spelled: &Spelled| matches!(spelled.context.kind, Kind::Text | Kind::CodeSpan);
    if !found[from..].iter().any(prose) {
        return;
    }
    let pieces = pieces(text, base, segments);
    let mut kept = from;
    for at in from..found.len() {
        let spelled = &found[at];
        if !(prose(spelled) && records_removal(&pieces, spelled.start)) {
            found.swap(kept, at);
            kept += 1;
        }
    }
    found.truncate(kept);
}

/// A word, code span or clause boundary of running text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Piece<'t> {
    start: usize,
    end: usize,
    shape: Shape<'t>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape<'t> {
    Word(&'t str),
    Code,
    Comma,
    Stop,
}

impl Piece<'_> {
    /// Whether the piece can name an item in a list, as a code span or a code-shaped word does.
    fn is_item(&self) -> bool {
        match self.shape {
            Shape::Code => true,
            Shape::Word(word) => code_shaped(word) || word.contains("::"),
            Shape::Comma | Shape::Stop => false,
        }
    }

    /// Whether the piece joins two items of a list.
    fn is_connector(&self) -> bool {
        match self.shape {
            Shape::Comma => true,
            Shape::Word(word) => is_any(word, &["and", "or", "nor"]),
            Shape::Code | Shape::Stop => false,
        }
    }

    fn word_in(&self, words: &[&str]) -> bool {
        matches!(self.shape, Shape::Word(word) if is_any(word, words))
    }
}

fn is_any(word: &str, words: &[&str]) -> bool {
    words.iter().any(|w| w.eq_ignore_ascii_case(word))
}

/// The pieces of `segments` of `text`, at offsets shifted by `base`, in order.
fn pieces<'t>(text: &'t str, base: usize, segments: &[Segment]) -> Vec<Piece<'t>> {
    let mut out = Vec::new();
    for segment in segments {
        match segment {
            Segment::Text(range) => words(text, base, range.clone(), &mut out),
            Segment::Code(range) => out.push(Piece {
                start: base + range.start,
                end: base + range.end,
                shape: Shape::Code,
            }),
            Segment::Stop => {
                let at = out.last().map_or(base, |piece: &Piece<'_>| piece.end);
                stop(&mut out, at);
            }
        }
    }
    out
}

/// Ends the clause at `at`, once.
fn stop(out: &mut Vec<Piece<'_>>, at: usize) {
    if out.last().is_some_and(|piece| piece.shape != Shape::Stop) {
        out.push(Piece {
            start: at,
            end: at,
            shape: Shape::Stop,
        });
    }
}

/// Whether `c` continues a word, as the characters of identifiers do.
fn word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The words, code spans and clause boundaries of `text[range]`. A word runs over identifier
/// characters, joined by `::`, an inner `-` or an inner apostrophe. A line holding no word
/// ends the clause, as the empty line of a comment does.
fn words<'t>(text: &'t str, base: usize, range: Range<usize>, out: &mut Vec<Piece<'t>>) {
    let end = range.end;
    let mut at = range.start;
    let mut blank_line = false;
    while at < end {
        let c = text[at..end]
            .chars()
            .next()
            .expect("at is inside the range");
        if word_char(c) {
            let start = at;
            at += c.len_utf8();
            while at < end {
                let rest = &text[at..end];
                let next = rest.chars().next().expect("at is inside the range");
                let joined = match next {
                    ':' if rest.starts_with("::") => Some(2),
                    '-' | '\'' | '\u{2019}' => Some(next.len_utf8()),
                    _ => None,
                };
                if word_char(next) {
                    at += next.len_utf8();
                } else if let Some(len) = joined
                    && rest[len..].starts_with(word_char)
                {
                    at += len;
                } else {
                    break;
                }
            }
            out.push(Piece {
                start: base + start,
                end: base + at,
                shape: Shape::Word(&text[start..at]),
            });
            blank_line = false;
            continue;
        }
        let mut len = c.len_utf8();
        match c {
            '`' => {
                let ticks = text[at..end].len() - text[at..end].trim_start_matches('`').len();
                let fence = &text[at..at + ticks];
                if let Some(close) = text[at + ticks..end].find(fence) {
                    len = ticks + close + ticks;
                    out.push(Piece {
                        start: base + at,
                        end: base + at + len,
                        shape: Shape::Code,
                    });
                    blank_line = false;
                } else {
                    len = ticks;
                }
            }
            ',' => out.push(Piece {
                start: base + at,
                end: base + at + 1,
                shape: Shape::Comma,
            }),
            '.' | '!' | '?' | ';' | ':' | '|' | '\u{2013}' | '\u{2014}' => stop(out, base + at),
            '\n' => {
                if blank_line {
                    stop(out, base + at);
                }
                blank_line = true;
            }
            _ => {}
        }
        at += len;
    }
}

/// Whether the mention starting at `start` sits in a list that a removing verb takes as its
/// object or of which the clause says it is gone or does not exist.
fn records_removal(pieces: &[Piece<'_>], start: usize) -> bool {
    let Some(at) = pieces
        .partition_point(|piece| piece.start <= start)
        .checked_sub(1)
        .filter(|&at| start < pieces[at].end && pieces[at].shape != Shape::Comma)
    else {
        return false;
    };
    let (first, last) = list(pieces, at);
    removed_by_verb(&pieces[..first]) || said_gone(&pieces[last + 1..])
}

/// The first and last piece of the list of items holding piece `at`.
fn list(pieces: &[Piece<'_>], at: usize) -> (usize, usize) {
    let mut first = at;
    loop {
        let mut k = first;
        while k > 0 && pieces[k - 1].is_connector() {
            k -= 1;
        }
        if k == first || k == 0 || !pieces[k - 1].is_item() {
            break;
        }
        first = k - 1;
    }
    let mut last = at;
    loop {
        let mut k = last + 1;
        while k < pieces.len() && pieces[k].is_connector() {
            k += 1;
        }
        if k == last + 1 || k == pieces.len() || !pieces[k].is_item() {
            break;
        }
        last = k;
    }
    (first, last)
}

/// Whether `before`, the pieces ahead of a list, ends in a removing verb or a removal `of`.
fn removed_by_verb(before: &[Piece<'_>]) -> bool {
    let mut end = before.len();
    while end > 0 && before[end - 1].word_in(&DETERMINERS) {
        end -= 1;
    }
    match before[..end] {
        [.., noun, of] if of.word_in(&["of"]) => noun.word_in(&REMOVAL),
        [.., verb] => verb.word_in(&REMOVING),
        [] => false,
    }
}

/// Whether `after`, the pieces following a list, say it is gone or does not exist.
fn said_gone(after: &[Piece<'_>]) -> bool {
    let mut negated = false;
    let mut at = 0;
    while let Some(Piece {
        shape: Shape::Word(word),
        ..
    }) = after.get(at)
    {
        if is_any(word, &["not", "never"]) || word.ends_with("n't") || word.ends_with("n\u{2019}t")
        {
            negated = true;
        } else if word.eq_ignore_ascii_case("no")
            && after
                .get(at + 1)
                .is_some_and(|next| next.word_in(&["longer"]))
        {
            negated = true;
            at += 1;
        } else if !is_any(word, &AUXILIARIES) {
            break;
        }
        at += 1;
    }
    let Some(piece) = after.get(at) else {
        return false;
    };
    if negated {
        return piece.word_in(&EXISTING);
    }
    // A participle right after the name is its verb when more follows, as in `X removed the rows`.
    let ends = after
        .get(at + 1)
        .is_none_or(|next| matches!(next.shape, Shape::Comma | Shape::Stop));
    piece.word_in(&["gone"]) || piece.word_in(&GONE) && (at > 0 || ends)
}
