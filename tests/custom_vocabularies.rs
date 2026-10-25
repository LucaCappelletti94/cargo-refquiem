//! Project-owned vocabulary loading and ambiguity boundaries.

mod common;

use common::Repo;
use refquiem::Options;

fn select(repo: &Repo, kind: &str, case: &str, words: &[&str]) {
    repo.write(
        "domain.toml",
        &format!(
            "format-version = 1\nkind = {kind:?}\ncase = {case:?}\nwords = {}\n",
            serde_json::to_string(words).unwrap()
        ),
    );
    repo.write(
        ".refquiem.toml",
        "[vocabularies]\nfiles = [\"domain.toml\"]\n",
    );
}

fn paths(options: &Options) -> Vec<String> {
    let mut paths: Vec<_> = refquiem::run(options)
        .unwrap()
        .groups
        .into_iter()
        .map(|group| group.path)
        .collect();
    paths.sort();
    paths
}

#[test]
fn custom_vocabulary_category_preserves_explicit_rust_references() {
    for (kind, expected) in [
        (
            "natural-language",
            vec![
                ("README.md", 2),
                ("README.md", 3),
                ("src/lib.rs", 2),
                ("src/lib.rs", 4),
            ],
        ),
        (
            "technical",
            vec![("README.md", 3), ("src/lib.rs", 2), ("src/lib.rs", 4)],
        ),
    ] {
        let repo = Repo::new();
        repo.write("src/lib.rs", "pub struct PrivateSDKThing;\n");
        repo.commit("public SDK name");
        repo.write(
            "src/lib.rs",
            "/// PrivateSDKThing\n/// [PrivateSDKThing]\n/// ```\n/// let _: PrivateSDKThing;\n/// ```\npub struct Kept;\npub fn diagnostic() { println!(\"PrivateSDKThing failed\"); }\n",
        );
        repo.write(
            "README.md",
            "PrivateSDKThing\n`PrivateSDKThing`\n`fixture::PrivateSDKThing`\n",
        );
        select(&repo, kind, "sensitive", &["PrivateSDKThing"]);
        let report = refquiem::run(&Options::new(repo.path().to_path_buf())).unwrap();
        let mut mentions: Vec<_> = report
            .groups
            .iter()
            .flat_map(|group| {
                group
                    .mentions
                    .iter()
                    .map(|mention| (mention.file.as_str(), mention.line))
            })
            .collect();
        mentions.sort_unstable();
        assert_eq!(mentions, expected, "{kind}");
    }
}

#[test]
fn custom_vocabulary_owns_its_case_rule_without_unicode_composition_or_case_folding() {
    for (name, word, case, ambiguous) in [
        ("PrivateSDKThing", "PrivateSDKThing", "sensitive", true),
        ("PrivateSDKThing", "privatesdkthing", "sensitive", false),
        (
            "PrivateSDKThing",
            "privatesdkthing",
            "ascii-insensitive",
            true,
        ),
        ("ÄTHER", "äther", "ascii-insensitive", false),
        ("ÄTHER", "äther", "unicode-lowercase", true),
        ("Straße", "STRASSE", "unicode-lowercase", false),
        ("İTEM", "i\u{307}tem", "unicode-lowercase", true),
        ("ÉTHER", "e\u{301}ther", "unicode-lowercase", false),
        ("ΟΣ", "ος", "unicode-lowercase", true),
        ("ος", "ΟΣ", "unicode-lowercase", true),
        ("ΟΣ", "ος", "ascii-insensitive", false),
    ] {
        let repo = Repo::new();
        repo.write("src/lib.rs", &format!("pub struct {name};\n"));
        repo.commit("public name");
        repo.write("src/lib.rs", "");
        repo.write("README.md", &format!("`{name}`\n"));
        repo.write(".refquiem.toml", "[vocabularies]\n");
        let options = Options::new(repo.path().to_path_buf());
        assert_eq!(paths(&options), [format!("fixture::{name}")]);
        select(&repo, "technical", case, &[word, word]);
        let expected = if ambiguous {
            vec![]
        } else {
            vec![format!("fixture::{name}")]
        };
        assert_eq!(paths(&options), expected, "{name} {word} {case}");
    }
}

