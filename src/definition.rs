//! Digests of what a declaration says, which tell whether a range changed it.

use proc_macro2::Span;
use serde::{Deserialize, Serialize};
use syn::punctuated::Punctuated;
use syn::{Attribute, Fields, Token, TraitItem, Visibility};

/// What a declaration says, as digests of its tokens. Whitespace, comments, visibility and
/// attributes other than `serde` and `repr` leave every digest as it is.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Definition {
    /// Its signature, fields, variants, items, bounds or target.
    pub(crate) shape: u64,
    /// The initializer of a `const` or `static`, zero without one.
    pub(crate) value: u64,
    /// The body of a function or method, zero without one.
    pub(crate) body: u64,
}

impl Definition {
    /// The definition of a declaration that says `self` and `other` at once, as two
    /// declarations of one path under different `cfg`s do, the same in whichever order.
    pub(crate) fn merge(self, other: Self) -> Self {
        Self {
            shape: self.shape.wrapping_add(other.shape),
            value: self.value.wrapping_add(other.value),
            body: self.body.wrapping_add(other.body),
        }
    }
}

/// A byte range of the source.
type Range = (usize, usize);

/// Digests declarations of the text the spans count from.
pub(crate) struct Definer<'s> {
    pub(crate) source: &'s str,
}

