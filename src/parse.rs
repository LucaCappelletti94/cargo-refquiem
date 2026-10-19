//! What one Rust source file declares, independent of where it sits in a crate.

use std::collections::HashSet;

use proc_macro2::{TokenStream, TokenTree};
use serde::{Deserialize, Serialize};
use syn::parse::{ParseStream, Parser};
use syn::{Attribute, Fields, ImplItem, Item, TraitItem, Type, UseTree, Visibility};

use crate::ItemKind;
use crate::serde_names::Container;

/// The segment before a serialized spelling in a path, which no identifier can spell.
pub(crate) const SERDE_MARKER: &str = "<serde>";

/// The declarations of one file. Every `module` field is the chain of inline modules
/// enclosing the declaration within the file.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(crate) struct FileItems {
    /// Named items, by path from the file's module.
    pub(crate) items: Vec<Decl>,
    /// Impl blocks, inherent and of traits.
    pub(crate) impls: Vec<Impl>,
    /// Re-exports and renaming imports.
    pub(crate) uses: Vec<Use>,
    /// Plain private imports used to resolve written names within their module.
    pub(crate) imports: Vec<Use>,
    /// `mod name;` declarations whose body lives in another file.
    pub(crate) mods: Vec<ModDecl>,
    /// Item-position `include!("file")` invocations.
    pub(crate) includes: Vec<Include>,
    /// Item-position macro invocations other than `include!`.
    pub(crate) invocations: Vec<Invocation>,
    /// `macro_rules!` definitions.
    pub(crate) macro_rules: Vec<MacroRules>,
    /// Members one path lends another: a `Deref` target its dereferencing type, a supertrait
    /// its subtrait.
    pub(crate) lends: Vec<Lend>,
    /// The parameter names of every function, method and trait method and the type and const
    /// parameter names of every generic item, deduplicated.
    pub(crate) params: Vec<String>,
}

impl FileItems {
    /// Moves every declaration into the inline modules `prefix`.
    pub(crate) fn nest(&mut self, prefix: &[String]) {
        let nest = |module: &mut Vec<String>| {
            module.splice(0..0, prefix.iter().cloned());
        };
        self.items.iter_mut().for_each(|d| nest(&mut d.path));
        self.impls.iter_mut().for_each(|i| nest(&mut i.module));
        self.uses.iter_mut().for_each(|u| nest(&mut u.module));
        self.imports.iter_mut().for_each(|u| nest(&mut u.module));
        self.mods.iter_mut().for_each(|m| nest(&mut m.module));
        self.includes.iter_mut().for_each(|i| nest(&mut i.module));
        self.invocations
            .iter_mut()
            .for_each(|i| nest(&mut i.module));
        self.macro_rules
            .iter_mut()
            .for_each(|r| nest(&mut r.module));
        self.lends.iter_mut().for_each(|l| nest(&mut l.module));
    }
}

/// A named item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Decl {
    pub(crate) path: Vec<String>,
    pub(crate) kind: ItemKind,
    /// Whether the item is `pub`, members counting only when their owner is too.
    pub(crate) public: bool,
    /// For a function or trait method, its parameter types and return type as normalized tokens.
    pub(crate) signature: Option<String>,
}

/// An impl block, inherent or of a trait.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Impl {
    pub(crate) module: Vec<String>,
    /// The self type's path segments without generic arguments.
    pub(crate) self_ty: Vec<String>,
    pub(crate) trait_path: Option<Vec<String>>,
    /// Each item's name, kind, whether it is visible, and an inherent method's signature.
    pub(crate) items: Vec<(String, ItemKind, bool, Option<String>)>,
}

/// A named import, type alias or glob re-export.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Use {
    pub(crate) module: Vec<String>,
    /// The imported path, `crate`, `self` and `super` kept as written.
    pub(crate) source: Vec<String>,
    /// The name created in `module`, `None` for a glob.
    pub(crate) name: Option<String>,
    /// Whether the `use` is `pub`.
    pub(crate) public: bool,
}

