//! `--json`, `--github` annotations, the job summary and `--no-fail`.

mod common;
#[path = "common/vendor.rs"]
mod vendor;

use common::Repo;
use std::process::{Command, Output};

fn cargo_refquiem(
    dir: &std::path::Path,
    args: &[&str],
    summary: Option<&std::path::Path>,
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-refquiem"));
    command
        .arg("refquiem")
        .args(args)
        .current_dir(dir)
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("GITHUB_STEP_SUMMARY")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null");
    if let Some(summary) = summary {
        command.env("GITHUB_STEP_SUMMARY", summary);
    }
    command.output().unwrap()
}

/// A repository whose `fixture::gone` was removed, mentioned twice, with one unused marker.
/// Returns the repository and the short id of the deleting commit.
fn fixture() -> (Repo, String) {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "// refquiem: allow(Nothing)\npub fn kept() {}\n",
    );
    let deleting = repo.commit("drop gone");
    repo.write("README.md", "Call `fixture::gone`.\n");
    repo.write("a,b.md", "`fixture::gone`\n");
    (repo, deleting[..7].to_owned())
}

#[test]
fn json_prints_the_report_with_snake_case_keys() {
    let (repo, deleting) = fixture();
    let output = cargo_refquiem(repo.path(), &["--json"], None);
    assert_eq!(output.status.code(), Some(1));

    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let keys = |value: &serde_json::Value| -> Vec<String> {
        value.as_object().unwrap().keys().cloned().collect()
    };
    assert_eq!(keys(&report), ["groups", "skipped", "unused_markers"]);
    let group = &report["groups"][0];
    assert_eq!(
        keys(group),
        [
            "headline",
            "kind",
            "member",
            "mentions",
            "path",
            "removed_in"
        ]
    );
    assert_eq!(group["path"], "fixture::gone");
    assert_eq!(group["kind"], "function");
    assert_eq!(group["member"], serde_json::Value::Null);
    assert_eq!(
        group["headline"],
        format!("`fixture::gone` (function), deleted in {deleting}")
    );
    assert!(
        group["removed_in"]
            .as_str()
            .is_some_and(|id| id.starts_with(&deleting) && id.len() == 40),
        "{group}"
    );
    let mention = &group["mentions"][0];
    assert_eq!(keys(mention), ["column", "context", "file", "line", "text"]);
    assert_eq!(mention["file"], "README.md");
    assert_eq!(mention["line"], 1);
    assert_eq!(mention["column"], 7);
    assert_eq!(mention["text"], "fixture::gone");
    assert_eq!(
        mention["context"],
        serde_json::json!({"source": "markdown", "kind": "code_span"})
    );
    assert_eq!(
        report["unused_markers"],
        serde_json::json!([{"file": "src/lib.rs", "line": 1, "column": 20, "name": "Nothing"}])
    );
    assert_eq!(report["skipped"], serde_json::json!([]));
}

#[test]
fn a_change_in_the_working_tree_is_removed_in_the_working_tree() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.commit("start");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "`fixture::gone`\n");

    let output = cargo_refquiem(repo.path(), &["--json"], None);
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["groups"][0]["removed_in"], "working_tree");
}

#[test]
fn github_annotations_follow_the_report() {
    let (repo, deleting) = fixture();
    let output = cargo_refquiem(repo.path(), &["--github"], None);
    assert_eq!(output.status.code(), Some(1));

    let stdout = String::from_utf8(output.stdout).unwrap();
    let annotations: Vec<&str> = stdout.lines().filter(|l| l.starts_with("::")).collect();
    assert_eq!(
        annotations,
        [
            format!("::error file=README.md,line=1,col=7,title=refquiem::`fixture::gone` (function), deleted in {deleting}, is still mentioned here"),
            format!("::error file=a%2Cb.md,line=1,col=2,title=refquiem::`fixture::gone` (function), deleted in {deleting}, is still mentioned here"),
            "::error file=src/lib.rs,line=1,col=20,title=refquiem::refquiem: allow(Nothing) suppresses nothing".to_owned(),
        ]
    );
    assert!(
        stdout.starts_with("refquiem: 1 removed name still mentioned, 2 mentions\n"),
        "{stdout}"
    );
}

