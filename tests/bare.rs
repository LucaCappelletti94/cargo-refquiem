//! Names written alone, the vocabularies that make them ambiguous, serialized spellings and the
//! dependency layer.

mod common;
#[path = "common/vendor.rs"]
mod vendor;

use common::Repo;
use refquiem::{Context, ItemKind, Kind, Source};

/// `(removed path, file, line, column, context, text)` for every mention, sorted.
fn found(repo: &Repo) -> Vec<(String, String, usize, usize, Context, String)> {
    let report = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap();
    let mut out: Vec<_> = report
        .groups
        .iter()
        .flat_map(|group| {
            group.mentions.iter().map(|m| {
                (
                    group.path.clone(),
                    m.file.clone(),
                    m.line,
                    m.column,
                    m.context,
                    m.text.clone(),
                )
            })
        })
        .collect();
    out.sort_by(|a, b| (&a.1, a.2, a.3).cmp(&(&b.1, b.2, b.3)));
    out
}

fn context(source: Source, kind: Kind) -> Context {
    Context { source, kind }
}

/// Writes `Cargo.lock` without touching the network, as every dependency is a path or vendored.
fn lock(repo: &Repo) {
    let status = std::process::Command::new("cargo")
        .args(["generate-lockfile", "--offline", "--quiet"])
        .current_dir(repo.path())
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn dropped_dependency_names_require_their_crate_root() {
    let repo = Repo::new();
    repo.vendor("helper", "1.0.0", "pub struct RetiredProbe842;");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\nhelper = \"1\"\n",
    );
    repo.write("src/lib.rs", "");
    lock(&repo);
    repo.commit("dependency");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write(
        "README.md",
        "`helper`, `RetiredProbe842` and `helper::RetiredProbe842`.\n",
    );
    repo.commit("drop dependency");
    let rows: Vec<_> = found(&repo).into_iter().map(|m| (m.0, m.5)).collect();
    assert_eq!(
        rows,
        [(
            "helper::RetiredProbe842".to_owned(),
            "helper::RetiredProbe842".to_owned()
        )]
    );
}

#[test]
fn compiled_examples_declare_their_own_names() {
    for (source, example, name) in [
        (
            "pub fn retired_probe_842() {}",
            "fn retired_probe_842() {}\nretired_probe_842();",
            "retired_probe_842",
        ),
        (
            "pub struct RetiredProbe842;",
            "struct RetiredProbe842;\nlet _ = RetiredProbe842;",
            "RetiredProbe842",
        ),
    ] {
        let repo = Repo::new();
        repo.write("src/lib.rs", source);
        repo.commit("workspace item");
        let block = format!("```\n{example}\nfixture::{name};\n```\n");
        let mut doc = String::new();
        for line in block.lines() {
            doc.push_str("/// ");
            doc.push_str(line);
            doc.push('\n');
        }
        repo.write("src/lib.rs", &format!("{doc}pub struct Kept;\n"));
        let rows: Vec<_> = found(&repo).into_iter().map(|m| m.5).collect();
        assert_eq!(rows, [format!("fixture::{name}")], "{example}");
    }
}

#[test]
fn only_methods_of_public_dependency_types_claim_bare_names() {
    let mut mismatches = Vec::new();
    for (source, stale) in [
        (
            "struct Hidden; impl Hidden { pub fn retired_probe_842(&self) {} }",
            true,
        ),
        (
            "enum Hidden {} impl Hidden { pub fn retired_probe_842(&self) {} }",
            true,
        ),
        (
            "pub struct Public; impl Public { pub fn retired_probe_842(&self) {} }",
            false,
        ),
        (
            "mod hidden { pub struct Public; impl Public { pub fn retired_probe_842(&self) {} } } pub use hidden::Public;",
            false,
        ),
        (
            "mod hidden { mod private_inner { pub struct Public; impl Public { pub fn retired_probe_842(&self) {} } } } pub use hidden::*;",
            true,
        ),
        (
            "#[cfg(feature = \"private\")] mod api { pub struct Public; impl Public { pub fn retired_probe_842(&self) {} } } #[cfg(not(feature = \"private\"))] pub mod api { pub struct Public; impl Public { pub fn retired_probe_842(&self) {} } }",
            false,
        ),
    ] {
        let repo = Repo::new();
        repo.vendor("helper", "1.0.0", source);
        repo.write("src/lib.rs", "pub fn retired_probe_842() {}\n");
        repo.commit("public workspace function");
        repo.write("src/lib.rs", "");
        repo.write(
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = \"1\"\n",
        );
        repo.write("README.md", "`retired_probe_842`\n");
        let paths: Vec<_> = found(&repo).into_iter().map(|mention| mention.0).collect();
        let expected = if stale {
            vec!["fixture::retired_probe_842".to_owned()]
        } else {
            Vec::new()
        };
        if paths != expected {
            mismatches.push((source, paths, expected));
        }
    }
    assert_eq!(mismatches, []);
}

#[test]
fn a_public_dependency_function_can_share_a_private_modules_name() {
    let repo = Repo::new();
    repo.vendor(
        "helper",
        "1.0.0",
        "mod shared_name { pub fn shared_name() {} } pub use shared_name::shared_name;",
    );
    repo.write("src/lib.rs", "pub fn shared_name() {}\n");
    repo.commit("workspace function");
    repo.write("src/lib.rs", "");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = \"1\"\n",
    );
    repo.write("README.md", "`helper::shared_name` and `shared_name`\n");
    assert_eq!(found(&repo), []);
}

