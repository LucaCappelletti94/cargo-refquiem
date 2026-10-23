//! Indexing of every declaration kind, of re-exports, serde spellings and manifests.

mod common;
#[path = "common/vendor.rs"]
mod vendor;

use common::Repo;
use refquiem::{ItemKind, Range, RemovedIn};

/// Runs refquiem over the full history against the working tree.
fn run(repo: &Repo) -> refquiem::Report {
    refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap()
}

/// The removed paths that still have mentions, sorted.
fn removed(report: &refquiem::Report) -> Vec<String> {
    let mut out: Vec<String> = report.groups.iter().map(|g| g.path.clone()).collect();
    out.sort();
    out
}

/// The kind of the group `path`.
fn kind_of(report: &refquiem::Report, path: &str) -> ItemKind {
    report.groups.iter().find(|g| g.path == path).unwrap().kind
}

/// Writes `Cargo.lock` without touching the network, as every dependency is a path dependency
/// or vendored.
fn lock(repo: &Repo) {
    let status = std::process::Command::new("cargo")
        .args(["generate-lockfile", "--offline", "--quiet"])
        .current_dir(repo.path())
        .status()
        .unwrap();
    assert!(status.success());
}

const BARE: &str = "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

#[test]
fn items_of_raw_named_modules_are_found_by_their_plain_path() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub mod r#type {\n    pub struct Gone;\n}\npub mod r#async;\n",
    );
    repo.write("src/async.rs", "pub struct Away;\n");
    repo.commit("raw modules");
    repo.write("src/lib.rs", "");
    repo.write(
        "README.md",
        "`fixture::r#type::Gone` and `fixture::r#async::Away`\n",
    );
    repo.commit("drop them");

    assert_eq!(
        removed(&run(&repo)),
        ["fixture::async::Away", "fixture::type::Gone"]
    );
}

#[test]
fn a_removed_const_is_reported() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub const ANSWER: u32 = 1;\n");
    repo.commit("const");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "The old `ANSWER`.\n");
    repo.commit("drop const");

    let report = run(&repo);

    assert_eq!(removed(&report), ["fixture::ANSWER"]);
    assert_eq!(kind_of(&report, "fixture::ANSWER"), ItemKind::Const);
}

#[test]
fn a_removed_static_is_reported() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub static COUNTER: u32 = 0;\n");
    repo.commit("static");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "The old `COUNTER`.\n");
    repo.commit("drop static");

    let report = run(&repo);

    assert_eq!(removed(&report), ["fixture::COUNTER"]);
    assert_eq!(kind_of(&report, "fixture::COUNTER"), ItemKind::Static);
}

#[test]
fn a_removed_trait_alias_is_reported() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub trait FilterBox = Fn(u32) -> bool;\n");
    repo.commit("trait alias");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "The old `FilterBox`.\n");
    repo.commit("drop trait alias");

    let report = run(&repo);

    assert_eq!(removed(&report), ["fixture::FilterBox"]);
    assert_eq!(kind_of(&report, "fixture::FilterBox"), ItemKind::TraitAlias);
}

#[test]
fn a_removed_union_is_reported() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub union ByteBag {\n    i: u32,\n    f: f32,\n}\n",
    );
    repo.commit("union");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "The old `ByteBag`.\n");
    repo.commit("drop union");

    let report = run(&repo);

    assert_eq!(removed(&report), ["fixture::ByteBag"]);
    assert_eq!(kind_of(&report, "fixture::ByteBag"), ItemKind::Union);
}

#[test]
fn items_of_a_removed_extern_block_are_reported() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "macro_rules! c_make {\n    () => {\n        struct Generated;\n    };\n}\n\nextern \"C\" {\n    pub fn c_free();\n    pub static mut c_buffer: u32;\n    pub type c_handle;\n    c_make!();\n}\n\npub struct Generated;\n",
    );
    repo.commit("extern block");
    repo.write(
        "src/lib.rs",
        "macro_rules! c_make {\n    () => {\n        struct Generated;\n    };\n}\n\nextern \"C\" {\n    c_make!();\n}\n",
    );
    repo.write(
        "README.md",
        "`c_free`, `c_buffer`, `c_handle` and `Generated`.\n",
    );
    repo.commit("drop extern block");

    let report = run(&repo);

    assert_eq!(
        removed(&report),
        ["fixture::c_buffer", "fixture::c_free", "fixture::c_handle"]
    );
    assert_eq!(kind_of(&report, "fixture::c_free"), ItemKind::Function);
    assert_eq!(kind_of(&report, "fixture::c_buffer"), ItemKind::Static);
    assert_eq!(kind_of(&report, "fixture::c_handle"), ItemKind::TypeAlias);
}

#[test]
fn trait_members_are_reported_and_a_trait_macro_keeps_its_names_alive() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "macro_rules! draw {\n    () => {\n        struct Rendered;\n    };\n}\n\npub trait Draw {\n    const COLOR: u32 = 0;\n    fn render(&self);\n    type Palette;\n    draw!();\n}\n\npub struct Rendered;\n",
    );
    repo.commit("trait");
    repo.write(
        "src/lib.rs",
        "macro_rules! draw {\n    () => {\n        struct Rendered;\n    };\n}\n\npub trait Draw {\n    draw!();\n}\n",
    );
    repo.write(
        "README.md",
        "`fixture::Draw`, `Draw::COLOR`, `Draw::render`, `Draw::Palette` and `Rendered`.\n",
    );
    repo.commit("drop trait members");

    let report = run(&repo);

    assert_eq!(
        removed(&report),
        [
            "fixture::Draw::COLOR",
            "fixture::Draw::Palette",
            "fixture::Draw::render"
        ]
    );
    assert_eq!(
        kind_of(&report, "fixture::Draw::COLOR"),
        ItemKind::AssociatedConst
    );
    assert_eq!(kind_of(&report, "fixture::Draw::render"), ItemKind::Method);
    assert_eq!(
        kind_of(&report, "fixture::Draw::Palette"),
        ItemKind::AssociatedType
    );
}

#[test]
fn associated_items_of_a_removed_impl_are_reported() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct Widget;\n\nimpl Widget {\n    pub const LIMIT: u32 = 0;\n    pub type Kind = u32;\n}\n",
    );
    repo.commit("impl items");
    repo.write("src/lib.rs", "pub struct Widget;\n");
    repo.write("README.md", "`Widget::LIMIT` and `Widget::Kind`.\n");
    repo.commit("drop impl items");

    let report = run(&repo);

    assert_eq!(
        removed(&report),
        ["fixture::Widget::Kind", "fixture::Widget::LIMIT"]
    );
    assert_eq!(
        kind_of(&report, "fixture::Widget::LIMIT"),
        ItemKind::AssociatedConst
    );
    assert_eq!(
        kind_of(&report, "fixture::Widget::Kind"),
        ItemKind::AssociatedType
    );
}

#[test]
fn a_removed_struct_is_kept_alive_by_a_macro_in_its_impl() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "macro_rules! spin {\n    () => {\n        struct Spinner;\n    };\n}\n\npub struct Frame;\n\nimpl Frame {\n    spin!();\n}\n\npub struct Spinner;\n",
    );
    repo.commit("impl macro");
    repo.write(
        "src/lib.rs",
        "macro_rules! spin {\n    () => {\n        struct Spinner;\n    };\n}\n\npub struct Frame;\n\nimpl Frame {\n    spin!();\n}\n",
    );
    repo.write("README.md", "`Spinner`.\n");
    repo.commit("drop Spinner");

    assert_eq!(removed(&run(&repo)), [] as [&str; 0]);
}

#[test]
fn a_method_parameter_name_of_head_is_live() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn needle() {}\n");
    repo.commit("needle");
    repo.write(
        "src/lib.rs",
        "pub struct Frame;\n\nimpl Frame {\n    pub fn method(needle: u8) {}\n}\n",
    );
    repo.write("README.md", "`needle`.\n");
    repo.commit("drop needle");

    assert_eq!(removed(&run(&repo)), [] as [&str; 0]);
}

