//! Cargo flags, TOML tables, dotted headers and feature arrays, and where each names its item.

mod common;
#[path = "common/features.rs"]
mod features;
#[path = "common/found_kinds.rs"]
mod found_kinds;
#[path = "common/vendor.rs"]
mod vendor;

use common::{PACKAGE, Repo};
use found_kinds::{found, mention};
use refquiem::ItemKind;

/// Vendors `crate_name`, commits it as a dependency, and drops it from the manifest.
fn gone_dependency(crate_name: &str) -> Repo {
    let repo = Repo::new();
    repo.vendor(crate_name, "0.3.0", "");
    repo.write("src/lib.rs", "");
    repo.write(
        "Cargo.toml",
        &format!("{PACKAGE}\n[dependencies]\n{crate_name} = \"0.3\"\n"),
    );
    repo.commit(crate_name);
    repo.write("Cargo.toml", PACKAGE);
    repo
}

#[test]
fn cargo_flags_stop_at_whitespace_and_glued_flags_are_skipped() {
    let repo = Repo::new();
    repo.write(
        "Cargo.toml",
        &format!("{PACKAGE}\n[features]\ngone_feature = []\n"),
    );
    repo.write("src/lib.rs", "pub fn other_gone() {}\n");
    repo.commit("start");
    repo.write("Cargo.toml", PACKAGE);
    repo.write("src/lib.rs", "");
    repo.write(
        "README.md",
        "Build with --features gone_feature\nBuild with --features gone_feature fixture::other_gone\nOr (--features gone_feature) here.\nNot x--features gone_feature glued.\n",
    );
    repo.commit("drop the feature");

    assert_eq!(
        found(&repo),
        [
            mention(
                "gone_feature",
                ItemKind::Feature,
                "README.md",
                1,
                23,
                "gone_feature"
            ),
            mention(
                "gone_feature",
                ItemKind::Feature,
                "README.md",
                2,
                23,
                "gone_feature"
            ),
            mention(
                "fixture::other_gone",
                ItemKind::Function,
                "README.md",
                2,
                36,
                "fixture::other_gone"
            ),
            mention(
                "gone_feature",
                ItemKind::Feature,
                "README.md",
                3,
                16,
                "gone_feature"
            ),
        ]
    );
}

#[test]
fn toml_dependency_headers_and_keys_name_removed_crates() {
    let repo = Repo::new();
    repo.vendor("gone_crate", "0.3.0", "");
    repo.vendor("also_gone", "0.3.0", "");
    repo.vendor("stale_header", "0.3.0", "");
    repo.write("src/lib.rs", "");
    repo.write(
        "Cargo.toml",
        &format!(
            "{PACKAGE}\n[dependencies]\ngone_crate = \"0.3\"\nalso_gone = \"0.3\"\nstale_header = \"0.3\"\n"
        ),
    );
    repo.commit("all three");
    repo.write("Cargo.toml", PACKAGE);
    repo.write(
        "README.md",
        "```toml\n[dependencies]\ngone_crate = \"1.0\"\n[dependencies.also_gone]\n[foo.stale_header]\n```\n",
    );
    repo.commit("drop all three");

    assert_eq!(
        found(&repo),
        [
            mention(
                "gone_crate",
                ItemKind::Dependency,
                "README.md",
                3,
                1,
                "gone_crate"
            ),
            mention(
                "also_gone",
                ItemKind::Dependency,
                "README.md",
                4,
                15,
                "also_gone"
            ),
        ]
    );
}

#[test]
fn toml_tables_count_their_feature_lines() {
    let repo = Repo::new();
    repo.gone_features(&["gone_feature", "another_gone"], "both");
    repo.write(
        "README.md",
        "```toml\n[dependencies.fixture]\nfeatures = [\"gone_feature\"]\nextra-features = [\"another_gone\"]\n```\n",
    );
    repo.commit("drop both");

    assert_eq!(
        found(&repo),
        [mention(
            "gone_feature",
            ItemKind::Feature,
            "README.md",
            3,
            14,
            "gone_feature"
        )]
    );
}

#[test]
fn indented_toml_feature_lines_keep_their_offset() {
    let repo = Repo::new();
    repo.gone_features(&["gone_feature"], "feature");
    repo.write(
        "src/lib.rs",
        "/// ```toml\n///     [dependencies.fixture]\n///     features = [\"gone_feature\"]\n/// ```\npub fn kept() {}\n",
    );
    repo.commit("drop the feature");

    assert_eq!(
        found(&repo),
        [mention(
            "gone_feature",
            ItemKind::Feature,
            "src/lib.rs",
            3,
            22,
            "gone_feature"
        )]
    );
}

