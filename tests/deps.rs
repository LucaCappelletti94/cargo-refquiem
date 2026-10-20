//! The dependency layer, removed dependencies at their locked version, and what a mention names.

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
    out.sort_by(|a, b| (&a.0, &a.1, a.2, a.3).cmp(&(&b.0, &b.1, b.2, b.3)));
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
fn a_removed_dependency_is_read_from_the_registry_its_manifest_named() {
    let repo = Repo::new();
    let dir = "vendor/locked-hdep-0.2.0";
    repo.write(
        &format!("{dir}/Cargo.toml"),
        "[package]\nname = \"locked-hdep\"\nversion = \"0.2.0\"\nedition = \"2021\"\n",
    );
    repo.write(&format!("{dir}/src/lib.rs"), "pub struct LockedItem;\n");
    repo.write(&format!("{dir}/.cargo-checksum.json"), "{\"files\":{}}");
    repo.write(
        ".cargo/config.toml",
        "[registries.myreg]\nindex = \"sparse+https://index.example/\"\n\n[source.myreg]\nregistry = \"sparse+https://index.example/\"\nreplace-with = \"vendored\"\n\n[source.vendored]\ndirectory = \"vendor\"\n",
    );
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nlocked-hdep = { version = \"0.2\", registry = \"myreg\" }\n",
    );
    repo.write(
        "Cargo.lock",
        "version = 3\n\n[[package]]\nname = \"fixture\"\nversion = \"0.1.0\"\ndependencies = [\n \"locked-hdep 0.2.0 (registry+https://index.example/)\",\n]\n\n[[package]]\nname = \"locked-hdep\"\nversion = \"0.2.0\"\nsource = \"registry+https://index.example/\"\n",
    );
    repo.write("src/lib.rs", "");
    repo.commit("locked on myreg");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    lock(&repo);
    repo.write("README.md", "The `locked_hdep::LockedItem` item.\n");
    repo.commit("drop locked-hdep");

    let report = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap();
    let rows: Vec<(String, String)> = report
        .groups
        .iter()
        .flat_map(|g| g.mentions.iter().map(|m| (g.path.clone(), m.text.clone())))
        .collect();
    assert_eq!(
        rows,
        [(
            "locked_hdep::LockedItem".to_owned(),
            "locked_hdep::LockedItem".to_owned(),
        )]
    );
    assert_eq!(report.skipped, []);
}

#[test]
fn a_removed_dependency_is_pinned_to_the_version_its_member_locked() {
    let repo = Repo::new();
    repo.vendor("helper", "0.1.0", "pub struct OldThing;\n");
    repo.vendor("helper", "0.2.0", "pub struct NewThing;\n");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = \"0\"\n",
    );
    repo.write(
        "Cargo.lock",
        "version = 3\n\n[[package]]\nname = \"fixture\"\nversion = \"0.1.0\"\ndependencies = [\n \"helper 0.1.0 (registry+https://github.com/rust-lang/crates.io-index)\",\n]\n\n[[package]]\nname = \"helper\"\nversion = \"0.1.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n\n[[package]]\nname = \"helper\"\nversion = \"0.2.0\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n",
    );
    repo.write("src/lib.rs", "");
    repo.commit("helper locked at 0.1.0");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    lock(&repo);
    repo.write("README.md", "`helper::OldThing` and `helper::NewThing`.\n");
    repo.commit("drop helper");

    let rows: Vec<(String, String)> = found(&repo).into_iter().map(|m| (m.0, m.5)).collect();
    assert_eq!(
        rows,
        [
            ("helper".to_owned(), "helper::NewThing".to_owned()),
            ("helper::OldThing".to_owned(), "helper::OldThing".to_owned()),
        ]
    );
}