#[test]
fn a_deref_target_is_read_from_its_target_associated_type() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct Real;\n\nimpl Real {\n    pub fn real_method() {}\n}\n\npub struct OtherThing;\n\npub struct W;\n\nimpl std::ops::Deref for W {\n    type Other = OtherThing;\n    type Target = Real;\n}\n",
    );
    repo.commit("deref");
    repo.write(
        "src/lib.rs",
        "pub struct Real;\n\npub struct OtherThing;\n\npub struct W;\n\nimpl std::ops::Deref for W {\n    type Other = OtherThing;\n    type Target = Real;\n}\n",
    );
    repo.write("README.md", "`W::real_method`.\n");
    repo.commit("drop real_method");

    let report = run(&repo);

    assert_eq!(removed(&report), ["fixture::W::real_method"]);
    assert_eq!(
        kind_of(&report, "fixture::W::real_method"),
        ItemKind::Method
    );
}

#[test]
fn private_uses_do_not_create_reported_paths() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub mod other {\n    pub struct Thing;\n}\n\nuse other::Thing;\nuse other::*;\n",
    );
    repo.commit("private uses");
    repo.write("src/lib.rs", "pub mod other {\n    pub struct Thing;\n}\n");
    repo.write("README.md", "`fixture::Thing`.\n");
    repo.commit("drop private uses");

    assert_eq!(removed(&run(&repo)), [] as [&str; 0]);
}

#[test]
fn trees_with_the_same_items_but_another_glob_source_reexport_differently() {
    let repo = Repo::new();
    let lib = |source: &str| {
        format!(
            "mod a {{\n    pub fn kept() {{}}\n}}\nmod b {{\n    pub fn moved() {{}}\n}}\npub use {source}::*;\n"
        )
    };
    repo.write("src/lib.rs", &lib("a"));
    repo.commit("glob from a");
    repo.write("src/lib.rs", &lib("b"));
    repo.commit("glob from b");
    repo.write("src/lib.rs", &lib("a"));
    repo.write("README.md", "`fixture::moved`\n");
    repo.commit("glob from a again");

    assert_eq!(removed(&run(&repo)), ["fixture::moved"]);
}

#[test]
fn successive_globs_keep_each_destinations_nested_items() {
    let repo = Repo::new();
    let lib = |left, right| {
        format!(
            "pub mod left {{ pub mod nested_left {{ {left} }} }}\npub mod right {{ pub mod nested_right {{ {right} pub fn kept() {{}} }} }}\npub mod first {{ pub use crate::left::*; pub use crate::right::*; }}\npub mod second {{ pub use crate::left::*; }}\n"
        )
    };
    repo.write(
        "src/lib.rs",
        &lib("pub fn gone_left() {}", "pub fn gone_right() {}"),
    );
    repo.commit("nested glob sources");
    repo.write("src/lib.rs", &lib("", ""));
    repo.write(
        "README.md",
        "`fixture::first::nested_left::gone_left`, `fixture::second::nested_left::gone_left`, `fixture::first::nested_right::gone_right` and `fixture::first::nested_right::kept`.\n",
    );
    repo.commit("remove the nested functions");

    assert_eq!(
        removed(&run(&repo)),
        [
            "fixture::first::nested_left::gone_left",
            "fixture::first::nested_right::gone_right",
            "fixture::second::nested_left::gone_left",
        ]
    );
}

#[test]
fn a_module_reexporting_its_parent_does_not_nest_the_crate_into_itself() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub mod alias {\n    pub use super::*;\n}\n\npub mod zone {\n    pub fn gone() {}\n}\n",
    );
    repo.commit("self alias");
    repo.write("src/lib.rs", "pub mod zone {}\n");
    repo.write("README.md", "`gone`\n");
    repo.commit("drop gone");

    assert_eq!(removed(&run(&repo)), ["fixture::alias::zone::gone"]);
}

#[test]
fn a_path_returning_through_aliases_to_a_module_it_passed_is_not_indexed() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub mod tour;\n");
    repo.write("src/tour/mod.rs", "pub mod c0;\npub mod c1;\npub mod c2;\n");
    repo.write("src/tour/c0.rs", "pub use super::c1 as next;\n");
    repo.write(
        "src/tour/c1.rs",
        "pub fn step() {}\npub use super::c0 as previous;\npub use super::c2 as next;\n",
    );
    repo.write("src/tour/c2.rs", "pub use super::c1 as previous;\n");
    repo.commit("tour");
    repo.write(
        "src/tour/c1.rs",
        "pub use super::c0 as previous;\npub use super::c2 as next;\n",
    );
    repo.write(
        "README.md",
        "`fixture::tour::c0::next::next::previous::step` and `fixture::tour::c0::next::step`\n",
    );
    repo.commit("drop step");

    let report = run(&repo);
    let mentioned: Vec<&str> = report
        .groups
        .iter()
        .flat_map(|g| g.mentions.iter().map(|m| m.text.as_str()))
        .collect();
    assert_eq!(mentioned, ["fixture::tour::c0::next::step"]);
}

const CFG_MACROS: &str = "macro_rules! cfg_sync {\n    ($($item:item)*) => {\n        $(\n            #[cfg(feature = \"sync\")]\n            $item\n        )*\n    };\n}\n\nmacro_rules! feature {\n    (#![$meta:meta] $($item:item)*) => {\n        $(\n            #[cfg($meta)]\n            $item\n        )*\n    };\n}\n";

#[test]
fn item_macros_among_impl_items_add_members() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "mod net {\n    pub struct Listener;\n    impl Listener {\n        pub fn bind() {}\n        pub fn open() {}\n    }\n}\npub use net::Listener;\n",
    );
    repo.write(
        "README.md",
        "`fixture::Listener::bind` and `fixture::Listener::open`.\n",
    );
    repo.commit("plain members");
    repo.write(
        "src/lib.rs",
        "macro_rules! cfg_net {\n    ($($i:item)*) => { $(#[cfg(feature = \"net\")] $i)* };\n}\nmod net {\n    pub struct Listener;\n    impl Listener {\n        cfg_net! { pub fn bind() {} }\n        undefined_mac! { pub fn open() {} }\n    }\n}\npub use net::Listener;\n",
    );
    repo.commit("members inside macros");

    assert_eq!(removed(&run(&repo)), Vec::<String>::new());
}

#[test]
fn modules_declared_inside_item_macros_are_indexed() {
    let repo = Repo::new();
    repo.write("src/macros.rs", CFG_MACROS);
    repo.write(
        "src/lib.rs",
        "#[macro_use]\nmod macros;\n\npub mod old {\n    pub struct Mutex;\n    pub struct Layer;\n}\n\ncfg_sync! {\n    pub mod sync;\n}\n\nfeature! {\n    #![feature = \"fmt\"]\n    pub mod fmt;\n}\n",
    );
    repo.write("src/sync.rs", "pub struct Mutex;\npub struct Gone;\n");
    repo.write("src/fmt.rs", "pub struct Layer;\n");
    repo.commit("macro modules");
    repo.write(
        "src/lib.rs",
        "#[macro_use]\nmod macros;\n\ncfg_sync! {\n    pub mod sync;\n}\n\nfeature! {\n    #![feature = \"fmt\"]\n    pub mod fmt;\n}\n",
    );
    repo.write("src/sync.rs", "pub struct Mutex;\n");
    repo.write(
        "README.md",
        "`Mutex`, `sync::Mutex`, `fmt::Layer` and `fixture::sync::Gone`\n",
    );
    repo.commit("drop old and Gone");

    assert_eq!(removed(&run(&repo)), ["fixture::sync::Gone"]);
}

