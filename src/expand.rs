//! Expansion of `macro_rules!` invocations at item position, enough to see the items they
//! declare.

use std::rc::Rc;

use proc_macro2::{Delimiter, Group, Ident, Spacing, TokenStream, TokenTree};
use syn::parse::{ParseStream, Parser};

use crate::HashMap;

/// The longest expansion kept, in characters.
const MAX_LEN: usize = 1 << 22;

/// A body's text, kept alive so its address stays unique, with its rules once parsed.
type ParsedBody = (Rc<str>, Option<Rc<[Rule]>>);

/// Expands invocations, parsing each `macro_rules!` body's rules once.
#[derive(Default)]
pub(crate) struct Expander {
    /// Every body seen, by the address of its text.
    rules: HashMap<*const u8, ParsedBody>,
}

impl Expander {
    /// The text `tokens` expand to under the `macro_rules!` rules `body`, with `$crate`
    /// spelled `home`. `None` when no rule matches or the rules use syntax this interpreter
    /// does not know.
    pub(crate) fn expand(&mut self, body: &Rc<str>, tokens: &str, home: &str) -> Option<String> {
        let (_, rules) = self
            .rules
            .entry(body.as_ptr())
            .or_insert_with(|| (Rc::clone(body), body.parse().ok().and_then(rules)));
        let expanded = rules
            .clone()
            .and_then(|rules| expand_tokens(&rules, tokens, home));
        // Span locations keep every parsed text alive in a thread-local map until invalidated.
        proc_macro2::extra::invalidate_current_thread_spans();
        expanded
    }
}

fn expand_tokens(rules: &[Rule], tokens: &str, home: &str) -> Option<String> {
    let input: Vec<TokenTree> = tokens.parse::<TokenStream>().ok()?.into_iter().collect();
    for rule in rules {
        if let Some((_, bindings)) = match_seq(&rule.matcher, &input, true) {
            let text = transcribe(rule.transcriber.clone(), &bindings, &[], home)?.to_string();
            return (text.len() <= MAX_LEN).then_some(text);
        }
    }
    None
}

/// One rule of a definition.
struct Rule {
    matcher: Vec<Matcher>,
    transcriber: TokenStream,
}

/// A piece of a rule's matcher.
enum Matcher {
    /// A token that must appear as written.
    Token(TokenTree),
    Group(Delimiter, Vec<Matcher>),
    Fragment(String, Fragment),
    Repeat {
        inner: Vec<Matcher>,
        /// The tokens between repetitions, empty for none.
        separator: Vec<TokenTree>,
        kleene: Kleene,
    },
}

/// Whether `token` of an invocation is the token `expected` of a matcher, which is no group.
fn same(token: &TokenTree, expected: &TokenTree) -> bool {
    match (token, expected) {
        (TokenTree::Ident(a), TokenTree::Ident(b)) => a == b,
        (TokenTree::Punct(a), TokenTree::Punct(b)) => a.as_char() == b.as_char(),
        (TokenTree::Literal(a), TokenTree::Literal(b)) => a.to_string() == b.to_string(),
        _ => false,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kleene {
    ZeroOrMore,
    OneOrMore,
    ZeroOrOne,
}

#[derive(Clone, Copy)]
enum Fragment {
    Tt,
    Ident,
    Lifetime,
    Literal,
    Path,
    Ty,
    Expr,
    Item,
    Block,
    Stmt,
    Pat,
    PatParam,
    Vis,
    Meta,
}

/// What a matcher variable captured.
#[derive(Clone)]
enum Binding {
    One(Vec<TokenTree>),
    /// One binding per repetition.
    Many(Vec<Binding>),
}

type Bindings = HashMap<String, Binding>;

/// The rules of a `macro_rules!` body, `(matcher) => {transcriber}` separated by `;`.
fn rules(body: TokenStream) -> Option<Rc<[Rule]>> {
    let tokens: Vec<TokenTree> = body.into_iter().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let TokenTree::Group(matcher) = &tokens[i] else {
            return None;
        };
        let arrow = matches!(
            (tokens.get(i + 1), tokens.get(i + 2)),
            (Some(TokenTree::Punct(a)), Some(TokenTree::Punct(b)))
                if a.as_char() == '=' && b.as_char() == '>'
        );
        let Some(TokenTree::Group(transcriber)) = tokens.get(i + 3) else {
            return None;
        };
        if !arrow {
            return None;
        }
        out.push(Rule {
            matcher: matchers(matcher.stream())?,
            transcriber: transcriber.stream(),
        });
        i += 4;
        if matches!(tokens.get(i), Some(TokenTree::Punct(p)) if p.as_char() == ';') {
            i += 1;
        }
    }
    Some(out.into())
}

fn matchers(stream: TokenStream) -> Option<Vec<Matcher>> {
    let tokens: Vec<TokenTree> = stream.into_iter().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            TokenTree::Punct(p) if p.as_char() == '$' => match tokens.get(i + 1) {
                Some(TokenTree::Ident(name)) => {
                    let colon = matches!(tokens.get(i + 2), Some(TokenTree::Punct(c)) if c.as_char() == ':');
                    let Some(TokenTree::Ident(kind)) = tokens.get(i + 3).filter(|_| colon) else {
                        return None;
                    };
                    out.push(Matcher::Fragment(name.to_string(), fragment(kind)?));
                    i += 4;
                }
                Some(TokenTree::Group(group)) if group.delimiter() == Delimiter::Parenthesis => {
                    let (separator, kleene, used) = kleene(&tokens[i + 2..])?;
                    out.push(Matcher::Repeat {
                        inner: matchers(group.stream())?,
                        separator,
                        kleene,
                    });
                    i += 2 + used;
                }
                _ => return None,
            },
            TokenTree::Group(group) => {
                out.push(Matcher::Group(group.delimiter(), matchers(group.stream())?));
                i += 1;
            }
            token => {
                out.push(Matcher::Token(token.clone()));
                i += 1;
            }
        }
    }
    Some(out)
}