/// A `mod name;` declaration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ModDecl {
    pub(crate) module: Vec<String>,
    pub(crate) name: String,
    /// `#[path]` values.
    pub(crate) paths: Vec<String>,
    /// `#[path]` values inside `cfg_attr`, which leave the default location possible.
    pub(crate) conditional_paths: Vec<String>,
}

/// An item-position `include!`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Include {
    pub(crate) module: Vec<String>,
    pub(crate) path: String,
}

/// An item-position macro invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Invocation {
    pub(crate) module: Vec<String>,
    /// The macro's name, the last segment of its path.
    pub(crate) name: String,
    /// The identifiers inside, kept when the tokens are not.
    pub(crate) idents: Vec<String>,
    /// The tokens as text, kept for item-position invocations, which may be expanded.
    pub(crate) tokens: Option<String>,
    /// For an invocation among an impl's items, the impl's header from `impl` to its brace,
    /// which wraps the expansion.
    pub(crate) impl_header: Option<String>,
}

/// A `macro_rules!` definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct MacroRules {
    pub(crate) module: Vec<String>,
    pub(crate) name: String,
    pub(crate) idents: Vec<String>,
    /// The rules as text.
    pub(crate) body: String,
    pub(crate) exported: bool,
}

/// A path whose members another path reaches, by auto-deref through an
/// `impl Deref for Owner { type Target = Source; }` or as the supertrait in `trait Owner: Source`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Lend {
    pub(crate) module: Vec<String>,
    pub(crate) owner: Vec<String>,
    pub(crate) source: Vec<String>,
}

/// The declarations of `source`, or the parser's message when it is not valid Rust.
pub(crate) fn parse(source: &str) -> Result<FileItems, String> {
    let parsed = syn::parse_file(source)
        .map(|file| {
            // Spans count from the text `syn` parsed, after a byte order mark and a shebang.
            let bom = if source.starts_with('\u{feff}') { 3 } else { 0 };
            let skipped = bom + file.shebang.as_ref().map_or(0, String::len);
            let mut collector = Collector {
                out: FileItems::default(),
                module: Vec::new(),
                source: source.get(skipped..).unwrap_or_default(),
            };
            collector.items(&file.items);
            collector.out
        })
        .map_err(|error| error_message(&error));
    // Span locations keep every parsed source alive in a thread-local map until invalidated.
    proc_macro2::extra::invalidate_current_thread_spans();
    parsed
}

/// Parses one expansion item at a time without retaining the preceding syntax trees.
pub(crate) fn parse_expansion(source: &str) -> Result<FileItems, String> {
    let parsed = (|input: ParseStream<'_>| -> syn::Result<FileItems> {
        let _ = Attribute::parse_inner(input)?;
        let mut collector = Collector {
            out: FileItems::default(),
            module: Vec::new(),
            source,
        };
        while !input.is_empty() {
            let item: Item = input.parse()?;
            collector.items(std::slice::from_ref(&item));
        }
        Ok(collector.out)
    })
    .parse_str(source)
    .map_err(|error| error_message(&error));
    // Span locations keep every parsed source alive in a thread-local map until invalidated.
    proc_macro2::extra::invalidate_current_thread_spans();
    parsed
}

/// The message of a parse `error`, prefixed by its line and column in the parsed text.
fn error_message(error: &syn::Error) -> String {
    let start = error.span().start();
    format!("{}:{}: {error}", start.line, start.column + 1)
}

/// A member of an item: its name, kind, whether it is `pub`, and its serialized spellings.
type Member = (String, ItemKind, bool, Vec<String>);

/// Walks a file's items, tracking the enclosing inline modules.
struct Collector<'s> {
    out: FileItems,
    module: Vec<String>,
    /// The text the spans count from.
    source: &'s str,
}

