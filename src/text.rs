//! Path-shaped mentions in comments, doc comments, string literals and Markdown.

use pulldown_cmark::{
    BrokenLink, CodeBlockKind, CowStr, Event, LinkType, Options, Parser, Tag, TagEnd,
};
use ra_ap_rustc_lexer::{
    FrontmatterAllowed, LiteralKind, TokenKind, is_id_continue, is_id_start, is_ident,
};

use crate::{Context, Kind, Source};

/// A path spelled in text, before resolution.
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
    /// For a rustdoc page link, the index of the first segment naming the item rather than a
    /// directory of the URL.
    pub(crate) url_item: Option<usize>,
    pub(crate) context: Context,
}

/// rustdoc page names of items with their own page, before the `.Name.html` part.
const PAGE_KINDS: [&str; 15] = [
    "struct",
    "union",
    "enum",
    "fn",
    "type",
    "static",
    "trait",
    "macro",
    "primitive",
    "constant",
    "foreigntype",
    "keyword",
    "attr",
    "derive",
    "traitalias",
];

/// rustdoc anchor names of members shown on their parent's page.
const MEMBER_KINDS: [&str; 6] = [
    "method",
    "tymethod",
    "structfield",
    "variant",
    "associatedconstant",
    "associatedtype",
];

/// Every mention in a Rust source file: comments, doc comments read as Markdown, and string
/// literals.
pub(crate) fn rust(source: &str) -> Vec<Spelled> {
    let skip = ra_ap_rustc_lexer::strip_shebang(source).unwrap_or(0);
    let mut tokens = Vec::new();
    let mut offset = skip;
    for token in ra_ap_rustc_lexer::tokenize(&source[skip..], FrontmatterAllowed::Yes) {
        let len = usize::try_from(token.len).expect("token lengths fit in usize");
        tokens.push((offset, len, token.kind));
        offset += len;
    }

    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let (offset, len, kind) = tokens[i];
        let text = &source[offset..offset + len];
        match kind {
            TokenKind::LineComment {
                doc_style: Some(style),
            } => {
                let mut lines = vec![(offset, len)];
                let mut next = i + 1;
                while let Some(&(o, l, k)) = tokens.get(next) {
                    match k {
                        TokenKind::Whitespace => next += 1,
                        TokenKind::LineComment { doc_style: Some(s) } if s == style => {
                            lines.push((o, l));
                            next += 1;
                            i = next - 1;
                        }
                        _ => break,
                    }
                }
                let doc = Doc::from_lines(source, &lines);
                markdown(
                    &doc.text,
                    |at| doc.file_offset(at),
                    Source::DocComment,
                    &mut out,
                );
            }
            TokenKind::BlockComment {
                doc_style: Some(_),
                terminated: true,
            } => {
                let doc = Doc::from_block(source, offset, len);
                markdown(
                    &doc.text,
                    |at| doc.file_offset(at),
                    Source::DocComment,
                    &mut out,
                );
            }
            TokenKind::LineComment { doc_style: None } | TokenKind::BlockComment { .. } => {
                out.extend(paths(text, offset, comment()));
            }
            TokenKind::Literal {
                kind:
                    LiteralKind::Str { .. }
                    | LiteralKind::ByteStr { .. }
                    | LiteralKind::CStr { .. }
                    | LiteralKind::RawStr { .. }
                    | LiteralKind::RawByteStr { .. }
                    | LiteralKind::RawCStr { .. },
                ..
            } => out.extend(paths(
                text,
                offset,
                Context {
                    source: Source::StringLiteral,
                    kind: Kind::Text,
                },
            )),
            _ => {}
        }
        i += 1;
    }
    out
}

/// Every mention in a Markdown file.
pub(crate) fn markdown_file(source: &str) -> Vec<Spelled> {
    let mut out = Vec::new();
    markdown(source, |at| at, Source::Markdown, &mut out);
    out
}

fn comment() -> Context {
    Context {
        source: Source::Comment,
        kind: Kind::Text,
    }
}

/// A doc comment's Markdown with the file offset of each of its lines.
struct Doc {
    text: String,
    /// `(offset in text, offset in file)` of each line start.
    lines: Vec<(usize, usize)>,
}

impl Doc {
    /// The Markdown of consecutive `///` or `//!` comments, each `(offset, len)` in `source`.
    fn from_lines(source: &str, comments: &[(usize, usize)]) -> Self {
        const PREFIX: usize = 3;
        let lines: Vec<(usize, &str)> = comments
            .iter()
            .map(|&(offset, len)| (offset + PREFIX, &source[offset + PREFIX..offset + len]))
            .collect();
        Self::unindent(&lines)
    }

