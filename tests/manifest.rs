//! Removed dependencies, features, binaries, examples and packages, and where they are still named.

mod common;

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
fn a_removed_dependency_is_found_in_paths_links_badges_and_toml_keys() {
    let repo = Repo::new();
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
                1,
                12,
                "https://img.shields.io/crates/v/structopt.svg"
            ),
            mention(
                "structopt",
                dependency,
                "README.md",
                1,
                60,
                "https://crates.io/crates/structopt"
            ),
            mention(
                "structopt",
                dependency,
                "README.md",
                3,
                6,
                "structopt::StructOpt"
            ),
            mention(
                "structopt",
                dependency,
                "README.md",
                4,
                6,
                "https://docs.rs/structopt"
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
fn a_renamed_dependency_is_stale_under_its_key_and_its_package_name() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "");
    repo.write(
        "Cargo.toml",
        &format!("{PACKAGE}\n[dependencies]\ncli = {{ package = \"clap\", version = \"4\" }}\n"),
    );
    repo.commit("renamed clap");
    repo.write("Cargo.toml", PACKAGE);
    repo.write(
        "README.md",
        "`cli::Parser` from https://crates.io/crates/clap\n",
    );
    repo.commit("drop clap");

    let names: Vec<(String, String)> = found(&repo).into_iter().map(|m| (m.0, m.5)).collect();
    assert_eq!(
        names,
        [
            ("cli".to_owned(), "cli::Parser".to_owned()),
            (
                "clap".to_owned(),
                "https://crates.io/crates/clap".to_owned()
            ),
        ]
    );
}

#[test]
fn a_dependency_moved_to_another_kind_is_not_removed() {
    let repo = Repo::new();
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
fn a_renamed_package_leaves_its_old_crate_links_stale() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "");
    repo.commit("fixture");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture-next\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write(
        "README.md",
        "https://crates.io/crates/fixture and https://crates.io/crates/fixture-next\n",
    );
    repo.commit("rename package");

    let names: Vec<(String, ItemKind, usize)> =
        found(&repo).into_iter().map(|m| (m.0, m.1, m.4)).collect();
    assert_eq!(names, [("fixture".to_owned(), ItemKind::Package, 1)]);
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
