//! `.refquiem.toml`, default excludes, generated files and `--package`.

mod common;

use common::Repo;
use refquiem::Options;

/// `(group path, file)` for every mention, sorted.
fn found(options: &Options) -> Vec<(String, String)> {
    let report = refquiem::run(options).unwrap();
    let mut out: Vec<(String, String)> = report
        .groups
        .iter()
        .flat_map(|g| g.mentions.iter().map(|m| (g.path.clone(), m.file.clone())))
        .collect();
    out.sort();
    out
}

/// A repository whose `fixture::Gone` was removed, for the mentions a test adds.
fn removed_gone() -> Repo {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct Gone;\npub struct Parser;\nimpl Parser {\n    pub fn parse_expr() {}\n}\n",
    );
    repo.commit("start");
    repo.write("src/lib.rs", "pub struct Parser;\n");
    repo
}

#[test]
fn vocabularies_select_english_independently_of_cached_declarations() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct PhD;\npub struct MyGone;\npub struct Vec;\n",
    );
    repo.commit("public names");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "A PhD student, MyGone, Vec.\n");
    repo.commit("remove names");
    let options = Options::new(repo.path().to_path_buf());
    let paths = || {
        found(&options)
            .into_iter()
            .map(|(path, _)| path)
            .collect::<Vec<_>>()
    };
    assert_eq!(paths(), ["fixture::MyGone"]);
    for (config, expected) in [
        ("", vec!["fixture::MyGone"]),
        ("[vocabularies]\n", vec!["fixture::MyGone", "fixture::PhD"]),
        (
            "[vocabularies]\nbuiltins = [\"english\"]\n",
            vec!["fixture::MyGone"],
        ),
        (
            "[vocabularies]\nbuiltins = []\n",
            vec!["fixture::MyGone", "fixture::PhD"],
        ),
        (
            "[vocabularies]\nbuiltins = [\"english\", \"english\"]\n",
            vec!["fixture::MyGone"],
        ),
    ] {
        repo.write(".refquiem.toml", config);
        assert_eq!(paths(), expected, "{config}");
    }
}

#[test]
fn vocabularies_do_not_suppress_explicit_rust_mentions() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub struct PhD;\n");
    repo.commit("public name");
    repo.write(
        "src/lib.rs",
        "/// A PhD student.\n/// [PhD]\n/// ```\n/// let _: PhD;\n/// ```\npub struct Kept;\npub fn diagnostic() { println!(\"A PhD student\"); }\n",
    );
    repo.write("README.md", "A PhD student.\n`PhD`\n`fixture::PhD`\n");
    repo.write(
        ".refquiem.toml",
        "[vocabularies]\nbuiltins = [\"english\"]\n",
    );
    let report = refquiem::run(&Options::new(repo.path().to_path_buf())).unwrap();
    let mut mentions: Vec<_> = report
        .groups
        .iter()
        .flat_map(|group| {
            group
                .mentions
                .iter()
                .map(|mention| (mention.file.as_str(), mention.line, mention.text.as_str()))
        })
        .collect();
    mentions.sort_unstable();
    assert_eq!(
        mentions,
        [
            ("README.md", 2, "PhD"),
            ("README.md", 3, "fixture::PhD"),
            ("src/lib.rs", 2, "PhD"),
            ("src/lib.rs", 4, "PhD"),
        ]
    );
}

#[test]
fn vocabularies_preserve_serialized_names_with_or_without_a_rust_name_collision() {
    for rust_item in ["", "pub struct PhD;\n"] {
        let repo = Repo::new();
        repo.write(
            "src/lib.rs",
            &format!(
                "{rust_item}#[derive(serde::Serialize)]\npub struct Data {{ #[serde(rename = \"PhD\")] pub value: u8 }}\n"
            ),
        );
        repo.commit("serialized name");
        repo.write("src/lib.rs", "");
        repo.write(
            "README.md",
            "A PhD student.\n\n```json\n{\"PhD\": 1}\n```\n",
        );
        let options = Options::new(repo.path().to_path_buf());
        let report = refquiem::run(&options).unwrap();
        let mut mentions: Vec<_> = report
            .groups
            .iter()
            .flat_map(|group| {
                group
                    .mentions
                    .iter()
                    .map(|mention| (group.path.as_str(), mention.line))
            })
            .collect();
        mentions.sort_unstable();
        assert_eq!(
            mentions,
            [("fixture::Data::\"PhD\"", 1), ("fixture::Data::\"PhD\"", 4)],
            "{rust_item}"
        );
    }
}