#[test]
fn items_a_macro_assembles_from_its_arguments_are_indexed() {
    let repo = Repo::new();
    repo.write(
        "src/macros.rs",
        "macro_rules! ast_struct {\n    (\n        $(#[$attr:meta])*\n        $pub:ident $struct:ident $name:ident #full $body:tt\n    ) => {\n        check_keyword_matches!(pub $pub);\n        $(#[$attr])* $pub $struct $name $body\n    };\n}\n\nmacro_rules! check_keyword_matches {\n    (pub pub) => {};\n}\n",
    );
    repo.write("src/lib.rs", "#[macro_use]\nmod macros;\npub mod expr;\n");
    repo.write(
        "src/expr.rs",
        "ast_struct! {\n    /// An array.\n    pub struct ExprArray #full {\n        pub elems: Vec<u8>,\n        pub gone: u8,\n    }\n}\n",
    );
    repo.commit("ast");
    repo.write(
        "src/expr.rs",
        "ast_struct! {\n    /// An array.\n    pub struct ExprArray #full {\n        pub elems: Vec<u8>,\n    }\n}\n",
    );
    repo.write("README.md", "`ExprArray::elems` and `ExprArray::gone`\n");
    repo.commit("drop gone");

    assert_eq!(removed(&run(&repo)), ["fixture::expr::ExprArray::gone"]);
}

#[test]
fn a_macro_without_arguments_declares_the_items_of_its_body() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub mod legacy {\n    pub mod de {\n        pub trait Deserialize {}\n    }\n}\n\nmacro_rules! crate_root {\n    () => {\n        pub mod de;\n        pub use self::de::Deserialize;\n    };\n}\n\ncrate_root!();\n",
    );
    repo.write("src/de.rs", "pub trait Deserialize {}\npub fn gone() {}\n");
    repo.commit("crate root");
    repo.write(
        "src/lib.rs",
        "macro_rules! crate_root {\n    () => {\n        pub mod de;\n        pub use self::de::Deserialize;\n    };\n}\n\ncrate_root!();\n",
    );
    repo.write("src/de.rs", "pub trait Deserialize {}\n");
    repo.write(
        "README.md",
        "`de::Deserialize`, `fixture::Deserialize` and `fixture::de::gone`\n",
    );
    repo.commit("drop legacy and gone");

    assert_eq!(removed(&run(&repo)), ["fixture::de::gone"]);
}

#[test]
fn a_changed_macro_body_expands_its_unchanged_invocation_anew_on_every_scan() {
    let repo = Repo::new();
    let lib = |name: &str| {
        format!(
            "macro_rules! make {{\n    () => {{\n        pub fn {name}() {{}}\n    }};\n}}\n\nmake!();\n"
        )
    };
    repo.write("src/lib.rs", &lib("first"));
    repo.commit("first");
    repo.write("src/lib.rs", &lib("second"));
    repo.write("README.md", "`fixture::first`\n");
    repo.commit("second");

    assert_eq!(removed(&run(&repo)), ["fixture::first"]);
    assert_eq!(removed(&run(&repo)), ["fixture::first"]);
}

#[test]
fn an_expansion_shared_by_two_inline_modules_indexes_its_items_under_each() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "macro_rules! pack {\n    ($name:ident) => {\n        pub mod $name {\n            pub struct Kept;\n            pub struct Gone;\n        }\n    };\n}\n\npub mod alpha {\n    pack!(inner);\n}\n\npub mod beta {\n    pack!(inner);\n}\n",
    );
    repo.commit("packed");
    repo.write("src/lib.rs", "");
    repo.write(
        "README.md",
        "`fixture::alpha::inner::Gone` and `fixture::beta::inner::Gone`\n",
    );
    repo.commit("drop the packs");

    assert_eq!(
        removed(&run(&repo)),
        ["fixture::alpha::inner::Gone", "fixture::beta::inner::Gone"]
    );
    assert_eq!(
        removed(&run(&repo)),
        ["fixture::alpha::inner::Gone", "fixture::beta::inner::Gone"]
    );
}

#[test]
fn a_macro_invoked_in_two_impls_indexes_its_items_under_each_receiver() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct Alpha;\npub struct Beta;\n\nmacro_rules! methods {\n    () => {\n        pub fn kept(&self) {}\n        pub fn gone(&self) {}\n    };\n}\n\nimpl Alpha {\n    methods!();\n}\n\nimpl Beta {\n    methods!();\n}\n",
    );
    repo.commit("methods");
    repo.write("src/lib.rs", "");
    repo.write(
        "README.md",
        "`fixture::Alpha::gone` and `fixture::Beta::gone`\n",
    );
    let deleting = repo.commit("drop the impls");

    let report = run(&repo);
    assert_eq!(
        removed(&report),
        ["fixture::Alpha::gone", "fixture::Beta::gone"]
    );
    let removed_in = Some(RemovedIn::Commit(deleting));
    assert_eq!(report.groups[0].removed_in, removed_in);
    assert_eq!(report.groups[1].removed_in, removed_in);
    assert_eq!(
        removed(&run(&repo)),
        ["fixture::Alpha::gone", "fixture::Beta::gone"]
    );
}

#[test]
fn an_unmatched_macro_invocation_expands_to_nothing_on_every_scan() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub struct Gone;\n");
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "macro_rules! only_alpha {\n    (alpha) => {\n        pub struct Alpha;\n    };\n}\n\nonly_alpha!(beta);\n",
    );
    repo.write("README.md", "`fixture::Gone`\n");
    repo.commit("unmatched");

    assert_eq!(removed(&run(&repo)), ["fixture::Gone"]);
    assert_eq!(removed(&run(&repo)), ["fixture::Gone"]);
}

#[test]
fn a_glob_out_of_a_macro_generated_module_reexports_its_source() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub mod helper_types {\n    pub struct Eq;\n    pub struct Gone;\n}\n\npub mod dsl {\n    pub use crate::helper_types::*;\n}\n",
    );
    repo.commit("plain glob");
    repo.write(
        "src/lib.rs",
        "macro_rules! make_proxy_mod {\n    ($name:ident, $path:path) => {\n        #[doc(hidden)]\n        pub mod $name {\n            pub use $path::*;\n        }\n    };\n}\n\npub mod helper_types {\n    pub struct Eq;\n}\n\npub mod dsl {\n    make_proxy_mod!(helper_types_proxy, crate::helper_types);\n    pub use self::helper_types_proxy::*;\n}\n",
    );
    repo.write("README.md", "`dsl::Eq` and `fixture::dsl::Gone`\n");
    repo.commit("proxy module");

    assert_eq!(removed(&run(&repo)), ["fixture::dsl::Gone"]);
}

const MAKE: &str = "macro_rules! make {\n    (unit $name:ident) => {\n        pub struct $name;\n    };\n    ($vis:vis enum $name:ident { $($variant:ident),* $(,)? } $(alias $alias:ident)?) => {\n        $vis enum $name { $($variant),* }\n        $(pub use self::$name as $alias;)?\n    };\n    ($($module:ident { $($item:ident),* });+ ;) => {\n        $(pub mod $module { $(pub struct $item;)* })+\n    };\n}\n";

#[test]
fn a_macro_expands_by_its_first_matching_rule_and_every_repetition() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        &format!(
            "{MAKE}make!(unit Lone);\nmake!(pub enum Color {{ Red, Green, Blue }} alias Colour);\nmake!(shapes {{ Circle, Square, Triangle }}; sizes {{ Big, Small }};);\n"
        ),
    );
    repo.commit("made");
    repo.write(
        "src/lib.rs",
        &format!(
            "{MAKE}make!(unit Lone);\nmake!(pub enum Color {{ Red, Green, }});\nmake!(shapes {{ Circle, Square }}; sizes {{ Big }};);\n"
        ),
    );
    repo.write(
        "README.md",
        "`fixture::Lone`, `fixture::Color::Red`, `fixture::shapes::Circle`, `fixture::sizes::Big`, `fixture::Color::Blue`, `fixture::Colour`, `fixture::shapes::Triangle` and `fixture::sizes::Small`\n",
    );
    repo.commit("trimmed");

    assert_eq!(
        removed(&run(&repo)),
        [
            "fixture::Color::Blue",
            "fixture::Colour",
            "fixture::shapes::Triangle",
            "fixture::sizes::Small"
        ]
    );
}

