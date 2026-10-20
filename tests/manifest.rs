//! Removed dependencies, features, binaries, examples and packages, and where they are still named.

mod common;
#[path = "common/vendor.rs"]
mod vendor;

use common::Repo;
use refquiem::{ItemKind, RemovedIn};

const PACKAGE: &str = "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

/// `(removed name, kind, file, line, column, text)` for every mention, sorted.
fn found(repo: &Repo) -> Vec<(String, ItemKind, String, usize, usize, String)> {
    let report = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap();
    let mut out: Vec<_> = report
        .groups
        .iter()
        .flat_map(|group| {
            group.mentions.iter().map(|m| {
                (
                    group.path.clone(),
                    group.kind,
                    m.file.clone(),
                    m.line,
                    m.column,
                    m.text.clone(),
                )
            })
        })
        .collect();
    out.sort_by(|a, b| (&a.2, a.3, a.4).cmp(&(&b.2, b.3, b.4)));
    out
}

fn mention(
    name: &str,
    kind: ItemKind,
    file: &str,
    line: usize,
    column: usize,
    text: &str,
) -> (String, ItemKind, String, usize, usize, String) {
    (
        name.to_owned(),
        kind,
        file.to_owned(),
        line,
        column,
        text.to_owned(),
    )
}

#[test]
fn historical_dependency_names_cannot_overwrite_files_outside_the_cache() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "");
    repo.write("victim-requirement/Cargo.toml", "protected manifest");
    repo.write("victim-requirement/lib.rs", "protected source");
    let package = repo.path().join("victim").display().to_string();
    let package = toml::Value::String(package);
    repo.write(
        "Cargo.toml",
        &format!("{PACKAGE}\n[dependencies]\nevil = {{ package = {package}, version = \"1\" }}\n"),
    );
    repo.commit("historical dependency");
    repo.write("Cargo.toml", PACKAGE);
    repo.commit("remove dependency");
    refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap();
    assert_eq!(
        std::fs::read_to_string(repo.path().join("victim-requirement/Cargo.toml")).unwrap(),
        "protected manifest"
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("victim-requirement/lib.rs")).unwrap(),
        "protected source"
    );
}

#[test]
fn historical_locked_versions_cannot_overwrite_files_outside_the_cache() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "");
    repo.write("victim-locked/Cargo.toml", "protected manifest");
    repo.write("victim-locked/lib.rs", "protected source");
    let lock = "version = 3\n\n[[package]]\nname = \"fixture\"\nversion = \"0.1.0\"\ndependencies = [\"evil ../../../../../victim-locked\"]\n\n[[package]]\nname = \"evil\"\nversion = \"../../../../../victim-locked\"\n";
    repo.write(
        "Cargo.toml",
        &format!("{PACKAGE}\n[dependencies]\nevil = \"1\"\n"),
    );
    repo.write("Cargo.lock", lock);
    repo.commit("historical dependency");
    repo.write("Cargo.toml", PACKAGE);
    std::fs::remove_file(repo.path().join("Cargo.lock")).unwrap();
    repo.commit("remove dependency");
    refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap();
    assert_eq!(
        std::fs::read_to_string(repo.path().join("victim-locked/Cargo.toml")).unwrap(),
        "protected manifest"
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("victim-locked/lib.rs")).unwrap(),
        "protected source"
    );
}

#[test]
fn published_crate_links_survive_dependency_removal() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "");
    repo.write(
        "Cargo.toml",
        &format!("{PACKAGE}\n[dependencies]\nserde = \"1\"\n"),
    );
    repo.commit("dependency");
    repo.write("Cargo.toml", PACKAGE);
    let links = "[Serde](https://docs.rs/serde/latest/serde/)\nhttps://crates.io/crates/serde\n[![serde](https://img.shields.io/crates/v/serde.svg)](https://lib.rs/crates/serde)\n";
    repo.write("README.md", links);
    repo.commit("recommend published crate without depending on it");
    assert_eq!(found(&repo), Vec::new());

    repo.write(
        "README.md",
        &format!("{links}\n`serde::Serialize`\n\n```toml\n[dependencies]\nserde = \"1\"\n```\n"),
    );
    let texts: Vec<_> = found(&repo).into_iter().map(|mention| mention.5).collect();
    assert_eq!(texts, ["serde::Serialize", "serde"]);
}