#[test]
fn custom_file_edits_refresh_policy_without_enabling_english() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct PrivateSDKThing;\npub struct OtherSDKThing;\npub struct PhD;\npub struct Vec;\n",
    );
    repo.commit("public names");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "PrivateSDKThing OtherSDKThing PhD Vec\n");
    let options = Options::new(repo.path().to_path_buf());
    select(&repo, "technical", "sensitive", &["PrivateSDKThing"]);
    assert_eq!(paths(&options), ["fixture::OtherSDKThing", "fixture::PhD"]);
    select(&repo, "technical", "sensitive", &["OtherSDKThing"]);
    assert_eq!(
        paths(&options),
        ["fixture::PhD", "fixture::PrivateSDKThing"]
    );
    repo.write(".refquiem.toml", "[vocabularies]\n");
    assert_eq!(
        paths(&options),
        [
            "fixture::OtherSDKThing",
            "fixture::PhD",
            "fixture::PrivateSDKThing",
        ]
    );
}

#[test]
fn punctuation_entries_do_not_exempt_separate_components_or_rust_paths() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct PrivateSDKGrow;\npub struct PrivateSDKThing;\npub struct BLUETOOTH_SCAN;\n",
    );
    repo.commit("public names");
    repo.write("src/lib.rs", "");
    repo.write(
        "README.md",
        "`PrivateSDKGrow` PrivateSDKThing BLUETOOTH_SCAN\n`fixture::PrivateSDKThing`\n",
    );
    select(
        &repo,
        "technical",
        "sensitive",
        &[
            "memory.grow",
            "memory.PrivateSDKGrow",
            "fixture::PrivateSDKThing",
            "android.permission.BLUETOOTH_SCAN",
        ],
    );
    assert_eq!(
        paths(&Options::new(repo.path().to_path_buf())),
        [
            "fixture::BLUETOOTH_SCAN",
            "fixture::PrivateSDKGrow",
            "fixture::PrivateSDKThing",
        ]
    );
}

#[test]
fn custom_vocabulary_preserves_serialized_contracts_with_or_without_name_collision() {
    for rust_item in ["", "pub struct PrivateSDKThing;\n"] {
        let repo = Repo::new();
        repo.write(
            "src/lib.rs",
            &format!(
                "{rust_item}#[derive(serde::Serialize)]\npub struct Data {{ #[serde(rename = \"PrivateSDKThing\")] pub value: u8 }}\n"
            ),
        );
        repo.commit("serialized contract");
        repo.write("src/lib.rs", "");
        repo.write(
            "README.md",
            "PrivateSDKThing\n\n```json\n{\"PrivateSDKThing\": 1}\n```\n",
        );
        select(&repo, "technical", "sensitive", &["PrivateSDKThing"]);
        let report = refquiem::run(&Options::new(repo.path().to_path_buf())).unwrap();
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
            [
                ("fixture::Data::\"PrivateSDKThing\"", 1),
                ("fixture::Data::\"PrivateSDKThing\"", 4),
            ],
            "{rust_item}"
        );
    }
}

#[test]
fn multiple_custom_files_keep_their_case_and_context_rules() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct PrivateSDKThing;\npub struct OtherSDKThing;\npub struct ÄTHER;\n",
    );
    repo.commit("public names");
    repo.write("src/lib.rs", "");
    repo.write(
        "README.md",
        "PrivateSDKThing OtherSDKThing ÄTHER\n`PrivateSDKThing` `OtherSDKThing` `ÄTHER`\n`fixture::PrivateSDKThing` `fixture::OtherSDKThing` `fixture::ÄTHER`\n",
    );
    for (file, kind, case, word) in [
        (
            "natural.toml",
            "natural-language",
            "sensitive",
            "PrivateSDKThing",
        ),
        (
            "ascii.toml",
            "technical",
            "ascii-insensitive",
            "othersdkthing",
        ),
        ("unicode.toml", "technical", "unicode-lowercase", "äther"),
    ] {
        repo.write(
            file,
            &format!("format-version = 1\nkind = {kind:?}\ncase = {case:?}\nwords = [{word:?}]\n"),
        );
    }
    repo.write(
        ".refquiem.toml",
        "[vocabularies]\nfiles = [\"unicode.toml\", \"natural.toml\", \"ascii.toml\"]\n",
    );
    let report = refquiem::run(&Options::new(repo.path().to_path_buf())).unwrap();
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
        [
            ("fixture::OtherSDKThing", 3),
            ("fixture::PrivateSDKThing", 2),
            ("fixture::PrivateSDKThing", 3),
            ("fixture::ÄTHER", 3),
        ]
    );
}

