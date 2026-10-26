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
        "[vocabularies]\nbuiltins = [\"sql\"]\n",
        "[vocabularies]\nbuiltins = [\"sql@2016\"]\n",
        "[vocabularies]\nbuiltins = [\"wasi\"]\n",
        "[vocabularies]\nbuiltins = [\"wasi@preview2\"]\n",
        "[vocabularies]\nbuiltins = [\"wasm\"]\n",
        "[vocabularies]\nbuiltins = [\"wasm@2.0\"]\n",
        "[vocabularies]\nbuiltins = [\"android\"]\n",
        "[vocabularies]\nbuiltins = [\"android@35\"]\n",
        "[vocabularies]\nbuiltins = [\"uikit\"]\n",
        "[vocabularies]\nbuiltins = [\"uikit@26.0\"]\n",
        "[vocabularies]\nbuiltins = [\"posix\"]\n",
        "[vocabularies]\nbuiltins = [\"posix@2024\"]\n",
        "[vocabularies]\nbuiltins = [\"web-api\"]\n",
        "[vocabularies]\nbuiltins = [\"web-api@2026-09\"]\n",
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
fn web_api_2026_10_claims_compound_interfaces_and_camel_case_members() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "#![allow(non_snake_case, non_camel_case_types)]\npub struct HTMLElement;\npub struct ANGLE_instanced_arrays;\npub fn querySelector() {}\npub fn atob() {}\n",
    );
    repo.commit("public names");
    repo.write(
        "src/lib.rs",
        "/// ```\n/// let _ = HTMLElement;\n/// ```\npub struct Kept;\n",
    );
    repo.write(
        "README.md",
        "The HTMLElement calls querySelector with ANGLE_instanced_arrays.\n`atob` and `fixture::HTMLElement`\n",
    );
    repo.commit("remove names");
    let options = Options::new(repo.path().to_path_buf());
    let mentions = || {
        let mut out: Vec<(String, usize, String)> = refquiem::run(&options)
            .unwrap()
            .groups
            .iter()
            .flat_map(|group| group.mentions.iter())
            .map(|m| (m.file.clone(), m.line, m.text.clone()))
            .collect();
        out.sort();
        out
    };
    let row = |file: &str, line, text: &str| (file.to_owned(), line, text.to_owned());
    let selected = vec![
        row("README.md", 2, "atob"),
        row("README.md", 2, "fixture::HTMLElement"),
        row("src/lib.rs", 2, "HTMLElement"),
    ];
    let mut unselected = selected.clone();
    unselected.extend([
        row("README.md", 1, "ANGLE_instanced_arrays"),
        row("README.md", 1, "HTMLElement"),
        row("README.md", 1, "querySelector"),
    ]);
    unselected.sort();
    for (config, expected) in [
        ("", &unselected),
        ("[vocabularies]\nbuiltins = [\"wasm@3.0\"]\n", &unselected),
        (
            "[vocabularies]\nbuiltins = [\"web-api@2026-10\"]\n",
            &selected,
        ),
    ] {
        repo.write(".refquiem.toml", config);
        assert_eq!(&mentions(), expected, "{config}");
    }
}

#[test]
fn posix_2017_claims_constants_errors_signals_and_underscore_functions() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "#![allow(non_upper_case_globals)]\npub const O_RDONLY: u8 = 0;\npub const EINVAL: u8 = 0;\npub const SIGTERM: u8 = 0;\npub const BUFSIZ: u8 = 0;\npub fn pthread_create() {}\npub fn strlen() {}\n",
    );
    repo.commit("public names");
    repo.write(
        "src/lib.rs",
        "/// ```\n/// let _ = O_RDONLY;\n/// ```\npub struct Kept;\n",
    );
    repo.write(
        "README.md",
        "Opening with O_RDONLY calls pthread_create.\n`EINVAL` `SIGTERM` `strlen` `BUFSIZ`\n`fixture::O_RDONLY`\n",
    );
    repo.commit("remove names");
    let options = Options::new(repo.path().to_path_buf());
    let mentions = || {
        let mut out: Vec<(String, usize, String)> = refquiem::run(&options)
            .unwrap()
            .groups
            .iter()
            .flat_map(|group| group.mentions.iter())
            .map(|m| (m.file.clone(), m.line, m.text.clone()))
            .collect();
        out.sort();
        out
    };
    let row = |file: &str, line, text: &str| (file.to_owned(), line, text.to_owned());
    let selected = vec![
        row("README.md", 2, "BUFSIZ"),
        row("README.md", 2, "strlen"),
        row("README.md", 3, "fixture::O_RDONLY"),
        row("src/lib.rs", 2, "O_RDONLY"),
    ];
    let mut unselected = selected.clone();
    unselected.extend([
        row("README.md", 1, "O_RDONLY"),
        row("README.md", 1, "pthread_create"),
        row("README.md", 2, "EINVAL"),
        row("README.md", 2, "SIGTERM"),
    ]);
    unselected.sort();
    for (config, expected) in [
        ("", &unselected),
        ("[vocabularies]\nbuiltins = [\"wasm@3.0\"]\n", &unselected),
        ("[vocabularies]\nbuiltins = [\"posix@2017\"]\n", &selected),
    ] {
        repo.write(".refquiem.toml", config);
        assert_eq!(&mentions(), expected, "{config}");
    }
}

