//! Path-shaped mentions in comments, doc comments, string literals and Markdown.

use std::collections::HashSet;

use pulldown_cmark::{
    BrokenLink, CodeBlockKind, CowStr, Event, LinkType, Options, Parser, Tag, TagEnd,
};
use ra_ap_rustc_lexer::{
    FrontmatterAllowed, LiteralKind, TokenKind, is_id_continue, is_id_start, is_ident,
};

use crate::{Context, Kind, Source};

/// What a mention names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Named {
    /// An item path, in [`Spelled::segments`].
    Path,
    /// A crate by package or dependency name.
    Crate(String),
    /// A feature, of `package` when the mention says which.
    Feature {
        package: Option<String>,
        name: String,
    },
    /// A binary target.
    Binary(String),
    /// An example target.
    Example(String),
    /// A name written alone, an identifier or a hyphenated word.
    Name {
        word: String,
        /// Whether the word sits in a data example, where only serialized spellings count.
        data: bool,
    },
}

/// A path or manifest name spelled in text, before resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Spelled {
    /// Byte offset of the first character in the file.
    pub(crate) start: usize,
    /// Byte offset one past the last character in the file.
    pub(crate) end: usize,
    pub(crate) named: Named,
    /// The segments of a path without generic arguments or `r#`, empty for other names.
    pub(crate) segments: Vec<String>,
    /// Whether the path starts with `::`.
    pub(crate) leading_colon: bool,
    /// For a rustdoc page link, the index of the first segment naming the item rather than a
    /// directory of the URL.
    pub(crate) url_item: Option<usize>,
    pub(crate) context: Context,
}