#[test]
fn a_removed_dependency_is_found_in_paths_and_toml_keys() {
    let repo = Repo::new();
    repo.vendor("structopt", "0.3.0", "");
    repo.vendor("clap", "4.0.0", "");
    repo.write("src/lib.rs", "");
    repo.write(
        "Cargo.toml",
        &format!("{PACKAGE}\n[dependencies]\nstructopt = \"0.3\"\nclap = \"4\"\n"),
    );
    repo.commit("both");
    repo.write(
        "Cargo.toml",
        &format!("{PACKAGE}\n[dependencies]\nclap = \"4\"\n"),
    );
    repo.write(
        "README.md",
        "[![crates](https://img.shields.io/crates/v/structopt.svg)](https://crates.io/crates/structopt)

Use `structopt::StructOpt` or `clap::Parser`.
See <https://docs.rs/structopt> and [docs](https://docs.rs/clap).

```toml
[dependencies]
structopt = \"0.3\"
clap = \"4\"
```
",
    );
    let removing = repo.commit("drop structopt");

    let dependency = ItemKind::Dependency;
    assert_eq!(
        found(&repo),
        [
            mention(
                "structopt",
                dependency,
                "README.md",
                3,
                6,
                "structopt::StructOpt"
            ),
            mention("structopt", dependency, "README.md", 8, 1, "structopt"),
        ]
    );
    let report = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap();
    assert_eq!(
        report.groups[0].removed_in,
        Some(RemovedIn::Commit(removing))
    );
    assert!(
        report
            .human()
            .contains("`structopt` (dependency), removed in "),
        "{}",
        report.human()
    );
}

#[test]
fn a_renamed_dependency_is_stale_under_its_key() {
    let repo = Repo::new();
    repo.vendor("clap", "4.0.0", "");
    repo.write("src/lib.rs", "");
    repo.write(
        "Cargo.toml",
        &format!("{PACKAGE}\n[dependencies]\ncli = {{ package = \"clap\", version = \"4\" }}\n"),
    );
    repo.commit("renamed clap");
    repo.write("Cargo.toml", PACKAGE);
    repo.write("README.md", "`cli::Parser`, added with `cargo add clap`.\n");
    repo.commit("drop clap");

    let names: Vec<(String, String)> = found(&repo).into_iter().map(|m| (m.0, m.5)).collect();
    assert_eq!(
        names,
        [
            ("cli".to_owned(), "cli::Parser".to_owned()),
            ("clap".to_owned(), "clap".to_owned()),
        ]
    );
}

#[test]
fn a_dependency_moved_to_another_kind_is_not_removed() {
    let repo = Repo::new();
    repo.vendor("serde", "1.0.0", "");
    repo.write("src/lib.rs", "");
    repo.write(
        "Cargo.toml",
        &format!("{PACKAGE}\n[dependencies]\nserde = \"1\"\n"),
    );
    repo.commit("normal");
    repo.write(
        "Cargo.toml",
        &format!("{PACKAGE}\n[dev-dependencies]\nserde = \"1\"\n"),
    );
    repo.write(
        "README.md",
        "https://crates.io/crates/serde and `serde::Serialize`\n",
    );
    repo.commit("dev only");

    assert_eq!(found(&repo), []);
}