#[test]
fn uikit_27_1_claims_objective_c_types_and_camel_case_selectors() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "#![allow(non_snake_case, non_upper_case_globals)]\npub struct UIViewController;\npub fn viewDidLoad() {}\npub fn isExclusiveTouch() {}\npub struct UIGraphicsPDFRenderer;\npub struct UIGraphicsPdfRenderer;\npub const NSBluetoothAlwaysUsageDescription: u8 = 0;\n",
    );
    repo.commit("public names");
    repo.write(
        "src/lib.rs",
        "/// ```\n/// let _ = UIViewController;\n/// ```\npub struct Kept;\n",
    );
    repo.write(
        "README.md",
        "The UIViewController calls `viewDidLoad` and `isExclusiveTouch`.\n`UIGraphicsPDFRenderer` `UIGraphicsPdfRenderer` NSBluetoothAlwaysUsageDescription\n`fixture::UIViewController`\n",
    );
    repo.commit("remove names");
    let options = Options::new(repo.path().to_path_buf());
    let mentions = || {
        let mut out: Vec<(String, usize, String)> = refquiem::run(&options)
            .unwrap()
            .groups
            .iter()
            .flat_map(|group| group.mentions.iter())
            .map(|m| (m.file.clone(), m.line, m.text.clone()))
            .collect();
        out.sort();
        out
    };
    let row = |file: &str, line, text: &str| (file.to_owned(), line, text.to_owned());
    let selected = vec![
        row("README.md", 2, "NSBluetoothAlwaysUsageDescription"),
        row("README.md", 2, "UIGraphicsPdfRenderer"),
        row("README.md", 3, "fixture::UIViewController"),
        row("src/lib.rs", 2, "UIViewController"),
    ];
    let mut unselected = selected.clone();
    unselected.extend([
        row("README.md", 1, "UIViewController"),
        row("README.md", 1, "isExclusiveTouch"),
        row("README.md", 1, "viewDidLoad"),
        row("README.md", 2, "UIGraphicsPDFRenderer"),
    ]);
    unselected.sort();
    for (config, expected) in [
        ("", &unselected),
        ("[vocabularies]\nbuiltins = [\"wasm@3.0\"]\n", &unselected),
        ("[vocabularies]\nbuiltins = [\"uikit@27.1\"]\n", &selected),
    ] {
        repo.write(".refquiem.toml", config);
        assert_eq!(&mentions(), expected, "{config}");
    }
}

#[test]
fn android_36_claims_compound_classes_and_camel_case_methods() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct NativeActivity;\n#[allow(non_snake_case)]\npub fn onCreate() {}\npub struct Addr;\npub const ACTION_VIEW: u8 = 0;\n",
    );
    repo.commit("public names");
    repo.write(
        "src/lib.rs",
        "/// ```\n/// let _ = NativeActivity;\n/// ```\npub struct Kept;\n",
    );
    repo.write(
        "README.md",
        "The NativeActivity calls `onCreate`.\n`Addr` and ACTION_VIEW\n`fixture::NativeActivity`\n",
    );
    repo.commit("remove names");
    let options = Options::new(repo.path().to_path_buf());
    let mentions = || {
        let mut out: Vec<(String, usize, String)> = refquiem::run(&options)
            .unwrap()
            .groups
            .iter()
            .flat_map(|group| group.mentions.iter())
            .map(|m| (m.file.clone(), m.line, m.text.clone()))
            .collect();
        out.sort();
        out
    };
    let row = |file: &str, line, text: &str| (file.to_owned(), line, text.to_owned());
    let selected = vec![
        row("README.md", 2, "ACTION_VIEW"),
        row("README.md", 2, "Addr"),
        row("README.md", 3, "fixture::NativeActivity"),
        row("src/lib.rs", 2, "NativeActivity"),
    ];
    let mut unselected = selected.clone();
    unselected.extend([
        row("README.md", 1, "NativeActivity"),
        row("README.md", 1, "onCreate"),
    ]);
    unselected.sort();
    for (config, expected) in [
        ("", &unselected),
        ("[vocabularies]\nbuiltins = [\"wasm@3.0\"]\n", &unselected),
        ("[vocabularies]\nbuiltins = [\"android@36\"]\n", &selected),
    ] {
        repo.write(".refquiem.toml", config);
        assert_eq!(&mentions(), expected, "{config}");
    }
}