#[test]
fn a_removed_dependency_is_read_at_the_version_locked_by_its_member_not_siblings() {
    let repo = Repo::new();
    repo.vendor("helper", "0.1.0", "pub struct OldThing;\n");
    repo.vendor("helper", "0.2.0", "pub struct NewThing;\n");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\nmembers = [\"widget\"]\n\n[dependencies]\nhelper = \"0.1\"\n",
    );
    repo.write(
        "widget/Cargo.toml",
        "[package]\nname = \"widget\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = \"0.2\"\n",
    );
    repo.write("src/lib.rs", "");
    repo.write("widget/src/lib.rs", "");
    lock(&repo);
    repo.commit("two helpers");
    repo.write(
        "widget/Cargo.toml",
        "[package]\nname = \"widget\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    lock(&repo);
    repo.write("README.md", "The `NewThing` and `OldThing` items.\n");
    repo.commit("drop the widget helper");

    let rows: Vec<(String, String)> = found(&repo).into_iter().map(|m| (m.0, m.5)).collect();
    assert_eq!(
        rows,
        [("helper::NewThing".to_owned(), "NewThing".to_owned())]
    );
}

#[test]
fn items_of_a_dependency_head_no_longer_has_match_only_from_its_crate_root() {
    let repo = Repo::new();
    repo.vendor(
        "gone_dep",
        "1.0.0",
        "pub struct Widget;\nimpl Widget {\n    pub fn build() {}\n}\n",
    );
    repo.vendor("helper", "0.1.0", "pub struct OldThing;\n");
    repo.vendor("helper", "0.2.0", "pub struct NewThing;\n");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\nmembers = [\"app\"]\n\n[dependencies]\nhelper = \"0.2\"\n",
    );
    repo.write(
        "app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ngone_dep = \"1\"\nhelper = \"0.1\"\n",
    );
    repo.write("src/lib.rs", "");
    repo.write("app/src/lib.rs", "");
    lock(&repo);
    repo.commit("app with gone_dep and the old helper");
    repo.write(
        "app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    lock(&repo);
    repo.write(
        "README.md",
        "`Widget`, `Widget::build`, `gone_dep::Widget::build`, `OldThing` and `helper::OldThing`.\n",
    );
    repo.commit("app drops both");

    let rows: Vec<(String, String)> = found(&repo).into_iter().map(|m| (m.0, m.5)).collect();
    assert_eq!(
        rows,
        [
            (
                "gone_dep::Widget::build".to_owned(),
                "gone_dep::Widget::build".to_owned()
            ),
            ("helper::OldThing".to_owned(), "OldThing".to_owned()),
            ("helper::OldThing".to_owned(), "helper::OldThing".to_owned())
        ]
    );
}

#[test]
fn a_live_dependency_built_as_an_rlib_or_dylib_keeps_its_names_alive() {
    for crate_type in ["rlib", "dylib"] {
        let repo = Repo::new();
        repo.write(
            "vendor/helper/Cargo.toml",
            &format!("[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\nname = \"helper\"\npath = \"src/lib.rs\"\ncrate-type = [\"{crate_type}\"]\n"),
        );
        repo.write("vendor/helper/src/lib.rs", "pub struct frame_tag;\n");
        // Marked as vendored, so only the dependency layer, not the tree's own index, reads it.
        repo.write("vendor/helper/.cargo-checksum.json", "{\"files\":{}}");
        repo.write(
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = { path = \"vendor/helper\" }\n",
        );
        repo.write(
            "src/lib.rs",
            "pub struct Frame {\n    pub frame_tag: u8,\n}\n",
        );
        lock(&repo);
        repo.commit("own frame_tag");
        repo.write("src/lib.rs", "");
        repo.write("README.md", "`frame_tag`\n");
        repo.commit("drop Frame");

        assert_eq!(found(&repo), [], "{crate_type}");
    }
}

#[test]
fn a_bare_name_in_a_data_example_names_only_serialized_spellings() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.commit("start");
    repo.write("src/lib.rs", "pub fn kept() {}\n");
    repo.write("README.md", "```json\n{\"gone\": 1}\n```\n");
    repo.commit("drop gone");

    assert_eq!(found(&repo), []);
}

#[test]
fn a_removed_entry_is_named_by_its_toml_key_not_its_bare_name() {
    let repo = Repo::new();
    repo.write(
        "vendor/helper/Cargo.toml",
        "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("vendor/helper/src/lib.rs", "pub struct Thing;\n");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = { path = \"vendor/helper\" }\n",
    );
    repo.write("src/lib.rs", "");
    lock(&repo);
    repo.commit("with helper");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    lock(&repo);
    std::fs::remove_dir_all(repo.path().join("vendor/helper")).unwrap();
    repo.write(
        "README.md",
        "The `helper` crate.\n\n```json\n{\"helper\": 1}\n```\n\n```toml\n[dependencies]\nhelper = \"1\"\n```\n",
    );
    repo.commit("drop helper");

    let rows: Vec<(String, String, usize)> =
        found(&repo).into_iter().map(|m| (m.0, m.1, m.2)).collect();
    assert_eq!(rows, [("helper".to_owned(), "README.md".to_owned(), 9)]);
}

#[test]
fn a_rustdoc_page_link_to_a_single_item_names_nothing() {
    let repo = Repo::new();
    repo.vendor("Tool", "1.0.0", "pub struct Tool;\n");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nTool = \"1\"\n",
    );
    repo.write("src/lib.rs", "");
    lock(&repo);
    repo.commit("with Tool");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    lock(&repo);
    repo.write("README.md", "[Tool](struct.Tool.html)\n");
    repo.commit("drop Tool");

    assert_eq!(found(&repo), []);
}

#[test]
fn a_path_spelled_from_a_live_crate_name_names_nothing_removed() {
    let repo = Repo::new();
    repo.vendor("widget", "1.0.0", "pub struct DepThing;\n");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\nmembers = [\"widget\"]\n\n[dependencies]\nwidget = \"1\"\n",
    );
    repo.write(
        "widget/Cargo.toml",
        "[package]\nname = \"widget\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("src/lib.rs", "");
    repo.write("widget/src/lib.rs", "");
    lock(&repo);
    repo.commit("with the widget dep");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\nmembers = [\"widget\"]\n",
    );
    repo.write(
        "src/lib.rs",
        "/// Was `widget::Brandnew`.\npub fn kept() {}\n",
    );
    lock(&repo);
    repo.commit("drop the widget dep");

    assert_eq!(found(&repo), []);
}

#[test]
fn a_toml_dependency_key_naming_a_head_package_is_alive() {
    let repo = Repo::new();
    repo.vendor("widget", "0.1.0", "");
    let app = |dependency: &str, features: &str| {
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\n{features}\n\n[dev-dependencies]\n{dependency}\n"
        )
    };
    repo.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"widget\", \"app\"]\n",
    );
    repo.write(
        "widget/Cargo.toml",
        "[package]\nname = \"widget\"\nversion = \"0.2.0\"\nedition = \"2021\"\n",
    );
    repo.write("widget/src/lib.rs", "");
    repo.write("app/Cargo.toml", &app("widget = \"0.1\"", "widget = []"));
    repo.write("app/src/lib.rs", "");
    lock(&repo);
    repo.commit("app depends on a published widget");
    repo.write("app/Cargo.toml", &app("", ""));
    lock(&repo);
    repo.write(
        "app/README.md",
        "```toml\n[dependencies]\nwidget = \"0.1\"\n```\n\nRun `cargo build --features widget`.\n",
    );
    repo.commit("app drops widget");

    let rows: Vec<(String, usize)> = found(&repo).into_iter().map(|m| (m.0, m.2)).collect();
    assert_eq!(rows, [("widget".to_owned(), 6)]);
}

