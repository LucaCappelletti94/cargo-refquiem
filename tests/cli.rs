//! The `cargo refquiem` command line: report text and exit codes.

mod common;

use common::Repo;
use std::process::{Command, Output};

fn cargo_refquiem(dir: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cargo-refquiem"))
        .arg("refquiem")
        .args(args)
        .current_dir(dir)
        .env_remove("CARGO_TARGET_DIR")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap()
}

#[test]
fn vocabulary_catalog_ignores_project_state_outside_a_repository() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".refquiem.toml"), "invalid configuration").unwrap();
    let output = cargo_refquiem(dir.path(), &["vocabularies", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let catalog: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let identities: Vec<_> = catalog
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["id"].as_str().unwrap())
        .collect();
    assert_eq!(identities, ["english", "sql@2023", "wasi@preview1"]);
    assert_eq!(
        catalog[1],
        serde_json::json!({
            "id": "sql@2023",
            "kind": "technical",
            "case": "sensitive",
            "source": "https://github.com/postgres/postgres",
            "source_version": "REL_17_11",
        })
    );
    assert_eq!(
        catalog[2],
        serde_json::json!({
            "id": "wasi@preview1",
            "kind": "technical",
            "case": "sensitive",
            "source": "https://github.com/WebAssembly/WASI",
            "source_version": "v0.2.9",
        })
    );
    let output = cargo_refquiem(dir.path(), &["vocabularies"]);
    assert_eq!(output.status.code(), Some(0));
    let catalog = String::from_utf8(output.stdout).unwrap();
    assert!(catalog.contains("english") && catalog.contains("sql@2023 (technical, sensitive)"));
    std::fs::write(
        dir.path().join("domain.toml"),
        "format-version = 1\nkind = \"technical\"\ncase = \"sensitive\"\nwords = [\"PrivateSDKThing\"]\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join(".refquiem.toml"),
        "[vocabularies]\nfiles = [\"domain.toml\"]\n",
    )
    .unwrap();
    let custom = cargo_refquiem(dir.path(), &["vocabularies", "--json"]);
    assert_eq!(custom.status.code(), Some(0));
    let custom: serde_json::Value = serde_json::from_slice(&custom.stdout).unwrap();
    assert!(!custom.to_string().contains("domain.toml"));
    assert!(
        custom
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry.get("files").is_none() && entry.get("path").is_none())
    );
    let effective = cargo_refquiem(dir.path(), &["vocabularies", "--effective", "--json"]);
    assert_eq!(effective.status.code(), Some(2));
    assert_eq!(effective.stdout, b"");
}