fn fragment(kind: &Ident) -> Option<Fragment> {
    Some(match kind.to_string().as_str() {
        "tt" => Fragment::Tt,
        "ident" => Fragment::Ident,
        "lifetime" => Fragment::Lifetime,
        "literal" => Fragment::Literal,
        "path" => Fragment::Path,
        "ty" => Fragment::Ty,
        "expr" | "expr_2021" => Fragment::Expr,
        "item" => Fragment::Item,
        "block" => Fragment::Block,
        "stmt" => Fragment::Stmt,
        "pat" => Fragment::Pat,
        "pat_param" => Fragment::PatParam,
        "vis" => Fragment::Vis,
        "meta" => Fragment::Meta,
        _ => return None,
    })
}

/// The separator and operator after a repetition's group, with the number of tokens they take.
/// A separator is one token or punctuation joined up to the operator, such as `::` or `=>`.
fn kleene(tokens: &[TokenTree]) -> Option<(Vec<TokenTree>, Kleene, usize)> {
    let operator = |token: &TokenTree| match token {
        TokenTree::Punct(p) => match p.as_char() {
            '*' => Some(Kleene::ZeroOrMore),
            '+' => Some(Kleene::OneOrMore),
            '?' => Some(Kleene::ZeroOrOne),
            _ => None,
        },
        _ => None,
    };
    if let Some(kleene) = operator(tokens.first()?) {
        return Some((Vec::new(), kleene, 1));
    }
    let mut end = 1;
    loop {
        let next = tokens.get(end)?;
        if let Some(kleene) = operator(next) {
            return Some((tokens[..end].to_vec(), kleene, end + 1));
        }
        let joined = matches!(&tokens[end - 1], TokenTree::Punct(p) if p.spacing() == Spacing::Joint)
            && matches!(next, TokenTree::Punct(_));
        if !joined {
            return None;
        }
        end += 1;
    }
}