impl Definer<'_> {
    pub(crate) fn constant(&self, item: &syn::ItemConst) -> Definition {
        let start = first(&[
            item.modifiers.defaultness.as_ref().map(|t| t.span),
            Some(item.const_token.span),
        ]);
        self.initialized(&item.attrs, start, &item.eq_token, &item.semi_token)
    }

    pub(crate) fn statik(&self, item: &syn::ItemStatic) -> Definition {
        self.initialized(
            &item.attrs,
            begin(item.static_token.span),
            &item.eq_token,
            &item.semi_token,
        )
    }

    pub(crate) fn impl_constant(&self, item: &syn::ImplItemConst) -> Definition {
        let start = first(&[
            item.modifiers.defaultness.as_ref().map(|t| t.span),
            Some(item.const_token.span),
        ]);
        self.initialized(&item.attrs, start, &item.eq_token, &item.semi_token)
    }

    /// A function or method, `body` absent for a declaration ending at `semi`.
    pub(crate) fn function(
        &self,
        attrs: &[Attribute],
        defaultness: Option<&Token![default]>,
        signature: &syn::Signature,
        body: Option<&syn::Block>,
        semi: Option<&Token![;]>,
    ) -> Definition {
        let start = signature_start(defaultness, signature);
        let (stop, body) = match body {
            Some(block) => {
                let open = block.brace_token.span.open().byte_range().start;
                let close = block.brace_token.span.close().byte_range().end;
                (open, self.digest(&[], (open, close), &[]))
            }
            None => (
                semi.map_or(self.source.len(), |semi| begin(semi.spans[0])),
                0,
            ),
        };
        Definition {
            shape: self.digest(attrs, (start, stop), &[]),
            value: 0,
            body,
        }
    }

    pub(crate) fn structure(&self, item: &syn::ItemStruct) -> Definition {
        // Only a struct with named fields ends at its brace, the others at their `;`.
        let stop = match &item.fields {
            Fields::Named(fields) => fields.brace_token.span.close().byte_range().end,
            Fields::Unnamed(_) | Fields::Unit => item
                .semi_token
                .as_ref()
                .map_or(self.source.len(), |semi| finish(semi.spans[0])),
        };
        let mut holes = Vec::new();
        field_holes(&item.fields, &mut holes);
        self.shaped(&item.attrs, (begin(item.struct_token.span), stop), &holes)
    }

    pub(crate) fn union(&self, item: &syn::ItemUnion) -> Definition {
        let stop = item.fields.brace_token.span.close().byte_range().end;
        let mut holes = Vec::new();
        for field in &item.fields.named {
            member_holes(&field.attrs, Some(&field.vis), &mut holes);
        }
        self.shaped(&item.attrs, (begin(item.union_token.span), stop), &holes)
    }

    pub(crate) fn enumeration(&self, item: &syn::ItemEnum) -> Definition {
        let stop = item.brace_token.span.close().byte_range().end;
        let mut holes = Vec::new();
        for variant in &item.variants {
            member_holes(&variant.attrs, None, &mut holes);
            field_holes(&variant.fields, &mut holes);
        }
        self.shaped(&item.attrs, (begin(item.enum_token.span), stop), &holes)
    }

    /// Each named field of a struct, union or variant with its definition, `close` being
    /// the closing brace of the fields.
    pub(crate) fn fields<'f>(
        &self,
        fields: &'f Punctuated<syn::Field, Token![,]>,
        close: Span,
    ) -> Vec<(&'f syn::Ident, Definition)> {
        fields
            .pairs()
            .filter_map(|pair| {
                let field = pair.value();
                let ident = field.ident.as_ref()?;
                let stop = pair
                    .punct()
                    .map_or(begin(close), |comma| begin(comma.spans[0]));
                let range = (begin(ident.span()), stop);
                Some((ident, self.shaped(&field.attrs, range, &[])))
            })
            .collect()
    }

    /// Each variant of `item` with its definition.
    pub(crate) fn variants<'v>(
        &self,
        item: &'v syn::ItemEnum,
    ) -> Vec<(&'v syn::Ident, Definition)> {
        let close = item.brace_token.span.close();
        item.variants
            .pairs()
            .map(|pair| {
                let variant = pair.value();
                let stop = pair
                    .punct()
                    .map_or(begin(close), |comma| begin(comma.spans[0]));
                let mut holes = Vec::new();
                field_holes(&variant.fields, &mut holes);
                let range = (begin(variant.ident.span()), stop);
                (&variant.ident, self.shaped(&variant.attrs, range, &holes))
            })
            .collect()
    }

    /// A trait: its bounds and its items' names and signatures, without their default
    /// bodies and values.
    pub(crate) fn trait_definition(&self, item: &syn::ItemTrait) -> Definition {
        let start = first(&[
            item.unsafety.as_ref().map(|t| t.span),
            item.modifiers.auto_token.as_ref().map(|t| t.span),
            Some(item.trait_token.span),
        ]);
        let stop = item.brace_token.span.close().byte_range().end;
        let mut holes = Vec::new();
        for trait_item in &item.items {
            match trait_item {
                TraitItem::Const(c) => {
                    member_holes(&c.attrs, None, &mut holes);
                    if let Some((eq, _)) = &c.default {
                        holes.push((begin(eq.spans[0]), begin(c.semi_token.spans[0])));
                    }
                }
                TraitItem::Fn(f) => {
                    member_holes(&f.attrs, None, &mut holes);
                    if let Some(block) = &f.default {
                        holes.push((
                            block.brace_token.span.open().byte_range().start,
                            block.brace_token.span.close().byte_range().end,
                        ));
                    }
                }
                TraitItem::Type(t) => member_holes(&t.attrs, None, &mut holes),
                TraitItem::Macro(m) => member_holes(&m.attrs, None, &mut holes),
                _ => {}
            }
        }
        self.shaped(&item.attrs, (start, stop), &holes)
    }

    pub(crate) fn trait_constant(&self, item: &syn::TraitItemConst) -> Definition {
        let start = first(&[
            item.modifiers.defaultness.as_ref().map(|t| t.span),
            Some(item.const_token.span),
        ]);
        let semi = begin(item.semi_token.spans[0]);
        match &item.default {
            Some((eq, _)) => self.initialized(&item.attrs, start, eq, &item.semi_token),
            None => self.shaped(&item.attrs, (start, semi), &[]),
        }
    }

    /// A type alias, an associated type or a foreign type, from its first token to `semi`.
    pub(crate) fn alias(
        &self,
        attrs: &[Attribute],
        defaultness: Option<&Token![default]>,
        type_token: &Token![type],
        semi: &Token![;],
    ) -> Definition {
        let start = first(&[defaultness.map(|t| t.span), Some(type_token.span)]);
        self.shaped(attrs, (start, begin(semi.spans[0])), &[])
    }

    pub(crate) fn trait_alias(&self, item: &syn::ItemTraitAlias) -> Definition {
        let range = (
            begin(item.trait_token.span),
            begin(item.semi_token.spans[0]),
        );
        self.shaped(&item.attrs, range, &[])
    }

    pub(crate) fn foreign_static(&self, item: &syn::ForeignItemStatic) -> Definition {
        let start = match &item.safety {
            syn::Safety::Safe(t) => begin(t.span),
            syn::Safety::Unsafe(t) => begin(t.span),
            syn::Safety::Default => begin(item.static_token.span),
        };
        self.shaped(&item.attrs, (start, begin(item.semi_token.spans[0])), &[])
    }

    /// A `const` or `static` whose shape runs from `start` to `eq` and whose value from `eq`
    /// to `semi`.
    fn initialized(
        &self,
        attrs: &[Attribute],
        start: usize,
        eq: &Token![=],
        semi: &Token![;],
    ) -> Definition {
        let eq = eq.spans[0].byte_range();
        Definition {
            shape: self.digest(attrs, (start, eq.start), &[]),
            value: self.digest(&[], (eq.end, begin(semi.spans[0])), &[]),
            body: 0,
        }
    }

    fn shaped(&self, attrs: &[Attribute], range: Range, holes: &[Range]) -> Definition {
        Definition {
            shape: self.digest(attrs, range, holes),
            value: 0,
            body: 0,
        }
    }

    /// The digest of the counted `attrs` followed by the tokens of `range` outside `holes`.
    fn digest(&self, attrs: &[Attribute], range: Range, holes: &[Range]) -> u64 {
        let mut hash = Fnv::new();
        for attr in attrs.iter().filter(|attr| counted(attr)) {
            self.feed(&mut hash, attribute_range(attr), &[]);
        }
        self.feed(&mut hash, range, holes);
        hash.0
    }

    /// Writes each token of `range` that starts outside `holes` into `hash`, comments,
    /// whitespace and a comma before a closing delimiter left out.
    fn feed(&self, hash: &mut Fnv, (start, stop): Range, holes: &[Range]) {
        use ra_ap_rustc_lexer::TokenKind;

        let Some(text) = self.source.get(start..stop) else {
            return;
        };
        let mut holes = holes.to_vec();
        holes.sort_unstable();
        let mut hole = 0;
        let mut at = start;
        let mut comma = false;
        for token in ra_ap_rustc_lexer::tokenize(text, ra_ap_rustc_lexer::FrontmatterAllowed::No) {
            let len = usize::try_from(token.len).expect("token lengths fit in usize");
            while holes.get(hole).is_some_and(|&(_, end)| end <= at) {
                hole += 1;
            }
            let skipped = holes[hole..]
                .iter()
                .take_while(|&&(from, _)| from <= at)
                .any(|&(_, end)| at < end);
            let trivia = matches!(
                token.kind,
                TokenKind::Whitespace
                    | TokenKind::LineComment { .. }
                    | TokenKind::BlockComment { .. }
            );
            if !skipped && !trivia {
                let closing = matches!(
                    token.kind,
                    TokenKind::CloseBrace
                        | TokenKind::CloseParen
                        | TokenKind::CloseBracket
                        | TokenKind::Gt
                );
                if comma && !closing {
                    hash.write(b",\xff");
                }
                comma = token.kind == TokenKind::Comma;
                if !comma {
                    hash.write(&self.source.as_bytes()[at..at + len]);
                    hash.write(&[0xff]);
                }
            }
            at += len;
        }
    }
}