#[test]
fn removed_and_hidden_features_are_found_in_flags_and_cfg() {
    let repo = Repo::new();
    repo.vendor("rayon", "1.0.0", "");
    repo.write("src/lib.rs", "");
    repo.write(
        "Cargo.toml",
        &format!("{PACKAGE}\n[features]\nfast = []\n\n[dependencies]\nrayon = {{ version = \"1\", optional = true }}\n"),
    );
    repo.commit("fast and implicit rayon");
    repo.write(
        "Cargo.toml",
        &format!("{PACKAGE}\n[features]\npar = [\"dep:rayon\"]\n\n[dependencies]\nrayon = {{ version = \"1\", optional = true }}\n"),
    );
    repo.write(
        "README.md",
        "```sh
cargo build --features fast,par
cargo run -F \"rayon fast\"
```

```rust
#[cfg(feature = \"fast\")]
fn quick() {}
```
",
    );
    repo.commit("drop fast, hide rayon");

    let feature = ItemKind::Feature;
    assert_eq!(
        found(&repo),
        [
            mention("fast", feature, "README.md", 2, 24, "fast"),
            mention("rayon", feature, "README.md", 3, 15, "rayon"),
            mention("fast", feature, "README.md", 3, 21, "fast"),
            mention("fast", feature, "README.md", 7, 18, "fast"),
        ]
    );
}

#[test]
fn features_are_scoped_to_the_member_whose_files_mention_them() {
    let repo = Repo::new();
    repo.write("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n");
    for (member, features) in [("alpha", "x = []\n"), ("beta", "x = []\ny = []\n")] {
        repo.write(
            &format!("crates/{member}/Cargo.toml"),
            &format!("[package]\nname = \"{member}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\n{features}"),
        );
        repo.write(&format!("crates/{member}/src/lib.rs"), "");
    }
    repo.commit("two members");
    repo.write(
        "crates/alpha/Cargo.toml",
        "[package]\nname = \"alpha\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("crates/alpha/README.md", "`--features x`\n");
    repo.write("crates/beta/README.md", "`--features x`\n");
    repo.write(
        "README.md",
        "`--features x`, `--features y`, `--features alpha/x`\n",
    );
    repo.commit("alpha drops x");

    let files: Vec<(String, String, usize)> =
        found(&repo).into_iter().map(|m| (m.0, m.2, m.4)).collect();
    assert_eq!(
        files,
        [
            ("x".to_owned(), "README.md".to_owned(), 51),
            ("x".to_owned(), "crates/alpha/README.md".to_owned(), 13),
        ]
    );
}

#[test]
fn a_package_moved_from_the_root_into_a_member_keeps_its_entries() {
    let repo = Repo::new();
    repo.vendor("serde", "1.0.0", "");
    repo.write(
        "Cargo.toml",
        &format!("{PACKAGE}\n[features]\nfast = []\n\n[dependencies]\nserde = \"1\"\n"),
    );
    repo.write("src/lib.rs", "");
    repo.commit("root package");
    std::fs::remove_file(repo.path().join("src/lib.rs")).unwrap();
    repo.write("Cargo.toml", "[workspace]\nmembers = [\"fixture\"]\n");
    repo.write(
        "fixture/Cargo.toml",
        &format!("{PACKAGE}\n[features]\nfast = []\n\n[dependencies]\nserde = \"1\"\n"),
    );
    repo.write("fixture/src/lib.rs", "");
    repo.write(
        "README.md",
        "https://crates.io/crates/fixture, `--features fast` and https://crates.io/crates/serde\n",
    );
    repo.commit("move into a member");

    assert_eq!(found(&repo), []);
}

#[test]
fn removed_binaries_and_examples_are_found_in_cargo_commands() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "");
    repo.write("src/bin/old-tool.rs", "fn main() {}\n");
    repo.write("examples/demo.rs", "fn main() {}\n");
    repo.commit("targets");
    std::fs::remove_file(repo.path().join("src/bin/old-tool.rs")).unwrap();
    std::fs::remove_file(repo.path().join("examples/demo.rs")).unwrap();
    repo.write(
        "README.md",
        "Run `cargo run --bin old-tool` or `cargo run --example demo`.\n",
    );
    repo.commit("drop targets");

    assert_eq!(
        found(&repo),
        [
            mention("old-tool", ItemKind::Binary, "README.md", 1, 22, "old-tool"),
            mention("demo", ItemKind::Example, "README.md", 1, 56, "demo"),
        ]
    );
}

#[test]
fn a_renamed_package_leaves_its_old_cargo_flags_stale() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "");
    repo.commit("fixture");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture-next\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write(
        "README.md",
        "`cargo test -p fixture` and `cargo test -p fixture-next`\n",
    );
    repo.commit("rename package");

    let names: Vec<(String, ItemKind, usize)> =
        found(&repo).into_iter().map(|m| (m.0, m.1, m.4)).collect();
    assert_eq!(names, [("fixture".to_owned(), ItemKind::Package, 16)]);
}

#[test]
fn features_requested_for_the_package_in_a_toml_block_are_checked() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "");
    repo.write("Cargo.toml", &format!("{PACKAGE}\n[features]\nfast = []\n"));
    repo.commit("fast");
    repo.write("Cargo.toml", PACKAGE);
    repo.write(
        "README.md",
        "```toml
[dependencies]
fixture = { version = \"0.1\", features = [\"fast\"] }
```
",
    );
    repo.commit("drop fast");

    assert_eq!(
        found(&repo),
        [mention(
            "fast",
            ItemKind::Feature,
            "README.md",
            3,
            43,
            "fast"
        )]
    );
}

#[test]
fn an_excluded_workspace_member_is_read_as_a_package_outside_the_workspace() {
    let repo = Repo::new();
    repo.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\nexclude = [\"crates/skipped\"]\n",
    );
    for name in ["kept", "skipped"] {
        repo.write(
            &format!("crates/{name}/Cargo.toml"),
            &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        );
        repo.write(&format!("crates/{name}/src/lib.rs"), "");
    }
    repo.commit("kept and excluded");
    let options = refquiem::Options::new(repo.path().to_path_buf());

    repo.write("crates/skipped/src/lib.rs", "pub fn (\n");
    assert!(refquiem::run(&options).is_ok());
    repo.write("crates/kept/src/lib.rs", "pub fn (\n");
    let error = refquiem::run(&options).unwrap_err();
    assert!(
        matches!(&error, refquiem::Error::Parse { path, .. } if path == "crates/kept/src/lib.rs"),
        "{error}"
    );
}

#[test]
fn workspace_inheritance_resolves_a_member_dependency() {
    let repo = Repo::new();
    repo.vendor("tool", "1.0.0", "");
    repo.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"m\"]\n\n[workspace.dependencies]\ntool = { version = \"1\" }\n",
    );
    repo.write(
        "m/Cargo.toml",
        "[package]\nname = \"m\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ntool = { workspace = true }\n",
    );
    repo.write("m/src/lib.rs", "");
    repo.commit("inherited tool");
    repo.write(
        "m/Cargo.toml",
        "[package]\nname = \"m\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("README.md", "Uses `tool::gone`.\n");
    repo.commit("drop tool");

    let report = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap();
    assert_eq!(
        found(&repo),
        [mention(
            "tool",
            ItemKind::Dependency,
            "README.md",
            1,
            7,
            "tool::gone"
        )]
    );
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
}

#[test]
fn an_inherited_dependency_keeps_its_member_optional_feature() {
    let repo = Repo::new();
    repo.vendor("tool", "1.0.0", "pub fn live() {}\n");
    repo.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"m\"]\n\n[workspace.dependencies]\ntool = \"1\"\n",
    );
    repo.write(
        "m/Cargo.toml",
        "[package]\nname = \"m\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\nsteady = []\n\n[dependencies]\ntool = { workspace = true, optional = true }\n",
    );
    repo.write("m/src/lib.rs", "");
    repo.commit("optional inherited tool");
    repo.write(
        "m/Cargo.toml",
        "[package]\nname = \"m\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\nsteady = []\n\n[dependencies]\ntool = { workspace = true }\n",
    );
    repo.write(
        "m/README.md",
        "`--features tool`, `--features steady` and `tool::live`.\n",
    );
    repo.commit("require inherited tool");

    assert_eq!(
        found(&repo),
        [mention(
            "tool",
            ItemKind::Feature,
            "m/README.md",
            1,
            13,
            "tool"
        )]
    );
}

#[test]
fn a_member_spec_ignores_the_workspace_entry_for_the_same_key() {
    let repo = Repo::new();
    repo.vendor("tool", "1.0.0", "");
    repo.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"m\"]\n\n[workspace.dependencies]\ntool = { version = \"1\", optional = true }\n",
    );
    repo.write(
        "m/Cargo.toml",
        "[package]\nname = \"m\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ntool = \"1\"\n",
    );
    repo.write("m/src/lib.rs", "");
    repo.commit("own tool spec");
    repo.write(
        "m/Cargo.toml",
        "[package]\nname = \"m\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("README.md", "```sh\ncargo build --features tool\n```\n");
    repo.commit("drop tool");

    assert_eq!(found(&repo), []);
}

#[test]
fn a_member_path_override_fills_the_inherited_spec() {
    let repo = Repo::new();
    repo.vendor("local", "1.0.0", "");
    repo.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"m\"]\n\n[workspace.dependencies]\nlocal = { version = \"1\" }\n",
    );
    repo.write(
        "m/Cargo.toml",
        "[package]\nname = \"m\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nlocal = { workspace = true, path = \"src-local\" }\n",
    );
    repo.write("m/src/lib.rs", "");
    repo.write(
        "src-local/Cargo.toml",
        "[package]\nname = \"local\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("src-local/src/lib.rs", "pub fn here() {}\n");
    repo.commit("path override");
    repo.write(
        "m/Cargo.toml",
        "[package]\nname = \"m\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    std::fs::remove_dir_all(repo.path().join("src-local")).unwrap();
    repo.write("README.md", "The docs name `local::here`.\n");
    repo.commit("drop local");

    assert_eq!(
        found(&repo),
        [mention(
            "local::here",
            ItemKind::Function,
            "README.md",
            1,
            16,
            "local::here"
        )]
    );
}

#[test]
fn autolib_false_keeps_the_lib_file_unindexed() {
    let repo = Repo::new();
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\nautolib = false\n",
    );
    repo.write("src/lib.rs", "pub fn hidden() {}\n");
    repo.commit("autolib off");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "The docs name `fixture::hidden`.\n");
    repo.commit("drop hidden");

    assert_eq!(found(&repo), []);
}

#[test]
fn autobins_discovers_src_main_as_the_package_bin() {
    let repo = Repo::new();
    repo.write("src/main.rs", "fn main() {}\n");
    repo.commit("main");
    std::fs::remove_file(repo.path().join("src/main.rs")).unwrap();
    repo.write("README.md", "Run `cargo run --bin fixture`.\n");
    repo.commit("drop main");

    assert_eq!(
        found(&repo),
        [mention(
            "fixture",
            ItemKind::Binary,
            "README.md",
            1,
            22,
            "fixture"
        )]
    );
}

#[test]
fn a_declared_build_path_is_indexed_as_the_build_script() {
    let repo = Repo::new();
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\nbuild = \"custom.rs\"\n",
    );
    repo.write("src/lib.rs", "");
    repo.write("custom.rs", "pub fn configure() {}\n");
    repo.commit("custom build");
    repo.write("custom.rs", "");
    repo.write(
        "README.md",
        "The build script defines `build_script_build::configure`.\n",
    );
    repo.commit("drop configure");

    assert_eq!(
        found(&repo),
        [mention(
            "build_script_build::configure",
            ItemKind::Function,
            "README.md",
            1,
            27,
            "build_script_build::configure"
        )]
    );
}

#[test]
fn build_false_keeps_build_rs_unindexed() {
    let repo = Repo::new();
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\nbuild = false\n",
    );
    repo.write("src/lib.rs", "");
    repo.write("build.rs", "pub fn hidden() {}\n");
    repo.commit("build off");
    repo.write("build.rs", "");
    repo.write("README.md", "The docs name `build_script_build::hidden`.\n");
    repo.commit("drop hidden");

    assert_eq!(found(&repo), []);
}

#[test]
fn a_declared_example_falls_back_to_its_directory_root() {
    let repo = Repo::new();
    repo.write(
        "Cargo.toml",
        &format!("{PACKAGE}\n[[example]]\nname = \"tool\"\n"),
    );
    repo.write("src/lib.rs", "");
    repo.write("examples/tool/main.rs", "pub fn gone() {}\n");
    repo.commit("tool example");
    std::fs::remove_file(repo.path().join("examples/tool/main.rs")).unwrap();
    repo.write("README.md", "The docs name `tool::gone`.\n");
    repo.commit("drop the example file");

    assert_eq!(
        found(&repo),
        [mention(
            "tool::gone",
            ItemKind::Function,
            "README.md",
            1,
            16,
            "tool::gone"
        )]
    );
}

#[test]
fn a_declared_bin_named_by_the_package_falls_back_to_src_main() {
    let repo = Repo::new();
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\nautobins = false\n\n[[bin]]\nname = \"fixture\"\n\n[[bin]]\nname = \"other\"\n\n[[example]]\nname = \"fixture\"\n",
    );
    repo.write("src/main.rs", "fn main() {}\n");
    repo.commit("declared targets");
    std::fs::remove_file(repo.path().join("src/main.rs")).unwrap();
    repo.write(
        "README.md",
        "Run `cargo run --bin fixture`, `cargo run --bin other` or `cargo run --example fixture`.\n",
    );
    repo.commit("drop main");

    assert_eq!(
        found(&repo),
        [mention(
            "fixture",
            ItemKind::Binary,
            "README.md",
            1,
            22,
            "fixture"
        )]
    );
}

#[test]
fn a_recursive_member_glob_reaches_nested_members_and_skips_target() {
    let repo = Repo::new();
    repo.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates\", \"nested/**\"]\n",
    );
    for (dir, name) in [
        ("crates", "flat"),
        ("nested/a/b", "deep"),
        ("nested/zzz", "shallow"),
        ("nested/target/x", "pruned"),
    ] {
        repo.write(
            &format!("{dir}/Cargo.toml"),
            &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        );
        repo.write(&format!("{dir}/src/lib.rs"), "");
    }
    repo.commit("nested members");
    let options = refquiem::Options::new(repo.path().to_path_buf());

    // An unreadable file fails the scan at head only inside a workspace member.
    repo.write("nested/target/x/src/lib.rs", "pub fn (\n");
    assert!(refquiem::run(&options).is_ok());
    for dir in ["nested/a/b", "nested/zzz"] {
        let file = format!("{dir}/src/lib.rs");
        repo.write(&file, "pub fn (\n");
        let error = refquiem::run(&options).unwrap_err();
        assert!(
            matches!(&error, refquiem::Error::Parse { path, .. } if *path == file),
            "{dir}: {error}"
        );
        repo.write(&file, "");
    }
}

#[test]
fn member_globs_match_shell_style_wildcards_classes_and_ranges() {
    let repo = Repo::new();
    repo.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"a?\", \"b?x\", \"a?x\", \"[cd]e\", \"[!e]f\", \"[g-i]j\", \"[!k-m]n\", \"[a-b]y\", \"[ab-]w\", \"o[\"]\n",
    );
    let matched = [
        ("ax", "ax"),
        ("box", "box"),
        ("ce", "ce"),
        ("de", "de"),
        ("bf", "bf"),
        ("gf", "gf"),
        ("hj", "hj"),
        ("bn", "bn"),
        ("by", "by"),
        ("bw", "bw"),
        ("o[", "bracket"),
        ("ae", "ae"),
    ];
    let unmatched = [
        ("a", "a"),
        ("aby", "aby"),
        ("cbx", "cbx"),
        ("be", "be"),
        ("ef", "ef"),
        ("bj", "bj"),
        ("jj", "jj"),
        ("ox", "ox"),
        ("-y", "dashy"),
    ];
    for (dir, name) in matched.into_iter().chain(unmatched) {
        repo.write(
            &format!("{dir}/Cargo.toml"),
            &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        );
        repo.write(&format!("{dir}/src/lib.rs"), "");
    }
    repo.commit("glob members");
    let options = refquiem::Options::new(repo.path().to_path_buf());

    // An unreadable file fails the scan at head only inside a workspace member.
    for (dir, _) in unmatched {
        repo.write(&format!("{dir}/src/lib.rs"), "pub fn (\n");
    }
    assert!(refquiem::run(&options).is_ok());
    for (dir, _) in matched {
        let file = format!("{dir}/src/lib.rs");
        repo.write(&file, "pub fn (\n");
        let error = refquiem::run(&options).unwrap_err();
        assert!(
            matches!(&error, refquiem::Error::Parse { path, .. } if *path == file),
            "{dir}: {error}"
        );
        repo.write(&file, "");
    }
}

#[test]
fn a_declared_bin_without_a_file_does_not_claim_the_package_main() {
    let repo = Repo::new();
    repo.write(
        "Cargo.toml",
        &format!("{PACKAGE}\n[[bin]]\nname = \"tool\"\n"),
    );
    repo.write("src/main.rs", "pub fn helper() {}\nfn main() {}\n");
    repo.commit("start");
    repo.write("src/main.rs", "fn main() {}\n");
    repo.write("README.md", "`tool::helper` and `fixture::helper`\n");
    repo.commit("drop helper");

    let names: Vec<(String, String)> = found(&repo).into_iter().map(|m| (m.0, m.5)).collect();
    assert_eq!(
        names,
        [("fixture::helper".to_owned(), "fixture::helper".to_owned())]
    );
}

#[test]
fn a_feature_of_a_moved_member_is_looked_up_where_the_member_is_now() {
    let repo = Repo::new();
    repo.write("Cargo.toml", "[workspace]\nmembers = [\"z\"]\n");
    repo.write(
        "z/Cargo.toml",
        &format!("{PACKAGE}\n[features]\nfast = []\n"),
    );
    repo.write("z/src/lib.rs", "");
    repo.commit("member in z");
    std::fs::remove_dir_all(repo.path().join("z")).unwrap();
    repo.write("Cargo.toml", "[workspace]\nmembers = [\"a\"]\n");
    repo.write(
        "a/Cargo.toml",
        &format!("{PACKAGE}\n[features]\nfast = []\n"),
    );
    repo.write("a/src/lib.rs", "");
    repo.write("README.md", "`--features fixture/fast`\n");
    repo.commit("move the member to a");

    assert_eq!(found(&repo), []);
}

#[test]
fn a_feature_in_a_cfg_attribute_spread_over_lines_is_found() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "");
    repo.write("Cargo.toml", &format!("{PACKAGE}\n[features]\nfast = []\n"));
    repo.commit("fast");
    repo.write("Cargo.toml", PACKAGE);
    repo.write(
        "README.md",
        "```rust\n#[cfg(\n    feature = \"fast\"\n)]\nfn quick() {}\n// Here feature = \"fast\" is no predicate.\n```\n\nA prose feature = \"fast\" is no predicate.\n",
    );
    repo.commit("drop fast");

    assert_eq!(
        found(&repo),
        [mention(
            "fast",
            ItemKind::Feature,
            "README.md",
            3,
            16,
            "fast"
        )]
    );
}

#[test]
fn only_the_closed_strings_of_a_features_array_are_features() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "");
    repo.write(
        "Cargo.toml",
        &format!("{PACKAGE}\n[features]\nfast = []\npar = []\n"),
    );
    repo.commit("fast and par");
    repo.write("Cargo.toml", PACKAGE);
    repo.write(
        "README.md",
        "```toml\n[dependencies]\nfixture = { branch = \"fast\", features = [\"par\"] }\nfixture = { features = [\"par\", \"fast] }\n```\n",
    );
    repo.commit("drop both");

    let feature = ItemKind::Feature;
    assert_eq!(
        found(&repo),
        [
            mention("par", feature, "README.md", 3, 43, "par"),
            mention("par", feature, "README.md", 4, 26, "par"),
        ]
    );
}

