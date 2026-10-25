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
    assert!(
        catalog
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["id"] == "english")
    );
    let output = cargo_refquiem(dir.path(), &["vocabularies"]);
    assert_eq!(output.status.code(), Some(0));
    let catalog = String::from_utf8(output.stdout).unwrap();
    assert!(catalog.contains("english"));
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
        "[vocabularies]\nbuiltins = [\"english\", \"english\"]\n",
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
    assert_eq!(identities, ["english"]);
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