#[test]
fn a_macro_invoking_itself_expands_each_level() {
    let repo = Repo::new();
    let each = "macro_rules! each {\n    () => {};\n    ($head:ident $($rest:ident)*) => {\n        pub struct $head;\n        each!($($rest)*);\n    };\n}\n";
    repo.write("src/lib.rs", &format!("{each}each!(A B C);\n"));
    repo.commit("three");
    repo.write("src/lib.rs", &format!("{each}each!(A B);\n"));
    repo.write("README.md", "`fixture::B` and `fixture::C`\n");
    repo.commit("two");

    assert_eq!(removed(&run(&repo)), ["fixture::C"]);
}

#[test]
fn every_definition_of_a_macro_name_is_expanded() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub mod old {\n    pub struct AlphaOne;\n    pub struct BetaTwo;\n}\n",
    );
    repo.commit("plain");
    repo.write(
        "src/lib.rs",
        "#[cfg(feature = \"a\")]\nmacro_rules! decl {\n    () => {\n        pub struct AlphaOne;\n    };\n}\n\n#[cfg(not(feature = \"a\"))]\nmacro_rules! decl {\n    () => {\n        pub struct BetaTwo;\n    };\n}\n\ndecl!();\n",
    );
    repo.write("README.md", "`AlphaOne` and `BetaTwo`\n");
    repo.commit("per cfg");

    assert_eq!(removed(&run(&repo)), [] as [&str; 0]);
}

#[test]
fn a_macro_defined_in_a_module_shadows_same_named_macros_elsewhere() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub mod first {\n    macro_rules! impl_case {\n        () => {};\n        ($head:ident $($rest:ident)*) => {\n            pub struct $head;\n            impl_case!($($rest)*);\n        };\n    }\n    impl_case!(A B);\n}\n\npub mod second {\n    macro_rules! impl_case {\n        ($name:ident $($rest:ident)*) => {\n            pub mod $name {}\n        };\n    }\n    impl_case!(C D);\n}\n",
    );
    repo.commit("local macros");
    repo.write("src/lib.rs", "pub mod first {}\npub mod second {}\n");
    repo.write(
        "README.md",
        "`fixture::first::A`, `fixture::first::B`, `fixture::second::C`, `fixture::first::C`, `fixture::second::A` and `fixture::second::D`\n",
    );
    repo.commit("drop the calls");

    assert_eq!(
        removed(&run(&repo)),
        [
            "fixture::first::A",
            "fixture::first::B",
            "fixture::second::C"
        ]
    );
}

#[test]
fn the_items_of_an_unknown_macro_are_read_from_its_tokens() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "unknown! {\n    pub struct Gone;\n    pub mod sub;\n}\n",
    );
    repo.write("src/sub.rs", "pub struct Deep;\n");
    repo.commit("unknown");
    repo.write("src/lib.rs", "unknown! {\n    pub mod sub;\n}\n");
    repo.write("src/sub.rs", "");
    repo.write("README.md", "`fixture::Gone` and `fixture::sub::Deep`\n");
    repo.commit("drop Gone and Deep");

    assert_eq!(
        removed(&run(&repo)),
        ["fixture::Gone", "fixture::sub::Deep"]
    );
}

#[test]
fn a_macro_expanding_to_nothing_keeps_no_name_alive() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub struct Gone;\n");
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "macro_rules! noop {\n    ($($t:tt)*) => {};\n}\n\nnoop!(Gone);\n",
    );
    repo.write("README.md", "`fixture::Gone`\n");
    repo.commit("drop Gone");

    assert_eq!(removed(&run(&repo)), ["fixture::Gone"]);
}

#[test]
fn items_a_head_macro_may_still_generate_out_of_sight_are_not_removed() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "macro_rules! table {\n    ($name:ident) => {\n        pub mod $name {\n            pub struct table;\n            pub mod columns {\n                pub struct name;\n            }\n            pub use self::columns::*;\n        }\n    };\n}\n\nmacro_rules! gone_macro {\n    ($name:ident) => {\n        pub mod $name {\n            pub struct row;\n        }\n    };\n}\n\nmacro_rules! wrap {\n    ($($t:tt)*) => {\n        $($t)*\n    };\n}\n\npub mod tests {\n    table!(users);\n    gone_macro!(posts);\n    wrap! {\n        table!(orders);\n    }\n}\n",
    );
    repo.commit("macro_rules schemas");
    repo.write("src/lib.rs", "pub mod schema {\n    table!(comments);\n}\n");
    repo.write(
        "README.md",
        "`users::table`, `users::name`, `orders::table` and `posts::row`\n",
    );
    repo.commit("procedural table");

    assert_eq!(removed(&run(&repo)), ["fixture::tests::posts::row"]);
}

/// The removed paths of a crate whose first commit is `lib` and whose second deletes it while
/// the README names `mentioned`.
fn removed_after_deleting(lib: &str, mentioned: &[&str]) -> Vec<String> {
    let repo = Repo::new();
    repo.write("src/lib.rs", lib);
    repo.commit("expanded");
    repo.write("src/lib.rs", "");
    let readme: Vec<String> = mentioned.iter().map(|m| format!("`{m}`\n")).collect();
    repo.write("README.md", &readme.concat());
    repo.commit("deleted");
    removed(&run(&repo))
}

#[test]
fn each_fragment_takes_what_its_grammar_parses_and_rules_need_their_tokens() {
    let lib = "macro_rules! kind {\n    (lifetime $l:lifetime) => { pub struct WasLifetime; };\n    (literal $l:literal) => { pub struct WasLiteral; };\n    (ty $t:ty) => { pub struct WasType; };\n    (expr $e:expr) => { pub struct WasExpr; };\n    (block $b:block) => { pub struct WasBlock; };\n    (stmt $s:stmt) => { pub struct WasStmt; };\n    (pat $p:pat) => { pub struct WasPat; };\n    (param $p:pat_param) => { pub struct WasParam; };\n    (lifetime $($t:tt)*) => { pub struct NotLifetime; };\n}\n\nkind!(lifetime 'a);\nkind!(literal -1);\nkind!(ty Vec<u8>);\nkind!(expr 1 + 2);\nkind!(block { 1 });\nkind!(stmt fn f() {});\nkind!(pat Some(_) | None);\nkind!(param x);\n\nmacro_rules! life {\n    ($l:lifetime) => { pub struct MinusIsLifetime; };\n    ($($t:tt)*) => { pub struct MinusIsNot; };\n}\n\nlife!(-x);\n\nmacro_rules! sep {\n    ($($x:ident),*) => { $(pub struct $x;)* };\n    ($($t:tt)*) => { pub struct Unseparated; };\n}\n\nsep!(A, B);\nsep!(C D E);\n";
    let lib = format!(
        "{lib}\nmacro_rules! speed {{\n    (\"fast\") => {{ pub struct Fast; }};\n    ($other:literal) => {{ pub struct Slow; }};\n}}\n\nspeed!(\"fast\");\n"
    );
    let names = [
        "fixture::A",
        "fixture::B",
        "fixture::Fast",
        "fixture::MinusIsNot",
        "fixture::Unseparated",
        "fixture::WasBlock",
        "fixture::WasExpr",
        "fixture::WasLifetime",
        "fixture::WasLiteral",
        "fixture::WasParam",
        "fixture::WasPat",
        "fixture::WasStmt",
        "fixture::WasType",
    ];
    let mut mentioned = names.to_vec();
    mentioned.extend([
        "fixture::C",
        "fixture::E",
        "fixture::MinusIsLifetime",
        "fixture::NotLifetime",
        "fixture::Slow",
    ]);

    assert_eq!(removed_after_deleting(&lib, &mentioned), names);
}

