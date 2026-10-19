//! What one Rust source file declares, independent of where it sits in a crate.

use proc_macro2::{TokenStream, TokenTree};
use serde::{Deserialize, Serialize};
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
    /// Inherent impl blocks.
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
    /// `Deref` impls with a plain path target.
    pub(crate) derefs: Vec<DerefImpl>,
    /// The parameter names of every function, method and trait method, deduplicated.
    pub(crate) params: Vec<String>,
}

/// A named item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Decl {
    pub(crate) path: Vec<String>,
    pub(crate) kind: ItemKind,
    /// Whether the item is `pub`, members counting only when their owner is too.
    pub(crate) public: bool,
}

/// An inherent impl block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Impl {
    pub(crate) module: Vec<String>,
    /// The self type's path segments without generic arguments.
    pub(crate) self_ty: Vec<String>,
    /// Each item's name, kind and whether it is `pub`.
    pub(crate) items: Vec<(String, ItemKind, bool)>,
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
    pub(crate) idents: Vec<String>,
}

/// A `macro_rules!` definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct MacroRules {
    pub(crate) module: Vec<String>,
    pub(crate) name: String,
    pub(crate) idents: Vec<String>,
    pub(crate) exported: bool,
}

/// An `impl Deref for SelfTy { type Target = Target; }`, whose target's members the self type
/// reaches by auto-deref.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DerefImpl {
    pub(crate) module: Vec<String>,
    pub(crate) self_ty: Vec<String>,
    pub(crate) target: Vec<String>,
}

/// The declarations of `source`, or the parser's message when it is not valid Rust.
pub(crate) fn parse(source: &str) -> Result<FileItems, String> {
    let parsed = syn::parse_file(source)
        .map(|file| {
            let mut collector = Collector {
                out: FileItems::default(),
                module: Vec::new(),
            };
            collector.items(&file.items);
            collector.out
        })
        .map_err(|error| {
            let start = error.span().start();
            format!("{}:{}: {error}", start.line, start.column + 1)
        });
    // Span locations keep every parsed source alive in a thread-local map until invalidated.
    proc_macro2::extra::invalidate_current_thread_spans();
    parsed
}

/// A member of an item: its name, kind, whether it is `pub`, and its serialized spellings.
type Member = (String, ItemKind, bool, Vec<String>);

/// Walks a file's items, tracking the enclosing inline modules.
struct Collector {
    out: FileItems,
    module: Vec<String>,
}

impl Collector {
    fn items(&mut self, items: &[Item]) {
        for item in items {
            match item {
                Item::Const(item) => self.decl(&item.ident.to_string(), ItemKind::Const, &item.vis),
                Item::Enum(item) => self.enumeration(item),
                Item::Fn(item) => {
                    self.decl(&item.sig.ident.to_string(), ItemKind::Function, &item.vis);
                    self.params(&item.sig);
                }
                Item::ForeignMod(foreign) => self.foreign_items(&foreign.items),
                Item::Impl(item) => self.impl_block(item),
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
                }
                Item::Trait(item) => self.trait_item(item),
                Item::TraitAlias(item) => {
                    self.decl(&item.ident.to_string(), ItemKind::TraitAlias, &item.vis);
                }
                Item::Type(item) => self.type_alias(item),
                Item::Union(item) => {
                    let container = Container::new(&item.attrs);
                    let fields = named_fields(&item.fields.named, &container);
                    self.owner(&item.ident.to_string(), ItemKind::Union, &item.vis, fields);
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
        let mut path = self.module.clone();
        path.extend(names.iter().map(|name| strip_raw(name).to_owned()));
        self.out.items.push(Decl { path, kind, public });
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
                    self.decl(&item.sig.ident.to_string(), ItemKind::Function, &item.vis);
                    self.params(&item.sig);
                }
                syn::ForeignItem::Static(item) => {
                    self.decl(&item.ident.to_string(), ItemKind::Static, &item.vis);
                }
                syn::ForeignItem::Type(item) => {
                    self.decl(&item.ident.to_string(), ItemKind::TypeAlias, &item.vis);
                }
                syn::ForeignItem::Macro(item) => self.invocation(&item.mac),
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
                exported,
            });
        } else if let Some(path) = include_path(&item.mac) {
            self.out.includes.push(Include {
                module: self.module.clone(),
                path,
            });
        } else {
            self.invocation(&item.mac);
        }
    }

    fn mod_item(&mut self, item: &syn::ItemMod) {
        let name = item.ident.to_string();
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
        for trait_item in &item.items {
            let (name, kind) = match trait_item {
                TraitItem::Const(c) => (c.ident.to_string(), ItemKind::AssociatedConst),
                TraitItem::Fn(f) => {
                    self.params(&f.sig);
                    (f.sig.ident.to_string(), ItemKind::Method)
                }
                TraitItem::Type(t) => (t.ident.to_string(), ItemKind::AssociatedType),
                TraitItem::Macro(m) => {
                    self.invocation(&m.mac);
                    continue;
                }
                _ => continue,
            };
            members.push((name, kind, true, Vec::new()));
        }
        self.owner(&item.ident.to_string(), ItemKind::Trait, &item.vis, members);
    }

    fn impl_block(&mut self, item: &syn::ItemImpl) {
        for impl_item in &item.items {
            match impl_item {
                ImplItem::Macro(m) => self.invocation(&m.mac),
                ImplItem::Fn(f) => self.params(&f.sig),
                _ => {}
            }
        }
        let Some(segments) = type_path(&item.self_ty) else {
            return;
        };
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
                self.out.derefs.push(DerefImpl {
                    module: self.module.clone(),
                    self_ty: segments,
                    target,
                });
            }
            return;
        }
        let items = item
            .items
            .iter()
            .filter_map(|impl_item| match impl_item {
                ImplItem::Const(c) => Some((&c.ident, ItemKind::AssociatedConst, &c.vis)),
                ImplItem::Fn(f) => Some((&f.sig.ident, ItemKind::Method, &f.vis)),
                ImplItem::Type(t) => Some((&t.ident, ItemKind::AssociatedType, &t.vis)),
                _ => None,
            })
            .map(|(ident, kind, vis)| {
                let name = strip_raw(&ident.to_string()).to_owned();
                (name, kind, is_pub(vis))
            })
            .collect();
        self.out.impls.push(Impl {
            module: self.module.clone(),
            self_ty: segments,
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
            if !self.out.params.contains(&name) {
                self.out.params.push(name);
            }
        }
    }

    fn invocation(&mut self, mac: &syn::Macro) {
        let Some(last) = mac.path.segments.last() else {
            return;
        };
        self.out.invocations.push(Invocation {
            module: self.module.clone(),
            name: strip_raw(&last.ident.to_string()).to_owned(),
            idents: idents(&mac.tokens),
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
        syn::Pat::Type(typed) => bound_names(&typed.pat, out),
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
    Some(
        path.path
            .segments
            .iter()
            .map(|s| strip_raw(&s.ident.to_string()).to_owned())
            .collect(),
    )
}

fn include_path(mac: &syn::Macro) -> Option<String> {
    if !mac.path.is_ident("include") {
        return None;
    }
    let literal: syn::LitStr = syn::parse2(mac.tokens.clone()).ok()?;
    Some(literal.value())
}

/// Every identifier in `tokens`, deduplicated.
fn idents(tokens: &TokenStream) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![tokens.clone()];
    while let Some(stream) = stack.pop() {
        for tree in stream {
            match tree {
                TokenTree::Ident(ident) => {
                    let name = strip_raw(&ident.to_string()).to_owned();
                    if !out.contains(&name) {
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