#[test]
fn a_rustdoc_url_is_read_from_its_item_not_its_directory() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct Thing;\nimpl Thing {\n    pub fn old(&self) {}\n}\n",
    );
    repo.commit("start");
    repo.write("src/lib.rs", "pub fn kept() {}\n");
    repo.write("README.md", "[m](something/struct.Thing.html#method.old)\n");
    repo.commit("drop Thing");

    let rows: Vec<(String, String, usize, usize, Context)> = found(&repo)
        .into_iter()
        .map(|m| (m.0, m.1, m.2, m.3, m.4))
        .collect();
    assert_eq!(
        rows,
        [(
            "fixture::Thing::old".to_owned(),
            "README.md".to_owned(),
            1,
            5,
            context(Source::Markdown, Kind::Link)
        )]
    );
}

#[test]
fn a_crate_relative_path_reads_only_from_its_own_crate() {
    let repo = Repo::new();
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\nmembers = [\"widget\"]\n",
    );
    repo.write(
        "widget/Cargo.toml",
        "[package]\nname = \"widget\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.write("widget/src/lib.rs", "pub fn gone() {}\n");
    repo.commit("two gones");
    repo.write("src/lib.rs", "/// Was `crate::gone`.\npub fn kept() {}\n");
    repo.commit("drop the fixture gone");

    let rows: Vec<(String, String, usize, usize)> = found(&repo)
        .into_iter()
        .map(|m| (m.0, m.1, m.2, m.3))
        .collect();
    assert_eq!(
        rows,
        [("fixture::gone".to_owned(), "src/lib.rs".to_owned(), 1, 10)]
    );
}

#[test]
fn an_item_a_head_macro_still_generates_is_not_reported() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub struct Bar;\n");
    repo.commit("start");
    let lib =
        "macro_rules! gen {\n    ($n:ident) => {\n        pub struct $n;\n    };\n}\ngen!(Bar);\n";
    repo.write(
        "src/lib.rs",
        &format!("{lib}/// Was `crate::Bar`.\npub fn kept() {{}}\n"),
    );
    repo.commit("drop Bar");

    assert_eq!(found(&repo), []);
}

#[test]
fn a_field_of_a_struct_an_unknown_head_macro_may_generate_is_not_reported() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub struct A {\n    pub B: u8,\n}\n");
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "gen!(A);\npub struct A {\n}\n/// Was `crate::A::B`.\npub fn kept() {}\n",
    );
    repo.commit("drop B");

    assert_eq!(found(&repo), []);
}

#[test]
fn a_module_a_dependency_macro_declares_is_indexed() {
    let repo = Repo::new();
    repo.vendor(
        "helper",
        "1.0.0",
        "#[macro_export]\nmacro_rules! declare {\n    ($name:ident) => {\n        pub mod $name;\n    };\n}\n",
    );
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = \"1\"\n",
    );
    repo.write(
        "src/lib.rs",
        "pub mod old {\n    pub struct Mutex;\n}\n\nhelper::declare!(sync);\n",
    );
    lock(&repo);
    repo.write("src/sync.rs", "pub struct Mutex;\npub struct Gone;\n");
    repo.commit("declared module");
    repo.write("src/lib.rs", "helper::declare!(sync);\n");
    repo.write("src/sync.rs", "pub struct Mutex;\n");
    repo.write("README.md", "`Mutex` and `fixture::sync::Gone`\n");
    repo.commit("drop old and Gone");

    let paths: Vec<String> = found(&repo).into_iter().map(|row| row.0).collect();
    assert_eq!(paths, ["fixture::sync::Gone"]);
}