impl Collector<'_> {
    fn items(&mut self, items: &[Item]) {
        for item in items {
            match item {
                Item::Const(item) => self.decl(&item.ident.to_string(), ItemKind::Const, &item.vis),
                Item::Enum(item) => {
                    self.enumeration(item);
                    self.generics(&item.generics);
                }
                Item::Fn(item) => {
                    let name = item.sig.ident.to_string();
                    self.decl(&name, ItemKind::Function, &item.vis);
                    let end = item.block.brace_token.span.open().byte_range().start;
                    let signature = self.signature(&item.sig, end);
                    self.sign(&[&name], ItemKind::Function, signature);
                    self.params(&item.sig);
                }
                Item::ForeignMod(foreign) => self.foreign_items(&foreign.items),
                Item::Impl(item) => {
                    self.impl_block(item);
                    self.generics(&item.generics);
                }
                Item::Macro(item) => self.macro_item(item),
                Item::Mod(item) => self.mod_item(item),
                Item::Static(item) => {
                    self.decl(&item.ident.to_string(), ItemKind::Static, &item.vis);
                }
                Item::Struct(item) => {
                    let container = Container::new(&item.attrs);
                    let fields = match &item.fields {
                        Fields::Named(fields) => named_fields(&fields.named, &container),
                        Fields::Unnamed(_) | Fields::Unit => Vec::new(),
                    };
                    let name = item.ident.to_string();
                    self.owner(&name, ItemKind::Struct, &item.vis, fields);
                    self.generics(&item.generics);
                }
                Item::Trait(item) => {
                    self.trait_item(item);
                    self.generics(&item.generics);
                }
                Item::TraitAlias(item) => {
                    self.decl(&item.ident.to_string(), ItemKind::TraitAlias, &item.vis);
                    self.generics(&item.generics);
                }
                Item::Type(item) => {
                    self.type_alias(item);
                    self.generics(&item.generics);
                }
                Item::Union(item) => {
                    let container = Container::new(&item.attrs);
                    let fields = named_fields(&item.fields.named, &container);
                    self.owner(&item.ident.to_string(), ItemKind::Union, &item.vis, fields);
                    self.generics(&item.generics);
                }
                Item::Use(item) => {
                    let public = is_pub(&item.vis);
                    let exported = !matches!(item.vis, Visibility::Inherited);
                    self.uses(&item.tree, &mut Vec::new(), exported, public);
                }
                _ => {}
            }
        }
    }

    fn decl(&mut self, name: &str, kind: ItemKind, vis: &Visibility) {
        self.push(&[name], kind, is_pub(vis));
    }

    fn push(&mut self, names: &[&str], kind: ItemKind, public: bool) {
        if names.contains(&"_") {
            return;
        }
        let path = self.path(names);
        self.out.items.push(Decl {
            path,
            kind,
            public,
            signature: None,
        });
    }

    /// `names` below the current module, raw identifiers stripped.
    fn path(&self, names: &[&str]) -> Vec<String> {
        let mut path = self.module.clone();
        path.extend(names.iter().map(|name| strip_raw(name).to_owned()));
        path
    }

    /// Records `signature` on the latest declaration of `names` with `kind`.
    fn sign(&mut self, names: &[&str], kind: ItemKind, signature: String) {
        let path = self.path(names);
        if let Some(decl) = self
            .out
            .items
            .iter_mut()
            .rev()
            .find(|decl| decl.kind == kind && decl.path == path)
        {
            decl.signature = Some(signature);
        }
    }

    /// The parameter types and return type of `signature` as normalized tokens, `end` being
    /// where the function's body or `;` starts.
    fn signature(&self, signature: &syn::Signature, end: usize) -> String {
        let close = signature.paren_token.span.close().byte_range().start;
        let mut params = Vec::new();
        for pair in signature.inputs.pairs() {
            let stop = pair
                .punct()
                .map_or(close, |comma| comma.span.byte_range().start);
            params.push(match pair.value() {
                syn::FnArg::Receiver(receiver) => match &receiver.kind {
                    syn::ReceiverKind::Typed(colon, _) => {
                        self.tokens(colon.span.byte_range().end, stop)
                    }
                    syn::ReceiverKind::Reference(_, _, Some(_)) => "& mut self".to_owned(),
                    syn::ReceiverKind::Reference(_, _, None) => "& self".to_owned(),
                    _ => "self".to_owned(),
                },
                syn::FnArg::Typed(typed) => {
                    self.tokens(typed.colon_token.span.byte_range().end, stop)
                }
            });
        }
        if signature.variadic.is_some() {
            params.push("...".to_owned());
        }
        let output = match &signature.output {
            syn::ReturnType::Default => String::new(),
            syn::ReturnType::Type(arrow, _) => {
                let stop = signature
                    .generics
                    .where_clause
                    .as_ref()
                    .map_or(end, |clause| clause.where_token.span.byte_range().start);
                format!(" -> {}", self.tokens(arrow.spans[1].byte_range().end, stop))
            }
        };
        format!("({}){output}", params.join(", "))
    }

    /// The tokens of the source between `start` and `stop`, joined by single spaces.
    fn tokens(&self, start: usize, stop: usize) -> String {
        let Some(text) = self.source.get(start..stop) else {
            return String::new();
        };
        let mut out = Vec::new();
        let mut at = 0;
        for token in ra_ap_rustc_lexer::tokenize(text, ra_ap_rustc_lexer::FrontmatterAllowed::No) {
            let len = usize::try_from(token.len).expect("token lengths fit in usize");
            if !matches!(
                token.kind,
                ra_ap_rustc_lexer::TokenKind::Whitespace
                    | ra_ap_rustc_lexer::TokenKind::LineComment { .. }
                    | ra_ap_rustc_lexer::TokenKind::BlockComment { .. }
            ) {
                out.push(&text[at..at + len]);
            }
            at += len;
        }
        out.join(" ")
    }

    /// Declares `name` and its `members`, each with its serialized spellings.
    fn owner(&mut self, name: &str, kind: ItemKind, vis: &Visibility, members: Vec<Member>) {
        let public = is_pub(vis);
        self.push(&[name], kind, public);
        for (member, member_kind, member_public, spellings) in members {
            self.push(&[name, &member], member_kind, public && member_public);
            self.spellings(&[name, &member], &spellings, public && member_public);
        }
    }

    /// Declares the serialized `spellings` of the member at `owner`, below a marker path so
    /// re-exports carry them like any other member.
    fn spellings(&mut self, owner: &[&str], spellings: &[String], public: bool) {
        if spellings.is_empty() {
            return;
        }
        let mut names = owner.to_vec();
        names.push(SERDE_MARKER);
        self.push(&names, ItemKind::Serialized, public);
        for spelling in spellings {
            names.push(spelling);
            self.push(&names, ItemKind::Serialized, public);
            names.pop();
        }
    }

    fn enumeration(&mut self, item: &syn::ItemEnum) {
        let container = Container::new(&item.attrs);
        let name = item.ident.to_string();
        let variants = item
            .variants
            .iter()
            .map(|v| {
                let variant = v.ident.to_string();
                let spellings = container.variant(strip_raw(&variant), &v.attrs);
                (variant, ItemKind::Variant, true, spellings)
            })
            .collect();
        self.owner(&name, ItemKind::Enum, &item.vis, variants);
        let public = is_pub(&item.vis);
        for variant in &item.variants {
            let Fields::Named(fields) = &variant.fields else {
                continue;
            };
            let fields_container = container.variant_fields(&variant.attrs);
            let variant_name = variant.ident.to_string();
            for field in &fields.named {
                let Some(ident) = &field.ident else { continue };
                let field_name = ident.to_string();
                self.push(
                    &[&name, &variant_name, &field_name],
                    ItemKind::Field,
                    public,
                );
                let spellings = fields_container.field(strip_raw(&field_name), &field.attrs);
                self.spellings(&[&name, &variant_name, &field_name], &spellings, public);
            }
        }
    }

    fn foreign_items(&mut self, items: &[syn::ForeignItem]) {
        for item in items {
            match item {
                syn::ForeignItem::Fn(item) => {
                    let name = item.sig.ident.to_string();
                    self.decl(&name, ItemKind::Function, &item.vis);
                    let end = item.semi_token.span.byte_range().start;
                    let signature = self.signature(&item.sig, end);
                    self.sign(&[&name], ItemKind::Function, signature);
                    self.params(&item.sig);
                }
                syn::ForeignItem::Static(item) => {
                    self.decl(&item.ident.to_string(), ItemKind::Static, &item.vis);
                }
                syn::ForeignItem::Type(item) => {
                    self.decl(&item.ident.to_string(), ItemKind::TypeAlias, &item.vis);
                }
                syn::ForeignItem::Macro(item) => self.invocation(&item.mac, false),
                _ => {}
            }
        }
    }

    fn macro_item(&mut self, item: &syn::ItemMacro) {
        if let Some(ident) = &item.ident {
            let name = ident.to_string();
            let exported = item.attrs.iter().any(|a| a.path().is_ident("macro_export"));
            self.push(&[&name], ItemKind::Macro, exported);
            self.out.macro_rules.push(MacroRules {
                module: self.module.clone(),
                name,
                idents: idents(&item.mac.tokens),
                body: item.mac.tokens.to_string(),
                exported,
            });
        } else if let Some(path) = include_path(&item.mac) {
            self.out.includes.push(Include {
                module: self.module.clone(),
                path,
            });
        } else {
            self.invocation(&item.mac, true);
        }
    }

    fn mod_item(&mut self, item: &syn::ItemMod) {
        let name = strip_raw(&item.ident.to_string()).to_owned();
        self.push(&[&name], ItemKind::Module, is_pub(&item.vis));
        if let Some((_, items)) = &item.content {
            self.module.push(name);
            self.items(items);
            self.module.pop();
        } else {
            let (paths, conditional_paths) = path_attributes(&item.attrs);
            self.out.mods.push(ModDecl {
                module: self.module.clone(),
                name,
                paths,
                conditional_paths,
            });
        }
    }

    fn trait_item(&mut self, item: &syn::ItemTrait) {
        let mut members = Vec::new();
        let mut signatures = Vec::new();
        for trait_item in &item.items {
            let (name, kind) = match trait_item {
                TraitItem::Const(c) => (c.ident.to_string(), ItemKind::AssociatedConst),
                TraitItem::Fn(f) => {
                    self.params(&f.sig);
                    let end = f.default.as_ref().map_or_else(
                        || {
                            f.semi_token
                                .as_ref()
                                .map_or(self.source.len(), |semi| semi.span.byte_range().start)
                        },
                        |block| block.brace_token.span.open().byte_range().start,
                    );
                    let name = f.sig.ident.to_string();
                    signatures.push((name.clone(), self.signature(&f.sig, end)));
                    (name, ItemKind::Method)
                }
                TraitItem::Type(t) => (t.ident.to_string(), ItemKind::AssociatedType),
                TraitItem::Macro(m) => {
                    self.invocation(&m.mac, false);
                    continue;
                }
                _ => continue,
            };
            members.push((name, kind, true, Vec::new()));
        }
        let trait_name = item.ident.to_string();
        for bound in &item.supertraits {
            if let syn::TypeParamBound::Trait(bound) = bound {
                self.out.lends.push(Lend {
                    module: self.module.clone(),
                    owner: vec![strip_raw(&trait_name).to_owned()],
                    source: path_segments(&bound.path),
                });
            }
        }
        self.owner(&trait_name, ItemKind::Trait, &item.vis, members);
        for (name, signature) in signatures {
            self.sign(&[&trait_name, &name], ItemKind::Method, signature);
        }
    }

    fn impl_block(&mut self, item: &syn::ItemImpl) {
        for impl_item in &item.items {
            match impl_item {
                ImplItem::Macro(m) => {
                    self.invocation(&m.mac, true);
                    let start = item.impl_token.span.byte_range().start;
                    let end = item.brace_token.span.open().byte_range().start;
                    if let Some(call) = self.out.invocations.last_mut() {
                        call.impl_header = self.source.get(start..end).map(str::to_owned);
                    }
                }
                ImplItem::Fn(f) => self.params(&f.sig),
                _ => {}
            }
        }
        let Some(segments) = type_path(&item.self_ty) else {
            return;
        };
        let of_trait = item.trait_.is_some();
        if let Some((trait_path, _)) = &item.trait_ {
            let is_deref = trait_path
                .segments
                .last()
                .is_some_and(|s| s.ident == "Deref");
            let target = item.items.iter().find_map(|impl_item| match impl_item {
                ImplItem::Type(t) if t.ident == "Target" => type_path(&t.ty),
                _ => None,
            });
            if is_deref && let Some(target) = target {
                self.out.lends.push(Lend {
                    module: self.module.clone(),
                    owner: segments.clone(),
                    source: target,
                });
            }
        }
        // A trait's items are as visible as the trait, and their signatures are the trait's.
        let items = item
            .items
            .iter()
            .filter_map(|impl_item| match impl_item {
                ImplItem::Const(c) => Some((&c.ident, ItemKind::AssociatedConst, &c.vis, None)),
                ImplItem::Fn(f) => {
                    let end = f.block.brace_token.span.open().byte_range().start;
                    let signature = (!of_trait).then(|| self.signature(&f.sig, end));
                    Some((&f.sig.ident, ItemKind::Method, &f.vis, signature))
                }
                ImplItem::Type(t) => Some((&t.ident, ItemKind::AssociatedType, &t.vis, None)),
                _ => None,
            })
            .map(|(ident, kind, vis, signature)| {
                let name = strip_raw(&ident.to_string()).to_owned();
                (name, kind, of_trait || is_pub(vis), signature)
            })
            .collect();
        self.out.impls.push(Impl {
            module: self.module.clone(),
            self_ty: segments,
            trait_path: item.trait_.as_ref().map(|(path, _)| path_segments(path)),
            items,
        });
    }

    /// A type alias declares its name and, for a plain path target, aliases the target's
    /// members.
    fn type_alias(&mut self, item: &syn::ItemType) {
        let name = item.ident.to_string();
        self.decl(&name, ItemKind::TypeAlias, &item.vis);
        if let Some(source) = type_path(&item.ty) {
            self.out.uses.push(Use {
                module: self.module.clone(),
                source,
                name: Some(strip_raw(&name).to_owned()),
                public: is_pub(&item.vis),
            });
        }
    }

    fn uses(&mut self, tree: &UseTree, prefix: &mut Vec<String>, exported: bool, public: bool) {
        match tree {
            UseTree::Path(path) => {
                prefix.push(strip_raw(&path.ident.to_string()).to_owned());
                self.uses(&path.tree, prefix, exported, public);
                prefix.pop();
            }
            UseTree::Name(name) => {
                let ident = strip_raw(&name.ident.to_string()).to_owned();
                let (source, name) = if ident == "self" {
                    (prefix.clone(), prefix.last().cloned())
                } else {
                    let mut source = prefix.clone();
                    source.push(ident.clone());
                    (source, Some(ident))
                };
                if name.is_some() {
                    let decl = Use {
                        module: self.module.clone(),
                        source,
                        name,
                        public,
                    };
                    if exported {
                        self.out.uses.push(decl);
                    } else {
                        self.out.imports.push(decl);
                    }
                }
            }
            UseTree::Rename(rename) => {
                let ident = strip_raw(&rename.ident.to_string()).to_owned();
                let alias = strip_raw(&rename.rename.to_string()).to_owned();
                if alias == "_" {
                    return;
                }
                let mut source = prefix.clone();
                if ident != "self" {
                    source.push(ident);
                }
                self.out.uses.push(Use {
                    module: self.module.clone(),
                    source,
                    name: Some(alias),
                    public,
                });
            }
            UseTree::Glob(_) if exported => self.out.uses.push(Use {
                module: self.module.clone(),
                source: prefix.clone(),
                name: None,
                public,
            }),
            UseTree::Group(group) => {
                for tree in &group.items {
                    self.uses(tree, prefix, exported, public);
                }
            }
            UseTree::Glob(_) => {}
        }
    }

    fn params(&mut self, signature: &syn::Signature) {
        let mut names = Vec::new();
        for input in &signature.inputs {
            if let syn::FnArg::Typed(typed) = input {
                bound_names(&typed.pat, &mut names);
            }
        }
        for name in names {
            self.param(name);
        }
        self.generics(&signature.generics);
    }

    /// Records the names of the type and const parameters `generics` declares.
    fn generics(&mut self, generics: &syn::Generics) {
        for param in &generics.params {
            let ident = match param {
                syn::GenericParam::Type(param) => &param.ident,
                syn::GenericParam::Const(param) => &param.ident,
                syn::GenericParam::Lifetime(_) => continue,
            };
            self.param(strip_raw(&ident.to_string()).to_owned());
        }
    }

    fn param(&mut self, name: String) {
        if !self.out.params.contains(&name) {
            self.out.params.push(name);
        }
    }

    fn invocation(&mut self, mac: &syn::Macro, item_position: bool) {
        let Some(last) = mac.path.segments.last() else {
            return;
        };
        self.out.invocations.push(Invocation {
            module: self.module.clone(),
            name: strip_raw(&last.ident.to_string()).to_owned(),
            idents: if item_position {
                Vec::new()
            } else {
                idents(&mac.tokens)
            },
            tokens: item_position.then(|| mac.tokens.to_string()),
            impl_header: None,
        });
    }
}