#[test]
fn vocabulary_inspection_uses_workspace_config_without_history_or_rust_parsing() {
    let repo = Repo::new();
    repo.write("Cargo.toml", "[workspace]\nmembers = [\"member\"]\n");
    repo.write(
        "member/Cargo.toml",
        "[package]\nname = \"member\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("member/src/lib.rs", "invalid Rust source {");
    repo.write(
        "member/.refquiem.toml",
        "[vocabularies]\nbuiltins = [\"english\"]\n",
    );
    repo.write(
        ".refquiem.toml",
        "since = \"nonexistent-history-reference\"\n[vocabularies]\n",
    );
    let member = repo.path().join("member");
    let output = cargo_refquiem(&member, &["vocabularies", "--effective", "--json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let selection: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(selection["builtins"], serde_json::json!([]));
    assert_eq!(selection["rust_stdlib"], true);
    let root_config = std::fs::canonicalize(repo.path())
        .unwrap()
        .join(".refquiem.toml");
    assert_eq!(selection["config"], root_config.to_str().unwrap());
    let output = cargo_refquiem(&member, &["vocabularies", "--effective"]);
    assert_eq!(output.status.code(), Some(0));
    let selection = String::from_utf8(output.stdout).unwrap();
    assert!(selection.contains(root_config.to_str().unwrap()));
    let external = tempfile::tempdir().unwrap();
    let config = external.path().join("selection.toml");
    std::fs::write(
        &config,
        "[vocabularies]\nbuiltins = [\"sql@2023\", \"english\", \"english\", \"sql@2023\"]\n",
    )
    .unwrap();
    let output = cargo_refquiem(
        &member,
        &[
            "vocabularies",
            "--effective",
            "--json",
            "--config",
            config.to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(0));
    let selection: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let identities: Vec<_> = selection["builtins"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["id"].as_str().unwrap())
        .collect();
    assert_eq!(identities, ["english", "sql@2023"]);
    assert_eq!(selection["config"], config.to_str().unwrap());
    let output = cargo_refquiem(
        &member,
        &[
            "vocabularies",
            "--effective",
            "--config",
            config.to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(0));
    let selection = String::from_utf8(output.stdout).unwrap();
    assert!(selection.contains(config.to_str().unwrap()));
}

#[test]
fn vocabulary_inspection_reports_the_same_selection_that_scanning_uses() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub struct PhD;\n");
    repo.commit("public name");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "A PhD student.\n");
    repo.commit("remove name");
    for (config, enabled) in [
        ("", true),
        ("[vocabularies]\n", false),
        ("[vocabularies]\nbuiltins = [\"english\"]\n", true),
    ] {
        repo.write(".refquiem.toml", config);
        let inspected = cargo_refquiem(repo.path(), &["vocabularies", "--effective", "--json"]);
        assert_eq!(inspected.status.code(), Some(0), "{config}");
        let selection: serde_json::Value = serde_json::from_slice(&inspected.stdout).unwrap();
        let identities: Vec<_> = selection["builtins"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            identities,
            if enabled { vec!["english"] } else { vec![] },
            "{config}"
        );
        let scanned = cargo_refquiem(repo.path(), &["--json"]);
        assert_eq!(scanned.status.code(), Some(i32::from(!enabled)), "{config}");
        let report: serde_json::Value = serde_json::from_slice(&scanned.stdout).unwrap();
        let findings: Vec<_> = report["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|group| group["path"].as_str().unwrap())
            .collect();
        assert_eq!(
            findings,
            if enabled {
                vec![]
            } else {
                vec!["fixture::PhD"]
            },
            "{config}"
        );
    }
}

#[test]
fn vocabulary_inspection_rejects_scan_arguments_and_catalog_configuration() {
    let dir = tempfile::tempdir().unwrap();
    for flag in ["--markdown", "--github", "--no-fail"] {
        for args in [[flag, "vocabularies"], ["vocabularies", flag]] {
            let output = cargo_refquiem(dir.path(), &args);
            assert_eq!(output.status.code(), Some(2), "{args:?}");
            assert!(String::from_utf8_lossy(&output.stderr).contains(flag));
        }
    }
    for flag in ["--base", "--since", "--package"] {
        for args in [
            [flag, "HEAD", "vocabularies"],
            ["vocabularies", flag, "HEAD"],
        ] {
            let output = cargo_refquiem(dir.path(), &args);
            assert_eq!(output.status.code(), Some(2), "{args:?}");
            assert!(String::from_utf8_lossy(&output.stderr).contains(flag));
        }
    }
    let output = cargo_refquiem(dir.path(), &["vocabularies", "--head", "HEAD"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--head"));
    let output = cargo_refquiem(dir.path(), &["vocabularies", "--config", "selection.toml"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--effective"));
}

#[test]
fn vocabulary_inspection_rejects_invalid_selected_configuration_without_partial_output() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "");
    for config in [
        "[vocabularies]\nbuiltins = [\"engilsh\"]\n",
        "[vocabularies\n",
    ] {
        repo.write(".refquiem.toml", config);
        let output = cargo_refquiem(repo.path(), &["vocabularies", "--effective", "--json"]);
        assert_eq!(output.status.code(), Some(2), "{config}");
        assert_eq!(output.stdout, b"");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(".refquiem.toml"),
            "{config}"
        );
    }
    let output = cargo_refquiem(
        repo.path(),
        &["vocabularies", "--effective", "--config", "missing.toml"],
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("missing.toml"));
}

#[test]
fn custom_vocabulary_inspection_resolves_config_relative_files_without_history() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "invalid Rust {");
    let external = tempfile::tempdir().unwrap();
    let config = external.path().join("selection.toml");
    let file = external.path().join("domain.toml");
    std::fs::write(
        &config,
        "[vocabularies]\nfiles = [\"domain.toml\", \"./domain.toml\"]\n",
    )
    .unwrap();
    std::fs::write(
        &file,
        "format-version = 1\nkind = \"technical\"\ncase = \"sensitive\"\nwords = [\"PrivateSDKThing\"]\n",
    )
    .unwrap();
    let output = cargo_refquiem(
        repo.path(),
        &[
            "vocabularies",
            "--effective",
            "--config",
            config.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let selection: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(selection["builtins"], serde_json::json!([]));
    assert_eq!(
        selection["files"],
        serde_json::json!([{
            "path": std::fs::canonicalize(file).unwrap(),
            "kind": "technical",
            "case": "sensitive",
            "format_version": 1,
        }])
    );
}

#[test]
fn effective_output_lists_custom_files_with_their_category_case_and_format_version() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "invalid Rust {");
    let external = tempfile::tempdir().unwrap();
    let config = external.path().join("selection.toml");
    let file = external.path().join("domain.toml");
    std::fs::write(&config, "[vocabularies]\nfiles = [\"domain.toml\"]\n").unwrap();
    std::fs::write(
        &file,
        "format-version = 1\nkind = \"technical\"\ncase = \"sensitive\"\nwords = [\"PrivateSDKThing\"]\n",
    )
    .unwrap();
    let output = cargo_refquiem(
        repo.path(),
        &[
            "vocabularies",
            "--effective",
            "--config",
            config.to_str().unwrap(),
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let selection = String::from_utf8(output.stdout).unwrap();
    let canonical = std::fs::canonicalize(file).unwrap();
    assert!(
        selection.contains(canonical.to_str().unwrap()),
        "{selection}"
    );
    assert!(selection.contains("technical"), "{selection}");
    assert!(selection.contains("sensitive"), "{selection}");
    assert!(selection.contains("format version 1"), "{selection}");
    assert!(
        !selection.contains("No optional vocabularies"),
        "{selection}"
    );
    assert!(!selection.contains("english"), "{selection}");
    repo.write(".refquiem.toml", "[vocabularies]\n");
    let output = cargo_refquiem(repo.path(), &["vocabularies", "--effective"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let selection = String::from_utf8(output.stdout).unwrap();
    assert!(
        selection.contains("No optional vocabularies"),
        "{selection}"
    );
    assert!(!selection.contains("english"), "{selection}");
}

#[test]
fn custom_file_metadata_is_deterministic_and_sorted_by_canonical_path() {
    let repo = Repo::new();
    let external = tempfile::tempdir().unwrap();
    std::fs::write(
        external.path().join("a.toml"),
        "format-version = 1\nkind = \"technical\"\ncase = \"sensitive\"\nwords = [\"Word\"]\n",
    )
    .unwrap();
    std::fs::write(
        external.path().join("m.toml"),
        "format-version = 1\nkind = \"natural-language\"\ncase = \"unicode-lowercase\"\nwords = [\"ÄTHER\"]\n",
    )
    .unwrap();
    std::fs::write(
        external.path().join("z.toml"),
        "format-version = 1\nkind = \"natural-language\"\ncase = \"ascii-insensitive\"\nwords = [\"Word\"]\n",
    )
    .unwrap();
    std::fs::write(
        external.path().join("first.toml"),
        "[vocabularies]\nfiles = [\"z.toml\", \"m.toml\", \"a.toml\"]\n",
    )
    .unwrap();
    std::fs::write(
        external.path().join("second.toml"),
        format!(
            "[vocabularies]\nfiles = {}\n",
            serde_json::to_string(&[
                external.path().join("a.toml"),
                external.path().join("m.toml"),
                external.path().join("z.toml"),
            ])
            .unwrap()
        ),
    )
    .unwrap();
    let selection_of = |config: &std::path::Path| -> serde_json::Value {
        let output = cargo_refquiem(
            repo.path(),
            &[
                "vocabularies",
                "--effective",
                "--config",
                config.to_str().unwrap(),
                "--json",
            ],
        );
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    };
    let first = selection_of(&external.path().join("first.toml"));
    let second = selection_of(&external.path().join("second.toml"));
    assert_eq!(first["files"], second["files"]);
    let a = std::fs::canonicalize(external.path().join("a.toml")).unwrap();
    let m = std::fs::canonicalize(external.path().join("m.toml")).unwrap();
    let z = std::fs::canonicalize(external.path().join("z.toml")).unwrap();
    assert_eq!(
        first["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["path"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            a.to_str().unwrap(),
            m.to_str().unwrap(),
            z.to_str().unwrap()
        ]
    );
    assert_eq!(first["files"][0]["kind"], "technical");
    assert_eq!(first["files"][1]["kind"], "natural-language");
    assert_eq!(first["files"][2]["kind"], "natural-language");
    assert_eq!(first["files"][0]["case"], "sensitive");
    assert_eq!(first["files"][1]["case"], "unicode-lowercase");
    assert_eq!(first["files"][2]["case"], "ascii-insensitive");
    assert_eq!(first["files"][0]["format_version"], serde_json::json!(1));
    assert_eq!(first["files"][1]["format_version"], serde_json::json!(1));
    assert_eq!(first["files"][2]["format_version"], serde_json::json!(1));
}

#[test]
fn invalid_custom_files_exit_2_without_partial_output_in_either_output_mode() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "invalid Rust {");
    let external = tempfile::tempdir().unwrap();
    let config = external.path().join("selection.toml");
    let file = external.path().join("domain.toml");
    std::fs::write(&config, "[vocabularies]\nfiles = [\"domain.toml\"]\n").unwrap();
    for (name, contents) in [
        ("missing file", None::<Vec<u8>>),
        (
            "unsupported format version",
            Some(
                b"format-version = 2\nkind = \"technical\"\ncase = \"sensitive\"\nwords = [\"Word\"]\n"
                    .to_vec(),
            ),
        ),
        (
            "unsupported kind",
            Some(
                b"format-version = 1\nkind = \"sql\"\ncase = \"sensitive\"\nwords = [\"Word\"]\n"
                    .to_vec(),
            ),
        ),
        (
            "blank word",
            Some(
                b"format-version = 1\nkind = \"technical\"\ncase = \"sensitive\"\nwords = [\"bad word\"]\n"
                    .to_vec(),
            ),
        ),
        (
            "non UTF-8 bytes",
            Some(
                b"format-version = 1\nkind = \"technical\"\ncase = \"sensitive\"\nwords = [\xff\xfe]\n"
                    .to_vec(),
            ),
        ),
    ] {
        match contents {
            Some(contents) => std::fs::write(&file, contents).unwrap(),
            None => std::fs::remove_file(&file)
                .unwrap_or_else(|error| assert_eq!(error.kind(), std::io::ErrorKind::NotFound)),
        }
        for json in [false, true] {
            let mut args = vec!["vocabularies", "--effective", "--config", config.to_str().unwrap()];
            if json {
                args.push("--json");
            }
            let output = cargo_refquiem(repo.path(), &args);
            assert_eq!(output.status.code(), Some(2), "{name}");
            assert_eq!(output.stdout, b"", "{name}");
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("domain.toml"),
                "{name}"
            );
        }
    }
}

#[test]
fn custom_files_mask_eligible_mentions_while_explicit_rust_references_stay_reported() {
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
    repo.write(
        "domain.toml",
        "format-version = 1\nkind = \"technical\"\ncase = \"sensitive\"\nwords = [\"PrivateSDKThing\"]\n",
    );
    let mentions = |report: &serde_json::Value| -> Vec<(String, u64)> {
        let mut rows: Vec<_> = report["groups"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|group| {
                group["mentions"].as_array().unwrap().iter().map(|mention| {
                    (
                        mention["file"].as_str().unwrap().to_owned(),
                        mention["line"].as_u64().unwrap(),
                    )
                })
            })
            .collect();
        rows.sort_unstable();
        rows
    };
    repo.write(".refquiem.toml", "[vocabularies]\n");
    let bare = cargo_refquiem(repo.path(), &["--json"]);
    assert_eq!(
        bare.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&bare.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&bare.stdout).unwrap();
    assert_eq!(report["groups"].as_array().unwrap().len(), 1);
    assert_eq!(report["groups"][0]["path"], "fixture::PrivateSDKThing");
    assert_eq!(
        mentions(&report),
        [
            ("README.md".to_owned(), 1),
            ("README.md".to_owned(), 2),
            ("README.md".to_owned(), 3),
            ("src/lib.rs".to_owned(), 1),
            ("src/lib.rs".to_owned(), 2),
            ("src/lib.rs".to_owned(), 4),
            ("src/lib.rs".to_owned(), 7),
        ]
    );
    repo.write(
        ".refquiem.toml",
        "[vocabularies]\nfiles = [\"domain.toml\"]\n",
    );
    let inspected = cargo_refquiem(repo.path(), &["vocabularies", "--effective", "--json"]);
    assert_eq!(
        inspected.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&inspected.stderr)
    );
    let selection: serde_json::Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(selection["builtins"], serde_json::json!([]));
    assert_eq!(selection["files"].as_array().unwrap().len(), 1);
    assert_eq!(selection["files"][0]["kind"], "technical");
    assert_eq!(selection["files"][0]["case"], "sensitive");
    let domain = std::fs::canonicalize(repo.path().join("domain.toml")).unwrap();
    assert_eq!(selection["files"][0]["path"], domain.to_str().unwrap());
    let scanned = cargo_refquiem(repo.path(), &["--json"]);
    assert_eq!(
        scanned.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&scanned.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&scanned.stdout).unwrap();
    assert_eq!(report["groups"].as_array().unwrap().len(), 1);
    assert_eq!(report["groups"][0]["path"], "fixture::PrivateSDKThing");
    assert_eq!(
        mentions(&report),
        [
            ("README.md".to_owned(), 3),
            ("src/lib.rs".to_owned(), 2),
            ("src/lib.rs".to_owned(), 4),
        ]
    );
}

#[test]
fn findings_print_one_group_per_removed_name_and_exit_1() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\npub fn kept() {}\n");
    repo.commit("start");
    repo.write("src/lib.rs", "// Was `crate::gone`.\npub fn kept() {}\n");
    let deleting = repo.commit("drop gone");
    repo.write("README.md", "Call `fixture::gone`.\n");

    let output = cargo_refquiem(repo.path(), &[]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!(
            "refquiem: 1 removed name still mentioned, 2 mentions\n\n`fixture::gone` (function), deleted in {}\n  README.md:1:7  Markdown code span\n  src/lib.rs:1:9  comment\n",
            &deleting[..7]
        )
    );
}

#[test]
fn a_clean_scan_exits_0() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn kept() {}\n");
    repo.commit("start");

    let output = cargo_refquiem(repo.path(), &[]);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "refquiem: no stale mentions\n"
    );
}

#[test]
fn a_base_and_head_range_is_accepted_from_the_command_line() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn early() {}\npub fn late() {}\n");
    repo.commit("both");
    repo.write("src/lib.rs", "pub fn late() {}\n");
    let base = repo.commit("drop early");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "`fixture::early` and `fixture::late`.\n");
    let head = repo.commit("drop late");

    let output = cargo_refquiem(repo.path(), &["--base", &base, "--head", &head]);

    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("`fixture::late`"), "{stdout}");
    assert!(!stdout.contains("`fixture::early`"), "{stdout}");
}