#[test]
fn vocabularies_apply_workspace_wide_and_follow_the_selected_config() {
    let repo = Repo::new();
    repo.write("Cargo.toml", "[workspace]\nmembers = [\"a\", \"b\"]\n");
    for member in ["a", "b"] {
        repo.write(
            &format!("{member}/Cargo.toml"),
            &format!("[package]\nname = \"{member}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        );
        repo.write(&format!("{member}/src/lib.rs"), "pub struct PhD;\n");
    }
    repo.commit("member names");
    for member in ["a", "b"] {
        repo.write(&format!("{member}/src/lib.rs"), "");
        repo.write(&format!("{member}/README.md"), "A PhD student.\n");
    }
    repo.commit("remove member names");
    repo.write(".refquiem.toml", "[vocabularies]\n");
    let mut options = Options::new(repo.path().to_path_buf());
    assert_eq!(
        found(&options),
        [
            ("a::PhD".to_owned(), "a/README.md".to_owned()),
            ("b::PhD".to_owned(), "b/README.md".to_owned()),
        ]
    );
    options.package = Some("b".to_owned());
    assert_eq!(
        found(&options),
        [("b::PhD".to_owned(), "b/README.md".to_owned())]
    );
    let external = tempfile::tempdir().unwrap();
    let config = external.path().join("selection.toml");
    std::fs::write(&config, "[vocabularies]\nbuiltins = [\"english\"]\n").unwrap();
    options.config = Some(config);
    assert_eq!(found(&options), []);
}

#[test]
fn vocabularies_reject_unknown_identities_and_unsupported_fields() {
    let repo = removed_gone();
    let options = Options::new(repo.path().to_path_buf());
    let config_path = repo.path().join(".refquiem.toml");
    for config in [
        "[vocabularies]\nbuiltins = [\"engilsh\"]\n",
        "[vocabularies]\nbuiltins = [\"english@missing\"]\n",
        "[vocabularies]\nbuiltins = [\"rust\"]\n",
        "[vocabularies]\nbuiltin = [\"english\"]\n",
        "[vocabularies]\nbuiltins = \"english\"\n",
        "[vocabularies]\nfiles = \"custom.toml\"\n",
    ] {
        repo.write(".refquiem.toml", config);
        let error = refquiem::run(&options).unwrap_err();
        assert!(
            matches!(&error, refquiem::Error::Config { path, .. } if path == &config_path),
            "{config} {error}"
        );
    }
}

#[test]
fn a_bare_allow_entry_keeps_serialized_names_alive() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "#[derive(serde::Serialize)]\npub struct Data { #[serde(rename = \"old_alias_842\")] pub value: u8 }\npub struct Gone;\n",
    );
    repo.commit("public names");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "`old_alias_842` and `fixture::Gone`\n");
    let options = Options::new(repo.path().to_path_buf());
    assert_eq!(
        found(&options),
        [
            (
                "fixture::Data::\"old_alias_842\"".to_owned(),
                "README.md".to_owned()
            ),
            ("fixture::Gone".to_owned(), "README.md".to_owned()),
        ]
    );
    repo.write(".refquiem.toml", "allow = [\"old_alias_842\"]\n");
    assert_eq!(
        found(&options),
        [("fixture::Gone".to_owned(), "README.md".to_owned())]
    );
}

#[test]
fn release_histories_are_not_scanned_by_default() {
    let repo = removed_gone();
    for file in [
        "RELEASES.md",
        "docs/RELEASES-1.0.md",
        "RELEASE-NOTES.md",
        "RELEASE_NOTES.md",
        "NEWS.md",
        "HISTORY.md",
        "CHANGES.md",
    ] {
        repo.write(file, "`fixture::Gone`\n");
    }
    repo.write("README.md", "`fixture::Gone`\n");
    assert_eq!(
        found(&Options::new(repo.path().to_path_buf())),
        [("fixture::Gone".to_owned(), "README.md".to_owned())]
    );
}

#[test]
fn changelogs_are_not_scanned_by_default() {
    let repo = removed_gone();
    repo.write("CHANGELOG.md", "Removed `fixture::Gone`.\n");
    repo.write("changelog/0.2.md", "Removed `fixture::Gone`.\n");
    repo.write("README.md", "`fixture::Gone`\n");
    repo.commit("drop Gone");

    assert_eq!(
        found(&Options::new(repo.path().to_path_buf())),
        [("fixture::Gone".to_owned(), "README.md".to_owned())]
    );
}