/// The named fields of a struct, union or variant with their visibility and spellings.
fn named_fields(
    fields: &syn::punctuated::Punctuated<syn::Field, syn::Token![,]>,
    container: &Container,
) -> Vec<Member> {
    fields
        .iter()
        .filter_map(|field| {
            let name = field.ident.as_ref()?.to_string();
            let spellings = container.field(strip_raw(&name), &field.attrs);
            Some((name, ItemKind::Field, is_pub(&field.vis), spellings))
        })
        .collect()
}

/// The names `pattern` binds.
fn bound_names(pattern: &syn::Pat, out: &mut Vec<String>) {
    match pattern {
        syn::Pat::Ident(ident) => {
            out.push(strip_raw(&ident.ident.to_string()).to_owned());
            if let Some((_, sub)) = &ident.subpat {
                bound_names(sub, out);
            }
        }
        syn::Pat::Tuple(tuple) => tuple.elems.iter().for_each(|p| bound_names(p, out)),
        syn::Pat::TupleStruct(tuple) => tuple.elems.iter().for_each(|p| bound_names(p, out)),
        syn::Pat::Slice(slice) => slice.elems.iter().for_each(|p| bound_names(p, out)),
        syn::Pat::Or(or) => or.cases.iter().for_each(|p| bound_names(p, out)),
        syn::Pat::Struct(structure) => structure
            .fields
            .iter()
            .for_each(|field| bound_names(&field.pat, out)),
        syn::Pat::Reference(reference) => bound_names(&reference.pat, out),
        syn::Pat::Paren(paren) => bound_names(&paren.pat, out),
        _ => {}
    }
}