#[test]
fn bare_names_are_stale_unless_a_vocabulary_or_live_item_claims_them() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct MyStructName;\npub struct Widget;\npub fn spawn() {}\npub struct Config {\n    pub field_in_a_struct: u8,\n}\npub struct Moved;\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "pub struct Config {}\npub mod elsewhere {\n    pub struct Moved;\n}\n",
    );
    repo.write(
        "README.md",
        "The `MyStructName` type and MyStructName in prose.\nA `Widget` and a widget, `spawn` and spawn.\nSet field_in_a_struct, see `Moved`.\n",
    );
    repo.commit("drop them");

    let md = |kind| context(Source::Markdown, kind);
    let row = |path: &str, line, column, kind, text: &str| {
        (
            path.to_owned(),
            "README.md".to_owned(),
            line,
            column,
            md(kind),
            text.to_owned(),
        )
    };
    assert_eq!(
        found(&repo),
        [
            row(
                "fixture::MyStructName",
                1,
                6,
                Kind::CodeSpan,
                "MyStructName"
            ),
            row("fixture::MyStructName", 1, 29, Kind::Text, "MyStructName"),
            row("fixture::Widget", 2, 4, Kind::CodeSpan, "Widget"),
            row(
                "fixture::Config::field_in_a_struct",
                3,
                5,
                Kind::Text,
                "field_in_a_struct"
            ),
        ]
    );
}

#[test]
fn hyphenated_english_in_prose_is_no_kebab_case_spelling() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "#[derive(serde::Serialize)]\n#[serde(rename_all = \"kebab-case\")]\npub struct View {\n    pub well_known: bool,\n    pub self_contained: bool,\n    pub frob_nicate: bool,\n}\n",
    );
    repo.commit("start");
    repo.write("src/lib.rs", "pub struct View;\n");
    repo.write(
        "README.md",
        "A well-known, self-contained view, then frob-nicate and `well-known`.\n",
    );
    repo.commit("drop the fields");

    let texts: Vec<(Kind, String)> = found(&repo).into_iter().map(|m| (m.4.kind, m.5)).collect();
    assert_eq!(
        texts,
        [
            (Kind::Text, "frob-nicate".to_owned()),
            (Kind::CodeSpan, "well-known".to_owned())
        ]
    );
}

#[test]
fn bare_names_inside_paths_urls_or_item_macros_are_left_alone() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub mod flags {\n    pub struct Gone;\n    pub struct Generated;\n}\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "macro_rules! declare {\n    ($name:ident) => {\n        pub struct $name;\n    };\n}\npub mod flags {\n    declare!(Generated);\n}\n",
    );
    repo.write(
        "README.md",
        "See https://example.com/Gone, `other::Gone` and `Generated`.\n",
    );
    repo.commit("drop Gone, generate Generated");

    assert_eq!(found(&repo), []);
}

#[test]
fn only_whole_code_spans_and_identifiers_of_compiled_code_are_bare_names() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\npub struct Kept;\n");
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "/// ```\n/// // gone here is a comment\n/// gone();\n/// ```\npub struct Kept;\n",
    );
    repo.write("README.md", "`SELECT gone FROM t` and `gone()`.\n");
    repo.commit("drop gone");

    let rows: Vec<(String, usize, usize, Context)> = found(&repo)
        .into_iter()
        .map(|m| (m.1, m.2, m.3, m.4))
        .collect();
    assert_eq!(
        rows,
        [
            (
                "README.md".to_owned(),
                1,
                27,
                context(Source::Markdown, Kind::CodeSpan)
            ),
            (
                "src/lib.rs".to_owned(),
                3,
                5,
                context(Source::DocComment, Kind::CodeBlock { compiled: true })
            ),
        ]
    );
}

#[test]
fn fields_of_struct_variants_keep_their_names_alive() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct Old {\n    pub length: u8,\n}\npub enum Size {\n    Fixed { length: u8 },\n}\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "pub enum Size {\n    Fixed { length: u8 },\n}\n",
    );
    repo.write("README.md", "`length`\n");
    repo.commit("drop Old");

    assert_eq!(found(&repo), []);
}

#[test]
fn serialized_spellings_are_checked_in_prose_and_data_examples() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "#[derive(serde::Serialize)]\n#[serde(rename_all = \"camelCase\")]\npub struct Config {\n    pub field_in_a_struct: u8,\n    pub kept_field: u8,\n}\n\n#[derive(serde::Serialize)]\n#[serde(rename_all = \"kebab-case\")]\npub enum Mode {\n    LongName,\n    Short,\n}\n",
    );
    repo.commit("serialized");
    repo.write(
        "src/lib.rs",
        "#[derive(serde::Serialize)]\n#[serde(rename_all = \"camelCase\")]\npub struct Config {\n    pub kept_field: u8,\n}\n\n#[derive(serde::Serialize)]\n#[serde(rename_all = \"kebab-case\")]\npub enum Mode {\n    Short,\n}\n",
    );
    repo.write(
        "README.md",
        "Set `fieldInAStruct`.\n\n```json\n{\"fieldInAStruct\": 1, \"keptField\": 2}\n```\n\n```yaml\nmode: long-name\n```\n",
    );
    repo.commit("drop a field and a variant");

    let rows: Vec<(String, usize, usize, String)> = found(&repo)
        .into_iter()
        .map(|m| (m.0, m.2, m.3, m.5))
        .collect();
    assert_eq!(
        rows,
        [
            (
                "fixture::Config::\"fieldInAStruct\"".to_owned(),
                1,
                6,
                "fieldInAStruct".to_owned()
            ),
            (
                "fixture::Config::\"fieldInAStruct\"".to_owned(),
                4,
                3,
                "fieldInAStruct".to_owned()
            ),
            (
                "fixture::Mode::\"long-name\"".to_owned(),
                8,
                7,
                "long-name".to_owned()
            ),
        ]
    );
}

#[test]
fn public_dependency_items_keep_names_alive_and_private_ones_do_not() {
    let repo = Repo::new();
    repo.vendor(
        "helper",
        "0.1.0",
        "pub struct Gadget;\nstruct Hidden;\nmod private {\n    pub struct Inner;\n    pub struct Exported;\n}\npub use private::Exported;\n",
    );
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = \"0.1\"\n",
    );
    repo.write(
        "src/lib.rs",
        "pub struct Gadget;\npub struct Hidden;\npub struct Inner;\npub struct Exported;\n",
    );
    lock(&repo);
    repo.commit("local names");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "`Gadget`, `Hidden`, `Inner` and `Exported`.\n");
    repo.commit("drop local names");

    let rows: Vec<(String, String)> = found(&repo).into_iter().map(|m| (m.0, m.5)).collect();
    assert_eq!(
        rows,
        [
            ("fixture::Hidden".to_owned(), "Hidden".to_owned()),
            ("fixture::Inner".to_owned(), "Inner".to_owned()),
        ]
    );
}