#[test]
fn a_package_that_leaves_the_workspace_but_stays_in_the_tree_keeps_its_items_and_entry() {
    let repo = Repo::new();
    repo.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"base\", \"examples/demo\"]\n",
    );
    repo.write(
        "base/Cargo.toml",
        "[package]\nname = \"base\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("base/src/lib.rs", "");
    repo.write(
        "examples/demo/Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write(
        "examples/demo/src/lib.rs",
        "pub fn helper() {}\npub fn gone() {}\n",
    );
    repo.commit("demo is a member");
    repo.write("Cargo.toml", "[workspace]\nmembers = [\"base\"]\n");
    repo.write("examples/Cargo.toml", "[workspace]\nmembers = [\"*\"]\n");
    repo.write("examples/demo/src/lib.rs", "pub fn helper() {}\n");
    repo.write(
        "README.md",
        "`demo::helper`, `demo::gone` and the `demo` package.\n",
    );
    repo.commit("demo moves to the examples workspace");

    let rows: Vec<(String, String)> = found(&repo).into_iter().map(|m| (m.0, m.5)).collect();
    assert_eq!(rows, [("demo::gone".to_owned(), "demo::gone".to_owned())]);
}

#[test]
fn broken_packages_outside_the_workspace_neither_fail_the_scan_nor_hide_blame() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.write("tests/fixtures/bad_toml/Cargo.toml", "[package\n");
    repo.write(
        "tests/fixtures/bad_rust/Cargo.toml",
        "[package]\nname = \"bad_rust\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("tests/fixtures/bad_rust/src/lib.rs", "pub fn (\n");
    repo.commit("start");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "`fixture::gone`\n");
    let removing = repo.commit("drop gone");

    let report = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap();
    let removed_in: Vec<(&str, Option<RemovedIn>)> = report
        .groups
        .iter()
        .map(|g| (g.path.as_str(), g.removed_in.clone()))
        .collect();
    assert_eq!(
        removed_in,
        [("fixture::gone", Some(RemovedIn::Commit(removing)))]
    );
}