#[test]
fn wasm_3_claims_undotted_instructions_in_prose_and_code_spans() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub fn call_indirect() {}\npub fn br_table() {}\npub fn try_table() {}\npub fn local_get() {}\n",
    );
    repo.commit("public names");
    repo.write(
        "src/lib.rs",
        "/// ```\n/// call_indirect();\n/// ```\npub struct Kept;\n",
    );
    repo.write(
        "README.md",
        "Lower call_indirect and `br_table`.\n`try_table`\n`fixture::call_indirect`\nlocal_get\n",
    );
    repo.commit("remove names");
    let options = Options::new(repo.path().to_path_buf());
    let mentions = || {
        let mut out: Vec<(String, usize, String)> = refquiem::run(&options)
            .unwrap()
            .groups
            .iter()
            .flat_map(|group| group.mentions.iter())
            .map(|m| (m.file.clone(), m.line, m.text.clone()))
            .collect();
        out.sort();
        out
    };
    let row = |file: &str, line, text: &str| (file.to_owned(), line, text.to_owned());
    let selected = vec![
        row("README.md", 3, "fixture::call_indirect"),
        row("README.md", 4, "local_get"),
        row("src/lib.rs", 2, "call_indirect"),
    ];
    let mut unselected = selected.clone();
    unselected.extend([
        row("README.md", 1, "br_table"),
        row("README.md", 1, "call_indirect"),
        row("README.md", 2, "try_table"),
    ]);
    unselected.sort();
    for (config, expected) in [
        ("", &unselected),
        (
            "[vocabularies]\nbuiltins = [\"wasi@preview1\"]\n",
            &unselected,
        ),
        ("[vocabularies]\nbuiltins = [\"wasm@3.0\"]\n", &selected),
    ] {
        repo.write(".refquiem.toml", config);
        assert_eq!(&mentions(), expected, "{config}");
    }
}

#[test]
fn wasi_preview1_claims_function_names_in_prose_and_code_spans() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub fn fd_write() {}\npub fn path_open() {}\npub fn sock_accept() {}\npub fn fd_bogus() {}\n",
    );
    repo.commit("public names");
    repo.write(
        "src/lib.rs",
        "/// ```\n/// fd_write();\n/// ```\npub struct Kept;\n",
    );
    repo.write(
        "README.md",
        "Call fd_write and `path_open`.\n`sock_accept`\n`fixture::fd_write`\nfd_bogus\n",
    );
    repo.commit("remove names");
    let options = Options::new(repo.path().to_path_buf());
    let mentions = || {
        let mut out: Vec<(String, usize, String)> = refquiem::run(&options)
            .unwrap()
            .groups
            .iter()
            .flat_map(|group| group.mentions.iter())
            .map(|m| (m.file.clone(), m.line, m.text.clone()))
            .collect();
        out.sort();
        out
    };
    let row = |file: &str, line, text: &str| (file.to_owned(), line, text.to_owned());
    let selected = vec![
        row("README.md", 3, "fixture::fd_write"),
        row("README.md", 4, "fd_bogus"),
        row("src/lib.rs", 2, "fd_write"),
    ];
    let mut unselected = selected.clone();
    unselected.extend([
        row("README.md", 1, "fd_write"),
        row("README.md", 1, "path_open"),
        row("README.md", 2, "sock_accept"),
    ]);
    unselected.sort();
    for (config, expected) in [
        ("", &unselected),
        ("[vocabularies]\nbuiltins = [\"sql@2023\"]\n", &unselected),
        (
            "[vocabularies]\nbuiltins = [\"wasi@preview1\"]\n",
            &selected,
        ),
    ] {
        repo.write(".refquiem.toml", config);
        assert_eq!(&mentions(), expected, "{config}");
    }
}