#[test]
fn items_of_a_removed_dependency_become_stale() {
    let repo = Repo::new();
    repo.vendor("helper", "0.1.0", "pub struct Thing;\n");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = \"0.1\"\n",
    );
    repo.write("src/lib.rs", "");
    lock(&repo);
    repo.commit("with helper");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    lock(&repo);
    repo.write("README.md", "`Thing`, `helper::Thing`, and more.\n");
    let removing = repo.commit("drop helper");

    let rows: Vec<(String, usize, String)> =
        found(&repo).into_iter().map(|m| (m.0, m.3, m.5)).collect();
    assert_eq!(
        rows,
        [("helper::Thing".to_owned(), 11, "helper::Thing".to_owned())]
    );
    let report = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap();
    let thing = report
        .groups
        .iter()
        .find(|g| g.path == "helper::Thing")
        .unwrap();
    assert_eq!(thing.kind, ItemKind::Struct);
    assert_eq!(
        thing.removed_in,
        Some(refquiem::RemovedIn::Commit(removing))
    );
}

#[test]
fn a_removed_registry_dependency_is_read_at_its_locked_version() {
    let repo = Repo::new();
    repo.vendor("helper", "0.2.0", "pub struct Locked;\n");
    repo.vendor("helper", "0.3.0", "pub struct Newer;\n");
    let manifest = "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = ";
    repo.write("Cargo.toml", &format!("{manifest}\"0.2\"\n"));
    repo.write("src/lib.rs", "");
    lock(&repo);
    // The lock keeps 0.2.0, which the wider requirement alone would not choose.
    repo.write("Cargo.toml", &format!("{manifest}\">=0.2\"\n"));
    repo.commit("helper locked at 0.2.0");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    lock(&repo);
    repo.write("README.md", "`helper::Locked` and `helper::Newer`.\n");
    repo.commit("drop helper");

    let rows: Vec<(String, String)> = found(&repo).into_iter().map(|m| (m.0, m.5)).collect();
    assert_eq!(
        rows,
        [
            ("helper::Locked".to_owned(), "helper::Locked".to_owned()),
            ("helper".to_owned(), "helper::Newer".to_owned()),
        ]
    );
}

#[test]
fn running_text_counts_only_code_shaped_words() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct Widget;\npub fn introducer() {}\npub struct TBD;\npub struct SQLUpdate;\npub fn old_name() {}\n",
    );
    repo.commit("start");
    repo.write("src/lib.rs", "");
    repo.write(
        "README.md",
        "Widget, introducer, TBD, SQLUpdate and old_name.\n",
    );
    repo.commit("drop them");

    let rows: Vec<(String, usize)> = found(&repo).into_iter().map(|m| (m.0, m.3)).collect();
    assert_eq!(
        rows,
        [
            ("fixture::SQLUpdate".to_owned(), 26),
            ("fixture::old_name".to_owned(), 40)
        ]
    );
}

#[test]
fn format_arguments_and_attribute_strings_are_not_bare_names() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub fn old_value() {}\npub fn visit_old() {}\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "pub fn show() -> String {\n    let old_value = 1;\n    format!(\"{old_value} and visit_old\")\n}\n#[deprecated(note = \"visit_old, see fixture::old_value\")]\npub struct Kept;\n",
    );
    repo.commit("drop them");

    let rows: Vec<(String, usize, usize)> =
        found(&repo).into_iter().map(|m| (m.0, m.2, m.3)).collect();
    assert_eq!(
        rows,
        [
            ("fixture::visit_old".to_owned(), 3, 30),
            ("fixture::old_value".to_owned(), 5, 37)
        ]
    );
}

#[test]
fn names_a_doc_example_binds_are_not_bare_names() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub fn actual() {}\npub fn used() {}\npub fn each() {}\npub fn called() {}\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "/// ```\n/// let actual = 1;\n/// let f = |used: u8| used + actual;\n/// for each in [actual] { f(each); }\n/// called(f(actual));\n/// ```\npub struct Kept;\n",
    );
    repo.commit("drop them");

    let rows: Vec<(String, usize, usize)> =
        found(&repo).into_iter().map(|m| (m.0, m.2, m.3)).collect();
    assert_eq!(rows, [("fixture::called".to_owned(), 5, 5)]);
}

#[test]
fn parameter_names_of_head_functions_are_live() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn found() {}\n");
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "/// Reports the `found` token.\npub fn report(found: u8) -> u8 {\n    found\n}\n",
    );
    repo.commit("drop found");

    assert_eq!(found(&repo), []);
}

#[test]
fn generic_parameter_names_of_head_items_are_live() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct Lanes;\npub struct Elem;\npub struct Seed;\npub struct Key;\npub struct Out;\npub struct Al;\npub struct Un;\npub struct En;\npub struct Ex;\npub struct Unused;\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "/// Uses `Lanes`, `Elem`, `Seed`, `Key`, `Out`, `Al`, `Un`, `En`, `Ex` and `Unused`.\npub fn sample<Elem, const Lanes: usize>() {}\npub struct Holder<Seed>(Seed);\nimpl<Key> Holder<Key> {\n    pub fn method<Ex>() {}\n}\npub trait Conv<Out> {}\npub type Alias<Al> = Vec<Al>;\npub union U<Un: Copy> {\n    a: Un,\n}\npub enum E<En> {\n    A(En),\n}\n",
    );
    repo.commit("generic parameters only");

    let rows: Vec<String> = found(&repo).into_iter().map(|m| m.0).collect();
    assert_eq!(rows, ["fixture::Unused"]);
}

#[test]
fn the_prose_of_a_doc_comment_sees_the_names_its_examples_declare() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone_reader() {}\n");
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "/// A `gone_reader` function may only read owned data.\n///\n/// ```\n/// fn gone_reader() {}\n/// ```\npub fn kept() {}\n\n/// Calls `gone_reader`.\npub fn other() {}\n",
    );
    repo.commit("drop gone_reader");

    let rows: Vec<(String, usize)> = found(&repo).into_iter().map(|m| (m.0, m.2)).collect();
    assert_eq!(rows, [("fixture::gone_reader".to_owned(), 8)]);
}