fn is_pub(vis: &Visibility) -> bool {
    matches!(vis, Visibility::Public(_))
}

/// The segments of a path type without `qself` or generic arguments.
fn type_path(ty: &Type) -> Option<Vec<String>> {
    let Type::Path(path) = ty else {
        return None;
    };
    if path.qself.is_some() {
        return None;
    }
    Some(path_segments(&path.path))
}

/// The segments of `path` without generic arguments.
fn path_segments(path: &syn::Path) -> Vec<String> {
    path.segments
        .iter()
        .map(|s| strip_raw(&s.ident.to_string()).to_owned())
        .collect()
}

fn include_path(mac: &syn::Macro) -> Option<String> {
    if !mac.path.is_ident("include") {
        return None;
    }
    let literal: syn::LitStr = syn::parse2(mac.tokens.clone()).ok()?;
    Some(literal.value())
}

/// Every identifier in the tokens `text`, deduplicated.
pub(crate) fn text_idents(text: &str) -> Vec<String> {
    let found = text
        .parse::<TokenStream>()
        .map(|tokens| idents(&tokens))
        .unwrap_or_default();
    proc_macro2::extra::invalidate_current_thread_spans();
    found
}

/// Every identifier in `tokens`, deduplicated.
fn idents(tokens: &TokenStream) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut stack = vec![tokens.clone()];
    while let Some(stream) = stack.pop() {
        for tree in stream {
            match tree {
                TokenTree::Ident(ident) => {
                    let name = strip_raw(&ident.to_string()).to_owned();
                    if seen.insert(name.clone()) {
                        out.push(name);
                    }
                }
                TokenTree::Group(group) => stack.push(group.stream()),
                TokenTree::Punct(_) | TokenTree::Literal(_) => {}
            }
        }
    }
    out
}