#[test]
fn a_removed_entry_is_labeled_by_the_package_that_owned_it() {
    let repo = Repo::new();
    repo.vendor("helper", "1.0.0", "pub struct Thing;\n");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\nmembers = [\"widget\"]\n",
    );
    repo.write(
        "widget/Cargo.toml",
        "[package]\nname = \"widget\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = \"1\"\n",
    );
    repo.write("src/lib.rs", "");
    repo.write("widget/src/lib.rs", "");
    lock(&repo);
    repo.commit("widget with helper");
    repo.write(
        "widget/Cargo.toml",
        "[package]\nname = \"widget\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    lock(&repo);
    repo.write(
        "widget/README.md",
        "Run `cargo add helper` for `helper::Thing`.\n",
    );
    repo.commit("drop helper");

    let report = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap();
    let entry = report.groups.iter().find(|g| g.path == "helper").unwrap();
    assert_eq!(entry.kind, ItemKind::Dependency);
    assert_eq!(entry.member, Some("widget".to_owned()));
}

#[test]
fn an_item_several_removed_dependencies_provided_is_blamed_on_the_last_removal() {
    let repo = Repo::new();
    repo.vendor("helper", "1.0.0", "pub struct Thing;\n");
    let member = |name: &str, dependency: &str| {
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{dependency}"
        )
    };
    let with_helper = "\n[dependencies]\nhelper = \"1\"\n";
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\nmembers = [\"a\", \"b\"]\n",
    );
    repo.write("a/Cargo.toml", &member("a", with_helper));
    repo.write("b/Cargo.toml", &member("b", with_helper));
    for file in ["src/lib.rs", "a/src/lib.rs", "b/src/lib.rs"] {
        repo.write(file, "");
    }
    lock(&repo);
    repo.commit("both use helper");
    repo.write("a/Cargo.toml", &member("a", ""));
    lock(&repo);
    repo.commit("a drops helper");
    repo.write("b/Cargo.toml", &member("b", ""));
    lock(&repo);
    repo.write("README.md", "`helper::Thing`\n");
    let last = repo.commit("b drops helper");

    for _ in 0..6 {
        let report = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap();
        let group = report
            .groups
            .iter()
            .find(|g| g.path == "helper::Thing")
            .unwrap();
        assert_eq!(
            group.removed_in,
            Some(refquiem::RemovedIn::Commit(last.clone()))
        );
    }
}

#[test]
fn a_path_the_workspace_removed_after_a_dependency_on_it_went_is_blamed_on_its_own_removal() {
    let repo = Repo::new();
    let app = |dependency: &str| {
        format!("[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{dependency}")
    };
    repo.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"core_lib\", \"app\"]\nresolver = \"2\"\n",
    );
    repo.write(
        "core_lib/Cargo.toml",
        "[package]\nname = \"core_lib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("core_lib/src/lib.rs", "pub struct Thing;\n");
    repo.write(
        "app/Cargo.toml",
        &app("\n[dependencies]\ncore_lib = { path = \"../core_lib\" }\n"),
    );
    repo.write("app/src/lib.rs", "");
    lock(&repo);
    repo.commit("app uses core_lib");
    repo.write("app/Cargo.toml", &app(""));
    lock(&repo);
    repo.commit("app drops core_lib");
    repo.write("core_lib/src/lib.rs", "");
    repo.write("README.md", "`core_lib::Thing`\n");
    let last = repo.commit("core_lib drops Thing");

    let report = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap();
    let group = report
        .groups
        .iter()
        .find(|g| g.path == "core_lib::Thing")
        .unwrap();
    assert_eq!(group.removed_in, Some(refquiem::RemovedIn::Commit(last)));
}

#[test]
fn a_module_public_on_one_cfg_branch_stays_public() {
    let repo = Repo::new();
    repo.vendor(
        "helper",
        "1.0.0",
        "macro_rules! cfg_a {\n    ($($i:item)*) => { $($i)* };\n}\nmacro_rules! cfg_b {\n    ($($i:item)*) => { $($i)* };\n}\ncfg_a! {\n    pub mod runtime {\n        pub struct Runtime;\n    }\n}\ncfg_b! {\n    pub(crate) mod runtime {\n        pub struct Runtime;\n    }\n}\nmod deep {\n    pub struct Deep;\n}\n",
    );
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = \"1\"\n",
    );
    repo.write("src/lib.rs", "pub struct Runtime;\npub struct Deep;\n");
    lock(&repo);
    repo.commit("start");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "`Runtime` and `Deep`.\n");
    repo.commit("drop both");

    let rows: Vec<String> = found(&repo).into_iter().map(|m| m.0).collect();
    assert_eq!(rows, ["fixture::Deep"]);
}