#[test]
fn names_a_dependency_declares_through_macros_are_live() {
    let repo = Repo::new();
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = { path = \"vendor/helper\" }\n",
    );
    repo.write(
        "vendor/helper/Cargo.toml",
        "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write(
        "vendor/helper/src/lib.rs",
        "macro_rules! declare {\n    ($($field:ident),*) => {\n        pub struct Declared { $(pub $field: u8),* }\n    };\n}\ndeclare!(mac);\n",
    );
    repo.write("src/lib.rs", "pub struct Frame {\n    pub mac: u8,\n}\n");
    lock(&repo);
    repo.commit("own mac");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "`mac`\n");
    repo.commit("drop Frame");

    assert_eq!(found(&repo), []);
}

#[test]
fn a_removed_dependency_cargo_cannot_resolve_is_skipped_with_a_warning() {
    let repo = Repo::new();
    repo.vendor("broken", "1.0.0", "pub struct Broken;\n");
    repo.write(
        "vendor/broken-1.0.0/Cargo.toml",
        "[package]\nname = \"broken\"\nversion = \"1.0.0\"\nedition = \"2021\"\n\n[dependencies]\nabsent = \"1\"\n",
    );
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nbroken = \"1\"\n",
    );
    repo.write("src/lib.rs", "");
    repo.commit("broken without a lock");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("README.md", "`Broken` and `broken::Broken`.\n");
    repo.commit("drop broken");

    let report = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap();
    let groups: Vec<(&str, ItemKind, &str)> = report
        .groups
        .iter()
        .flat_map(|g| {
            g.mentions
                .iter()
                .map(|m| (g.path.as_str(), g.kind, m.text.as_str()))
        })
        .collect();
    assert_eq!(groups, [("broken", ItemKind::Dependency, "broken::Broken")]);
    let skipped: Vec<&str> = report
        .skipped
        .iter()
        .map(|s| s.dependency.as_str())
        .collect();
    assert_eq!(skipped, ["broken"]);
    assert!(
        report
            .human()
            .contains("warning: removed dependency `broken` could not be resolved"),
        "{}",
        report.human()
    );
}

#[test]
fn a_removed_path_dependency_with_an_unreadable_manifest_is_skipped() {
    let repo = Repo::new();
    repo.write("vendor/helper/Cargo.toml", "this is not toml [\n");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = { path = \"vendor/helper\" }\n",
    );
    repo.write("src/lib.rs", "");
    repo.commit("helper");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("README.md", "Call `helper::run`.\n");
    repo.commit("drop helper");

    let report = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap();
    let groups: Vec<(&str, ItemKind)> = report
        .groups
        .iter()
        .map(|g| (g.path.as_str(), g.kind))
        .collect();
    assert_eq!(groups, [("helper", ItemKind::Dependency)]);
    let skipped: Vec<&str> = report
        .skipped
        .iter()
        .map(|s| s.dependency.as_str())
        .collect();
    assert_eq!(skipped, ["helper"]);
}

#[test]
fn serialized_spellings_a_dependency_re_exports_are_live() {
    let repo = Repo::new();
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nfacade = { path = \"vendor/facade\" }\n",
    );
    repo.write(
        "vendor/facade/Cargo.toml",
        "[package]\nname = \"facade\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ninner = { path = \"../inner\" }\n",
    );
    repo.write("vendor/facade/src/lib.rs", "pub use inner::models;\n");
    repo.write(
        "vendor/inner/Cargo.toml",
        "[package]\nname = \"inner\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write(
        "vendor/inner/src/lib.rs",
        "pub mod models {\n    pub struct HostConfig {\n        #[serde(rename = \"CapAdd\")]\n        pub cap_add: u8,\n    }\n}\n",
    );
    repo.write(
        "src/lib.rs",
        "pub struct Old {\n    #[serde(rename = \"CapAdd\")]\n    pub cap: u8,\n}\n",
    );
    lock(&repo);
    repo.commit("own spelling");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "Set \"CapAdd\".\n");
    repo.commit("drop Old");

    assert_eq!(found(&repo), []);
}

#[test]
fn a_removed_dependency_is_rooted_at_its_library_name() {
    let repo = Repo::new();
    repo.vendor("weird-sys", "1.0.0", "pub struct Thing;\n");
    repo.write(
        "vendor/weird-sys-1.0.0/Cargo.toml",
        "[package]\nname = \"weird-sys\"\nversion = \"1.0.0\"\nedition = \"2021\"\n\n[lib]\nname = \"weird\"\n",
    );
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nweird-sys = \"1\"\n",
    );
    repo.write("src/lib.rs", "");
    repo.commit("weird-sys");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("README.md", "`weird::Thing`\n");
    repo.commit("drop weird-sys");

    let report = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap();
    let paths: Vec<&str> = report.groups.iter().map(|g| g.path.as_str()).collect();
    assert_eq!(paths, ["weird::Thing"]);
    assert_eq!(report.skipped, []);
}

#[test]
fn names_a_doc_example_declares_are_local_to_its_block() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub fn run_test() {}\npub async fn root() {}\npub static RE: u8 = 0;\npub struct Ip;\npub mod kw {}\npub fn from_reader() {}\npub struct R;\npub struct T;\npub fn rdr() {}\n#[macro_export]\nmacro_rules! gone_mac {\n    () => {};\n}\npub const LIMIT: u8 = 0;\npub enum Mode {}\npub type Alias = u8;\npub trait Draw {}\npub union Bits {\n    bits_field: u8,\n}\npub fn only_called() {}\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "/// ```\n/// # fn run_test() {}\n/// async fn root() {}\n/// lazy_static::lazy_static! { static ref RE: u8 = 0; }\n/// struct r#Ip;\n/// mod kw {}\n/// fn from_reader<R, T: Fn() -> u8>(rdr: R) -> T { run_test(); root(); RE; Ip; kw; rdr; loop {} }\n/// macro_rules! gone_mac { () => {} }\n/// const LIMIT: u8 = 0;\n/// enum Mode {}\n/// type Alias = u8;\n/// trait Draw {}\n/// union Bits { b: u8 }\n/// gone_mac!(); let _ = (LIMIT, Mode, Alias, Draw, Bits);\n/// ```\n///\n/// ```\n/// only_called(); run_test();\n/// ```\npub struct Kept;\n",
    );
    repo.commit("drop them");

    let rows: Vec<(String, usize, usize)> =
        found(&repo).into_iter().map(|m| (m.0, m.2, m.3)).collect();
    assert_eq!(
        rows,
        [
            ("fixture::only_called".to_owned(), 18, 5),
            ("fixture::run_test".to_owned(), 18, 20)
        ]
    );
}