/// Whether `attr` changes how its item is represented, which a definition counts.
fn counted(attr: &Attribute) -> bool {
    attr.path().is_ident("serde") || attr.path().is_ident("repr")
}

/// The source range of `attr`, from `#` to its closing bracket.
fn attribute_range(attr: &Attribute) -> Range {
    (
        begin(attr.pound_token.spans[0]),
        attr.bracket_token.span.close().byte_range().end,
    )
}

/// Adds to `holes` the uncounted `attrs` of a member and its visibility.
fn member_holes(attrs: &[Attribute], vis: Option<&Visibility>, holes: &mut Vec<Range>) {
    holes.extend(attrs.iter().filter(|a| !counted(a)).map(attribute_range));
    match vis {
        Some(Visibility::Public(token)) => holes.push((begin(token.span), finish(token.span))),
        Some(Visibility::Restricted(restricted)) => holes.push((
            begin(restricted.pub_token.span),
            restricted.paren_token.span.close().byte_range().end,
        )),
        _ => {}
    }
}

/// Adds to `holes` the uncounted attributes and the visibility of every field of `fields`.
fn field_holes(fields: &Fields, holes: &mut Vec<Range>) {
    let fields = match fields {
        Fields::Named(fields) => &fields.named,
        Fields::Unnamed(fields) => &fields.unnamed,
        Fields::Unit => return,
    };
    for field in fields {
        member_holes(&field.attrs, Some(&field.vis), holes);
    }
}

