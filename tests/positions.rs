//! Where the starting revision of a range declared each removed name.

mod common;

use std::collections::BTreeMap;
use std::process::Command;

use common::Repo;
use serde_json::{Value, json};

/// The `declared_at` of every reported group, by path, for `args` run in `repo`.
fn declared(repo: &Repo, args: &[&str]) -> BTreeMap<String, Value> {
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-refquiem"))
        .arg("refquiem")
        .arg("--json")
        .args(args)
        .current_dir(repo.path())
        .env_remove("CARGO_TARGET_DIR")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    report["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|group| {
            (
                group["path"].as_str().unwrap().to_owned(),
                group["declared_at"].clone(),
            )
        })
        .collect()
}

fn at(file: &str, line: u64) -> Value {
    json!({"file": file, "line": line})
}

#[test]
fn items_members_and_variants_point_at_their_lines_in_the_base() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "//! Docs.\n\npub fn gone() {}\n\npub struct Shape {\n    pub width: u32,\n}\n\nimpl Shape {\n    pub fn area(&self) -> u32 {\n        0\n    }\n}\n\npub enum Mode {\n    Fast,\n    Slow,\n}\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "//! Docs.\n\npub struct Shape {}\n\nimpl Shape {}\n\npub enum Mode {\n    Slow,\n}\n",
    );
    repo.write(
        "README.md",
        "`fixture::gone` `fixture::Shape::width` `fixture::Shape::area` `fixture::Mode::Fast`\n",
    );
    repo.commit("drop");

    assert_eq!(
        declared(&repo, &["--base", "HEAD^", "--head", "HEAD"]),
        BTreeMap::from([
            ("fixture::Mode::Fast".to_owned(), at("src/lib.rs", 16)),
            ("fixture::Shape::area".to_owned(), at("src/lib.rs", 10)),
            ("fixture::Shape::width".to_owned(), at("src/lib.rs", 6)),
            ("fixture::gone".to_owned(), at("src/lib.rs", 3)),
        ])
    );
}

#[test]
fn module_files_and_inline_modules_point_at_their_own_file() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub mod a;\npub mod b {\n    pub fn inner() {}\n}\n",
    );
    repo.write("src/a.rs", "\npub fn deep() {}\n");
    repo.commit("start");
    repo.write("src/lib.rs", "pub mod a;\npub mod b {}\n");
    repo.write("src/a.rs", "\n");
    repo.write("README.md", "`fixture::a::deep` `fixture::b::inner`\n");
    repo.commit("drop");

    assert_eq!(
        declared(&repo, &["--base", "HEAD^", "--head", "HEAD"]),
        BTreeMap::from([
            ("fixture::a::deep".to_owned(), at("src/a.rs", 2)),
            ("fixture::b::inner".to_owned(), at("src/lib.rs", 3)),
        ])
    );
}

#[test]
fn a_dropped_reexport_points_at_its_use_line() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub mod hidden {\n    pub fn thing() {}\n}\npub use hidden::thing as alias;\n",
    );
    repo.commit("start");
    repo.write("src/lib.rs", "pub mod hidden {\n    pub fn thing() {}\n}\n");
    repo.write("README.md", "`fixture::alias`\n");
    repo.commit("drop");

    assert_eq!(
        declared(&repo, &["--base", "HEAD^", "--head", "HEAD"]),
        BTreeMap::from([("fixture::alias".to_owned(), at("src/lib.rs", 4))])
    );
}

#[test]
fn a_reexported_name_whose_source_is_gone_points_at_the_source() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "mod hidden {\n    pub fn globbed() {}\n    pub fn named() {}\n}\npub use hidden::*;\npub use hidden::named as renamed;\n",
    );
    repo.commit("start");
    repo.write("src/lib.rs", "mod hidden {}\npub use hidden::*;\n");
    repo.write("README.md", "`fixture::globbed` `fixture::renamed`\n");
    repo.commit("drop");

    assert_eq!(
        declared(&repo, &["--base", "HEAD^", "--head", "HEAD"]),
        BTreeMap::from([
            ("fixture::globbed".to_owned(), at("src/lib.rs", 2)),
            ("fixture::renamed".to_owned(), at("src/lib.rs", 3)),
        ])
    );
}

#[test]
fn manifest_entries_point_at_their_key_in_the_member_manifest() {
    let repo = Repo::new();
    repo.write("Cargo.toml", "[workspace]\nmembers = [\"crates/alpha\"]\n");
    repo.write(
        "crates/alpha/Cargo.toml",
        "[package]\nname = \"alpha\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\nfast = []\n\n[[bin]]\nname = \"tool\"\npath = \"src/main.rs\"\n",
    );
    repo.write("crates/alpha/src/lib.rs", "");
    repo.write("crates/alpha/src/main.rs", "fn main() {}\n");
    repo.commit("start");
    repo.write(
        "crates/alpha/Cargo.toml",
        "[package]\nname = \"alpha\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("crates/alpha/src/main.rs", "");
    repo.write(
        "README.md",
        "`--features fast` and `cargo run --bin tool`\n",
    );
    repo.commit("drop");

    assert_eq!(
        declared(&repo, &["--base", "HEAD^", "--head", "HEAD"]),
        BTreeMap::from([
            ("fast".to_owned(), at("crates/alpha/Cargo.toml", 7)),
            ("tool".to_owned(), at("crates/alpha/Cargo.toml", 10)),
        ])
    );
}

#[test]
fn since_uses_its_revision_and_a_full_scan_has_no_position() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "\npub fn gone() {}\n");
    repo.commit("start");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "`fixture::gone`\n");
    repo.commit("drop");

    assert_eq!(
        declared(&repo, &["--since", "HEAD^"]),
        BTreeMap::from([("fixture::gone".to_owned(), at("src/lib.rs", 2))])
    );
    assert_eq!(
        declared(&repo, &[]),
        BTreeMap::from([("fixture::gone".to_owned(), Value::Null)])
    );
}

#[test]
fn a_name_the_base_never_had_has_no_position() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "");
    repo.commit("start");
    repo.write("src/lib.rs", "pub fn brief() {}\n");
    repo.commit("add");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "`fixture::brief`\n");
    repo.commit("drop");

    assert_eq!(
        declared(&repo, &["--base", "HEAD~2", "--head", "HEAD"]),
        BTreeMap::from([("fixture::brief".to_owned(), Value::Null)])
    );
}