    /// The Markdown of a `/** */` or `/*! */` comment at `offset`, dropping leading `*` decoration.
    fn from_block(source: &str, offset: usize, len: usize) -> Self {
        const OPEN: usize = 3;
        const CLOSE: usize = 2;
        let body_start = offset + OPEN;
        let body = &source[body_start..offset + len - CLOSE];
        let mut lines = Vec::new();
        let mut start = body_start;
        for line in body.split('\n') {
            lines.push((start, line));
            start += line.len() + 1;
        }
        let decorated = lines
            .iter()
            .skip(1)
            .filter(|(_, line)| !line.trim().is_empty())
            .all(|(_, line)| line.trim_start().starts_with('*'));
        if decorated {
            for (start, line) in lines.iter_mut().skip(1) {
                let trimmed = line.trim_start();
                if let Some(rest) = trimmed.strip_prefix('*') {
                    *start += line.len() - rest.len();
                    *line = rest;
                }
            }
        }
        Self::unindent(&lines)
    }

    /// Joins `lines` after removing their common leading whitespace, as rustdoc does.
    fn unindent(lines: &[(usize, &str)]) -> Self {
        let indent = lines
            .iter()
            .filter(|(_, line)| !line.trim().is_empty())
            .map(|(_, line)| line.len() - line.trim_start_matches([' ', '\t']).len())
            .min()
            .unwrap_or(0);
        let mut text = String::new();
        let mut map = Vec::with_capacity(lines.len());
        for (start, line) in lines {
            let cut = indent.min(line.len() - line.trim_start_matches([' ', '\t']).len());
            map.push((text.len(), start + cut));
            text.push_str(&line[cut..]);
            text.push('\n');
        }
        Self { text, lines: map }
    }

    fn file_offset(&self, at: usize) -> usize {
        let line = self.lines.partition_point(|&(start, _)| start <= at) - 1;
        let (doc_start, file_start) = self.lines[line];
        file_start + (at - doc_start)
    }
}

/// Keeps an undefined link label as its destination, so intra-doc links reach the event stream.
#[expect(
    clippy::unnecessary_wraps,
    reason = "pulldown-cmark's broken link callback returns an Option"
)]
fn keep_label(link: BrokenLink<'_>) -> Option<(CowStr<'_>, CowStr<'_>)> {
    Some((link.reference, CowStr::Borrowed("")))
}

/// Mentions in Markdown `text`, whose offsets `map` turns into file offsets.
fn markdown(text: &str, map: impl Fn(usize) -> usize, source: Source, out: &mut Vec<Spelled>) {
    let in_context = |kind| Context { source, kind };
    let mut events =
        Parser::new_with_broken_link_callback(text, Options::empty(), Some(keep_label))
            .into_offset_iter();
    let mut code_block = None;
    let mut in_label_link = false;
    let mut found = Vec::new();
    for (event, range) in events.by_ref() {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => code_block = Some(compiled(&kind, source)),
            Event::End(TagEnd::CodeBlock) => code_block = None,
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                ..
            }) => match link_type {
                LinkType::Shortcut
                | LinkType::ShortcutUnknown
                | LinkType::Collapsed
                | LinkType::CollapsedUnknown => {
                    let raw = &text[range.clone()];
                    let label_end = raw.find(']').unwrap_or(raw.len());
                    let label = &raw[1.min(label_end)..label_end];
                    let content = label.trim_matches('`');
                    let at = range.start + 1 + label.find(content).unwrap_or(0);
                    if let Some(path) = link_path(content, at, in_context(Kind::Link)) {
                        found.push(path);
                        in_label_link = true;
                    }
                }
                LinkType::Inline | LinkType::ReferenceUnknown | LinkType::Autolink => {
                    let raw = &text[range.clone()];
                    if let Some(at) = raw.rfind(&*dest_url) {
                        found.extend(link_path(
                            &dest_url,
                            range.start + at,
                            in_context(Kind::Link),
                        ));
                    }
                }
                _ => {}
            },
            Event::End(TagEnd::Link) => in_label_link = false,
            Event::Code(_) if !in_label_link => {
                let raw = &text[range.clone()];
                let inner = raw.trim_start_matches('`');
                let at = range.start + raw.len() - inner.len();
                found.extend(paths(
                    inner.trim_end_matches('`'),
                    at,
                    in_context(Kind::CodeSpan),
                ));
            }
            Event::Text(_) if !in_label_link => {
                let kind = code_block.map_or(Kind::Text, |compiled| Kind::CodeBlock { compiled });
                found.extend(paths(&text[range.clone()], range.start, in_context(kind)));
            }
            _ => {}
        }
    }
    for (_, definition) in events.reference_definitions().iter() {
        let raw = &text[definition.span.clone()];
        if let Some(at) = raw.rfind(&*definition.dest) {
            found.extend(link_path(
                &definition.dest,
                definition.span.start + at,
                in_context(Kind::Link),
            ));
        }
    }
    for mut path in found {
        path.start = map(path.start);
        path.end = map(path.end);
        out.push(path);
    }
}