#[test]
fn sql_2023_claims_exact_uppercase_key_words_in_prose_and_code_spans() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub const ARRAY_AGG: u8 = 0;\npub fn array_agg() {}\npub enum Keyword { LISTAGG, Other }\npub struct SQLUpdate;\n",
    );
    repo.commit("public names");
    repo.write(
        "src/lib.rs",
        "/// ```\n/// let _ = ARRAY_AGG;\n/// ```\npub enum Keyword { Other }\n",
    );
    repo.write(
        "README.md",
        "Aggregate with ARRAY_AGG or `LISTAGG`.\nLowercase array_agg.\n`Keyword::LISTAGG`\nSQLUpdate\n",
    );
    repo.commit("remove names");
    let options = Options::new(repo.path().to_path_buf());
    let mentions = || {
        let mut out: Vec<(String, usize, String)> = refquiem::run(&options)
            .unwrap()
            .groups
            .iter()
            .flat_map(|group| {
                group
                    .mentions
                    .iter()
                    .map(|m| (m.file.clone(), m.line, m.text.clone()))
            })
            .collect();
        out.sort();
        out
    };
    let row = |file: &str, line, text: &str| (file.to_owned(), line, text.to_owned());
    let unselected = vec![
        row("README.md", 1, "ARRAY_AGG"),
        row("README.md", 1, "LISTAGG"),
        row("README.md", 2, "array_agg"),
        row("README.md", 3, "Keyword::LISTAGG"),
        row("README.md", 4, "SQLUpdate"),
        row("src/lib.rs", 2, "ARRAY_AGG"),
    ];
    let selected = vec![
        row("README.md", 2, "array_agg"),
        row("README.md", 3, "Keyword::LISTAGG"),
        row("README.md", 4, "SQLUpdate"),
        row("src/lib.rs", 2, "ARRAY_AGG"),
    ];
    for (config, expected) in [
        ("", &unselected),
        ("[vocabularies]\nbuiltins = [\"english\"]\n", &unselected),
        ("[vocabularies]\nbuiltins = [\"sql@2023\"]\n", &selected),
        (
            "[vocabularies]\nbuiltins = [\"sql@2023\", \"english\", \"sql@2023\"]\n",
            &selected,
        ),
    ] {
        repo.write(".refquiem.toml", config);
        assert_eq!(&mentions(), expected, "{config}");
    }
}

/// `(file, line, text, (spelling, vocabulary))` of a mention.
type Hint = (String, usize, String, Option<(String, String)>);

#[test]
fn uppercase_hints_name_the_exact_case_entry_an_eligible_bare_mention_misses() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub fn array_agg() {}\npub fn my_flag() {}\npub fn gone_away() {}\n",
    );
    repo.commit("public names");
    repo.write(
        "src/lib.rs",
        "/// ```\n/// array_agg();\n/// ```\npub struct Kept;\n",
    );
    repo.write(
        "README.md",
        "Run array_agg and `array_agg`.\n`fixture::array_agg`\nSet my_flag and `my_flag`.\n`gone_away`\n",
    );
    repo.write(
        "domain.toml",
        "format-version = 1\nkind = \"natural-language\"\ncase = \"sensitive\"\nwords = [\"MY_FLAG\"]\n",
    );
    repo.commit("remove names");
    let domain = std::fs::canonicalize(repo.path().join("domain.toml"))
        .unwrap()
        .display()
        .to_string();
    let options = Options::new(repo.path().to_path_buf());
    let hints = || {
        let mut out: Vec<Hint> = refquiem::run(&options)
            .unwrap()
            .groups
            .iter()
            .flat_map(|group| group.mentions.iter())
            .map(|m| {
                (
                    m.file.clone(),
                    m.line,
                    m.text.clone(),
                    m.uppercase
                        .as_ref()
                        .map(|hint| (hint.spelling.clone(), hint.vocabulary.clone())),
                )
            })
            .collect();
        out.sort();
        out
    };
    let sql = || Some(("ARRAY_AGG".to_owned(), "sql@2023".to_owned()));
    let row = |file: &str, line, text: &str, hint| (file.to_owned(), line, text.to_owned(), hint);
    repo.write(
        ".refquiem.toml",
        "[vocabularies]\nbuiltins = [\"sql@2023\"]\nfiles = [\"domain.toml\"]\n",
    );
    assert_eq!(
        hints(),
        [
            row("README.md", 1, "array_agg", sql()),
            row("README.md", 1, "array_agg", sql()),
            row("README.md", 2, "fixture::array_agg", None),
            row("README.md", 3, "my_flag", None),
            row(
                "README.md",
                3,
                "my_flag",
                Some(("MY_FLAG".to_owned(), domain.clone()))
            ),
            row("README.md", 4, "gone_away", None),
            row("src/lib.rs", 2, "array_agg", None),
        ]
    );
    repo.write(".refquiem.toml", "[vocabularies]\n");
    assert!(hints().iter().all(|(.., hint)| hint.is_none()));
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
