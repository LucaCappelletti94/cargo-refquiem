//! What one Rust source file declares, independent of where it sits in a crate.

use proc_macro2::{TokenStream, TokenTree};
use serde::{Deserialize, Serialize};
use syn::{Attribute, Fields, ImplItem, Item, TraitItem, Type, UseTree, Visibility};

use crate::ItemKind;

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
}

/// A named item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Decl {
    pub(crate) path: Vec<String>,
    pub(crate) kind: ItemKind,
}

/// An inherent impl block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Impl {
    pub(crate) module: Vec<String>,
    /// The self type's path segments without generic arguments.
    pub(crate) self_ty: Vec<String>,
    pub(crate) items: Vec<(String, ItemKind)>,
}

/// A named import, type alias or glob re-export.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Use {
    pub(crate) module: Vec<String>,
    /// The imported path, `crate`, `self` and `super` kept as written.
    pub(crate) source: Vec<String>,
    /// The name created in `module`, `None` for a glob.
    pub(crate) name: Option<String>,
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
            let mut out = FileItems::default();
            collect(&file.items, &mut Vec::new(), &mut out);
            out
        })
        .map_err(|error| {
            let start = error.span().start();
            format!("{}:{}: {error}", start.line, start.column + 1)
        });
    // Span locations keep every parsed source alive in a thread-local map until invalidated.
    proc_macro2::extra::invalidate_current_thread_spans();
    parsed
}

fn collect(items: &[Item], module: &mut Vec<String>, out: &mut FileItems) {
    for item in items {
        match item {
            Item::Const(item) => decl(out, module, &item.ident.to_string(), ItemKind::Const),
            Item::Enum(item) => {
                let variants = item
                    .variants
                    .iter()
                    .map(|v| (v.ident.to_string(), ItemKind::Variant));
                owner(
                    out,
                    module,
                    &item.ident.to_string(),
                    ItemKind::Enum,
                    variants,
                );
            }
            Item::Fn(item) => decl(out, module, &item.sig.ident.to_string(), ItemKind::Function),
            Item::ForeignMod(foreign) => foreign_items(&foreign.items, module, out),
            Item::Impl(item) => impl_block(item, module, out),
            Item::Macro(item) => macro_item(item, module, out),
            Item::Mod(item) => mod_item(item, module, out),
            Item::Static(item) => decl(out, module, &item.ident.to_string(), ItemKind::Static),
            Item::Struct(item) => {
                let fields = match &item.fields {
                    Fields::Named(fields) => named_fields(&fields.named),
                    Fields::Unnamed(_) | Fields::Unit => Vec::new(),
                };
                owner(
                    out,
                    module,
                    &item.ident.to_string(),
                    ItemKind::Struct,
                    fields,
                );
            }
            Item::Trait(item) => trait_item(item, module, out),
            Item::TraitAlias(item) => {
                decl(out, module, &item.ident.to_string(), ItemKind::TraitAlias);
            }
            Item::Type(item) => type_alias(item, module, out),
            Item::Union(item) => {
                let fields = named_fields(&item.fields.named);
                owner(
                    out,
                    module,
                    &item.ident.to_string(),
                    ItemKind::Union,
                    fields,
                );
            }
            Item::Use(item) => {
                let public = !matches!(item.vis, Visibility::Inherited);
                uses(&item.tree, &mut Vec::new(), public, module, out);
            }
            _ => {}
        }
    }
}

fn foreign_items(items: &[syn::ForeignItem], module: &[String], out: &mut FileItems) {
    for item in items {
        match item {
            syn::ForeignItem::Fn(item) => {
                decl(out, module, &item.sig.ident.to_string(), ItemKind::Function);
            }
            syn::ForeignItem::Static(item) => {
                decl(out, module, &item.ident.to_string(), ItemKind::Static);
            }
            syn::ForeignItem::Type(item) => {
                decl(out, module, &item.ident.to_string(), ItemKind::TypeAlias);
            }
            syn::ForeignItem::Macro(item) => invocation(out, module, &item.mac),
            _ => {}
        }
    }
}

fn macro_item(item: &syn::ItemMacro, module: &[String], out: &mut FileItems) {
    if let Some(ident) = &item.ident {
        let name = ident.to_string();
        decl(out, module, &name, ItemKind::Macro);
        out.macro_rules.push(MacroRules {
            module: module.to_vec(),
            name,
            idents: idents(&item.mac.tokens),
            exported: item.attrs.iter().any(|a| a.path().is_ident("macro_export")),
        });
    } else if let Some(path) = include_path(&item.mac) {
        out.includes.push(Include {
            module: module.to_vec(),
            path,
        });
    } else {
        invocation(out, module, &item.mac);
    }
}

