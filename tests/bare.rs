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
        "pub struct Gadget;\nstruct Hidden;\nmod private {\n    pub struct Inner;\n    pub struct Exported;\n}\npub use private::Exported;\n",
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
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = { path = \"vendor/helper\" }\n",
    );
    repo.write(
        "vendor/helper/Cargo.toml",
        "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("vendor/helper/src/lib.rs", "pub struct Thing;\n");
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
        [
            ("helper::Thing".to_owned(), 2, "Thing".to_owned()),
            ("helper::Thing".to_owned(), 11, "helper::Thing".to_owned()),
        ]
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
    repo.write("README.md", "`Locked` and `Newer`.\n");
    repo.commit("drop helper");

    let rows: Vec<(String, String)> = found(&repo).into_iter().map(|m| (m.0, m.5)).collect();
    assert_eq!(rows, [("helper::Locked".to_owned(), "Locked".to_owned())]);
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