#[test]
fn transcription_repeats_substitutes_and_spells_crate() {
    let lib = "macro_rules! groups {\n    ($($($x:ident)* ;)*) => { $($(pub struct $x;)*)* };\n}\ngroups!(F G ; H ;);\n\npub mod inner {\n    pub struct Thing;\n}\nmacro_rules! glob {\n    () => { pub(crate) use $crate::inner::*; };\n}\nglob!();\n\nmacro_rules! outer {\n    ($name:ident) => {\n        macro_rules! nested {\n            ($x:ident) => { pub struct $x; };\n        }\n        nested!($name);\n    };\n}\nouter!(Made);\n\nmacro_rules! pair {\n    ($($a:ident)* ; $($b:ident)*) => { $(pub struct $a;)* $(pub fn $b() -> a {})* };\n}\npair!(P Q ; r);\n\nmacro_rules! zip {\n    ($($a:ident)* ; $($b:ident)*) => { $(pub struct $b; pub struct $a;)* };\n}\nzip!(Odd Even ; Third);\n";
    let names = [
        "fixture::F",
        "fixture::G",
        "fixture::H",
        "fixture::Made",
        "fixture::P",
        "fixture::Q",
        "fixture::Thing",
        "fixture::r",
    ];
    let mut mentioned = names.to_vec();
    mentioned.extend(["fixture::Odd", "fixture::Third"]);

    assert_eq!(removed_after_deleting(lib, &mentioned), names);
}

#[test]
fn a_separator_may_span_several_punctuation_marks() {
    let lib = "pub mod a {\n    pub mod b {\n        pub struct Deep;\n    }\n}\n\nmacro_rules! import {\n    ($($segment:ident)::+ ; $($name:ident)=>*) => {\n        pub use $($segment)::+;\n        $(pub struct $name;)*\n    };\n}\n\nimport!(a::b::Deep ; One => Two);\n";
    let names = ["fixture::Deep", "fixture::One", "fixture::Two"];

    assert_eq!(removed_after_deleting(lib, &names), names);
}

#[test]
fn a_rename_drops_underscore_aliases_and_keeps_its_source_name() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub mod other {\n    pub struct T;\n    pub struct Z;\n}\n\nuse other::T as _;\nuse other::Z as A;\n",
    );
    repo.commit("renames");
    repo.write(
        "src/lib.rs",
        "pub mod other {\n    pub struct T;\n    pub struct Z;\n}\n",
    );
    repo.write("README.md", "`A` and `A::Z`.\n");
    repo.commit("drop renames");

    let report = run(&repo);

    assert_eq!(removed(&report), ["fixture::A"]);
    assert_eq!(kind_of(&report, "fixture::A"), ItemKind::Reexport);
}

#[test]
fn parameter_names_bound_by_complex_patterns_of_head_are_live() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub fn tup() {}\npub fn tst() {}\npub fn sli() {}\npub fn orr2() {}\npub fn strc() {}\npub fn refd() {}\npub fn parn() {}\n",
    );
    repo.commit("seven names");
    repo.write(
        "src/lib.rs",
        "pub struct Pair(u8, u8);\npub struct Point {\n    x: u8,\n    y: u8,\n}\n\npub fn tuple_pat((tup, _): (u8, u8)) {}\npub fn tuple_struct_pat(Pair(tst, _): Pair) {}\npub fn slice_pat([sli, _]: [u8; 2]) {}\npub fn or_pat(orr @ (orr2 | _): u8) {}\npub fn struct_pat(Point { x: strc, .. }: Point) {}\npub fn ref_pat(&refd: &u8) {}\npub fn paren_pat((parn): u8) {}\n",
    );
    repo.write(
        "README.md",
        "`tup`, `tst`, `sli`, `orr2`, `strc`, `refd` and `parn`.\n",
    );
    repo.commit("drop the seven names");

    assert_eq!(removed(&run(&repo)), [] as [&str; 0]);
}

#[test]
fn an_item_position_include_pulls_its_file_into_the_module() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub struct Own;\ninclude!(\"extra.rs\");\n");
    repo.write("src/extra.rs", "pub struct Includer;\n");
    repo.commit("include");
    repo.write("src/lib.rs", "pub struct Own;\n");
    repo.write("README.md", "`Includer`.\n");
    repo.commit("drop include");

    let report = run(&repo);

    assert_eq!(removed(&report), ["fixture::Includer"]);
    assert_eq!(kind_of(&report, "fixture::Includer"), ItemKind::Struct);
}

#[test]
fn mod_declarations_of_an_included_file_resolve_against_its_directory() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub mod m {\n    pub fn in_a() {}\n    pub fn decoy() {}\n}\npub mod outer {\n    pub mod m {\n        pub fn in_b() {}\n        pub fn decoy_b() {}\n    }\n}\n",
    );
    repo.write(
        "README.md",
        "`fixture::m::in_a`, `fixture::m::decoy`, `fixture::outer::m::in_b` and `fixture::outer::m::decoy_b`.\n",
    );
    repo.commit("inline modules");
    repo.write(
        "src/lib.rs",
        "include!(\"a/inc.rs\");\npub mod outer {\n    include!(\"b/inc.rs\");\n}\n",
    );
    repo.write("src/a/inc.rs", "pub mod m;\n");
    repo.write("src/a/m.rs", "pub fn in_a() {}\n");
    repo.write("src/b/inc.rs", "pub mod m;\n");
    repo.write("src/b/m.rs", "pub fn in_b() {}\n");
    repo.write("src/m.rs", "pub fn decoy() {}\n");
    repo.write("src/outer/m.rs", "pub fn decoy_b() {}\n");
    repo.commit("modules of included files");

    assert_eq!(
        removed(&run(&repo)),
        ["fixture::m::decoy", "fixture::outer::m::decoy_b"]
    );
}

#[test]
fn every_rename_all_case_rule_spells_its_members() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "#[serde(rename_all = \"lowercase\")]\npub enum A {\n    MyVariant,\n}\n\n#[serde(rename_all = \"UPPERCASE\")]\npub enum B {\n    MyVariant,\n}\n\n#[serde(rename_all = \"PascalCase\")]\npub struct C {\n    myField: u32,\n}\n\n#[serde(rename_all = \"snake_case\")]\npub enum D {\n    MyVariant,\n}\n\n#[serde(rename_all = \"SCREAMING_SNAKE_CASE\")]\npub struct E {\n    myField: u32,\n}\n\n#[serde(rename_all = \"SCREAMING-KEBAB-CASE\")]\npub enum F {\n    MyVariant,\n}\n",
    );
    repo.commit("case rules");
    repo.write("src/lib.rs", "");
    repo.write(
        "README.md",
        "```json\n{\"myvariant\": 1, \"MYVARIANT\": 1, \"MyField\": 1, \"my_variant\": 1, \"MYFIELD\": 1, \"MY-VARIANT\": 1}\n```\n",
    );
    repo.commit("drop them all");

    let report = run(&repo);

    assert_eq!(
        removed(&report),
        [
            "fixture::A::\"myvariant\"",
            "fixture::B::\"MYVARIANT\"",
            "fixture::C::\"MyField\"",
            "fixture::D::\"my_variant\"",
            "fixture::E::\"MYFIELD\"",
            "fixture::F::\"MY-VARIANT\""
        ]
    );
    for path in [
        "fixture::A::\"myvariant\"",
        "fixture::B::\"MYVARIANT\"",
        "fixture::C::\"MyField\"",
        "fixture::D::\"my_variant\"",
        "fixture::E::\"MYFIELD\"",
        "fixture::F::\"MY-VARIANT\"",
    ] {
        assert_eq!(kind_of(&report, path), ItemKind::Serialized);
    }
}