/// Matches `matchers` against a prefix of `tokens`, all of them when `whole`. Returns the
/// number of tokens matched and the bindings.
fn match_seq(matchers: &[Matcher], tokens: &[TokenTree], whole: bool) -> Option<(usize, Bindings)> {
    let Some((first, rest)) = matchers.split_first() else {
        return (!whole || tokens.is_empty()).then(|| (0, Bindings::new()));
    };
    match first {
        Matcher::Token(expected) => {
            if !same(tokens.first()?, expected) {
                return None;
            }
            let (len, bindings) = match_seq(rest, &tokens[1..], whole)?;
            Some((len + 1, bindings))
        }
        Matcher::Group(delimiter, inner) => {
            let Some(TokenTree::Group(group)) = tokens.first() else {
                return None;
            };
            if group.delimiter() != *delimiter {
                return None;
            }
            let inside: Vec<TokenTree> = group.stream().into_iter().collect();
            let (_, mut bindings) = match_seq(inner, &inside, true)?;
            let (len, after) = match_seq(rest, &tokens[1..], whole)?;
            bindings.extend(after);
            Some((len + 1, bindings))
        }
        Matcher::Fragment(name, fragment) => {
            let taken = fragment_len(*fragment, tokens)?;
            let (len, mut bindings) = match_seq(rest, &tokens[taken..], whole)?;
            bindings.insert(name.clone(), Binding::One(tokens[..taken].to_vec()));
            Some((len + taken, bindings))
        }
        Matcher::Repeat {
            inner,
            separator,
            kleene,
        } => {
            let mut ends = vec![0];
            let mut iterations: Vec<Bindings> = Vec::new();
            let mut at = 0;
            while !(*kleene == Kleene::ZeroOrOne && iterations.len() == 1) {
                let mut start = at;
                if !(separator.is_empty() || iterations.is_empty()) {
                    let following = tokens.get(at..at + separator.len());
                    let fits = following.is_some_and(|following| {
                        following
                            .iter()
                            .zip(separator)
                            .all(|(token, expected)| same(token, expected))
                    });
                    if !fits {
                        break;
                    }
                    start += separator.len();
                }
                let Some((len, bindings)) = match_seq(inner, &tokens[start..], false) else {
                    break;
                };
                if len == 0 {
                    break;
                }
                at = start + len;
                iterations.push(bindings);
                ends.push(at);
            }
            let least = usize::from(*kleene == Kleene::OneOrMore);
            for count in (least..=iterations.len()).rev() {
                if let Some((len, mut bindings)) = match_seq(rest, &tokens[ends[count]..], whole) {
                    let mut names = Vec::new();
                    variables(inner, &mut names);
                    for name in names {
                        let each = iterations[..count]
                            .iter()
                            .map(|it| it.get(&name).cloned().unwrap_or(Binding::Many(Vec::new())))
                            .collect();
                        bindings.insert(name, Binding::Many(each));
                    }
                    return Some((ends[count] + len, bindings));
                }
            }
            None
        }
    }
}

/// The variable names `matchers` bind, at any depth.
fn variables(matchers: &[Matcher], out: &mut Vec<String>) {
    for matcher in matchers {
        match matcher {
            Matcher::Token(_) => {}
            Matcher::Group(_, inner) | Matcher::Repeat { inner, .. } => variables(inner, out),
            Matcher::Fragment(name, _) => out.push(name.clone()),
        }
    }
}

/// How many leading `tokens` a fragment of `kind` takes.
fn fragment_len(kind: Fragment, tokens: &[TokenTree]) -> Option<usize> {
    match kind {
        Fragment::Tt => (!tokens.is_empty()).then_some(1),
        Fragment::Ident => {
            matches!(tokens.first(), Some(TokenTree::Ident(i)) if i != "_").then_some(1)
        }
        Fragment::Lifetime => match tokens {
            [TokenTree::Punct(p), TokenTree::Ident(_), ..] if p.as_char() == '\'' => Some(2),
            _ => None,
        },
        Fragment::Literal => parsed_len(tokens, |input| {
            input.parse::<Option<syn::Token![-]>>()?;
            input.parse::<syn::Lit>().map(drop)
        }),
        Fragment::Path => parsed_len(tokens, |input| input.parse::<syn::Path>().map(drop)),
        Fragment::Ty => parsed_len(tokens, |input| input.parse::<syn::Type>().map(drop)),
        Fragment::Expr => parsed_len(tokens, |input| input.parse::<syn::Expr>().map(drop)),
        Fragment::Item => parsed_len(tokens, |input| input.parse::<syn::Item>().map(drop)),
        Fragment::Block => parsed_len(tokens, |input| input.parse::<syn::Block>().map(drop)),
        Fragment::Stmt => parsed_len(tokens, |input| input.parse::<syn::Stmt>().map(drop)),
        Fragment::Pat => parsed_len(tokens, |input| {
            syn::Pat::parse_multi_with_leading_vert(input).map(drop)
        }),
        Fragment::PatParam => parsed_len(tokens, |input| syn::Pat::parse_single(input).map(drop)),
        Fragment::Vis => parsed_len(tokens, |input| input.parse::<syn::Visibility>().map(drop)),
        Fragment::Meta => parsed_len(tokens, |input| input.parse::<syn::Meta>().map(drop)),
    }
}