#[test]
fn a_hyphenated_spelling_matches_a_name_with_underscores() {
    let repo = Repo::new();
    repo.vendor("helper_lib", "1.0.0", "pub struct Thing;\n");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper_lib = \"1\"\n",
    );
    repo.write("src/lib.rs", "");
    lock(&repo);
    repo.commit("with helper_lib");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    lock(&repo);
    repo.write("README.md", "Run `cargo add helper-lib`.\n");
    repo.commit("drop helper_lib");

    let rows: Vec<(String, String)> = found(&repo).into_iter().map(|m| (m.0, m.5)).collect();
    assert_eq!(rows, [("helper_lib".to_owned(), "helper-lib".to_owned())]);
}

#[test]
fn a_public_reexport_keeps_the_path_it_shares_with_a_private_module() {
    let repo = Repo::new();
    repo.vendor(
        "helper",
        "1.0.0",
        "mod blank_out {\n    pub fn blank_out() {}\n    pub fn hidden_fn() {}\n}\npub use self::blank_out::blank_out;\n",
    );
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = \"1\"\n",
    );
    repo.write(
        "src/lib.rs",
        "pub fn blank_out() {}\npub fn hidden_fn() {}\n",
    );
    lock(&repo);
    repo.commit("start");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "`blank_out` and `hidden_fn`.\n");
    repo.commit("drop both");

    let rows: Vec<String> = found(&repo).into_iter().map(|m| m.0).collect();
    assert_eq!(rows, ["fixture::hidden_fn"]);
}

#[test]
fn a_renamed_dependency_also_reads_under_its_crate_name() {
    let repo = Repo::new();
    repo.vendor("helper", "0.1.0", "pub struct OldThing;\n");
    repo.vendor(
        "helper",
        "0.2.0",
        "pub struct OldThing;\npub struct NewThing;\n",
    );
    let root = |dependency: &str| {
        format!(
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\nmembers = [\"app\"]\n\n[dependencies]\n{dependency}\n"
        )
    };
    repo.write("Cargo.toml", &root(""));
    repo.write(
        "app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = \"0.2\"\n",
    );
    repo.write("src/lib.rs", "");
    repo.write("app/src/lib.rs", "");
    lock(&repo);
    repo.commit("app with helper 0.2");
    repo.write(
        "app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write(
        "Cargo.toml",
        &root("helper_01 = { package = \"helper\", version = \"0.1\" }"),
    );
    lock(&repo);
    repo.write("README.md", "`helper::OldThing` and `helper::NewThing`.\n");
    repo.commit("the root renames the old helper");

    let rows: Vec<String> = found(&repo).into_iter().map(|m| m.0).collect();
    assert_eq!(rows, ["helper::NewThing"]);
}

#[test]
fn a_renamed_old_version_of_a_workspace_crate_keeps_nothing_of_it_alive() {
    let repo = Repo::new();
    repo.vendor("helper", "0.1.0", "pub struct Gone;\n");
    let app = |dependency: &str| {
        format!(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{dependency}\n"
        )
    };
    repo.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"helper\", \"app\"]\n",
    );
    repo.write(
        "helper/Cargo.toml",
        "[package]\nname = \"helper\"\nversion = \"0.2.0\"\nedition = \"2021\"\n",
    );
    repo.write("helper/src/lib.rs", "pub struct Gone;\n");
    repo.write("app/Cargo.toml", &app(""));
    repo.write("app/src/lib.rs", "");
    lock(&repo);
    repo.commit("helper has Gone");
    repo.write("helper/src/lib.rs", "");
    repo.write(
        "app/Cargo.toml",
        &app("helper_old = { package = \"helper\", version = \"0.1\" }"),
    );
    lock(&repo);
    repo.write("README.md", "`helper::Gone`.\n");
    repo.commit("Gone goes, an old helper stays");

    let rows: Vec<String> = found(&repo).into_iter().map(|m| m.0).collect();
    assert_eq!(rows, ["helper::Gone"]);
}

#[test]
fn a_scan_succeeds_when_a_default_target_directory_is_a_file() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.write("examples", "not a directory\n");
    repo.commit("start");
    repo.write("src/lib.rs", "/// Was `fixture::gone`.\npub fn kept() {}\n");
    repo.commit("drop gone");

    let rows: Vec<(String, String, usize, usize)> = found(&repo)
        .into_iter()
        .map(|m| (m.0, m.1, m.2, m.3))
        .collect();
    assert_eq!(
        rows,
        [("fixture::gone".to_owned(), "src/lib.rs".to_owned(), 1, 10)]
    );
}