#[test]
fn a_variant_rename_all_wins_over_the_container_rename_all_fields() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "#[serde(rename_all_fields = \"PascalCase\")]\npub enum E {\n    #[serde(rename_all = \"UPPERCASE\")]\n    MyVariant {\n        myField: u8,\n    },\n}\n",
    );
    repo.commit("variant fields");
    repo.write("src/lib.rs", "");
    repo.write(
        "README.md",
        "```json\n{\"MYFIELD\": 1, \"MyField\": 1}\n```\n",
    );
    repo.commit("drop E");

    assert_eq!(removed(&run(&repo)), ["fixture::E::MyVariant::\"MYFIELD\""]);
}

#[test]
fn rename_and_alias_add_spellings_and_skip_suppresses_them() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct R {\n    #[serde(rename = \"renamed\")]\n    pub field: u32,\n}\n\npub struct L {\n    #[serde(alias = \"aliased\")]\n    pub field: u32,\n}\n\npub struct K {\n    #[serde(skip, rename = \"skipped\")]\n    pub field: u32,\n}\n",
    );
    repo.commit("field attributes");
    repo.write("src/lib.rs", "");
    repo.write(
        "README.md",
        "```json\n{\"renamed\": 1, \"aliased\": 1, \"skipped\": 1}\n```\n",
    );
    repo.commit("drop them all");

    let report = run(&repo);

    assert_eq!(
        removed(&report),
        ["fixture::L::\"aliased\"", "fixture::R::\"renamed\""]
    );
}

#[test]
fn a_private_member_of_a_removed_dependency_is_not_reported() {
    let repo = Repo::new();
    repo.vendor(
        "helper",
        "0.1.0",
        "pub struct Gadget {\n    #[serde(rename = \"secret_field\")]\n    secret: u8,\n}\n",
    );
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = \"0.1\"\n",
    );
    repo.write("src/lib.rs", "pub struct Local;\n");
    lock(&repo);
    repo.commit("dependency with a private field");
    repo.write("Cargo.toml", BARE);
    repo.write("src/lib.rs", "");
    lock(&repo);
    repo.write(
        "README.md",
        "A `helper::Gadget` with a `helper::Gadget::secret` and `secret_field`.\n",
    );
    repo.commit("drop the dependency");

    let report = run(&repo);

    assert_eq!(removed(&report), ["helper::Gadget"]);
    assert_eq!(kind_of(&report, "helper::Gadget"), ItemKind::Struct);
}

#[test]
fn public_modules_of_a_removed_dependency_keep_their_items_reportable() {
    let repo = Repo::new();
    repo.vendor(
        "helper",
        "0.1.0",
        "pub mod shelf {\n    pub struct Inside;\n}\n",
    );
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = \"0.1\"\n",
    );
    repo.write("src/lib.rs", "pub struct Local;\n");
    lock(&repo);
    repo.commit("dependency with a public module");
    repo.write("Cargo.toml", BARE);
    repo.write("src/lib.rs", "");
    lock(&repo);
    repo.write("README.md", "`helper::shelf::Inside` and `Local`.\n");
    repo.commit("drop the dependency");

    let report = run(&repo);

    assert_eq!(
        removed(&report),
        ["fixture::Local", "helper::shelf::Inside"]
    );
    assert_eq!(kind_of(&report, "helper::shelf::Inside"), ItemKind::Struct);
}

#[test]
fn an_unreadable_manifest_at_the_head_commit_is_an_error() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub struct Thing;\n");
    let base = repo.commit("thing");
    repo.write("Cargo.toml", "not [valid toml\n");
    let broken = repo.commit("break the manifest");
    repo.write("Cargo.toml", BARE);
    repo.write("src/lib.rs", "");
    repo.write("README.md", "`Thing`.\n");
    repo.commit("restore the manifest");

    let mut options = refquiem::Options::new(repo.path().to_path_buf());
    options.range = Range::Base {
        base,
        head: Some(broken),
    };

    assert!(refquiem::run(&options).is_err());
}

#[test]
fn an_unreadable_manifest_in_history_does_not_break_the_scan() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub struct Thing;\n");
    repo.commit("thing");
    repo.write("Cargo.toml", "not [valid toml\n");
    repo.commit("break the manifest");
    repo.write("Cargo.toml", BARE);
    repo.write("src/lib.rs", "");
    repo.write("README.md", "`Thing`.\n");
    repo.commit("restore the manifest");

    assert_eq!(removed(&run(&repo)), ["fixture::Thing"]);
}

#[test]
fn a_file_reached_twice_resolves_super_against_its_first_module() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct Thing;\n\npub mod a {\n    include!(\"shared.rs\");\n}\n\npub mod b {\n    include!(\"shared.rs\");\n}\n",
    );
    repo.write(
        "src/shared.rs",
        "/// Calls `super::Thing` for setup.\npub fn setup() {}\n",
    );
    repo.commit("shared include");
    repo.write(
        "src/lib.rs",
        "pub mod a {\n    include!(\"shared.rs\");\n}\n\npub mod b {\n    include!(\"shared.rs\");\n}\n",
    );
    repo.commit("drop Thing");

    let report = run(&repo);

    assert_eq!(removed(&report), ["fixture::Thing"]);
}

#[test]
fn a_super_impl_reaches_its_self_type_one_module_up() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct Thing;\n\nmod m {\n    impl super::Thing {\n        pub fn method() {}\n    }\n}\n",
    );
    repo.commit("impl through super");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "`Thing::method`.\n");
    repo.commit("drop Thing");

    let report = run(&repo);

    assert_eq!(removed(&report), ["fixture::Thing::method"]);
    assert_eq!(kind_of(&report, "fixture::Thing::method"), ItemKind::Method);
}

#[test]
fn chained_reexports_keep_their_copies_reportable() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub mod m {\n    pub mod n {\n        pub struct S;\n    }\n}\n\nmod x {\n    pub use crate::m::n as A;\n}\n\nmod y {\n    pub use crate::x::A as B;\n}\n",
    );
    repo.commit("chained reexports");
    repo.write(
        "src/lib.rs",
        "pub mod m {\n    pub mod n {\n        pub struct S;\n    }\n}\n",
    );
    repo.write("README.md", "`y::B::S`.\n");
    repo.commit("drop the reexports");

    assert_eq!(removed(&run(&repo)), ["fixture::y::B::S"]);
}

#[test]
fn a_glob_copies_what_its_source_gains_from_later_globs() {
    let repo = Repo::new();
    let lib = |item| {
        format!(
            "pub mod a {{ pub use crate::b::*; }}\npub mod b {{ pub use crate::c::*; }}\npub mod c {{ pub use crate::d::*; }}\npub mod d {{ pub mod inner {{ {item} pub fn kept() {{}} }} }}\n"
        )
    };
    repo.write("src/lib.rs", &lib("pub fn gone() {}"));
    repo.commit("a chain of globs");
    repo.write("src/lib.rs", &lib(""));
    repo.write(
        "README.md",
        "`fixture::a::inner::gone`, `fixture::b::inner::gone` and `fixture::a::inner::kept`.\n",
    );
    repo.commit("drop gone");

    assert_eq!(
        removed(&run(&repo)),
        ["fixture::a::inner::gone", "fixture::b::inner::gone"]
    );
}

#[test]
fn a_glob_through_an_alias_resolved_in_a_later_round_copies_its_items() {
    let repo = Repo::new();
    let lib = |item| {
        format!(
            "pub mod m {{ pub mod n {{ {item} pub fn kept() {{}} }} }}\npub mod x {{ pub use crate::y::B as A; }}\npub mod y {{ pub use crate::m::n as B; }}\npub mod z {{ pub use crate::x::*; }}\n"
        )
    };
    repo.write("src/lib.rs", &lib("pub fn gone() {}"));
    repo.commit("a glob over a late alias");
    repo.write("src/lib.rs", &lib(""));
    repo.write(
        "README.md",
        "`fixture::z::A::gone`, `fixture::x::A::gone` and `fixture::z::A::kept`.\n",
    );
    repo.commit("drop gone");

    assert_eq!(
        removed(&run(&repo)),
        ["fixture::x::A::gone", "fixture::z::A::gone"]
    );
}