/// How many leading `tokens` `parse` takes.
fn parsed_len(
    tokens: &[TokenTree],
    parse: impl FnOnce(ParseStream<'_>) -> syn::Result<()>,
) -> Option<usize> {
    let stream: TokenStream = tokens.iter().cloned().collect();
    let parser = |input: ParseStream<'_>| -> syn::Result<usize> {
        parse(input)?;
        let rest: TokenStream = input.parse()?;
        Ok(rest.into_iter().count())
    };
    parser.parse2(stream).ok().map(|rest| tokens.len() - rest)
}

/// `transcriber` with the variables of `bindings` substituted, inside the repetitions `at`,
/// and `$crate` spelled `home`.
fn transcribe(
    transcriber: TokenStream,
    bindings: &Bindings,
    at: &[usize],
    home: &str,
) -> Option<TokenStream> {
    let tokens: Vec<TokenTree> = transcriber.into_iter().collect();
    let mut out = TokenStream::new();
    let mut i = 0;
    while i < tokens.len() {
        let dollar = matches!(&tokens[i], TokenTree::Punct(p) if p.as_char() == '$');
        match (&tokens[i], tokens.get(i + 1)) {
            (_, Some(TokenTree::Ident(name))) if dollar && name == "crate" => {
                out.extend([TokenTree::Ident(Ident::new(home, name.span()))]);
                i += 2;
            }
            (_, Some(TokenTree::Ident(name))) if dollar => {
                match bindings.get(&name.to_string()) {
                    Some(binding) => out.extend(captured(binding, at)?.iter().cloned()),
                    // A variable of a `macro_rules!` the transcriber itself defines.
                    None => out.extend(tokens[i..i + 2].iter().cloned()),
                }
                i += 2;
            }
            (_, Some(TokenTree::Group(group)))
                if dollar && group.delimiter() == Delimiter::Parenthesis =>
            {
                let (separator, _, used) = kleene(&tokens[i + 2..])?;
                let count = repetitions(&group.stream(), bindings, at)?;
                let mut deeper = at.to_vec();
                deeper.push(0);
                for k in 0..count {
                    if k > 0 {
                        out.extend(separator.iter().cloned());
                    }
                    *deeper.last_mut()? = k;
                    out.extend(transcribe(group.stream(), bindings, &deeper, home)?);
                }
                i += 2 + used;
            }
            (TokenTree::Group(group), _) => {
                let inner = transcribe(group.stream(), bindings, at, home)?;
                let mut copy = Group::new(group.delimiter(), inner);
                copy.set_span(group.span());
                out.extend([TokenTree::Group(copy)]);
                i += 1;
            }
            (token, _) => {
                out.extend([token.clone()]);
                i += 1;
            }
        }
    }
    Some(out)
}

/// The tokens `binding` captured inside the repetitions `at`.
fn captured<'b>(mut binding: &'b Binding, at: &[usize]) -> Option<&'b [TokenTree]> {
    for &k in at {
        match binding {
            Binding::One(_) => break,
            Binding::Many(each) => binding = each.get(k)?,
        }
    }
    match binding {
        Binding::One(tokens) => Some(tokens),
        Binding::Many(_) => None,
    }
}

/// How many times the repetition `stream` runs inside the repetitions `at`, decided by the
/// repeated variables it uses.
fn repetitions(stream: &TokenStream, bindings: &Bindings, at: &[usize]) -> Option<usize> {
    let mut names = Vec::new();
    used_variables(stream, &mut names);
    let mut count = None;
    for name in names {
        let Some(mut binding) = bindings.get(&name) else {
            continue;
        };
        for &k in at {
            match binding {
                Binding::One(_) => break,
                Binding::Many(each) => binding = each.get(k)?,
            }
        }
        if let Binding::Many(each) = binding {
            match count {
                None => count = Some(each.len()),
                Some(n) if n != each.len() => return None,
                Some(_) => {}
            }
        }
    }
    count
}

/// The names after every `$` in `stream`, at any depth.
fn used_variables(stream: &TokenStream, out: &mut Vec<String>) {
    let tokens: Vec<TokenTree> = stream.clone().into_iter().collect();
    for (i, token) in tokens.iter().enumerate() {
        match token {
            TokenTree::Punct(p) if p.as_char() == '$' => {
                if let Some(TokenTree::Ident(name)) = tokens.get(i + 1) {
                    out.push(name.to_string());
                }
            }
            TokenTree::Group(group) => used_variables(&group.stream(), out),
            _ => {}
        }
    }
}
