//! Manifest entries share the path index as `[<member:DIR>, <kind>, name]`, segments no
//! identifier can spell.

use crate::ItemKind;

/// The first segment of every entry of the member in `dir`.
pub(crate) fn member(dir: &str) -> String {
    format!("<member:{dir}>")
}

/// The member directory of a first segment written by [`member`].
pub(crate) fn member_dir(segment: &str) -> Option<&str> {
    segment.strip_prefix("<member:")?.strip_suffix('>')
}

/// The second segment of entries of `kind`, `None` for the kinds that have no manifest entry.
pub(crate) fn marker(kind: ItemKind) -> Option<&'static str> {
    match kind {
        ItemKind::Package => Some("<package>"),
        ItemKind::Dependency => Some("<dependency>"),
        ItemKind::Feature => Some("<feature>"),
        ItemKind::Binary => Some("<binary>"),
        ItemKind::Example => Some("<example>"),
        ItemKind::Module
        | ItemKind::Struct
        | ItemKind::Enum
        | ItemKind::Union
        | ItemKind::Trait
        | ItemKind::TraitAlias
        | ItemKind::TypeAlias
        | ItemKind::Const
        | ItemKind::Static
        | ItemKind::Function
        | ItemKind::Macro
        | ItemKind::Variant
        | ItemKind::Field
        | ItemKind::Method
        | ItemKind::AssociatedConst
        | ItemKind::AssociatedType
        | ItemKind::Reexport
        | ItemKind::Serialized => None,
    }
}