/// Whether rustdoc compiles a code block of `kind` found in `source`.
fn compiled(kind: &CodeBlockKind<'_>, source: Source) -> bool {
    if source != Source::DocComment {
        return false;
    }
    let CodeBlockKind::Fenced(info) = kind else {
        return true;
    };
    without_comments(info)
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|token| !token.is_empty())
        .all(|token| {
            matches!(
                token,
                "rust"
                    | "should_panic"
                    | "no_run"
                    | "compile_fail"
                    | "test_harness"
                    | "standalone_crate"
            ) || token.starts_with("edition")
                || token.starts_with('{')
                || (token.starts_with('E') && token[1..].bytes().all(|b| b.is_ascii_digit()))
        })
}

/// A fence info line without its `(…)` comments, which rustdoc skips.
fn without_comments(info: &str) -> String {
    let mut out = String::with_capacity(info.len());
    let mut inside = false;
    for c in info.chars() {
        match c {
            '(' => inside = true,
            ')' => inside = false,
            _ if !inside => out.push(c),
            _ => {}
        }
    }
    out
}

/// The path a link destination or intra-doc label names, located at `at`.
fn link_path(dest: &str, at: usize, context: Context) -> Option<Spelled> {
    let end = at + dest.len();
    if dest.contains("://") || dest.contains(".html") || dest.starts_with(['#', '/', '.']) {
        let (segments, item) = page_path(dest)?;
        return Some(Spelled {
            start: at,
            end,
            segments,
            leading_colon: false,
            url_item: Some(item),
            context,
        });
    }
    let unprefixed = match dest.split_once('@') {
        Some((kind, rest)) if kind.bytes().all(|b| b.is_ascii_lowercase()) => rest,
        _ => dest,
    };
    let bare = unprefixed
        .trim_end_matches("()")
        .trim_end_matches(['!', '(', ')', '[', ']', '{', '}']);
    let path = paths(bare, 0, context).into_iter().next()?;
    if path.start != 0 || path.end != bare.len() {
        return None;
    }
    Some(Spelled {
        start: at,
        end,
        ..path
    })
}

/// The segments a rustdoc page URL names and the index where the item part starts.
fn page_path(dest: &str) -> Option<(Vec<String>, usize)> {
    let dest = dest.split('?').next().unwrap_or(dest);
    let (mut path, anchor) = match dest.split_once('#') {
        Some((path, anchor)) => (path, Some(anchor)),
        None => (dest, None),
    };
    let mut skip = 0;
    if let Some((_, rest)) = path.split_once("://") {
        let (host, rest) = rest.split_once('/').unwrap_or((rest, ""));
        path = rest;
        if host == "docs.rs" {
            skip = 2;
        }
    }
    let mut components: Vec<&str> = path
        .split('/')
        .filter(|c| !c.is_empty() && *c != "." && *c != "..")
        .skip(skip)
        .collect();
    let file = components.pop()?;
    let mut item = Vec::new();
    if file != "index.html" {
        let (kind, name) = file.strip_suffix(".html")?.split_once('.')?;
        if !PAGE_KINDS.contains(&kind) || !is_ident(name) {
            return None;
        }
        item.push(name.to_owned());
    }
    if let Some((kind, name)) = anchor.and_then(|a| a.split_once('.'))
        && MEMBER_KINDS.contains(&kind)
        && is_ident(name)
    {
        item.push(name.to_owned());
    }
    let mut segments: Vec<String> = components
        .into_iter()
        .filter(|c| is_ident(c))
        .map(str::to_owned)
        .collect();
    let start = segments.len();
    segments.extend(item);
    (!segments.is_empty()).then_some((segments, start))
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
                    url_item: None,
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