#[test]
fn a_module_that_later_stands_for_a_function_lets_a_glob_follow_its_self_alias() {
    let repo = Repo::new();
    let lib = |item| {
        format!(
            "pub fn real() {{}}\npub mod a_glob {{ pub use super::p::*; }}\npub mod p {{\n    pub use super::q::real2 as n;\n    pub mod n {{ pub use super::n as again; {item} pub fn kept() {{}} }}\n}}\npub mod q {{ pub use super::real as real2; }}\n"
        )
    };
    repo.write("src/lib.rs", &lib("pub fn gone() {}"));
    repo.commit("a module sharing its name with a re-exported function");
    repo.write("src/lib.rs", &lib(""));
    repo.write(
        "README.md",
        "`fixture::a_glob::n::again::gone` and `fixture::a_glob::n::again::kept`.\n",
    );
    repo.commit("drop gone");

    assert_eq!(removed(&run(&repo)), ["fixture::a_glob::n::again::gone"]);
}

#[test]
fn a_glob_reexport_copies_top_level_items_to_the_crate_root() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub mod m {\n    pub struct S;\n}\n\npub use crate::m::*;\n",
    );
    repo.commit("glob reexport");
    repo.write("src/lib.rs", "pub mod m {\n}\n");
    repo.write("README.md", "`fixture::S`.\n");
    repo.commit("drop S");

    assert_eq!(removed(&run(&repo)), ["fixture::S"]);
}

#[test]
fn a_glob_copies_the_members_a_type_gains_before_a_glob_declares_the_type() {
    let repo = Repo::new();
    let lib = |item| {
        format!(
            "pub trait Named {{ fn name(); }}\nimpl Named for Gadget {{ {item} }}\npub mod other {{ pub struct Gadget; }}\npub use other::*;\npub mod g {{ pub use super::*; }}\n"
        )
    };
    repo.write("src/lib.rs", &lib("fn name() {}"));
    repo.commit("a trait implemented for a type a glob brings in");
    repo.write("src/lib.rs", &lib(""));
    repo.write("README.md", "`fixture::g::Gadget::name`.\n");
    repo.commit("drop the member");

    assert_eq!(removed(&run(&repo)), ["fixture::g::Gadget::name"]);
}

#[test]
fn members_declared_under_a_path_no_snapshot_interned_yet_join_it_once_a_glob_creates_it() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub trait Named { fn name(); }\nimpl Named for Gadget { fn name() {} }\npub mod other { pub struct Gadget; }\npub use other::*;\npub mod g { pub use super::*; }\n",
    );
    repo.commit("a trait implemented for a type a glob brings in");
    repo.write(
        "src/lib.rs",
        "pub trait Named { fn name(); }\npub mod other { pub struct Gadget; }\npub mod g { pub use super::*; }\n",
    );
    repo.write("README.md", "`fixture::g::Gadget::name`.\n");
    repo.commit("drop the implementation and the glob");

    assert_eq!(removed(&run(&repo)), ["fixture::g::Gadget::name"]);
}

#[test]
fn a_glob_into_a_module_that_takes_an_origin_late_follows_an_alias_it_stopped_at() {
    let repo = Repo::new();
    let lib = |item| {
        format!(
            "pub mod src {{ pub use crate::p::n as d; }}\npub mod p {{\n    pub use super::w::deep as n;\n    pub mod n {{ pub use crate::src::*; {item} pub fn kept() {{}} }}\n}}\npub mod w {{ pub use super::v::*; }}\npub mod v {{ pub use super::u::*; }}\npub mod u {{ pub fn deep() {{}} }}\n"
        )
    };
    repo.write("src/lib.rs", &lib("pub fn inner() {}"));
    repo.commit("a module named like a function three globs away");
    repo.write("src/lib.rs", &lib(""));
    repo.write(
        "README.md",
        "`fixture::p::n::d::inner` and `fixture::p::n::d::kept`.\n",
    );
    repo.commit("drop inner");

    assert_eq!(removed(&run(&repo)), ["fixture::p::n::d::inner"]);
}

#[test]
fn a_dependency_glob_reexport_of_another_crate_keeps_its_items_alive() {
    let repo = Repo::new();
    repo.vendor("inner", "0.1.0", "pub struct Gadget;\n");
    repo.vendor("helper", "0.1.0", "pub use inner::*;\n");
    repo.write(
        "vendor/helper-0.1.0/Cargo.toml",
        "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ninner = \"0.1\"\n",
    );
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = \"0.1\"\n",
    );
    repo.write("src/lib.rs", "pub struct Gadget;\n");
    lock(&repo);
    repo.commit("own Gadget");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "`Gadget`\n");
    repo.commit("drop Gadget");

    assert_eq!(removed(&run(&repo)), Vec::<String>::new());
}

#[test]
fn members_reached_through_a_trait_impl_or_a_supertrait_are_live() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct Ident;\nimpl Ident {\n    pub fn parse_any() {}\n}\npub trait RngCore {\n    fn next_u32(&mut self) -> u32;\n}\n",
    );
    repo.commit("inherent members");
    repo.write(
        "src/lib.rs",
        "pub struct Ident;\npub trait IdentExt {\n    fn parse_any();\n}\nimpl IdentExt for Ident {\n    fn parse_any() {}\n}\npub trait Rng {\n    fn next_u32(&mut self) -> u32;\n}\npub trait RngCore: Rng {}\n",
    );
    repo.write("README.md", "`Ident::parse_any` and `RngCore::next_u32`.\n");
    repo.commit("members through traits");

    assert_eq!(removed(&run(&repo)), Vec::<String>::new());
}

#[test]
fn a_removed_trait_impl_removes_its_members_from_the_type() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct Widget;\npub trait Render {\n    fn render(&self);\n}\nimpl Render for Widget {\n    fn render(&self) {}\n}\n",
    );
    repo.commit("widget renders");
    repo.write(
        "src/lib.rs",
        "pub struct Widget;\npub trait Render {\n    fn render(&self);\n}\n",
    );
    repo.write("README.md", "`Widget::render`\n");
    repo.commit("widget no longer renders");

    let report = run(&repo);
    assert_eq!(removed(&report), ["fixture::Widget::render"]);
    assert_eq!(
        kind_of(&report, "fixture::Widget::render"),
        ItemKind::Method
    );
}

#[cfg(unix)]
#[test]
fn a_module_behind_a_symlinked_directory_is_indexed_in_history() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "#[path = \"core/inner.rs\"]\npub mod inner;\n",
    );
    repo.write("shared/inner.rs", "pub fn kept() {}\npub fn gone() {}\n");
    std::os::unix::fs::symlink("../shared", repo.path().join("src/core")).unwrap();
    repo.commit("inner behind a symlink");
    repo.write("shared/inner.rs", "pub fn kept() {}\n");
    repo.write("README.md", "`fixture::inner::gone`\n");
    let removing = repo.commit("drop gone");

    let report = run(&repo);
    assert_eq!(removed(&report), ["fixture::inner::gone"]);
    assert_eq!(
        report.groups[0].removed_in,
        Some(refquiem::RemovedIn::Commit(removing))
    );
}

