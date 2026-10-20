//! Indexing of every declaration kind, of re-exports, serde spellings and manifests.

mod common;

use common::Repo;
use refquiem::{ItemKind, Range};

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

/// Writes `Cargo.lock` without touching the network, as every dependency is a path dependency.
fn lock(repo: &Repo) {
    let status = std::process::Command::new("cargo")
        .args(["generate-lockfile", "--offline", "--quiet"])
        .current_dir(repo.path())
        .status()
        .unwrap();
    assert!(status.success());
}

const DEPENDENCY: &str = "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = { path = \"vendor/helper\" }\n";

const BARE: &str = "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

const HELPER_MANIFEST: &str =
    "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

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
    repo.write("Cargo.toml", DEPENDENCY);
    repo.write("vendor/helper/Cargo.toml", HELPER_MANIFEST);
    repo.write(
        "vendor/helper/src/lib.rs",
        "pub struct Gadget {\n    #[serde(rename = \"secret_field\")]\n    secret: u8,\n}\n",
    );
    repo.write("src/lib.rs", "pub struct Local;\n");
    lock(&repo);
    repo.commit("dependency with a private field");
    repo.write("Cargo.toml", BARE);
    repo.write("src/lib.rs", "");
    repo.write(
        "README.md",
        "A `helper::Gadget` with a `secret` and `secret_field`.\n",
    );
    repo.commit("drop the dependency");

    let report = run(&repo);

    assert_eq!(removed(&report), ["helper::Gadget"]);
    assert_eq!(kind_of(&report, "helper::Gadget"), ItemKind::Struct);
}

#[test]
fn public_modules_of_a_removed_dependency_keep_their_items_reportable() {
    let repo = Repo::new();
    repo.write("Cargo.toml", DEPENDENCY);
    repo.write("vendor/helper/Cargo.toml", HELPER_MANIFEST);
    repo.write(
        "vendor/helper/src/lib.rs",
        "pub mod shelf {\n    pub struct Inside;\n}\n",
    );
    repo.write("src/lib.rs", "pub struct Local;\n");
    lock(&repo);
    repo.commit("dependency with a public module");
    repo.write("Cargo.toml", BARE);
    repo.write("src/lib.rs", "");
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
fn a_dependency_glob_reexport_of_another_crate_keeps_its_items_alive() {
    let repo = Repo::new();
    repo.write("Cargo.toml", DEPENDENCY);
    repo.write(
        "vendor/helper/Cargo.toml",
        "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ninner = { path = \"../inner\" }\n",
    );
    repo.write("vendor/helper/src/lib.rs", "pub use inner::*;\n");
    repo.write(
        "vendor/inner/Cargo.toml",
        "[package]\nname = \"inner\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("vendor/inner/src/lib.rs", "pub struct Gadget;\n");
    repo.write("src/lib.rs", "pub struct Gadget;\n");
    lock(&repo);
    repo.commit("own Gadget");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "`Gadget`\n");
    repo.commit("drop Gadget");

    assert_eq!(removed(&run(&repo)), Vec::<String>::new());
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