fn mod_item(item: &syn::ItemMod, module: &mut Vec<String>, out: &mut FileItems) {
    let name = item.ident.to_string();
    decl(out, module, &name, ItemKind::Module);
    if let Some((_, items)) = &item.content {
        module.push(name);
        collect(items, module, out);
        module.pop();
    } else {
        let (paths, conditional_paths) = path_attributes(&item.attrs);
        out.mods.push(ModDecl {
            module: module.clone(),
            name,
            paths,
            conditional_paths,
        });
    }
}

fn trait_item(item: &syn::ItemTrait, module: &[String], out: &mut FileItems) {
    let mut members = Vec::new();
    for trait_item in &item.items {
        match trait_item {
            TraitItem::Const(c) => members.push((c.ident.to_string(), ItemKind::AssociatedConst)),
            TraitItem::Fn(f) => members.push((f.sig.ident.to_string(), ItemKind::Method)),
            TraitItem::Type(t) => members.push((t.ident.to_string(), ItemKind::AssociatedType)),
            TraitItem::Macro(m) => invocation(out, module, &m.mac),
            _ => {}
        }
    }
    owner(
        out,
        module,
        &item.ident.to_string(),
        ItemKind::Trait,
        members,
    );
}

fn named_fields(
    fields: &syn::punctuated::Punctuated<syn::Field, syn::Token![,]>,
) -> Vec<(String, ItemKind)> {
    fields
        .iter()
        .filter_map(|field| field.ident.as_ref())
        .map(|ident| (ident.to_string(), ItemKind::Field))
        .collect()
}

fn decl(out: &mut FileItems, module: &[String], name: &str, kind: ItemKind) {
    if name == "_" {
        return;
    }
    let mut path = module.to_vec();
    path.push(strip_raw(name).to_owned());
    out.items.push(Decl { path, kind });
}

/// Declares `name` and the `members` below it.
fn owner(
    out: &mut FileItems,
    module: &[String],
    name: &str,
    kind: ItemKind,
    members: impl IntoIterator<Item = (String, ItemKind)>,
) {
    decl(out, module, name, kind);
    let mut path = module.to_vec();
    path.push(strip_raw(name).to_owned());
    for (member, kind) in members {
        path.push(strip_raw(&member).to_owned());
        out.items.push(Decl {
            path: path.clone(),
            kind,
        });
        path.pop();
    }
}

fn impl_block(item: &syn::ItemImpl, module: &[String], out: &mut FileItems) {
    for impl_item in &item.items {
        if let ImplItem::Macro(m) = impl_item {
            invocation(out, module, &m.mac);
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
            out.derefs.push(DerefImpl {
                module: module.to_vec(),
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
            ImplItem::Const(c) => Some((c.ident.to_string(), ItemKind::AssociatedConst)),
            ImplItem::Fn(f) => Some((f.sig.ident.to_string(), ItemKind::Method)),
            ImplItem::Type(t) => Some((t.ident.to_string(), ItemKind::AssociatedType)),
            _ => None,
        })
        .map(|(name, kind)| (strip_raw(&name).to_owned(), kind))
        .collect();
    out.impls.push(Impl {
        module: module.to_vec(),
        self_ty: segments,
        items,
    });
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

/// A type alias declares its name and, for a plain path target, aliases the target's members.
fn type_alias(item: &syn::ItemType, module: &[String], out: &mut FileItems) {
    let name = item.ident.to_string();
    decl(out, module, &name, ItemKind::TypeAlias);
    if let Some(source) = type_path(&item.ty) {
        out.uses.push(Use {
            module: module.to_vec(),
            source,
            name: Some(strip_raw(&name).to_owned()),
        });
    }
}

fn uses(
    tree: &UseTree,
    prefix: &mut Vec<String>,
    public: bool,
    module: &[String],
    out: &mut FileItems,
) {
    match tree {
        UseTree::Path(path) => {
            prefix.push(strip_raw(&path.ident.to_string()).to_owned());
            uses(&path.tree, prefix, public, module, out);
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
                    module: module.to_vec(),
                    source,
                    name,
                };
                if public {
                    out.uses.push(decl);
                } else {
                    out.imports.push(decl);
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
            out.uses.push(Use {
                module: module.to_vec(),
                source,
                name: Some(alias),
            });
        }
        UseTree::Glob(_) if public => out.uses.push(Use {
            module: module.to_vec(),
            source: prefix.clone(),
            name: None,
        }),
        UseTree::Group(group) => {
            for tree in &group.items {
                uses(tree, prefix, public, module, out);
            }
        }
        UseTree::Glob(_) => {}
    }
}

fn invocation(out: &mut FileItems, module: &[String], mac: &syn::Macro) {
    let Some(last) = mac.path.segments.last() else {
        return;
    };
    out.invocations.push(Invocation {
        module: module.to_vec(),
        name: strip_raw(&last.ident.to_string()).to_owned(),
        idents: idents(&mac.tokens),
    });
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