#[test]
fn a_doc_example_declaration_binds_no_bound_type_or_body_name() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub const N: usize = 0;\npub struct GoneInner;\npub trait GoneBound {}\npub fn gone_second() {}\npub fn gone_px() {}\npub struct gone_type;\npub fn gone_body() {}\npub struct gone_field;\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "/// ```\n/// fn first<const N: usize, T: Into<Wrap<u8, GoneInner>> + GoneBound>(gone_second: u8, Wrapper { inner: gone_px }: Wrapper, pair: (u8, gone_type)) {}\n/// fn other() { gone_body(N); }\n/// struct Pair(gone_field);\n/// ```\npub struct Kept;\n",
    );
    repo.commit("drop them");

    let rows: Vec<(String, usize, usize)> =
        found(&repo).into_iter().map(|m| (m.0, m.2, m.3)).collect();
    assert_eq!(
        rows,
        [
            ("fixture::GoneInner".to_owned(), 2, 47),
            ("fixture::GoneBound".to_owned(), 2, 61),
            ("fixture::gone_type".to_owned(), 2, 137),
            ("fixture::gone_body".to_owned(), 3, 18),
            ("fixture::gone_field".to_owned(), 4, 17)
        ]
    );
}

#[test]
fn an_impl_header_for_leaves_later_declarations_bound() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub fn gone_body() {}\npub fn gone_item() {}\npub fn gone_call() {}\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "/// ```\n/// impl Clone for Lower {\n///     fn clone(&self) -> Self {\n///         gone_body()\n///     }\n/// }\n/// for gone_item in list {\n///     gone_item;\n/// }\n/// gone_call();\n/// ```\npub struct Kept;\n",
    );
    repo.commit("drop them");

    let rows: Vec<(String, usize, usize)> =
        found(&repo).into_iter().map(|m| (m.0, m.2, m.3)).collect();
    assert_eq!(
        rows,
        [
            ("fixture::gone_body".to_owned(), 4, 13),
            ("fixture::gone_call".to_owned(), 10, 5)
        ]
    );
}

#[test]
fn a_for_bound_leaves_later_declarations_bound_and_a_leading_for_loop_binds() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub fn gone_x() {}\npub struct GoneLater;\npub fn gone_call() {}\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "/// ```\n/// for gone_x in list {\n///     gone_x;\n/// }\n/// ```\n///\n/// ```\n/// fn apply<F>(f: F) where F: for<'a> Fn(&'a u8) {}\n/// struct GoneLater;\n/// let _ = GoneLater;\n/// gone_call();\n/// ```\npub struct Kept;\n",
    );
    repo.commit("drop them");

    let rows: Vec<(String, usize, usize)> =
        found(&repo).into_iter().map(|m| (m.0, m.2, m.3)).collect();
    assert_eq!(rows, [("fixture::gone_call".to_owned(), 11, 5)]);
}

#[test]
fn a_path_in_a_let_pattern_leaves_its_bindings_bound() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub fn gone_int() {}\n#[allow(non_camel_case_types)]\npub struct gone_ty;\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "/// ```\n/// if let Lit::Int(gone_int) = lit {\n///     gone_int;\n/// }\n/// let a: gone_ty = v;\n/// ```\npub struct Kept;\n",
    );
    repo.commit("drop them");

    let rows: Vec<(String, usize, usize)> =
        found(&repo).into_iter().map(|m| (m.0, m.2, m.3)).collect();
    assert_eq!(rows, [("fixture::gone_ty".to_owned(), 5, 12)]);
}

#[test]
fn a_keyword_of_a_doc_example_names_no_serialized_spelling() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "#[derive(serde::Serialize)]\npub struct Config {\n    #[serde(rename = \"enum\")]\n    pub kind: u8,\n}\n",
    );
    repo.commit("renamed");
    repo.write(
        "src/lib.rs",
        "/// ```\n/// enum Choice { A }\n/// call(r#enum);\n/// ```\n#[derive(serde::Serialize)]\npub struct Config {\n    pub b: u8,\n}\n",
    );
    repo.commit("drop the field");

    let rows: Vec<(String, usize)> = found(&repo).into_iter().map(|m| (m.0, m.2)).collect();
    assert_eq!(rows, [("fixture::Config::\"enum\"".to_owned(), 3)]);
}

#[test]
fn a_string_literal_holding_one_identifier_is_data() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub const EXCEPT_TABLE: u8 = 0;\npub fn seq_shuffle_100() {}\npub fn gone_fn() {}\npub mod old {\n    pub struct Item;\n}\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "pub const ARG: &str = \"EXCEPT_TABLE\";\npub fn bench(c: &mut Bencher) { c.bench_function(\"seq_shuffle_100\", |b| b.iter(|| ())); }\npub const PATH: &str = \"fixture::old::Item\";\npub const HINT: &str = \"use gone_fn instead\";\n",
    );
    repo.commit("drop them");

    let rows: Vec<(usize, usize, String)> =
        found(&repo).into_iter().map(|m| (m.2, m.3, m.5)).collect();
    assert_eq!(
        rows,
        [
            (3, 25, "fixture::old::Item".to_owned()),
            (4, 29, "gone_fn".to_owned())
        ]
    );
}

