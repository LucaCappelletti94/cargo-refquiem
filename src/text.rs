//! Path-shaped mentions in comments and Markdown.

use ra_ap_rustc_lexer::{DocStyle, FrontmatterAllowed, TokenKind, is_id_continue, is_id_start};

use crate::Context;

/// A path spelled in prose, before resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Spelled {
    /// Byte offset of the first character in the file.
    pub(crate) start: usize,
    /// Byte offset one past the last character in the file.
    pub(crate) end: usize,
    /// The segments without generic arguments or `r#`.
    pub(crate) segments: Vec<String>,
    /// Whether the path starts with `::`.
    pub(crate) leading_colon: bool,
    pub(crate) context: Context,
}

/// The comments of a Rust source file as `(byte offset, text, context)`.
pub(crate) fn rust_comments(source: &str) -> Vec<(usize, &str, Context)> {
    let skip = ra_ap_rustc_lexer::strip_shebang(source).unwrap_or(0);
    let mut offset = skip;
    let mut out = Vec::new();
    for token in ra_ap_rustc_lexer::tokenize(&source[skip..], FrontmatterAllowed::Yes) {
        let len = usize::try_from(token.len).expect("token lengths fit in usize");
        let context = match token.kind {
            TokenKind::LineComment { doc_style } | TokenKind::BlockComment { doc_style, .. } => {
                Some(match doc_style {
                    Some(DocStyle::Inner | DocStyle::Outer) => Context::DocComment,
                    None => Context::Comment,
                })
            }
            _ => None,
        };
        if let Some(context) = context {
            out.push((offset, &source[offset..offset + len], context));
        }
        offset += len;
    }
    out
}

/// Every path of two or more segments in `text`, offsets shifted by `base`.
pub(crate) fn paths(text: &str, base: usize, context: Context) -> Vec<Spelled> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let Some(c) = text[i..].chars().next() else {
            break;
        };
        let previous = text[..i].chars().next_back();
        let boundary = previous.is_none_or(|p| !is_id_continue(p) && p != ':' && p != '#');
        let leading_colon = boundary && text[i..].starts_with("::") && starts_ident(&text[i + 2..]);
        if boundary && (leading_colon || starts_ident(&text[i..])) {
            let start = i;
            let mut cursor = if leading_colon { i + 2 } else { i };
            let mut segments = Vec::new();
            let mut end;
            loop {
                let (segment, after) = ident(text, cursor).expect("an identifier starts here");
                segments.push(segment);
                end = after;
                cursor = after;
                let mut probe = cursor;
                if text[probe..].starts_with('<') {
                    match skip_angles(text, probe) {
                        Some(after_angles) if text[after_angles..].starts_with("::") => {
                            probe = after_angles;
                        }
                        _ => break,
                    }
                }
                if !text[probe..].starts_with("::") {
                    break;
                }
                probe += 2;
                if text[probe..].starts_with('<') {
                    let Some(after_angles) = skip_angles(text, probe) else {
                        break;
                    };
                    end = after_angles;
                    if !text[after_angles..].starts_with("::") {
                        break;
                    }
                    probe = after_angles + 2;
                }
                if !starts_ident(&text[probe..]) {
                    break;
                }
                cursor = probe;
            }
            if segments.len() >= 2 {
                out.push(Spelled {
                    start: base + start,
                    end: base + end,
                    segments,
                    leading_colon,
                    context,
                });
            }
            i = end.max(i + c.len_utf8());
        } else {
            i += c.len_utf8();
        }
    }
    out
}

fn starts_ident(text: &str) -> bool {
    let text = text.strip_prefix("r#").unwrap_or(text);
    text.chars().next().is_some_and(is_id_start)
}

/// The identifier at `at` without `r#`, and the offset after it.
fn ident(text: &str, at: usize) -> Option<(String, usize)> {
    let body = if text[at..].starts_with("r#") {
        at + 2
    } else {
        at
    };
    let mut chars = text[body..].char_indices();
    let (_, first) = chars.next()?;
    if !is_id_start(first) {
        return None;
    }
    let len = chars
        .find(|(_, c)| !is_id_continue(*c))
        .map_or(text.len() - body, |(offset, _)| offset);
    Some((text[body..body + len].to_owned(), body + len))
}

/// The offset after the balanced `<...>` starting at `at`.
fn skip_angles(text: &str, at: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, c) in text[at..].char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(at + offset + 1);
                }
            }
            '\n' | '`' | '(' | ')' | '{' | '}' | ';' => return None,
            _ => {}
        }
    }
    None
}

/// One-based line and character column of byte `offset` in `text`.
pub(crate) fn line_column(line_starts: &[usize], text: &str, offset: usize) -> (usize, usize) {
    let line = line_starts.partition_point(|&start| start <= offset);
    let start = line_starts[line - 1];
    (line, text[start..offset].chars().count() + 1)
}

/// The byte offsets where each line of `text` starts.
pub(crate) fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect()
}