#[test]
fn files_under_the_target_directory_of_the_working_tree_are_not_scanned() {
    let repo = Repo::new();
    repo.write(".gitignore", "");
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.commit("start");
    repo.write("src/lib.rs", "/// Was `fixture::gone`.\npub fn kept() {}\n");
    repo.write("target/debug/note.rs", "// `fixture::gone`.\n");
    repo.commit("drop gone");

    let rows: Vec<(String, String, usize, usize)> = found(&repo)
        .into_iter()
        .map(|m| (m.0, m.1, m.2, m.3))
        .collect();
    assert_eq!(
        rows,
        [("fixture::gone".to_owned(), "src/lib.rs".to_owned(), 1, 10)]
    );
}

#[test]
fn files_under_the_target_directory_of_a_committed_head_are_not_scanned() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    let base = repo.commit("start");
    repo.write("src/lib.rs", "/// Was `fixture::gone`.\npub fn kept() {}\n");
    repo.write("target/debug/note.rs", "// `fixture::gone`.\n");
    // The fixture ignores `/target`, so the file is added explicitly to sit in the head commit.
    repo.git(&["add", "-f", "target/debug/note.rs"]);
    let head = repo.commit("drop gone");

    let mut options = refquiem::Options::new(repo.path().to_path_buf());
    options.range = refquiem::Range::Base {
        base,
        head: Some(head),
    };
    let report = refquiem::run(&options).unwrap();
    let rows: Vec<(String, String, usize, usize)> = report
        .groups
        .iter()
        .flat_map(|g| {
            g.mentions
                .iter()
                .map(|m| (g.path.clone(), m.file.clone(), m.line, m.column))
        })
        .collect();
    assert_eq!(
        rows,
        [("fixture::gone".to_owned(), "src/lib.rs".to_owned(), 1, 10)]
    );
}

#[test]
fn a_vocabulary_word_in_prose_is_not_reported_as_removed() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub struct Config;\n");
    repo.commit("start");
    repo.write("src/lib.rs", "pub struct Kept;\n");
    repo.write("README.md", "The Config was dropped. See `Config`.\n");
    repo.commit("drop Config");

    let rows: Vec<(String, String, String)> =
        found(&repo).into_iter().map(|m| (m.0, m.1, m.5)).collect();
    assert_eq!(
        rows,
        [(
            "fixture::Config".to_owned(),
            "README.md".to_owned(),
            "Config".to_owned()
        )]
    );
}

#[test]
fn a_scan_from_a_subdirectory_resolves_the_workspace_at_the_repository_root() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.commit("start");
    repo.write("src/lib.rs", "/// Was `fixture::gone`.\npub fn kept() {}\n");
    repo.write("sub/keep.txt", "kept\n");
    repo.commit("drop gone");

    let options = refquiem::Options::new(repo.path().join("sub"));
    let report = refquiem::run(&options).unwrap();
    let rows: Vec<(String, String, usize, usize)> = report
        .groups
        .iter()
        .flat_map(|g| {
            g.mentions
                .iter()
                .map(|m| (g.path.clone(), m.file.clone(), m.line, m.column))
        })
        .collect();
    assert_eq!(
        rows,
        [("fixture::gone".to_owned(), "src/lib.rs".to_owned(), 1, 10)]
    );
}

#[test]
fn a_workspace_in_a_subdirectory_is_exported_at_the_export_root() {
    let repo = Repo::new();
    repo.vendor("helper", "1.0.0", "pub struct mac;\n");
    repo.write(
        "ws/Cargo.toml",
        "[package]\nname = \"ws\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = \"1\"\n",
    );
    repo.write("ws/src/lib.rs", "pub struct Frame {\n    pub mac: u8,\n}\n");
    repo.commit("ws with helper");
    repo.write("ws/src/lib.rs", "");
    repo.write("ws/README.md", "`mac`\n");
    repo.commit("drop Frame");

    let options = refquiem::Options::new(repo.path().join("ws"));
    let report = refquiem::run(&options).unwrap();
    assert_eq!(report.groups.len(), 0);
}

#[test]
fn a_removed_crate_name_counts_only_where_it_acts_as_a_dependency() {
    let repo = Repo::new();
    repo.vendor("tower", "0.4.0", "pub struct Service;\n");
    let manifest = "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
    repo.write(
        "Cargo.toml",
        &format!("{manifest}\n[dependencies]\ntower = \"0.4\"\n"),
    );
    repo.write("src/lib.rs", "");
    lock(&repo);
    repo.commit("with tower");
    repo.write("Cargo.toml", manifest);
    lock(&repo);
    repo.write(
        "README.md",
        "[docs](https://docs.rs/tower), the `tower` crate, `cargo add tower`, `cargo update -p tower`.\n[page](https://docs.rs/tower/0.4.0/tower/fn.gone.html)\n\n```toml\n[dependencies]\ntower = \"0.4\"\n```\n",
    );
    repo.commit("drop tower");

    let rows: Vec<(String, usize, usize, String)> = found(&repo)
        .into_iter()
        .map(|m| (m.0, m.2, m.3, m.5))
        .collect();
    let row = |line, column| ("tower".to_owned(), line, column, "tower".to_owned());
    let page = (
        "tower".to_owned(),
        2,
        8,
        "https://docs.rs/tower/0.4.0/tower/fn.gone.html".to_owned(),
    );
    assert_eq!(rows, [row(1, 62), row(1, 87), page, row(6, 1)]);
    let report = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap();
    let kinds: Vec<ItemKind> = report.groups.iter().map(|g| g.kind).collect();
    assert_eq!(kinds, [ItemKind::Dependency]);
}