#[test]
fn toml_feature_arrays_count_each_quoted_name() {
    let repo = Repo::new();
    repo.gone_features(&["gone_feature"], "feature");
    repo.write(
        "README.md",
        "```toml\n[dependencies.fixture]\nfeatures = [ \"gone_feature\" ]\n```\n",
    );
    repo.commit("drop the feature");

    assert_eq!(
        found(&repo),
        [mention(
            "gone_feature",
            ItemKind::Feature,
            "README.md",
            3,
            15,
            "gone_feature"
        )]
    );
}

#[test]
fn quoted_kind_headers_name_their_second_key() {
    let repo = gone_dependency("gone_crate");
    repo.write(
        "README.md",
        "```toml\n[\"dependencies\".gone_crate]\nkeep = \"1.0\"\n```\n",
    );
    repo.commit("drop gone_crate");

    assert_eq!(
        found(&repo),
        [mention(
            "gone_crate",
            ItemKind::Dependency,
            "README.md",
            2,
            17,
            "gone_crate"
        )]
    );
}

#[test]
fn dotted_headers_end_at_their_closing_quote() {
    let repo = gone_dependency("gone_crate2");
    repo.write(
        "README.md",
        "```toml\n[dependencies.\"gone_crate\ngone_crate2 = \"1.0\"\n```\n",
    );
    repo.commit("drop gone_crate2");

    assert_eq!(
        found(&repo),
        [mention(
            "gone_crate2",
            ItemKind::Dependency,
            "README.md",
            3,
            1,
            "gone_crate2"
        )]
    );
}

#[test]
fn whitespace_after_dots_offsets_toml_keys() {
    let repo = gone_dependency("gone_crate");
    repo.write("README.md", "```toml\n[dependencies. gone_crate]\n```\n");
    repo.commit("drop gone_crate");

    assert_eq!(
        found(&repo),
        [mention(
            "gone_crate",
            ItemKind::Dependency,
            "README.md",
            2,
            16,
            "gone_crate"
        )]
    );
}

#[test]
fn quoted_keys_adjust_their_offset_once() {
    let repo = gone_dependency("gone_crate");
    repo.write("README.md", "```toml\n[dependencies.\"gone_crate\"]\n```\n");
    repo.commit("drop gone_crate");

    assert_eq!(
        found(&repo),
        [mention(
            "gone_crate",
            ItemKind::Dependency,
            "README.md",
            2,
            16,
            "gone_crate"
        )]
    );
}

#[test]
fn unquoted_keys_never_adjust_their_offset() {
    let repo = gone_dependency("gone_crate");
    repo.write("README.md", "```toml\n[dependencies.gone_crate]\n```\n");
    repo.commit("drop gone_crate");

    assert_eq!(
        found(&repo),
        [mention(
            "gone_crate",
            ItemKind::Dependency,
            "README.md",
            2,
            15,
            "gone_crate"
        )]
    );
}

#[test]
fn three_key_toml_headers_end_at_their_last_key() {
    let repo = gone_dependency("gone_crate");
    repo.write(
        "README.md",
        "```toml\n[target.x86_64.dependencies]\ngone_crate = \"1.0\"\n```\n",
    );
    repo.commit("drop gone_crate");

    assert_eq!(
        found(&repo),
        [mention(
            "gone_crate",
            ItemKind::Dependency,
            "README.md",
            3,
            1,
            "gone_crate"
        )]
    );
}

/// A bare word in a dependencies table is not a key, so it names nothing.
#[test]
fn bare_words_in_dependency_tables_are_not_keys() {
    let repo = Repo::new();
    repo.vendor("bareword", "0.1.0", "");
    repo.write(
        "Cargo.toml",
        r#"
        [package]
        name = "fixture"
        version = "0.1.0"

        [dependencies]
        bareword = "0.1"
        "#,
    );
    repo.commit("bareword");
    repo.write(
        "Cargo.toml",
        r#"
        [package]
        name = "fixture"
        version = "0.1.0"

        [dependencies]
        "#,
    );
    repo.write(
        "README.md",
        "```toml\n\
         [dependencies]\n\
         bareword\n\
         ```\n",
    );
    repo.commit("drop bareword");

    assert_eq!(found(&repo), []);
}

#[test]
fn every_feature_of_a_feature_array_is_checked() {
    let repo = Repo::new();
    repo.gone_features(&["gone_a", "gone_b"], "two features");
    repo.write(
        "README.md",
        "```toml\n[dependencies]\nfixture = { version = \"0.1\", features = [\"gone_a\", \"gone_b\"] }\n```\n",
    );
    repo.commit("drop both");

    assert_eq!(
        found(&repo),
        [
            mention("gone_a", ItemKind::Feature, "README.md", 3, 43, "gone_a"),
            mention("gone_b", ItemKind::Feature, "README.md", 3, 53, "gone_b"),
        ]
    );
}