impl Spelled {
    fn name(start: usize, end: usize, named: Named, context: Context) -> Self {
        Self {
            start,
            end,
            named,
            segments: Vec::new(),
            leading_colon: false,
            url_item: None,
            context,
        }
    }
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
/// literals. Plain comments holding nothing but Rust code are skipped unless
/// `scan_commented_code`.
pub(crate) fn rust(source: &str, scan_commented_code: bool) -> Vec<Spelled> {
    let skip = ra_ap_rustc_lexer::strip_shebang(source).unwrap_or(0);
    let mut tokens = Vec::new();
    let mut offset = skip;
    for token in ra_ap_rustc_lexer::tokenize(&source[skip..], FrontmatterAllowed::Yes) {
        let len = usize::try_from(token.len).expect("token lengths fit in usize");
        tokens.push((offset, len, token.kind));
        offset += len;
    }

    let mut out = Vec::new();
    let mut attribute = Attribute::default();
    let mut i = 0;
    while i < tokens.len() {
        let (offset, len, kind) = tokens[i];
        attribute.step(kind);
        let text = &source[offset..offset + len];
        match kind {
            TokenKind::LineComment {
                doc_style: Some(style),
            } => {
                let mut lines = vec![(offset, len)];
                for (next, &(o, l, k)) in tokens.iter().enumerate().skip(i + 1) {
                    match k {
                        TokenKind::Whitespace => {}
                        TokenKind::LineComment { doc_style: Some(s) } if s == style => {
                            lines.push((o, l));
                            i = next;
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
            TokenKind::LineComment { doc_style: None } => {
                let mut lines = vec![(offset, len)];
                while let (Some(&(gap, gap_len, TokenKind::Whitespace)), Some(&(o, l, next))) =
                    (tokens.get(i + 1), tokens.get(i + 2))
                    && next == (TokenKind::LineComment { doc_style: None })
                    && source[gap..gap + gap_len].matches('\n').count() == 1
                {
                    lines.push((o, l));
                    i += 2;
                }
                let body: Vec<&str> = lines.iter().map(|&(o, l)| &source[o + 2..o + l]).collect();
                if scan_commented_code || !is_code(&body.join("\n")) {
                    for (o, l) in lines {
                        scan(&source[o..o + l], o, comment(), &mut out);
                    }
                }
            }
            TokenKind::BlockComment { .. } => {
                let body = text.strip_prefix("/*").unwrap_or(text);
                let body = body.strip_suffix("*/").unwrap_or(body);
                if scan_commented_code || !is_code(body) {
                    scan(text, offset, comment(), &mut out);
                }
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
            } => {
                let context = Context {
                    source: Source::StringLiteral,
                    kind: Kind::Text,
                };
                // A string inside an attribute is a macro argument, whose words are no names.
                if attribute.inside() {
                    spelled(text, offset, context, &mut out);
                } else {
                    scan(text, offset, context, &mut out);
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// Whether comment `body` is Rust code, items or the statements of a block, rather than prose.
fn is_code(body: &str) -> bool {
    if let Ok(file) = syn::parse_str::<syn::File>(body) {
        return !file.items.is_empty();
    }
    syn::parse_str::<syn::Block>(&format!("{{{body}\n}}"))
        .is_ok_and(|block| !block.stmts.is_empty())
}

/// Whether the tokens read so far sit inside an attribute, `#[...]` or `#![...]`.
#[derive(Default)]
struct Attribute {
    /// The brackets open inside the attribute.
    depth: usize,
    /// Whether a `#` that may open an attribute was just read.
    pound: bool,
}

impl Attribute {
    fn step(&mut self, kind: TokenKind) {
        match kind {
            TokenKind::Pound => self.pound = true,
            TokenKind::Bang | TokenKind::Whitespace => {}
            TokenKind::OpenBracket => {
                if self.pound || self.depth > 0 {
                    self.depth += 1;
                }
                self.pound = false;
            }
            TokenKind::CloseBracket => {
                self.depth = self.depth.saturating_sub(1);
                self.pound = false;
            }
            _ => self.pound = false,
        }
    }

    fn inside(&self) -> bool {
        self.depth > 0
    }
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
    // pulldown-cmark panics on some text (https://github.com/pulldown-cmark/pulldown-cmark/issues/1129).
    let Ok(found) = std::panic::catch_unwind(|| markdown_mentions(text, source)) else {
        return;
    };
    for mut path in found {
        path.start = map(path.start);
        path.end = map(path.end);
        out.push(path);
    }
}

/// Mentions in Markdown `text`, at offsets into `text`.
fn markdown_mentions(text: &str, source: Source) -> Vec<Spelled> {
    let in_context = |kind| Context { source, kind };
    let mut events =
        Parser::new_with_broken_link_callback(text, Options::empty(), Some(keep_label))
            .into_offset_iter();
    let mut block: Option<Block> = None;
    let mut in_label_link = false;
    let mut found = Vec::new();
    for (event, range) in events.by_ref() {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => block = Some(Block::new(&kind, source)),
            Event::End(TagEnd::CodeBlock) => {
                if let Some(block) = block.take() {
                    block.finish(text, source, &mut found);
                }
            }
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                ..
            }) => {
                in_label_link = link_start(
                    &text[range.clone()],
                    range.start,
                    link_type,
                    &dest_url,
                    in_context(Kind::Link),
                    &mut found,
                );
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                let raw = &text[range.clone()];
                destination(
                    raw,
                    range.start,
                    &dest_url,
                    in_context(Kind::Link),
                    &mut found,
                );
            }
            Event::End(TagEnd::Link) => in_label_link = false,
            Event::Code(_) if !in_label_link => {
                let raw = &text[range.clone()];
                let inner = raw.trim_start_matches('`');
                let at = range.start + raw.len() - inner.len();
                scan(
                    inner.trim_end_matches('`'),
                    at,
                    in_context(Kind::CodeSpan),
                    &mut found,
                );
            }
            // Label and autolink text repeats its link at the same offset, which grouping drops.
            Event::Text(_) => {
                let kind = match &mut block {
                    Some(block) => {
                        block.read(range.clone());
                        Kind::CodeBlock {
                            compiled: block.compiled,
                        }
                    }
                    None => Kind::Text,
                };
                scan(
                    &text[range.clone()],
                    range.start,
                    in_context(kind),
                    &mut found,
                );
            }
            _ => {}
        }
    }
    for (_, def) in events.reference_definitions().iter() {
        let raw = &text[def.span.clone()];
        destination(
            raw,
            def.span.start,
            &def.dest,
            in_context(Kind::Link),
            &mut found,
        );
    }
    found
}

/// Fence languages of data formats whose keys and values serde may spell.
const DATA_FORMATS: [&str; 5] = ["json", "yaml", "yml", "toml", "ron"];

/// A code block being read.
struct Block {
    compiled: bool,
    /// Whether the fence names `toml`, whose dependency tables count.
    toml: bool,
    /// Whether the fence names one of [`DATA_FORMATS`].
    data: bool,
    /// The offsets of the block's text in the Markdown.
    span: Option<(usize, usize)>,
}

impl Block {
    fn new(kind: &CodeBlockKind<'_>, source: Source) -> Self {
        let language = match kind {
            CodeBlockKind::Fenced(info) => info.split([',', ' ']).next().unwrap_or_default(),
            CodeBlockKind::Indented => "",
        };
        Self {
            compiled: compiled(kind, source),
            toml: language == "toml",
            data: DATA_FORMATS.contains(&language),
            span: None,
        }
    }

    fn read(&mut self, range: std::ops::Range<usize>) {
        let start = self.span.map_or(range.start, |(start, _)| start);
        self.span = Some((start, range.end));
    }

    /// Records the dependency tables, serialized spellings and code identifiers of a finished
    /// block in Markdown `text` from `source`.
    fn finish(self, text: &str, source: Source, out: &mut Vec<Spelled>) {
        let Some((start, end)) = self.span else {
            return;
        };
        let block = &text[start..end];
        let kind = Kind::CodeBlock {
            compiled: self.compiled,
        };
        let context = Context { source, kind };
        if self.toml {
            toml(block, start, context, out);
        }
        if self.data {
            for (from, to) in runs(block, true) {
                if !block[from..].starts_with(|c: char| c.is_ascii_digit()) {
                    out.push(name(block, start, (from, to), true, context));
                }
            }
        }
        if self.compiled {
            code_names(block, start, context, out);
        }
    }
}

/// The identifier tokens of compiled `code` at `base`, outside its paths and other than the
/// names it binds itself.
fn code_names(code: &str, base: usize, context: Context, out: &mut Vec<Spelled>) {
    let mut tokens = Vec::new();
    let mut offset = 0;
    for token in ra_ap_rustc_lexer::tokenize(code, FrontmatterAllowed::No) {
        let len = usize::try_from(token.len).expect("token lengths fit in usize");
        let trivia = matches!(
            token.kind,
            TokenKind::Whitespace | TokenKind::LineComment { .. } | TokenKind::BlockComment { .. }
        );
        if !trivia {
            tokens.push((offset, offset + len, token.kind));
        }
        offset += len;
    }
    let bound = bindings(code, &tokens);
    let taken: Vec<(usize, usize)> = paths(code, 0, context)
        .iter()
        .map(|path| (path.start, path.end))
        .collect();
    for &(start, end, kind) in &tokens {
        let span = match kind {
            TokenKind::Ident => (start, end),
            TokenKind::RawIdent => (start + 2, end),
            _ => continue,
        };
        let free = taken.iter().all(|&(s, e)| span.1 <= s || e <= span.0);
        if free && !bound.contains(&code[span.0..span.1]) {
            out.push(name(code, base, span, false, context));
        }
    }
}

/// The lowercase names of `let` and `for` patterns and of closure and declared `fn` parameters
/// in `code`, and every name and generic parameter its item declarations introduce, from its
/// non-trivia `tokens` as `(start, end, kind)`.
fn bindings<'c>(code: &'c str, tokens: &[(usize, usize, TokenKind)]) -> HashSet<&'c str> {
    let text = |i: usize| &code[tokens[i].0..tokens[i].1];
    let mut bound = HashSet::new();
    let mut i = 0;
    while i < tokens.len() {
        let opens_closure = tokens[i].2 == TokenKind::Or
            && (i == 0
                || matches!(
                    tokens[i - 1].2,
                    TokenKind::OpenParen
                        | TokenKind::Comma
                        | TokenKind::Eq
                        | TokenKind::OpenBrace
                        | TokenKind::Semi
                )
                || text(i - 1) == "move");
        let pattern = match (tokens[i].2, text(i)) {
            (TokenKind::Ident, "let") => Pattern::Let,
            (TokenKind::Ident, "for") => Pattern::For,
            _ if opens_closure => Pattern::Closure,
            (TokenKind::Ident, keyword) => {
                declaration(code, tokens, i, keyword, &mut bound);
                i += 1;
                continue;
            }
            _ => {
                i += 1;
                continue;
            }
        };
        let mut depth = 0usize;
        i += 1;
        while i < tokens.len() {
            let kind = tokens[i].2;
            match kind {
                TokenKind::OpenParen | TokenKind::OpenBracket | TokenKind::OpenBrace => depth += 1,
                TokenKind::CloseParen | TokenKind::CloseBracket | TokenKind::CloseBrace => {
                    let Some(inner) = depth.checked_sub(1) else {
                        break;
                    };
                    depth = inner;
                }
                _ if depth == 0 && pattern.ends(kind, text(i)) => break,
                TokenKind::Ident if binds(text(i)) => {
                    bound.insert(text(i));
                }
                _ => {}
            }
            i += 1;
        }
        i += 1;
    }
    bound
}

/// Whether pattern identifier `name` binds a variable, as lowercase names do apart from `mut`
/// and `ref`.
fn binds(name: &str) -> bool {
    name.starts_with(|c: char| c.is_lowercase() || c == '_') && !matches!(name, "mut" | "ref")
}

/// Adds to `bound` the name, the generic parameters and, for `fn`, the parameter names of the
/// item declaration opened by `keyword` at token `at`.
fn declaration<'c>(
    code: &'c str,
    tokens: &[(usize, usize, TokenKind)],
    at: usize,
    keyword: &str,
    bound: &mut HashSet<&'c str>,
) {
    let text = |i: usize| &code[tokens[i].0..tokens[i].1];
    let kind = |i: usize| tokens.get(i).map(|token| token.2);
    let mut i = at + 1;
    match keyword {
        "fn" | "struct" | "enum" | "union" | "trait" | "type" | "mod" | "const" => {}
        "static" => {
            while kind(i) == Some(TokenKind::Ident) && matches!(text(i), "mut" | "ref") {
                i += 1;
            }
        }
        "macro_rules" if kind(i) == Some(TokenKind::Bang) => i += 1,
        _ => return,
    }
    match kind(i) {
        Some(TokenKind::Ident) => bound.insert(text(i)),
        Some(TokenKind::RawIdent) => bound.insert(&text(i)[2..]),
        _ => return,
    };
    i += 1;
    if kind(i) == Some(TokenKind::Lt) {
        let mut depth = 0usize;
        let mut opens_parameter = true;
        while let Some(token) = kind(i) {
            match token {
                TokenKind::Lt => depth += 1,
                TokenKind::Gt
                    if tokens[i - 1].2 == TokenKind::Minus && tokens[i - 1].1 == tokens[i].0 => {}
                TokenKind::Gt => depth -= 1,
                TokenKind::Comma => opens_parameter = true,
                TokenKind::Ident if depth == 1 && opens_parameter => {
                    if text(i) != "const" {
                        bound.insert(text(i));
                        opens_parameter = false;
                    }
                }
                _ => opens_parameter = false,
            }
            i += 1;
            if depth == 0 {
                break;
            }
        }
    }
    if keyword != "fn" || kind(i) != Some(TokenKind::OpenParen) {
        return;
    }
    let mut depth = 0usize;
    let mut in_pattern = true;
    while let Some(token) = kind(i) {
        match token {
            TokenKind::OpenParen | TokenKind::OpenBracket | TokenKind::OpenBrace => depth += 1,
            TokenKind::CloseParen | TokenKind::CloseBracket | TokenKind::CloseBrace => {
                depth -= 1;
                if depth == 0 {
                    return;
                }
            }
            TokenKind::Comma if depth == 1 => in_pattern = true,
            TokenKind::Colon if depth == 1 => in_pattern = false,
            TokenKind::Ident if in_pattern && binds(text(i)) => {
                bound.insert(text(i));
            }
            _ => {}
        }
        i += 1;
    }
}

/// A binding pattern of compiled code, by the keyword or token that opens it.
#[derive(Clone, Copy)]
enum Pattern {
    Let,
    For,
    Closure,
}

impl Pattern {
    /// Whether the token of `kind` spelled `text`, outside any brackets, ends the pattern.
    fn ends(self, kind: TokenKind, text: &str) -> bool {
        match self {
            Self::Let => matches!(kind, TokenKind::Eq | TokenKind::Semi | TokenKind::Colon),
            Self::For => text == "in",
            Self::Closure => kind == TokenKind::Or,
        }
    }
}

/// Records what destination `dest`, last written in `raw` at `start`, names.
fn destination(raw: &str, start: usize, dest: &str, context: Context, found: &mut Vec<Spelled>) {
    if let Some(at) = raw.rfind(dest) {
        found.extend(link_path(dest, start + at, context));
    }
}

/// Records what the link spanning `raw` at `start` names. Returns whether the link is an
/// intra-doc label whose text must not be scanned again.
fn link_start(
    raw: &str,
    start: usize,
    link_type: LinkType,
    dest: &str,
    context: Context,
    found: &mut Vec<Spelled>,
) -> bool {
    match link_type {
        LinkType::Shortcut
        | LinkType::ShortcutUnknown
        | LinkType::Collapsed
        | LinkType::CollapsedUnknown => {
            let label_end = raw.find(']').unwrap_or(raw.len());
            let label = &raw[1.min(label_end)..label_end];
            let written = label.trim_matches('`');
            let at = start + 1 + label.find(written).unwrap_or(0);
            let path = link_path(written, at, context);
            let label_link = path.is_some();
            found.extend(path);
            label_link
        }
        LinkType::Inline | LinkType::ReferenceUnknown | LinkType::Autolink => {
            destination(raw, start, dest, context, found);
            false
        }
        _ => false,
    }
}

/// Paths, cargo flags, `cfg(feature)` predicates, URLs and bare names in plain `text` at `base`.
fn scan(text: &str, base: usize, context: Context, out: &mut Vec<Spelled>) {
    let taken = spelled(text, base, context, out);
    names(text, base, context, &taken, out);
}

/// Paths, cargo flags, `cfg(feature)` predicates and URLs in plain `text` at `base`. Returns the
/// ranges of `text` they cover, with every URL and, in a string literal, every `{...}` format
/// argument.
fn spelled(
    text: &str,
    base: usize,
    context: Context,
    out: &mut Vec<Spelled>,
) -> Vec<(usize, usize)> {
    let first = out.len();
    out.extend(paths(text, base, context));
    cargo_flags(text, base, context, out);
    cargo_add(text, base, context, out);
    cfg_features(text, base, context, out);
    bare_urls(text, base, context, out);
    let mut taken: Vec<(usize, usize)> = out[first..]
        .iter()
        .map(|spelled| (spelled.start - base, spelled.end - base))
        .collect();
    taken.extend(urls(text));
    if context.source == Source::StringLiteral {
        for (open, _) in text.match_indices('{') {
            if let Some(close) = text[open..].find('}') {
                taken.push((open, open + close + 1));
            }
        }
    }
    taken
}

/// Bare names in `text` at `base` outside the `taken` ranges of `text`. Those are the
/// code-shaped words of running text and a code span holding one identifier or hyphenated
/// word. Compiled code blocks yield theirs once whole, in [`Block::finish`].
fn names(
    text: &str,
    base: usize,
    context: Context,
    taken: &[(usize, usize)],
    out: &mut Vec<Spelled>,
) {
    let free = |(start, end): (usize, usize)| taken.iter().all(|&(s, e)| end <= s || e <= start);
    let named = |word: &str| word.contains('-') || word.starts_with(is_id_start);
    match context.kind {
        Kind::Text => {
            for span in runs(text, true) {
                let word = &text[span.0..span.1];
                if free(span) && code_shaped(word) && standalone(text, span.0, span.1) {
                    out.push(name(text, base, span, false, context));
                }
            }
        }
        Kind::CodeSpan => {
            let trimmed = text.trim_start();
            let whole = trimmed
                .trim_end()
                .trim_end_matches("()")
                .trim_end_matches('!');
            let from = text.len() - trimmed.len();
            let span = (from, from + whole.len());
            if runs(whole, true) == [(0, whole.len())] && named(whole) && free(span) {
                out.push(name(text, base, span, false, context));
            }
        }
        Kind::CodeBlock { .. } | Kind::Link => {}
    }
}

/// Whether `word` of running text looks like code rather than prose. It has a `_` or a `-`, or
/// an uppercase letter after its first character together with a lowercase letter.
fn code_shaped(word: &str) -> bool {
    if !word.starts_with(is_id_start) && !word.contains('-') {
        return false;
    }
    let inner_upper = word.chars().skip(1).any(char::is_uppercase);
    word.contains(['_', '-']) || (inner_upper && word.chars().any(char::is_lowercase))
}

/// The bare name at `span` of `text`, which sits at `base`.
fn name(text: &str, base: usize, span: (usize, usize), data: bool, context: Context) -> Spelled {
    let word = text[span.0..span.1].to_owned();
    Spelled::name(
        base + span.0,
        base + span.1,
        Named::Name { word, data },
        context,
    )
}

/// The maximal runs of identifier characters in `text`, joined by inner `-` when `hyphens`.
fn runs(text: &str, hyphens: bool) -> Vec<(usize, usize)> {
    let inside = |c: char| is_id_continue(c) || (hyphens && c == '-');
    let mut out = Vec::new();
    let mut start = None;
    for (at, c) in text
        .char_indices()
        .chain(std::iter::once((text.len(), ' ')))
    {
        match (start, inside(c)) {
            (None, true) => start = Some(at),
            (Some(from), false) => {
                start = None;
                let run = &text[from..at];
                let trimmed = run.trim_start_matches('-');
                let from = from + run.len() - trimmed.len();
                let to = from + trimmed.trim_end_matches('-').len();
                if from < to {
                    out.push((from, to));
                }
            }
            _ => {}
        }
    }
    out
}

/// Whether the word at `start..end` of running text stands alone rather than being part of a
/// file name, an address, an email or a path.
fn standalone(text: &str, start: usize, end: usize) -> bool {
    let before = text[..start].chars().next_back();
    let mut after = text[end..].chars();
    let (next, then) = (after.next(), after.next());
    let glued_before = matches!(
        before,
        Some('.' | ':' | '/' | '\\' | '@' | '#' | '$' | '%' | '&' | '=' | '~')
    );
    let glued_after = match next {
        Some('/' | '\\' | '@') => true,
        Some('.') => then.is_some_and(is_id_continue),
        Some(':') => then == Some(':'),
        _ => false,
    };
    !glued_before && !glued_after
}

/// `--features`, `-F`, `--bin`, `--example`, `-p` and `--package` arguments.
fn cargo_flags(text: &str, base: usize, context: Context, out: &mut Vec<Spelled>) {
    const FLAGS: [&str; 6] = ["--features", "-F", "--bin", "--example", "-p", "--package"];
    for flag in FLAGS {
        for (at, _) in text.match_indices(flag) {
            let before = text[..at].chars().next_back();
            if before.is_some_and(|c| !c.is_whitespace() && !matches!(c, '`' | '"' | '\'' | '(')) {
                continue;
            }
            let mut cursor = at + flag.len();
            let rest = &text[cursor..];
            let separators = rest.len() - rest.trim_start_matches([' ', '\t', '=']).len();
            if separators == 0 {
                continue;
            }
            cursor += separators;
            let (value, value_at) =
                if let Some(quote @ ('"' | '\'')) = text[cursor..].chars().next() {
                    let body = &text[cursor + 1..];
                    let len = body.find([quote, '\n']).unwrap_or(body.len());
                    (&body[..len], cursor + 1)
                } else {
                    let body = &text[cursor..];
                    let len = body
                        .find(|c: char| c.is_whitespace() || "`\"')|;&<>".contains(c))
                        .unwrap_or(body.len());
                    (&body[..len], cursor)
                };
            for (offset, part) in split_list(value) {
                let start = base + value_at + offset;
                let end = start + part.len();
                let named = match flag {
                    "--bin" => Named::Binary(part.to_owned()),
                    "--example" => Named::Example(part.to_owned()),
                    "-p" | "--package" => Named::Crate(part.to_owned()),
                    _ => match part.split_once('/') {
                        Some((package, name)) => Named::Feature {
                            package: Some(package.trim_end_matches('?').to_owned()),
                            name: name.to_owned(),
                        },
                        None => Named::Feature {
                            package: None,
                            name: part.to_owned(),
                        },
                    },
                };
                let start = match &named {
                    Named::Feature { name, .. } => end - name.len(),
                    _ => start,
                };
                out.push(Spelled::name(start, end, named, context));
            }
        }
    }
}

/// The crates a `cargo add` command names, each word of crate name characters up to the first
/// that is none, such as a flag or punctuation, without its `@version` suffix.
fn cargo_add(text: &str, base: usize, context: Context, out: &mut Vec<Spelled>) {
    const COMMAND: &str = "cargo add";
    let is_name = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_');
    for (at, _) in text.match_indices(COMMAND) {
        let mut cursor = at + COMMAND.len();
        loop {
            let rest = &text[cursor..];
            let word = rest.trim_start_matches([' ', '\t']);
            let len = word.find(|c: char| !is_name(c)).unwrap_or(word.len());
            if word.len() == rest.len() || len == 0 || word.starts_with('-') {
                break;
            }
            let word_at = text.len() - word.len();
            let named = Named::Crate(word[..len].to_owned());
            out.push(Spelled::name(
                base + word_at,
                base + word_at + len,
                named,
                context,
            ));
            let version = word[len..]
                .strip_prefix('@')
                .map_or(0, |v| 1 + v.find(char::is_whitespace).unwrap_or(v.len()));
            cursor = word_at + len + version;
        }
    }
}

/// The non-empty items of a comma or space separated list with their offsets.
fn split_list(value: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in value
        .char_indices()
        .chain(std::iter::once((value.len(), ',')))
    {
        if c == ',' || c.is_whitespace() {
            if i > start {
                out.push((start, &value[start..i]));
            }
            start = i + c.len_utf8();
        }
    }
    out
}

/// `feature = "name"` predicates inside the parentheses of a `cfg`, `cfg!` or `cfg_attr`.
fn cfg_features(text: &str, base: usize, context: Context, out: &mut Vec<Spelled>) {
    for (at, _) in text.match_indices("feature") {
        if !inside_cfg(&text[..at]) {
            continue;
        }
        let rest = &text[at + "feature".len()..];
        let after_eq = rest.trim_start();
        let Some(after_eq) = after_eq.strip_prefix('=') else {
            continue;
        };
        let value = after_eq.trim_start();
        let Some(body) = value.strip_prefix('"') else {
            continue;
        };
        let Some(len) = body.find('"') else {
            continue;
        };
        let start = base + text.len() - body.len();
        out.push(Spelled::name(
            start,
            start + len,
            Named::Feature {
                package: None,
                name: body[..len].to_owned(),
            },
            context,
        ));
    }
}

/// Whether `before` ends inside the still open parentheses of its last `cfg`, `cfg!` or
/// `cfg_attr`, however many lines they span.
fn inside_cfg(before: &str) -> bool {
    let Some(start) = before.rfind("cfg") else {
        return false;
    };
    let rest = &before[start + "cfg".len()..];
    let Some(open) = rest.find('(') else {
        return false;
    };
    if !matches!(rest[..open].trim(), "" | "!" | "_attr") {
        return false;
    }
    let mut depth = 0_usize;
    for c in rest[open..].chars() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    depth > 0
}

/// `http://` and `https://` URLs in running text, read as crate links or rustdoc pages.
fn bare_urls(text: &str, base: usize, context: Context, out: &mut Vec<Spelled>) {
    for (start, end) in urls(text) {
        out.extend(link_path(&text[start..end], base + start, context));
    }
}

/// The spans of the `http://` and `https://` URLs in `text`.
fn urls(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for scheme in ["https://", "http://"] {
        for (at, _) in text.match_indices(scheme) {
            let body = &text[at..];
            let len = body
                .find(|c: char| c.is_whitespace() || "<>()[]\"'`".contains(c))
                .unwrap_or(body.len());
            let url = body[..len].trim_end_matches(['.', ',', ';', ':', '!', '?']);
            out.push((at, at + url.len()));
        }
    }
    out
}

/// Dependency keys and their requested features in a `toml` code block at `base`.
fn toml(block: &str, base: usize, context: Context, out: &mut Vec<Spelled>) {
    const KINDS: [&str; 5] = [
        "dependencies",
        "dev-dependencies",
        "dev_dependencies",
        "build-dependencies",
        "build_dependencies",
    ];
    let mut table: Option<Option<String>> = None;
    let mut line_start = 0;
    for line in block.split_inclusive('\n') {
        let at = line_start;
        line_start += line.len();
        let trimmed = line.trim_start();
        let indent = at + line.len() - trimmed.len();
        if trimmed.starts_with('[') {
            let header = trimmed.trim_start_matches('[');
            let inner_start = indent + trimmed.len() - header.len();
            let inner = &header[..header.find(']').unwrap_or(header.len())];
            let keys = dotted_keys(inner);
            table = match keys.as_slice() {
                [.., (_, last)] if KINDS.contains(&last.as_str()) => Some(None),
                [.., (_, kind), (offset, key)] if KINDS.contains(&kind.as_str()) => {
                    let start = base + inner_start + offset;
                    out.push(Spelled::name(
                        start,
                        start + key.len(),
                        Named::Crate(key.clone()),
                        context,
                    ));
                    Some(Some(key.clone()))
                }
                _ => None,
            };
            continue;
        }
        match &table {
            Some(None) => {
                let key_len = trimmed
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
                    .unwrap_or(trimmed.len());
                let after = trimmed[key_len..].trim_start();
                if key_len == 0 || !(after.starts_with('=') || after.starts_with('.')) {
                    continue;
                }
                let key = &trimmed[..key_len];
                let start = base + indent;
                out.push(Spelled::name(
                    start,
                    start + key_len,
                    Named::Crate(key.to_owned()),
                    context,
                ));
                requested_features(trimmed, base + indent, key, context, out);
            }
            Some(Some(key)) if trimmed.starts_with("features") => {
                requested_features(trimmed, base + indent, key, context, out);
            }
            _ => {}
        }
    }
}

/// The strings of a `features = [...]` array on `line`, features of `package`.
fn requested_features(
    line: &str,
    base: usize,
    package: &str,
    context: Context,
    out: &mut Vec<Spelled>,
) {
    let Some(features) = line.find("features") else {
        return;
    };
    let Some(open) = line[features..].find('[').map(|i| features + i) else {
        return;
    };
    let close = line[open..].find(']').map_or(line.len(), |i| open + i);
    // Between the brackets, every odd part of a split at `"` is a string, when a quote closes it.
    let mut parts = line[open + 1..close].split('"').enumerate().peekable();
    let mut at = base + open + 1;
    while let Some((index, part)) = parts.next() {
        if index % 2 == 1 && parts.peek().is_some() {
            out.push(Spelled::name(
                at,
                at + part.len(),
                Named::Feature {
                    package: Some(package.to_owned()),
                    name: part.to_owned(),
                },
                context,
            ));
        }
        at += part.len() + 1;
    }
}

/// The keys of a dotted TOML table name with their offsets, quotes removed.
fn dotted_keys(header: &str) -> Vec<(usize, String)> {
    let mut keys = Vec::new();
    let mut start = 0;
    let mut quote = None;
    for (i, c) in header
        .char_indices()
        .chain(std::iter::once((header.len(), '.')))
    {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (None, '"' | '\'') => quote = Some(c),
            (None, '.') => {
                let raw = &header[start..i];
                let trimmed = raw.trim();
                let offset = start + raw.len() - raw.trim_start().len();
                let key = trimmed.trim_matches(['"', '\'']);
                let offset = offset + usize::from(trimmed.len() != key.len());
                keys.push((offset, key.to_owned()));
                start = i + 1;
            }
            _ => {}
        }
    }
    keys
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

/// The path or name a link destination or intra-doc label names, located at `at`.
fn link_path(dest: &str, at: usize, context: Context) -> Option<Spelled> {
    let end = at + dest.len();
    if dest.contains("://") || dest.contains(".html") || dest.starts_with(['#', '/', '.']) {
        let (segments, item) = page_path(dest)?;
        return Some(Spelled {
            start: at,
            end,
            named: Named::Path,
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
    if let Some((word, after)) = ident(bare, 0)
        && after == bare.len()
    {
        let named = Named::Name { word, data: false };
        return Some(Spelled::name(at, end, named, context));
    }
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
    while let Some(c) = text[i..].chars().next() {
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
                    let Some(after_angles) = skip_angles(text, probe) else {
                        break;
                    };
                    probe = after_angles;
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
                    named: Named::Path,
                    segments,
                    leading_colon,
                    url_item: None,
                    context,
                });
            }
            // The path's first identifier ends past `i`.
            i = end;
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

/// A name in a `refquiem: allow(...)` suppression marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MarkerName {
    /// Byte offset of the name in the file.
    pub(crate) start: usize,
    pub(crate) name: String,
}

/// The marker names in the `//` and doc comments of a Rust file, or the HTML comments of a Markdown file.
pub(crate) fn markers(source: &str, rust: bool) -> Vec<MarkerName> {
    let mut out = Vec::new();
    if rust {
        let mut offset = ra_ap_rustc_lexer::strip_shebang(source).unwrap_or(0);
        for token in ra_ap_rustc_lexer::tokenize(&source[offset..], FrontmatterAllowed::Yes) {
            let len = usize::try_from(token.len).expect("token lengths fit in usize");
            let comment = matches!(
                token.kind,
                TokenKind::LineComment { .. }
                    | TokenKind::BlockComment {
                        doc_style: Some(_),
                        ..
                    }
            );
            if comment {
                marker_names(&source[offset..offset + len], offset, &mut out);
            }
            offset += len;
        }
    } else {
        for (open, _) in source.match_indices("<!--") {
            let close = source[open..]
                .find("-->")
                .map_or(source.len(), |i| open + i);
            marker_names(&source[open..close], open, &mut out);
        }
    }
    out
}

/// The names of every `refquiem: allow(...)` in `text`, which sits at `base`, skipping a
/// marker that holds anything but names and paths.
fn marker_names(text: &str, base: usize, out: &mut Vec<MarkerName>) {
    const OPEN: &str = "refquiem: allow(";
    for (at, _) in text.match_indices(OPEN) {
        let inner = at + OPEN.len();
        let Some(len) = text[inner..].find(')') else {
            continue;
        };
        let mut names = Vec::new();
        let mut start = inner;
        for part in text[inner..inner + len].split(',') {
            let name = part.trim();
            if !name.is_empty() {
                let lead = part.len() - part.trim_start().len();
                names.push(MarkerName {
                    start: base + start + lead,
                    name: name.to_owned(),
                });
            }
            start += part.len() + 1;
        }
        if names.iter().all(|marker| is_marker_name(&marker.name)) {
            out.append(&mut names);
        }
    }
}

/// Whether `name` is an item path or a manifest entry name, whose segments may also hold
/// `-`, `+` and `.`.
fn is_marker_name(name: &str) -> bool {
    name.split("::").all(|segment| {
        segment.chars().any(|c| c.is_alphanumeric() || c == '_')
            && segment
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '+' | '.'))
    })
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