#[test]
fn cargo_add_names_every_crate_up_to_its_first_flag_or_punctuation() {
    let repo = Repo::new();
    repo.vendor("tower", "0.4.0", "pub struct Service;\n");
    let manifest = "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
    repo.write(
        "Cargo.toml",
        &format!("{manifest}\n[dependencies]\ntower = \"0.4\"\n"),
    );
    repo.write("src/lib.rs", "");
    lock(&repo);
    repo.commit("with tower");
    repo.write("Cargo.toml", manifest);
    lock(&repo);
    repo.write(
        "README.md",
        "`cargo add tower@0.4 tower -F x tower`\n\nRun cargo add tower, then tower.\n\n`cargo address tower`\n",
    );
    repo.commit("drop tower");

    let rows: Vec<(usize, usize)> = found(&repo).into_iter().map(|m| (m.2, m.3)).collect();
    assert_eq!(rows, [(1, 12), (1, 22), (3, 15)]);
}

#[test]
fn a_reexported_dependency_item_keeps_its_members_alive_under_the_alias() {
    let repo = Repo::new();
    repo.vendor(
        "helper",
        "1.0.0",
        "pub struct Request;\nimpl Request {\n    pub fn new() {}\n}\npub mod shapes {\n    pub struct Circle;\n    impl Circle {\n        pub fn area() {}\n    }\n}\n",
    );
    let manifest = "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = \"1\"\n";
    repo.write("Cargo.toml", manifest);
    repo.write(
        "src/lib.rs",
        "pub struct Request;\nimpl Request {\n    pub fn new() {}\n    pub fn gone() {}\n}\npub mod shapes {\n    pub struct Circle;\n    impl Circle {\n        pub fn area() {}\n    }\n}\n",
    );
    lock(&repo);
    repo.commit("own request and shapes");
    repo.write(
        "src/lib.rs",
        "pub use helper;\npub use helper::{Request};\npub mod shapes {\n    pub use helper::shapes::*;\n}\n",
    );
    repo.write(
        "README.md",
        "`fixture::Request::new`, `fixture::shapes::Circle::area` and `fixture::Request::gone`.\n",
    );
    repo.commit("re-export the helper's");

    let rows: Vec<String> = found(&repo).into_iter().map(|m| m.5).collect();
    assert_eq!(rows, ["fixture::Request::gone"]);
}

#[test]
fn a_bare_name_goes_to_a_workspace_item_before_a_dependency_item() {
    let repo = Repo::new();
    repo.vendor("able", "0.1.0", "pub struct Gadget;\n");
    repo.vendor("able", "0.2.0", "");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\nmembers = [\"app\"]\n\n[dependencies]\nable = \"0.2\"\n",
    );
    let app = |dependency: &str| {
        format!("[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{dependency}")
    };
    repo.write("app/Cargo.toml", &app("\n[dependencies]\nable = \"0.1\"\n"));
    repo.write("app/src/lib.rs", "");
    repo.write("src/lib.rs", "pub struct Gadget;\n");
    lock(&repo);
    repo.commit("both have a Gadget");
    repo.write("src/lib.rs", "");
    repo.commit("fixture drops Gadget");
    repo.write("app/Cargo.toml", &app(""));
    lock(&repo);
    // A member owning neither candidate leaves the choice to the workspace rule.
    repo.write("app/README.md", "`Gadget`\n");
    repo.commit("app drops the old able");

    let rows: Vec<String> = found(&repo).into_iter().map(|m| m.0).collect();
    assert_eq!(rows, ["fixture::Gadget"]);
}