#[test]
fn an_item_a_package_outside_the_workspace_lost_is_blamed_on_its_removal() {
    let repo = Repo::new();
    repo.write("Cargo.toml", "[workspace]\nmembers = [\"base\"]\n");
    repo.write(
        "base/Cargo.toml",
        "[package]\nname = \"base\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("base/src/lib.rs", "");
    repo.write(
        "tools/gen/Cargo.toml",
        "[package]\nname = \"gen\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("tools/gen/src/lib.rs", "pub fn old_gen() {}\n");
    repo.commit("a stray generator");
    repo.write("tools/gen/src/lib.rs", "");
    repo.write("README.md", "`gen::old_gen`\n");
    let removing = repo.commit("drop old_gen");

    let report = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap();
    let removed_in: Vec<(&str, Option<RemovedIn>)> = report
        .groups
        .iter()
        .map(|g| (g.path.as_str(), g.removed_in.clone()))
        .collect();
    assert_eq!(
        removed_in,
        [("gen::old_gen", Some(RemovedIn::Commit(removing)))]
    );
}

#[test]
fn an_excluded_broken_package_cannot_break_a_workspace_glob() {
    let repo = Repo::new();
    repo.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\nexclude = [\"crates/ignored\"]\n",
    );
    repo.write("crates/good/Cargo.toml", PACKAGE);
    repo.write("crates/good/src/lib.rs", "pub fn retired_probe_842() {}\n");
    repo.write("crates/ignored/Cargo.toml", "not a manifest");
    repo.commit("an excluded package beside a workspace member");
    repo.write("crates/good/src/lib.rs", "");
    repo.write("crates/good/README.md", "`fixture::retired_probe_842`\n");
    repo.commit("drop the function");

    let paths: Vec<_> = found(&repo).into_iter().map(|row| row.0).collect();
    assert_eq!(paths, ["fixture::retired_probe_842"]);
}