#[test]
fn release_history_files_are_not_scanned_by_default() {
    let repo = removed_gone();
    for file in [
        "RELEASES.md",
        "RELEASE-NOTES.md",
        "RELEASE_NOTES.md",
        "NEWS.md",
        "HISTORY.md",
        "CHANGES.md",
        "docs/CHANGES.md",
    ] {
        repo.write(file, "Removed `fixture::Gone`.\n");
    }
    repo.write("README.md", "`fixture::Gone`\n");
    repo.commit("drop Gone");

    assert_eq!(
        found(&Options::new(repo.path().to_path_buf())),
        [("fixture::Gone".to_owned(), "README.md".to_owned())]
    );
}

#[test]
fn exclude_globs_add_to_the_defaults() {
    let repo = removed_gone();
    repo.write(".refquiem.toml", "exclude = [\"docs/legacy/\"]\n");
    repo.write("docs/legacy/old.md", "`fixture::Gone`\n");
    repo.write("docs/new.md", "`fixture::Gone`\n");
    repo.write("CHANGELOG.md", "`fixture::Gone`\n");
    repo.commit("drop Gone");

    assert_eq!(
        found(&Options::new(repo.path().to_path_buf())),
        [("fixture::Gone".to_owned(), "docs/new.md".to_owned())]
    );
}