#[cfg(unix)]
#[test]
fn symlinks_resolve_inside_the_snapshot_up_to_forty_hops() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "#[path = \"dot/inner.rs\"]\npub mod dotted;\n#[path = \"../chain1/inner.rs\"]\npub mod chained;\n#[path = \"../abs/inner.rs\"]\npub mod absolute;\n#[path = \"../cycle/inner.rs\"]\npub mod looped;\n",
    );
    repo.write("shared/inner.rs", "pub fn kept() {}\npub fn gone() {}\n");
    let link = |target: &str, name: &str| {
        std::os::unix::fs::symlink(target, repo.path().join(name)).unwrap();
    };
    link("./../shared", "src/dot");
    for n in 1..40 {
        link(&format!("chain{}", n + 1), &format!("chain{n}"));
    }
    link("shared", "chain40");
    link("/shared", "abs");
    link("cycle", "cycle");
    repo.commit("modules behind symlinks");
    repo.write("shared/inner.rs", "pub fn kept() {}\n");
    repo.write(
        "README.md",
        "`fixture::dotted::gone`, `fixture::chained::gone`, `fixture::absolute::gone` and `fixture::looped::gone`.\n",
    );
    repo.commit("drop gone");

    assert_eq!(
        removed(&run(&repo)),
        ["fixture::chained::gone", "fixture::dotted::gone"]
    );
}

#[test]
fn privately_imported_names_resolve_impls_lends_and_aliases() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct Conn;\nimpl Conn {\n    pub fn set_limits() {}\n}\npub struct Outer;\nimpl Outer {\n    pub fn shared() {}\n}\npub struct Inner;\npub struct Alias;\nimpl Alias {\n    pub fn establish() {}\n}\n",
    );
    repo.commit("members at the root");
    repo.write(
        "src/lib.rs",
        "pub struct Conn;\npub struct Outer;\npub struct Inner;\nimpl Inner {\n    pub fn shared() {}\n}\nmod limits;\npub use limits::Alias;\nmod like {\n    pub struct Like;\n    impl Like {\n        pub fn establish() {}\n    }\n}\n",
    );
    repo.write(
        "src/limits.rs",
        "use super::Conn;\nuse super::{Inner, Outer};\nuse crate::like::Like;\npub type Alias = Like;\nimpl Conn {\n    pub fn set_limits() {}\n}\nimpl core::ops::Deref for Outer {\n    type Target = Inner;\n    fn deref(&self) -> &Inner {\n        &Inner\n    }\n}\n",
    );
    repo.write(
        "README.md",
        "`fixture::Conn::set_limits`, `fixture::Outer::shared` and `fixture::Alias::establish`.\n",
    );
    repo.commit("members behind private imports");

    assert_eq!(removed(&run(&repo)), Vec::<String>::new());
}

#[test]
fn a_declared_module_outranks_an_imported_function_of_its_name() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "mod rng;\npub use rng::Ext;\n");
    repo.write("src/rng.rs", "pub trait Ext {\n    fn range();\n}\n");
    repo.write("README.md", "`fixture::Ext::range`\n");
    repo.commit("a trait in a private module");
    repo.write(
        "src/lib.rs",
        "pub mod rngs {\n    pub fn rng() {}\n}\npub use crate::rngs::rng;\nmod rng;\npub use rng::Ext;\n",
    );
    repo.commit("a function of the module's name");

    assert_eq!(removed(&run(&repo)), Vec::<String>::new());
}

#[test]
fn a_glob_of_another_crate_copies_only_its_public_items() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub struct Inside;\npub struct Open;\n");
    repo.write("tests/glob.rs", "pub use fixture::*;\n");
    repo.write(
        "README.md",
        "`fixture::Inside`, `glob::Inside` and `glob::Open`.\n",
    );
    repo.commit("a glob of the library");
    repo.write(
        "src/lib.rs",
        "pub struct Open;\nmod hidden {\n    pub struct Inside;\n}\n",
    );
    repo.commit("Inside goes private");

    let report = run(&repo);
    assert_eq!(removed(&report), ["fixture::Inside", "glob::Inside"]);
    let named: Vec<&String> = report
        .groups
        .iter()
        .flat_map(|g| g.suggestion.iter().map(|s| &s.path).chain(&g.candidates))
        .collect();
    assert_eq!(
        named,
        ["fixture::hidden::Inside", "fixture::hidden::Inside"]
    );
}

#[test]
fn a_glob_of_another_crate_keeps_a_public_name_a_private_module_shares() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn ready() {}\npub fn tool() {}\n");
    repo.write("tests/glob.rs", "pub use fixture::*;\n");
    repo.write("README.md", "`glob::ready` and `glob::tool`.\n");
    repo.commit("public functions");
    repo.write(
        "src/lib.rs",
        "mod ready {\n    pub fn ready() {}\n}\npub use self::ready::ready;\npub fn tool() {}\nmod tool {}\n",
    );
    repo.commit("private modules of the same names");

    assert_eq!(removed(&run(&repo)), Vec::<String>::new());
}

#[test]
fn a_glob_of_another_crate_drops_a_member_that_turns_private_behind_an_alias() {
    let repo = Repo::new();
    let lib = |vis: &str| {
        format!(
            "pub mod outer {{\n    pub struct Thing;\n    impl Thing {{\n        {vis}fn secret() {{}}\n    }}\n}}\npub use outer::Thing as Alias;\n"
        )
    };
    repo.write("src/lib.rs", &lib("pub "));
    repo.write("tests/glob.rs", "pub use fixture::*;\n");
    repo.write(
        "README.md",
        "`fixture::Alias::secret` and `glob::Alias::secret`.\n",
    );
    repo.commit("a public member");
    repo.write("src/lib.rs", &lib(""));
    repo.commit("the member turns private");

    assert_eq!(removed(&run(&repo)), ["glob::Alias::secret"]);
}

#[test]
fn historical_trait_visibility_changes_the_copied_api() {
    let repo = Repo::new();
    repo.write("Cargo.toml", "[workspace]\nmembers = [\"a\", \"b\"]\n");
    repo.write(
        "a/Cargo.toml",
        "[package]\nname = \"a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write(
        "b/Cargo.toml",
        "[package]\nname = \"b\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\na = { path = \"../a\" }\n",
    );
    repo.write("b/src/lib.rs", "pub use a::Widget;\n");
    let declarations = "pub struct Widget;\npub trait Exposed { fn retired_probe_842(&self); }\ntrait Hidden { fn retired_probe_842(&self); }\n";
    repo.write(
        "a/src/lib.rs",
        &format!(
            "{declarations}\nimpl Exposed for Widget {{ fn retired_probe_842(&self) {{}} }}\n"
        ),
    );
    repo.commit("public implementation");
    repo.write(
        "a/src/lib.rs",
        &format!("{declarations}\nimpl Hidden for Widget {{ fn retired_probe_842(&self) {{}} }}\n"),
    );
    let hiding = repo.commit("private implementation");
    repo.write("a/src/lib.rs", declarations);
    repo.write("README.md", "`b::Widget::retired_probe_842`\n");
    repo.commit("remove implementation");

    let report = run(&repo);
    let rows: Vec<_> = report
        .groups
        .iter()
        .map(|group| (group.path.as_str(), group.removed_in.clone()))
        .collect();
    assert_eq!(
        rows,
        [(
            "b::Widget::retired_probe_842",
            Some(RemovedIn::Commit(hiding)),
        )]
    );
}

#[test]
fn a_private_member_kept_through_a_same_crate_alias_is_alive() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "mod scope {\n    mod alias { pub fn retired_probe_842() {} }\n    fn kept() { alias::retired_probe_842(); }\n}\n",
    );
    repo.commit("a direct module path");
    repo.write(
        "src/lib.rs",
        "mod scope {\n    use crate::scope as alias;\n    fn retired_probe_842() {}\n    fn kept() { alias::retired_probe_842(); }\n}\n",
    );
    repo.write("README.md", "`fixture::scope::alias::retired_probe_842`\n");
    repo.commit("a same-crate alias to a private function");

    assert_eq!(removed(&run(&repo)), Vec::<String>::new());

    repo.write(
        "src/lib.rs",
        "mod scope {\n    use crate::scope as alias;\n    fn kept() {}\n}\n",
    );
    repo.commit("drop the private function");
    assert_eq!(
        removed(&run(&repo)),
        ["fixture::scope::alias::retired_probe_842"]
    );
}
