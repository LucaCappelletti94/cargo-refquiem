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
            ("helper".to_owned(), "helper::Newer".to_owned())
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