#[test]
fn outside_a_git_repository_the_scan_fails_with_exit_2() {
    let dir = tempfile::tempdir().unwrap();

    let output = cargo_refquiem(dir.path(), &[]);

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .starts_with("refquiem: ")
    );
}

#[test]
fn a_scan_failure_prints_the_full_error_chain() {
    let dir = tempfile::tempdir().unwrap();
    let status = Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(dir.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .status()
        .unwrap();
    assert!(status.success());

    let output = cargo_refquiem(dir.path(), &[]);

    let path = std::fs::canonicalize(dir.path()).unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        format!("refquiem: no Cargo.toml at or above {}\n", path.display())
    );
}

#[test]
fn the_config_and_package_flags_reach_the_scan() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.commit("start");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "`fixture::gone`\n");
    repo.write("quiet.toml", "allow = [\"gone\"]\n");

    let output = cargo_refquiem(repo.path(), &["--config", "quiet.toml"]);
    assert_eq!(output.status.code(), Some(0));

    let output = cargo_refquiem(repo.path(), &["--package", "fixture"]);
    assert_eq!(output.status.code(), Some(1));

    let output = cargo_refquiem(repo.path(), &["--package", "missing"]);
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn since_scans_only_the_commits_after_a_revision() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.commit("start");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "`fixture::gone`\n");
    repo.commit("drop gone");

    let output = cargo_refquiem(repo.path(), &["--since", "HEAD~1"]);
    assert_eq!(output.status.code(), Some(1));
    let output = cargo_refquiem(repo.path(), &["--since", "HEAD"]);
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn the_cache_lives_under_cargo_target_dir_when_it_is_set() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn kept() {}\n");
    repo.commit("start");
    let target = tempfile::tempdir().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_cargo-refquiem"))
        .arg("refquiem")
        .current_dir(repo.path())
        .env("CARGO_TARGET_DIR", target.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let cached = std::fs::read_dir(target.path().join("refquiem"))
        .unwrap()
        .count();
    assert!(cached > 0);
    assert!(!repo.path().join("target/refquiem").exists());
}