/// Where a function's signature starts, at its first qualifier.
fn signature_start(defaultness: Option<&Token![default]>, signature: &syn::Signature) -> usize {
    let safety = match &signature.safety {
        syn::Safety::Safe(t) => Some(t.span),
        syn::Safety::Unsafe(t) => Some(t.span),
        syn::Safety::Default => None,
    };
    first(&[
        defaultness.map(|t| t.span),
        signature.constness.as_ref().map(|t| t.span),
        signature.asyncness.as_ref().map(|t| t.span),
        safety,
        signature.abi.as_ref().map(|abi| abi.extern_token.span),
        Some(signature.fn_token.span),
    ])
}

/// The earliest start among `spans`.
fn first(spans: &[Option<Span>]) -> usize {
    spans
        .iter()
        .flatten()
        .map(|&span| begin(span))
        .min()
        .expect("a declaration has its keyword")
}

fn begin(span: Span) -> usize {
    span.byte_range().start
}

fn finish(span: Span) -> usize {
    span.byte_range().end
}

/// 64-bit FNV-1a, which stays the same across builds as cached digests must.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::parse::parse;

    use super::Definition;

    /// The definition the parser records for the declaration at `path` in `source`.
    fn definition(source: &str, path: &[&str]) -> Definition {
        parse(source)
            .expect("the source parses")
            .items
            .iter()
            .find(|decl| decl.path == path)
            .and_then(|decl| decl.definition)
            .expect("the declaration has a definition")
    }

    #[test]
    fn reformatting_and_comments_leave_a_struct_alone() {
        let before = definition("pub struct S { pub a: u32, b: Vec<u8> }", &["S"]);
        let after = definition(
            "/// Docs.\npub struct S {\n    // why\n    pub a: u32,\n    b: Vec< u8 >,\n}\n",
            &["S"],
        );
        assert_eq!(before, after);
    }

    #[test]
    fn a_renamed_or_retyped_field_changes_the_struct_shape() {
        let before = definition("pub struct S { a: u32 }", &["S"]);
        assert_ne!(
            before.shape,
            definition("pub struct S { b: u32 }", &["S"]).shape
        );
        assert_ne!(
            before.shape,
            definition("pub struct S { a: u64 }", &["S"]).shape
        );
    }

    #[test]
    fn visibility_and_uncounted_attributes_leave_the_shape_alone() {
        let before = definition("pub struct S { pub a: u32 }", &["S"]);
        let after = definition(
            "#[derive(Clone)]\npub(crate) struct S {\n    #[doc(hidden)] pub(crate) a: u32,\n}\n",
            &["S"],
        );
        assert_eq!(before, after);
    }

    #[test]
    fn serde_and_repr_attributes_change_the_shape() {
        let plain = definition("pub struct S { a: u32 }", &["S"]);
        let renamed = definition("pub struct S { #[serde(rename = \"b\")] a: u32 }", &["S"]);
        let packed = definition("#[repr(C)]\npub struct S { a: u32 }", &["S"]);
        assert_ne!(plain.shape, renamed.shape);
        assert_ne!(plain.shape, packed.shape);
        assert_ne!(
            definition("pub struct S { a: u32 }", &["S", "a"]).shape,
            definition("pub struct S { #[serde(default)] a: u32 }", &["S", "a"]).shape,
        );
    }

    #[test]
    fn a_const_value_is_apart_from_its_shape() {
        let one = definition("pub const V: u32 = 1;", &["V"]);
        let two = definition("pub const V: u32 = 2;", &["V"]);
        let wide = definition("pub const V: u64 = 1;", &["V"]);
        assert_eq!(one.shape, two.shape);
        assert_ne!(one.value, two.value);
        assert_ne!(one.shape, wide.shape);
        assert_eq!(one.value, wide.value);
    }

    #[test]
    fn a_function_body_is_apart_from_its_signature() {
        let one = definition("pub fn f(a: u32) -> u32 { a }", &["f"]);
        let two = definition("pub fn f(a: u32) -> u32 { a + 1 }", &["f"]);
        let generic = definition("pub fn f<T>(a: u32) -> u32 { a }", &["f"]);
        assert_eq!(one.shape, two.shape);
        assert_ne!(one.body, two.body);
        assert_ne!(one.shape, generic.shape);
        assert_ne!(
            one.shape,
            definition("pub async fn f(a: u32) -> u32 { a }", &["f"]).shape
        );
    }

    #[test]
    fn a_variant_payload_changes_the_variant_and_the_enum() {
        let before = "pub enum E { A(u32), B }";
        let after = "pub enum E { A(u64), B }";
        assert_ne!(
            definition(before, &["E"]).shape,
            definition(after, &["E"]).shape
        );
        assert_ne!(
            definition(before, &["E", "A"]).shape,
            definition(after, &["E", "A"]).shape
        );
        assert_eq!(
            definition(before, &["E", "B"]),
            definition(after, &["E", "B"])
        );
    }

    #[test]
    fn a_trailing_comma_in_generics_is_formatting() {
        assert_eq!(
            definition("pub struct S { a: HashMap<u8, u8> }", &["S"]),
            definition("pub struct S { a: HashMap<u8, u8,> }", &["S"]),
        );
    }

    #[test]
    fn an_alias_target_is_its_shape() {
        let before = definition("pub type A = B;", &["A"]);
        assert_eq!(before, definition("pub type A =\n    B;", &["A"]));
        assert_ne!(before.shape, definition("pub type A = C;", &["A"]).shape);
    }

    #[test]
    fn a_trait_default_body_is_no_part_of_the_trait_shape() {
        let before = definition("pub trait T { fn f(&self) -> u32 { 1 } }", &["T"]);
        let after = definition("pub trait T { fn f(&self) -> u32 { 2 } }", &["T"]);
        let resigned = definition("pub trait T { fn f(&self) -> u64 { 1 } }", &["T"]);
        assert_eq!(before, after);
        assert_ne!(before.shape, resigned.shape);
        assert_ne!(
            definition("pub trait T { fn f(&self) -> u32 { 1 } }", &["T", "f"]).body,
            definition("pub trait T { fn f(&self) -> u32 { 2 } }", &["T", "f"]).body,
        );
    }

    #[test]
    fn a_trait_const_default_is_its_value_and_no_part_of_the_trait_shape() {
        let one = "pub trait T { const N: u32 = 1; const M: u32; }";
        let two = "pub trait T { const N: u32 = 2; const M: u32; }";
        assert_eq!(definition(one, &["T"]), definition(two, &["T"]));
        assert_ne!(
            definition(one, &["T", "N"]).value,
            definition(two, &["T", "N"]).value
        );
        assert_ne!(
            definition(one, &["T", "M"]).shape,
            definition(
                "pub trait T { const N: u32 = 1; const M: u64; }",
                &["T", "M"]
            )
            .shape,
        );
    }

    #[test]
    fn a_tuple_struct_shape_holds_its_field_types() {
        let before = definition("pub struct S(pub u8, u16);", &["S"]);
        assert_eq!(
            before,
            definition("pub struct S(u8, pub(crate) u16,);", &["S"])
        );
        assert_ne!(
            before.shape,
            definition("pub struct S(u8, u32);", &["S"]).shape
        );
    }

    #[test]
    fn a_foreign_item_safety_qualifier_is_its_shape() {
        let block = |items: &str| format!("unsafe extern \"C\" {{ {items} }}");
        assert_ne!(
            definition(&block("pub fn f();"), &["f"]).shape,
            definition(&block("pub safe fn f();"), &["f"]).shape,
        );
        assert_ne!(
            definition(&block("pub static X: u8;"), &["X"]).shape,
            definition(&block("pub safe static X: u8;"), &["X"]).shape,
        );
    }
}