/// `#[path = "..."]` values, then those inside `#[cfg_attr(predicate, path = "...")]`.
fn path_attributes(attrs: &[Attribute]) -> (Vec<String>, Vec<String>) {
    let mut direct = Vec::new();
    let mut conditional = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("path") {
            if let Ok(value) = attr.meta.require_name_value() {
                direct.extend(string_literal(&value.value));
            }
        } else if attr.path().is_ident("cfg_attr") {
            let parsed = attr.parse_args_with(
                syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
            );
            for meta in parsed.iter().flat_map(|metas| metas.iter().skip(1)) {
                if let syn::Meta::NameValue(value) = meta
                    && value.path.is_ident("path")
                {
                    conditional.extend(string_literal(&value.value));
                }
            }
        }
    }
    (direct, conditional)
}

fn string_literal(expr: &syn::Expr) -> Option<String> {
    match expr {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(s),
            ..
        }) => Some(s.value()),
        _ => None,
    }
}

/// `name` without a leading `r#`.
pub(crate) fn strip_raw(name: &str) -> &str {
    name.strip_prefix("r#").unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::{FileItems, ItemKind, parse_expansion};

    /// The named items of `out`, in order.
    fn named(out: &FileItems) -> Vec<(Vec<String>, ItemKind, bool, Option<String>)> {
        out.items
            .iter()
            .map(|d| (d.path.clone(), d.kind, d.public, d.signature.clone()))
            .collect()
    }

    /// The re-exports of `out`, in order.
    fn reexports(out: &FileItems) -> Vec<(Vec<String>, Option<String>, bool)> {
        out.uses
            .iter()
            .map(|u| (u.source.clone(), u.name.clone(), u.public))
            .collect()
    }

    #[test]
    fn a_malformed_trailing_item_fails_the_whole_expansion() {
        let source = "pub fn ok() -> u8 {\n    1\n}\nstruct;\n";
        assert!(parse_expansion(source).is_err());
        let unterminated = "pub fn ok() -> u8 {\n    1\n}\nfn broken(\n";
        assert!(parse_expansion(unterminated).is_err());
    }

    #[test]
    fn a_multi_item_expansion_keeps_its_declarations_and_signatures() {
        let source = "\n#![doc = \"Geometry\"]\npub use crate::geo::{Circle, Circle as Ring};\npub trait Shape: Default {\n    fn area(&self) -> f64;\n}\npub struct Point {\n    pub x: u32,\n    pub y: u32,\n}\npub fn distance(a: u32, b: &u32) -> f64 {\n    0.\n}\nimpl Point {\n    pub fn norm(self, scale: u32) -> f64 {\n        0.\n    }\n}\n";
        let out = parse_expansion(source).expect("a valid expansion parses");
        assert_eq!(
            named(&out),
            vec![
                (vec!["Shape".into()], ItemKind::Trait, true, None),
                (
                    vec!["Shape".into(), "area".into()],
                    ItemKind::Method,
                    true,
                    Some("(& self) -> f64".into())
                ),
                (vec!["Point".into()], ItemKind::Struct, true, None),
                (
                    vec!["Point".into(), "x".into()],
                    ItemKind::Field,
                    true,
                    None
                ),
                (
                    vec!["Point".into(), "y".into()],
                    ItemKind::Field,
                    true,
                    None
                ),
                (
                    vec!["distance".into()],
                    ItemKind::Function,
                    true,
                    Some("(u32, & u32) -> f64".into())
                ),
            ]
        );
        assert_eq!(
            reexports(&out),
            vec![
                (
                    vec!["crate".into(), "geo".into(), "Circle".into()],
                    Some("Circle".into()),
                    true
                ),
                (
                    vec!["crate".into(), "geo".into(), "Circle".into()],
                    Some("Ring".into()),
                    true
                ),
            ]
        );
        assert_eq!(out.impls[0].self_ty, ["Point"]);
        assert_eq!(
            out.impls[0].items,
            [(
                "norm".into(),
                ItemKind::Method,
                true,
                Some("(self, u32) -> f64".into())
            )]
        );
        assert_eq!(out.lends[0].owner, ["Shape"]);
        assert_eq!(out.lends[0].source, ["Default"]);
        assert_eq!(out.params, ["a", "b", "scale"]);
    }
}