#[test]
fn no_fail_exits_0_and_turns_annotations_into_warnings() {
    let (repo, _) = fixture();
    let output = cargo_refquiem(repo.path(), &["--no-fail"], None);
    assert_eq!(output.status.code(), Some(0));

    let output = cargo_refquiem(repo.path(), &["--github", "--no-fail"], None);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let levels: Vec<&str> = stdout
        .lines()
        .filter_map(|l| l.strip_prefix("::"))
        .filter_map(|l| l.split(' ').next())
        .collect();
    assert_eq!(levels, ["warning", "warning", "warning"]);

    let failing = cargo_refquiem(
        repo.path(),
        &["--no-fail", "--config", "missing.toml"],
        None,
    );
    assert_eq!(failing.status.code(), Some(2));
}

#[test]
fn github_appends_the_job_summary_when_github_provides_the_file() {
    let (repo, deleting) = fixture();
    let summary = repo.path().join("summary.md");
    std::fs::write(&summary, "earlier step\n").unwrap();

    let output = cargo_refquiem(repo.path(), &["--github"], Some(&summary));
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        std::fs::read_to_string(&summary).unwrap(),
        format!(
            "earlier step\n## refquiem\n\n1 removed name still mentioned, 2 mentions\n\n| Removed name | Kind | Removed in | Mentions |\n|---|---|---|---|\n| `fixture::gone` | function | {deleting} | `README.md:1:7`<br>`a,b.md:1:2` |\n\n### Unused suppression markers\n\n- `src/lib.rs:1:20` `refquiem: allow(Nothing)`\n"
        )
    );

    std::fs::write(&summary, "").unwrap();
    cargo_refquiem(repo.path(), &[], Some(&summary));
    assert_eq!(std::fs::read_to_string(&summary).unwrap(), "");
}

#[test]
fn a_skipped_dependency_is_a_warning_annotation_and_a_summary_section() {
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
    repo.commit("drop broken");
    let summary = repo.path().join("summary.md");

    let output = cargo_refquiem(repo.path(), &["--github"], Some(&summary));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("::warning title=refquiem::removed dependency `broken` could not be resolved, so its items were not checked\n"),
        "{stdout}"
    );
    let summary = std::fs::read_to_string(&summary).unwrap();
    assert!(
        summary.ends_with("\n### Skipped dependencies\n\n- `broken` could not be resolved\n"),
        "{summary}"
    );
}

#[test]
fn a_clean_job_summary_says_nothing_is_stale() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn kept() {}\n");
    repo.commit("start");
    let summary = repo.path().join("summary.md");

    let output = cargo_refquiem(repo.path(), &["--github"], Some(&summary));
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        std::fs::read_to_string(&summary).unwrap(),
        "## refquiem\n\nNo stale mentions\n"
    );
}

#[test]
fn the_output_names_the_member_and_a_removal_in_the_working_tree() {
    let repo = Repo::new();
    repo.write("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n");
    for member in ["alpha", "beta"] {
        repo.write(
            &format!("crates/{member}/Cargo.toml"),
            &format!("[package]\nname = \"{member}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\nx = []\n"),
        );
        repo.write(&format!("crates/{member}/src/lib.rs"), "");
    }
    repo.commit("two members");
    repo.write(
        "crates/alpha/Cargo.toml",
        "[package]\nname = \"alpha\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("crates/alpha/README.md", "`--features x`\n");
    let summary = repo.path().join("summary.md");

    let output = cargo_refquiem(repo.path(), &["--github"], Some(&summary));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("::error file=crates/alpha/README.md,line=1,col=13,title=refquiem::`x` (feature of `alpha`), removed in the working tree, is still mentioned here\n"),
        "{stdout}"
    );
    let summary = std::fs::read_to_string(&summary).unwrap();
    assert!(
        summary.contains(
            "| `x` of `alpha` | feature | working tree | `crates/alpha/README.md:1:13` |\n"
        ),
        "{summary}"
    );
}

#[test]
fn a_job_summary_that_cannot_be_written_fails_the_run() {
    let (repo, _) = fixture();
    let summary = repo.path().join("summary-dir");
    std::fs::create_dir(&summary).unwrap();

    let output = cargo_refquiem(repo.path(), &["--github", "--no-fail"], Some(&summary));
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("summary-dir"), "{stderr}");
}