#[test]
fn invalid_custom_documents_abort_scanning_and_effective_selection_with_their_path() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub struct Kept;\n");
    repo.commit("public name");
    repo.write(
        ".refquiem.toml",
        "[vocabularies]\nfiles = [\"domain.toml\"]\n",
    );
    let options = Options::new(repo.path().to_path_buf());
    let valid = "format-version = 1\nkind = \"technical\"\ncase = \"sensitive\"\nwords = [\"PrivateSDKThing\"]\n";
    for (from, to) in [
        ("format-version = 1\n", ""),
        ("format-version = 1", "format-version = 0"),
        ("format-version = 1", "format-version = 2"),
        ("format-version = 1", "format-version = -1"),
        ("format-version = 1", "format-version = \"1\""),
        ("kind = \"technical\"\n", ""),
        ("kind = \"technical\"", "kind = \"sdk\""),
        ("case = \"sensitive\"\n", ""),
        ("case = \"sensitive\"", "case = \"fold\""),
        ("words = [\"PrivateSDKThing\"]\n", ""),
        ("[\"PrivateSDKThing\"]", "\"PrivateSDKThing\""),
        ("[\"PrivateSDKThing\"]", "[1]"),
        ("[\"PrivateSDKThing\"]", "[\"\"]"),
        ("[\"PrivateSDKThing\"]", "[\" leading\"]"),
        ("[\"PrivateSDKThing\"]", "[\"trailing \"]"),
        ("[\"PrivateSDKThing\"]", "[\"two words\"]"),
        ("[\"PrivateSDKThing\"]", "[\"line\\nfeed\"]"),
        ("[\"PrivateSDKThing\"]", "[\"word\\u2003space\"]"),
        ("[\"PrivateSDKThing\"]", "[\"word\\u007Fcontrol\"]"),
        ("format-version = 1", "extra = 1\nformat-version = 1"),
    ] {
        let contents = valid.replacen(from, to, 1);
        repo.write("domain.toml", &contents);
        for result in [
            refquiem::run(&options).map(|_| ()),
            refquiem::vocabulary::effective(&options).map(|_| ()),
        ] {
            let error = result.unwrap_err();
            assert!(
                error.to_string().contains("domain.toml"),
                "{contents:?} {error}"
            );
        }
    }
}

#[test]
fn missing_unreadable_and_non_utf8_custom_files_abort_selection() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "");
    repo.commit("empty library");
    repo.write(
        ".refquiem.toml",
        "[vocabularies]\nfiles = [\"missing.toml\"]\n",
    );
    let options = Options::new(repo.path().to_path_buf());
    for name in ["missing.toml", "directory.toml", "bytes.toml"] {
        if name == "directory.toml" {
            std::fs::create_dir(repo.path().join(name)).unwrap();
        } else if name == "bytes.toml" {
            std::fs::write(repo.path().join(name), [0xff]).unwrap();
        }
        repo.write(
            ".refquiem.toml",
            &format!("[vocabularies]\nfiles = [{name:?}]\n"),
        );
        for result in [
            refquiem::run(&options).map(|_| ()),
            refquiem::vocabulary::effective(&options).map(|_| ()),
        ] {
            let error = result.unwrap_err();
            assert!(error.to_string().contains(name), "{error}");
        }
    }
}