#[test]
fn identifier_strings_retain_removed_serialized_spellings() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "#[derive(serde::Serialize)]\npub struct Data {\n    #[serde(rename = \"retired_label_842\")]\n    pub value: u8,\n}\npub fn retired_api_842() {}\n",
    );
    repo.commit("serialized field and API");
    repo.write(
        "src/lib.rs",
        "pub const LABEL: &str = \"retired_label_842\";\npub const API: &str = \"retired_api_842\";\n",
    );
    let rows: Vec<_> = found(&repo).into_iter().map(|m| (m.0, m.5)).collect();
    assert_eq!(
        rows,
        [(
            "fixture::Data::\"retired_label_842\"".to_owned(),
            "retired_label_842".to_owned()
        )]
    );
}

#[test]
fn identifiers_in_attribute_arguments_of_a_compiled_example_are_not_names() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct VariadicArgs {\n    pub argument_count: u8,\n}\npub struct Container {\n    pub rename_all: u8,\n}\npub struct RemovedDerive;\npub struct GoneType;\npub fn gone_attr() {}\npub fn gone_inner() {}\npub fn gone_arg() {}\npub fn gone_lint() {}\npub fn gone_idx() {}\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "/// ```\n/// #![allow(gone_inner)]\n/// #[variadic(argument_count)]\n/// #[serde(rename_all = \"camelCase\")]\n/// #[cfg_attr(test, derive(RemovedDerive), gone_arg)]\n/// #[cfg_attr(test, allow(gone_lint))]\n/// #[gone_attr]\n/// struct S { field: GoneType }\n/// let x = data[(gone_idx)];\n/// ```\npub struct Kept;\n",
    );
    repo.commit("drop them");

    let rows: Vec<(usize, usize, String)> =
        found(&repo).into_iter().map(|m| (m.2, m.3, m.5)).collect();
    assert_eq!(
        rows,
        [
            (5, 29, "RemovedDerive".to_owned()),
            (7, 7, "gone_attr".to_owned()),
            (8, 23, "GoneType".to_owned()),
            (9, 19, "gone_idx".to_owned())
        ]
    );
}

#[test]
fn a_root_derive_attribute_still_names_a_removed_item() {
    assert_eq!(
        example_reports(&["RemovedDerive"], "#[derive(RemovedDerive)]\nstruct Kept;"),
        [("RemovedDerive".to_owned(), 1)]
    );
}

#[test]
fn fields_variants_arms_and_macro_keys_of_a_doc_example_are_its_own_names() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub fn gone_field() {}\npub struct GoneVariant;\npub fn gone_inner() {}\npub fn gone_data() {}\npub fn gone_check() {}\npub fn gone_res() {}\npub fn gone_fut() {}\npub fn gone_key() {}\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "/// ```\n/// struct Pagination { gone_field: usize }\n/// enum E { GoneVariant(u8), Other { gone_inner: u8 } }\n/// let p = Pagination { gone_field: 1 };\n/// let _ = (p.gone_field, GoneVariant, gone_inner);\n/// match v {\n///     Some(gone_data) if gone_check(gone_data) => {}\n///     _ => {}\n/// }\n/// tokio::select! {\n///     gone_res = gone_fut => { gone_res; }\n/// }\n/// info_span!(\"x\", gone_key = \"yes\");\n/// ```\npub struct Kept;\n",
    );
    repo.commit("drop them");

    let rows: Vec<(String, usize)> = found(&repo).into_iter().map(|m| (m.0, m.2)).collect();
    assert_eq!(
        rows,
        [
            ("fixture::gone_check".to_owned(), 7),
            ("fixture::gone_fut".to_owned(), 11)
        ]
    );
}

/// The removed functions `names` a compiled doc example spelled as `example` still reports,
/// with the example line each is on.
fn example_reports(names: &[&str], example: &str) -> Vec<(String, usize)> {
    let repo = Repo::new();
    let mut lib = String::new();
    for name in names {
        lib.push_str("pub fn ");
        lib.push_str(name);
        lib.push_str("() {}\n");
    }
    repo.write("src/lib.rs", &lib);
    repo.commit("start");
    let mut doc = String::new();
    for line in example.lines() {
        doc.push_str("/// ");
        doc.push_str(line);
        doc.push('\n');
    }
    repo.write(
        "src/lib.rs",
        &format!("/// ```\n{doc}/// ```\npub struct Kept;\n"),
    );
    repo.commit("drop them");
    let mut rows: Vec<(String, usize)> = found(&repo)
        .into_iter()
        .map(|m| (m.0.trim_start_matches("fixture::").to_owned(), m.2 - 1))
        .collect();
    rows.sort_by(|a, b| (a.1, &a.0).cmp(&(b.1, &b.0)));
    rows
}

#[test]
fn impl_generic_parameters_are_local_to_the_compiled_example() {
    assert_eq!(
        example_reports(
            &["RetiredProbe842", "free_probe_842"],
            "struct Container<T>(T);\nimpl<RetiredProbe842> Container<RetiredProbe842> {}\nfree_probe_842();",
        ),
        [("free_probe_842".to_owned(), 3)]
    );
}

#[test]
fn only_the_fields_and_variants_an_example_declares_are_its_own() {
    let names = [
        "gone_w",
        "gone_mod",
        "GoneMapV",
        "GoneVar",
        "GoneSecond",
        "gone_named",
        "gone_lit",
        "gone_v",
    ];
    let example = "struct W<T> where T: Fn([u8; 2]) -> u8 { gone_w: T, f: gone_mod::Ty, m: HashMap<u8, GoneMapV> }\nenum E { #[default] GoneVar, Tup(u8, GoneSecond), Named { gone_named: u8 } }\nstruct Unit;\nlet _ = Other { gone_lit: 1 };\nlet _ = (gone_w, GoneVar, gone_named, gone_mod, GoneMapV, GoneSecond);\nstruct V where [u8; { 1 }]: Copy { gone_v: u8 }\nlet _ = gone_v;\nstruct Trailing";
    let rows = example_reports(&names, example);
    let row = |name: &str, line| (name.to_owned(), line);
    assert_eq!(
        rows,
        [
            row("GoneMapV", 1),
            row("GoneSecond", 2),
            row("gone_lit", 4),
            row("GoneMapV", 5),
            row("GoneSecond", 5),
            row("gone_mod", 5)
        ]
    );
}