#[test]
fn allow_entries_drop_groups_by_path_or_path_ending() {
    let repo = Repo::new();
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nstructopt = { path = \"vendor/structopt\" }\n",
    );
    repo.write(
        "vendor/structopt/Cargo.toml",
        "[package]\nname = \"structopt\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("vendor/structopt/src/lib.rs", "");
    repo.write(
        "src/lib.rs",
        "pub struct Gone;\npub struct Parser;\nimpl Parser {\n    pub fn parse_expr() {}\n}\n",
    );
    repo.commit("start");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("src/lib.rs", "pub struct Parser;\n");
    repo.write(
        ".refquiem.toml",
        "allow = [\"Parser::parse_expr\", \"structopt\"]\n",
    );
    repo.write(
        "README.md",
        "`fixture::Parser::parse_expr`, `structopt::App` and `fixture::Gone`\n",
    );
    repo.commit("drop them");

    assert_eq!(
        found(&Options::new(repo.path().to_path_buf())),
        [("fixture::Gone".to_owned(), "README.md".to_owned())]
    );
}

#[test]
fn generated_files_are_skipped_unless_scanned_on_purpose() {
    let repo = removed_gone();
    repo.write(
        "src/generated.rs",
        "// @generated by a tool\n// `fixture::Gone`\n",
    );
    repo.write(
        "src/proto.rs",
        "// Code generated by protoc. DO NOT EDIT.\n// `fixture::Gone`\n",
    );
    repo.write(
        "src/bindings.rs",
        "/* automatically generated by rust-bindgen */\n// `fixture::Gone`\n",
    );
    repo.write(
        "src/handwritten.rs",
        "// Code generated by hand, edit freely\n// `fixture::Gone`\n",
    );
    repo.write(".refquiem.toml", "generated-markers = [\"rust-bindgen\"]\n");
    repo.commit("drop Gone");
    let options = Options::new(repo.path().to_path_buf());
    assert_eq!(
        found(&options),
        [("fixture::Gone".to_owned(), "src/handwritten.rs".to_owned())]
    );

    repo.write(
        ".refquiem.toml",
        "generated-markers = [\"rust-bindgen\"]\nscan-generated = true\n",
    );
    let files: Vec<String> = found(&options).into_iter().map(|(_, file)| file).collect();
    assert_eq!(
        files,
        [
            "src/bindings.rs",
            "src/generated.rs",
            "src/handwritten.rs",
            "src/proto.rs"
        ]
    );
}

#[test]
fn exclude_globs_are_relative_to_a_workspace_in_a_subdirectory() {
    let repo = Repo::new();
    std::fs::remove_file(repo.path().join("Cargo.toml")).unwrap();
    repo.write(
        "ws/Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("ws/src/lib.rs", "pub struct Gone;\n");
    repo.commit("start");
    repo.write("ws/src/lib.rs", "");
    repo.write("ws/.refquiem.toml", "exclude = [\"/docs/\"]\n");
    repo.write("ws/docs/old.md", "`fixture::Gone`\n");
    repo.write("ws/README.md", "`fixture::Gone`\n");
    repo.commit("drop Gone");

    assert_eq!(
        found(&Options::new(repo.path().join("ws"))),
        [("fixture::Gone".to_owned(), "ws/README.md".to_owned())]
    );
}

#[test]
fn exclude_globs_leave_files_above_a_subdirectory_workspace_alone() {
    let repo = Repo::new();
    std::fs::remove_file(repo.path().join("Cargo.toml")).unwrap();
    repo.write(
        "ws/Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("ws/src/lib.rs", "pub struct Gone;\n");
    repo.commit("start");
    repo.write("ws/src/lib.rs", "");
    repo.write("ws/.refquiem.toml", "exclude = [\"NOTES.md\"]\n");
    repo.write("ws/NOTES.md", "`fixture::Gone`\n");
    repo.write("NOTES.md", "`fixture::Gone`\n");
    repo.commit("drop Gone");

    assert_eq!(
        found(&Options::new(repo.path().join("ws"))),
        [("fixture::Gone".to_owned(), "NOTES.md".to_owned())]
    );
}

#[test]
fn since_in_the_config_starts_the_range() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub struct Early;\npub struct Late;\n");
    repo.commit("start");
    repo.write("src/lib.rs", "pub struct Late;\n");
    repo.commit("drop Early");
    let cut = repo.commit("cut");
    repo.write("src/lib.rs", "");
    repo.write(".refquiem.toml", &format!("since = \"{cut}\"\n"));
    repo.write("README.md", "`fixture::Early` and `fixture::Late`\n");
    repo.commit("drop Late");

    assert_eq!(
        found(&Options::new(repo.path().to_path_buf())),
        [("fixture::Late".to_owned(), "README.md".to_owned())]
    );
}

#[test]
fn an_explicit_config_file_replaces_the_workspace_one() {
    let repo = removed_gone();
    repo.write(".refquiem.toml", "exclude = [\"README.md\"]\n");
    repo.write("other.toml", "exclude = [\"NOTES.md\"]\n");
    repo.write("README.md", "`fixture::Gone`\n");
    repo.write("NOTES.md", "`fixture::Gone`\n");
    repo.commit("drop Gone");

    let mut options = Options::new(repo.path().to_path_buf());
    assert_eq!(
        found(&options),
        [("fixture::Gone".to_owned(), "NOTES.md".to_owned())]
    );
    options.config = Some(repo.path().join("other.toml"));
    assert_eq!(
        found(&options),
        [("fixture::Gone".to_owned(), "README.md".to_owned())]
    );
}

#[test]
fn an_invalid_config_is_an_error() {
    let repo = removed_gone();
    repo.write(".refquiem.toml", "unknown-key = 1\n");
    repo.commit("drop Gone");

    let error = refquiem::run(&Options::new(repo.path().to_path_buf())).unwrap_err();
    assert!(matches!(error, refquiem::Error::Config { .. }), "{error}");
}

#[test]
fn package_scans_only_the_files_of_that_member() {
    let repo = Repo::new();
    repo.write("Cargo.toml", "[workspace]\nmembers = [\"a\", \"b\"]\n");
    for member in ["a", "b"] {
        repo.write(
            &format!("{member}/Cargo.toml"),
            &format!("[package]\nname = \"{member}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        );
    }
    repo.write("a/src/lib.rs", "pub struct Gone;\n");
    repo.write("b/src/lib.rs", "");
    repo.commit("start");
    repo.write("a/src/lib.rs", "");
    repo.write("a/README.md", "`a::Gone`\n");
    repo.write("b/README.md", "`a::Gone`\n");
    repo.write("README.md", "`a::Gone`\n");
    repo.commit("drop Gone");

    let mut options = Options::new(repo.path().to_path_buf());
    options.package = Some("a".to_owned());
    assert_eq!(
        found(&options),
        [("a::Gone".to_owned(), "a/README.md".to_owned())]
    );
    options.package = Some("missing".to_owned());
    let error = refquiem::run(&options).unwrap_err();
    assert!(
        matches!(&error, refquiem::Error::UnknownPackage(name) if name == "missing"),
        "{error}"
    );
}