#[cfg(unix)]
#[test]
fn symlink_aliases_select_one_canonical_custom_file() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "invalid Rust {");
    select(&repo, "technical", "sensitive", &["PrivateSDKThing"]);
    std::os::unix::fs::symlink("domain.toml", repo.path().join("alias.toml")).unwrap();
    repo.write(
        ".refquiem.toml",
        "[vocabularies]\nfiles = [\"alias.toml\", \"domain.toml\", \"./domain.toml\"]\n",
    );
    let selection =
        refquiem::vocabulary::effective(&Options::new(repo.path().to_path_buf())).unwrap();
    let json = serde_json::to_value(selection).unwrap();
    let selected: Vec<_> = json["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap())
        .collect();
    assert_eq!(
        selected,
        [std::fs::canonicalize(repo.path().join("domain.toml"))
            .unwrap()
            .to_str()
            .unwrap()]
    );
}

#[test]
fn vocabulary_ambiguity_does_not_consume_a_redundant_marker() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub struct PrivateSDKThing;\n");
    repo.commit("public name");
    repo.write("src/lib.rs", "");
    repo.write(
        "README.md",
        "<!-- refquiem: allow(PrivateSDKThing) -->\nPrivateSDKThing\n",
    );
    select(&repo, "technical", "sensitive", &["PrivateSDKThing"]);
    let options = Options::new(repo.path().to_path_buf());
    let report = refquiem::run(&options).unwrap();
    assert_eq!(report.groups, []);
    let markers: Vec<_> = report
        .unused_markers
        .iter()
        .map(|marker| (marker.file.as_str(), marker.line, marker.name.as_str()))
        .collect();
    assert_eq!(markers, [("README.md", 1, "PrivateSDKThing")]);
    repo.write(".refquiem.toml", "[vocabularies]\n");
    let report = refquiem::run(&options).unwrap();
    assert_eq!(report.groups, []);
    assert_eq!(report.unused_markers, []);
}

#[test]
fn builtins_and_empty_or_populated_custom_files_form_one_optional_selection() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct PrivateSDKThing;\npub struct PhD;\npub struct Vec;\n",
    );
    repo.commit("public names");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "PrivateSDKThing PhD Vec\n");
    let options = Options::new(repo.path().to_path_buf());
    for (words, expected) in [
        (vec!["privatesdkthing"], vec![]),
        (vec![], vec!["fixture::PrivateSDKThing"]),
    ] {
        select(&repo, "technical", "ascii-insensitive", &words);
        repo.write(
            ".refquiem.toml",
            "[vocabularies]\nbuiltins = [\"english\"]\nfiles = [\"domain.toml\"]\n",
        );
        assert_eq!(paths(&options), expected);
    }
}

#[test]
fn overlapping_categories_preserve_the_technical_context_regardless_of_file_order() {
    for (first, second) in [
        ("natural-language", "technical"),
        ("technical", "natural-language"),
    ] {
        for case in ["sensitive", "ascii-insensitive", "unicode-lowercase"] {
            let repo = Repo::new();
            repo.write("src/lib.rs", "pub struct PrivateSDKThing;\n");
            repo.commit("public name");
            repo.write("src/lib.rs", "");
            repo.write(
                "README.md",
                "`PrivateSDKThing`\n`fixture::PrivateSDKThing`\n",
            );
            for (file, kind) in [("a.toml", first), ("z.toml", second)] {
                repo.write(
                    file,
                    &format!("format-version = 1\nkind = {kind:?}\ncase = {case:?}\nwords = [\"PrivateSDKThing\"]\n"),
                );
            }
            repo.write(
                ".refquiem.toml",
                "[vocabularies]\nfiles = [\"z.toml\", \"a.toml\"]\n",
            );
            let report = refquiem::run(&Options::new(repo.path().to_path_buf())).unwrap();
            let mentions: Vec<_> = report
                .groups
                .iter()
                .flat_map(|group| group.mentions.iter().map(|mention| mention.line))
                .collect();
            assert_eq!(mentions, [2], "{first} {second} {case}");
        }
    }
}