#[test]
fn match_and_select_patterns_end_at_their_guard_future_or_arrow() {
    let names = [
        "gone_inside",
        "gone_a",
        "gone_guard",
        "gone_body",
        "gone_z",
        "gone_b",
        "gone_block",
        "gone_c",
        "gone_then",
        "gone_else",
        "gone_rest",
        "gone_res",
        "gone_fut",
        "gone_cond",
        "gone_v",
        "gone_rx",
        "gone_handle",
        "gone_last",
    ];
    let example = "match run(|| { gone_inside() }) {\n    Some(gone_a) if gone_guard(gone_a) => gone_body(gone_a, gone_z),\n    Pair(gone_b, _) => { gone_block(gone_b); match q { _ => 1 } }\n    Other { gone_c, .. } => if gone_c { gone_then() } else { gone_else() },\n    1..=5 | gone_rest => gone_rest,\n}\ntokio::select! {\n    gone_res = gone_fut(), if gone_cond => { gone_res; }\n    Some(gone_v) = gone_rx => gone_handle(gone_v),\n    else => gone_last(),\n}";
    let rows = example_reports(&names, example);
    let row = |name: &str, line| (name.to_owned(), line);
    assert_eq!(
        rows,
        [
            row("gone_inside", 1),
            row("gone_body", 2),
            row("gone_guard", 2),
            row("gone_z", 2),
            row("gone_block", 3),
            row("gone_else", 4),
            row("gone_then", 4),
            row("gone_cond", 8),
            row("gone_fut", 8),
            row("gone_handle", 9),
            row("gone_rx", 9),
            row("gone_last", 10)
        ]
    );
}

#[test]
fn only_a_single_eq_at_the_top_of_macro_arguments_binds() {
    let names = [
        "gone_key",
        "gone_eq",
        "gone_arrow",
        "gone_nested",
        "gone_member",
        "gone_ne",
        "gone_late",
        "gone_tight",
    ];
    let example = "info_span!(\"x\", gone_key = \"yes\", gone_eq == 1, gone_arrow => 2, f(gone_nested = 1), s.gone_member = 1, gone_late = 2, gone_tight=3);\nlet _ = (gone_key, gone_ne != 1, gone_late, gone_tight);";
    let rows = example_reports(&names, example);
    let row = |name: &str, line| (name.to_owned(), line);
    assert_eq!(
        rows,
        [
            row("gone_arrow", 1),
            row("gone_eq", 1),
            row("gone_member", 1),
            row("gone_nested", 1),
            row("gone_ne", 2)
        ]
    );
}

#[test]
fn a_doc_example_binds_only_the_names_its_patterns_take() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct gone_a;\npub struct gone_b;\npub struct gone_c;\npub struct gone_d;\npub struct gone_rhs;\npub struct GoneUp;\npub struct _gone;\npub struct gone_m;\npub fn gone_call() {}\npub struct gone_p;\npub struct gone_b2;\npub struct gone_c2;\npub struct gone_pair;\npub struct r#loop;\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "/// ```\n/// let ((gone_a, gone_b), (gone_c, gone_d)) = 1;\n/// let (x) = (gone_rhs,);\n/// let (GoneUp) = 1;\n/// let _gone = 1;\n/// let y = move |gone_m| gone_m;\n/// let f = one(gone_call);\n/// let f2 = |gone_p, | gone_b2;\n/// let g = |gone_c2\n/// fixture::gone_pair;\n/// let r#loop = 1;\n/// ```\npub struct Kept;\n",
    );
    repo.commit("drop them");

    let block = context(Source::DocComment, Kind::CodeBlock { compiled: true });
    let rows: Vec<(String, usize, usize, Context, String)> = found(&repo)
        .into_iter()
        .map(|m| (m.0, m.2, m.3, m.4, m.5))
        .collect();
    assert_eq!(
        rows,
        [
            (
                "fixture::gone_rhs".to_owned(),
                3,
                16,
                block,
                "gone_rhs".to_owned()
            ),
            (
                "fixture::GoneUp".to_owned(),
                4,
                10,
                block,
                "GoneUp".to_owned()
            ),
            (
                "fixture::gone_call".to_owned(),
                7,
                17,
                block,
                "gone_call".to_owned()
            ),
            (
                "fixture::gone_b2".to_owned(),
                8,
                25,
                block,
                "gone_b2".to_owned()
            ),
            (
                "fixture::gone_pair".to_owned(),
                10,
                5,
                block,
                "fixture::gone_pair".to_owned()
            ),
            ("fixture::loop".to_owned(), 11, 11, block, "loop".to_owned()),
        ]
    );
}

#[test]
fn a_flag_value_is_spelled_and_words_after_it_still_count() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct old_feature;\npub struct gone_flag;\npub struct x;\n",
    );
    repo.commit("start");
    repo.write("src/lib.rs", "pub struct x;\n");
    repo.write(
        "README.md",
        "# Setup\nUse `x` and --features old_feature and gone_flag\n",
    );
    repo.commit("drop the names");

    let rows: Vec<(String, usize, usize, Context, String)> = found(&repo)
        .into_iter()
        .map(|m| (m.0, m.2, m.3, m.4, m.5))
        .collect();
    assert_eq!(
        rows,
        [(
            "fixture::gone_flag".to_owned(),
            2,
            40,
            context(Source::Markdown, Kind::Text),
            "gone_flag".to_owned()
        )]
    );
}

#[test]
fn a_format_argument_in_braces_is_not_a_bare_name() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub struct gone_name;\n");
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "pub fn show() -> String {\n    format!(\"x {gone_name}\")\n}\n",
    );
    repo.commit("drop it");

    assert_eq!(found(&repo), []);
}