#[test]
fn a_removed_dependency_and_its_items_are_reported_from_qualified_and_toml_names() {
    let repo = Repo::new();
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = { path = \"vendor/helper\" }\n",
    );
    repo.write(
        "vendor/helper/Cargo.toml",
        "[package]\nname = \"helper\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("vendor/helper/src/lib.rs", "pub struct Thing;\n");
    repo.write("src/lib.rs", "");
    lock(&repo);
    repo.commit("with helper");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    lock(&repo);
    std::fs::remove_dir_all(repo.path().join("vendor/helper")).unwrap();
    repo.write("README.md", "The `helper` crate and its `helper::Thing` item.\n\n```toml\n[dependencies]\nhelper = \"1\"\n```\n");
    repo.commit("drop helper");

    let report = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap();
    let groups: Vec<(&str, ItemKind)> = report
        .groups
        .iter()
        .map(|g| (g.path.as_str(), g.kind))
        .collect();
    assert_eq!(
        groups,
        [
            ("helper", ItemKind::Dependency),
            ("helper::Thing", ItemKind::Struct),
        ]
    );
}

#[test]
fn dependency_trait_members_follow_the_traits_public_reachability() {
    let method = "fn retired_probe_842(&self);";
    for (declaration, inherent, stale) in [
        (format!("trait Hidden {{ {method} }}"), "", true),
        (
            format!("trait Hidden {{ {method} }}\npub fn Hidden() {{}}"),
            "",
            true,
        ),
        (format!("pub trait Hidden {{ {method} }}"), "", false),
        (
            format!("mod inner {{ pub trait Hidden {{ {method} }} }}\nuse inner::Hidden;"),
            "",
            true,
        ),
        (
            format!("mod inner {{ pub trait Hidden {{ {method} }} }}\npub use inner::Hidden;"),
            "",
            false,
        ),
        (
            format!(
                "mod inner {{ pub trait Original {{ {method} }} }}\npub use inner::Original as Exported;\nuse Exported as Hidden;"
            ),
            "",
            false,
        ),
        (
            format!(
                "mod inner {{ pub trait Original {{ {method} }} }}\npub use inner::*;\nuse Original as Hidden;"
            ),
            "",
            false,
        ),
        (
            format!("trait Hidden {{ {method} }}"),
            "impl Widget { pub fn retired_probe_842(&self) {} }",
            false,
        ),
    ] {
        let repo = Repo::new();
        repo.vendor(
            "helper",
            "1.0.0",
            &format!("pub struct Widget;\n{declaration}\nimpl Hidden for Widget {{ fn retired_probe_842(&self) {{}} }}\n{inherent}\n"),
        );
        repo.write(
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = \"1\"\n",
        );
        repo.write(
            "src/lib.rs",
            "pub struct Local;\nimpl Local { pub fn retired_probe_842(&self) {} }\n",
        );
        lock(&repo);
        repo.commit("method");
        repo.write("src/lib.rs", "pub struct Local;\n");
        repo.write("README.md", "`retired_probe_842`\n");
        repo.commit("remove method");

        let rows: Vec<String> = found(&repo).into_iter().map(|row| row.0).collect();
        let expected = if stale {
            vec!["fixture::Local::retired_probe_842".to_owned()]
        } else {
            Vec::new()
        };
        assert_eq!(rows, expected, "{declaration}");
    }
}

#[test]
fn explicit_package_identity_keeps_a_custom_library_under_the_dependency_key() {
    let repo = Repo::new();
    let manifest = "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\nexclude = [\"vendor/helper\"]\n";
    repo.write(
        "Cargo.toml",
        &format!("{manifest}\n[dependencies]\nhelper = {{ package = \"helper\", path = \"vendor/helper\" }}\n"),
    );
    repo.write(
        "vendor/helper/Cargo.toml",
        "[package]\nname = \"helper\"\nversion = \"1.0.0\"\nedition = \"2021\"\n\n[lib]\nname = \"custom_helper\"\n\n[workspace]\n",
    );
    repo.write("vendor/helper/src/lib.rs", "pub struct RetiredProbe842;\n");
    repo.write("src/lib.rs", "");
    lock(&repo);
    repo.commit("explicit package with a custom library name");
    repo.write("Cargo.toml", manifest);
    std::fs::remove_dir_all(repo.path().join("vendor")).unwrap();
    lock(&repo);
    repo.write("README.md", "`helper::RetiredProbe842`\n");
    repo.commit("drop the dependency");

    let report = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap();
    let groups: Vec<_> = report
        .groups
        .iter()
        .map(|group| (group.path.as_str(), group.kind))
        .collect();
    assert_eq!(groups, [("helper::RetiredProbe842", ItemKind::Struct)]);
    assert_eq!(report.skipped, []);
}

#[test]
fn only_public_dependency_fields_claim_removed_local_names() {
    for (visibility, expected) in [("", vec!["fixture::retired_probe_842"]), ("pub ", vec![])] {
        let repo = Repo::new();
        repo.vendor(
            "helper",
            "1.0.0",
            &format!("pub struct Live {{ {visibility}retired_probe_842: u8 }}\n"),
        );
        repo.write(
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nhelper = \"1\"\n",
        );
        repo.write("src/lib.rs", "pub fn retired_probe_842() {}\n");
        lock(&repo);
        repo.commit("local function and dependency fields");
        repo.write("src/lib.rs", "");
        repo.write("README.md", "`retired_probe_842`\n");
        repo.commit("drop the local function");

        let report = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap();
        let paths: Vec<_> = report
            .groups
            .iter()
            .map(|group| group.path.as_str())
            .collect();
        assert_eq!(paths, expected, "{visibility:?}");
    }
}