#[test]
fn a_digit_started_hyphenated_name_counts_in_running_text() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "#[derive(serde::Serialize)]\npub struct Config {\n    #[serde(rename = \"1-2\")]\n    pub a: u8,\n}\n",
    );
    repo.commit("renamed");
    repo.write(
        "src/lib.rs",
        "#[derive(serde::Serialize)]\npub struct Config {\n    pub b: u8,\n}\n",
    );
    repo.write("README.md", "Send 1-2 to the endpoint.\n");
    repo.commit("drop the field");

    let rows: Vec<(String, usize, usize, Context, String)> = found(&repo)
        .into_iter()
        .map(|m| (m.0, m.2, m.3, m.4, m.5))
        .collect();
    assert_eq!(
        rows,
        [(
            "fixture::Config::\"1-2\"".to_owned(),
            1,
            6,
            context(Source::Markdown, Kind::Text),
            "1-2".to_owned()
        )]
    );
}

#[test]
fn words_glued_to_addresses_and_colons_are_not_bare_names() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub struct old_name;\n");
    repo.commit("start");
    repo.write("src/lib.rs", "pub struct Kept;\n");
    repo.write(
        "README.md",
        "Mail old_name@example.com and read v2.old_name/docs.\nCheck old_name.foo and copy v2.old_name:thing out.\nUse old_name::<u8> or old_name:thing here.\n",
    );
    repo.commit("drop old_name");

    let rows: Vec<(String, usize, usize, Context, String)> = found(&repo)
        .into_iter()
        .map(|m| (m.0, m.2, m.3, m.4, m.5))
        .collect();
    assert_eq!(
        rows,
        [(
            "fixture::old_name".to_owned(),
            3,
            23,
            context(Source::Markdown, Kind::Text),
            "old_name".to_owned()
        )]
    );
}

#[test]
fn a_code_span_holding_two_words_is_not_a_bare_name() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "#[derive(serde::Serialize)]\npub struct Config {\n    #[serde(rename = \"a b\")]\n    pub a: u8,\n}\n",
    );
    repo.commit("renamed");
    repo.write(
        "src/lib.rs",
        "#[derive(serde::Serialize)]\npub struct Config {\n    pub b: u8,\n}\n",
    );
    repo.write("README.md", "See `a b` in the payload.\n");
    repo.commit("drop the field");

    assert_eq!(found(&repo), []);
}

#[test]
fn a_mixed_case_english_word_in_prose_is_not_a_bare_name() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub struct PhD;\npub struct MyGone;\n");
    repo.commit("start");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "A PhD student and MyGone.\n");
    repo.commit("drop them");

    let rows: Vec<(String, usize)> = found(&repo).into_iter().map(|m| (m.0, m.3)).collect();
    assert_eq!(rows, [("fixture::MyGone".to_owned(), 19)]);
}

#[test]
fn a_doc_example_that_binds_nothing_names_every_identifier() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn called() {}\npub fn gone_value() {}\n");
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "/// ```\n/// called(gone_value);\n/// ```\npub struct Kept;\n",
    );
    repo.commit("drop them");

    let rows: Vec<(String, usize, usize)> =
        found(&repo).into_iter().map(|m| (m.0, m.2, m.3)).collect();
    assert_eq!(
        rows,
        [
            ("fixture::called".to_owned(), 2, 5),
            ("fixture::gone_value".to_owned(), 2, 12)
        ]
    );
}

#[test]
fn a_bitwise_or_in_a_doc_example_opens_no_closure() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub fn gone_left() {}\npub fn gone_right() {}\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "/// ```\n/// gone_left | gone_right;\n/// ```\npub struct Kept;\n",
    );
    repo.commit("drop them");

    let rows: Vec<(String, usize, usize)> =
        found(&repo).into_iter().map(|m| (m.0, m.2, m.3)).collect();
    assert_eq!(
        rows,
        [
            ("fixture::gone_left".to_owned(), 2, 5),
            ("fixture::gone_right".to_owned(), 2, 17)
        ]
    );
}

#[test]
fn a_name_of_a_head_feature_binary_example_or_package_is_live() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub mod chacha {}\npub fn cli_tool() {}\npub fn demo_ex() {}\npub mod fixture {}\npub fn gone_fn() {}\n",
    );
    repo.commit("start");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\nchacha = []\n",
    );
    repo.write("src/lib.rs", "");
    repo.write("src/bin/cli_tool.rs", "fn main() {}\n");
    repo.write("examples/demo_ex.rs", "fn main() {}\n");
    repo.write(
        "README.md",
        "`chacha`, `cli_tool`, `demo_ex`, `fixture` and `gone_fn`.\n",
    );
    repo.commit("drop them");

    let rows: Vec<String> = found(&repo).into_iter().map(|m| m.5).collect();
    assert_eq!(rows, ["gone_fn"]);
}

#[test]
fn a_path_segment_inside_a_doc_example_pattern_binds_nothing() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub fn gone_tail() {}\npub fn gone_head() {}\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "/// ```\n/// let (m::gone_tail(a), c) = v;\n/// let (gone_head::Wrap(b), d) = v;\n/// gone_tail(gone_head);\n/// ```\npub struct Kept;\n",
    );
    repo.commit("drop them");

    let rows: Vec<(String, usize, usize)> =
        found(&repo).into_iter().map(|m| (m.0, m.2, m.3)).collect();
    assert_eq!(
        rows,
        [
            ("fixture::gone_tail".to_owned(), 4, 5),
            ("fixture::gone_head".to_owned(), 4, 15)
        ]
    );
}

#[test]
fn generic_impl_parameters_leave_later_declarations_bound() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct ST;\npub fn gone_item() {}\npub fn gone_call() {}\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "/// ```\n/// impl Clone for Lower {\n///     fn clone(&self) -> Self {\n///         Lower\n///     }\n/// }\n/// impl<DB, ST> FromSql<ST, DB> for Lower {}\n/// for gone_item in list {\n///     gone_item;\n/// }\n/// gone_call();\n/// ```\npub struct Kept;\n",
    );
    repo.commit("drop them");

    let rows: Vec<(String, usize, usize)> =
        found(&repo).into_iter().map(|m| (m.0, m.2, m.3)).collect();
    assert_eq!(rows, [("fixture::gone_call".to_owned(), 11, 5)]);
}
