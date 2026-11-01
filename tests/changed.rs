//! Mentions of items a `--base` range changed without removing them, in files it left alone.

mod common;

use common::Repo;
use refquiem::{ChangedPart, Range};

/// The scan of `base..HEAD` against the head commit.
fn scan(repo: &Repo, base: &str) -> refquiem::Report {
    let mut options = refquiem::Options::new(repo.path().to_path_buf());
    options.range = Range::Base {
        base: base.to_owned(),
        head: Some(repo.rev("HEAD")),
    };
    refquiem::run(&options).unwrap()
}

/// `(item path, part, file, line)` for every changed item mention, sorted.
fn changed(report: &refquiem::Report) -> Vec<(String, ChangedPart, String, usize)> {
    let mut out: Vec<_> = report
        .changed
        .iter()
        .flat_map(|item| {
            item.mentions
                .iter()
                .map(|m| (item.path.clone(), item.part, m.file.clone(), m.line))
        })
        .collect();
    out.sort();
    out
}

/// A repository whose `fixture::version::PROTOCOL_VERSION` is 1 and whose protocol chapter
/// says so, committed as the base.
fn protocol() -> (Repo, String) {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub mod version;\n");
    repo.write("src/version.rs", "pub const PROTOCOL_VERSION: u32 = 1;\n");
    repo.write(
        "docs/protocol.md",
        "# Protocol\n\n`PROTOCOL_VERSION` stays frozen at 1 until the first release.\n",
    );
    let base = repo.commit("protocol 1");
    (repo, base)
}

#[test]
fn a_changed_const_value_reports_the_markdown_naming_it() {
    let (repo, base) = protocol();
    repo.write("src/version.rs", "pub const PROTOCOL_VERSION: u32 = 2;\n");
    repo.commit("protocol 2");

    let report = scan(&repo, &base);

    assert_eq!(
        changed(&report),
        [(
            "fixture::version::PROTOCOL_VERSION".to_owned(),
            ChangedPart::Value,
            "docs/protocol.md".to_owned(),
            3
        )]
    );
    let declared = report.changed[0].declared_at.as_ref().unwrap();
    assert_eq!(
        (declared.file.as_str(), declared.line),
        ("src/version.rs", 1)
    );
    assert!(report.is_clean(), "a changed item warns without failing");
}

#[test]
fn a_range_editing_the_mentioning_file_too_reports_nothing() {
    let (repo, base) = protocol();
    repo.write("src/version.rs", "pub const PROTOCOL_VERSION: u32 = 2;\n");
    repo.write(
        "docs/protocol.md",
        "# Protocol\n\n`PROTOCOL_VERSION` is 2.\n",
    );
    repo.commit("protocol 2 with docs");

    assert_eq!(changed(&scan(&repo, &base)), []);
}

#[test]
fn a_full_history_scan_compares_no_definitions() {
    let (repo, _) = protocol();
    repo.write("src/version.rs", "pub const PROTOCOL_VERSION: u32 = 2;\n");
    repo.commit("protocol 2");

    let options = refquiem::Options::new(repo.path().to_path_buf());
    assert_eq!(changed(&refquiem::run(&options).unwrap()), []);
}

#[test]
fn a_renamed_field_reports_the_struct_and_a_reformat_reports_nothing() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub struct Hello { pub name: String }\n");
    repo.write("README.md", "Send a `Hello` first.\n");
    let base = repo.commit("hello");
    repo.write(
        "src/lib.rs",
        "/// The greeting.\npub struct Hello {\n    // who\n    pub name: String,\n}\n",
    );
    repo.commit("reformat");
    assert_eq!(changed(&scan(&repo, &base)), []);

    repo.write("src/lib.rs", "pub struct Hello { pub peer: String }\n");
    repo.commit("rename the field");
    assert_eq!(
        changed(&scan(&repo, &base)),
        [(
            "fixture::Hello".to_owned(),
            ChangedPart::Shape,
            "README.md".to_owned(),
            1
        )]
    );
}

#[test]
fn a_body_edit_reports_only_when_bodies_are_compared() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn limit() -> u32 {\n    10\n}\n");
    repo.write("README.md", "Call `fixture::limit` for the cap.\n");
    let base = repo.commit("limit");
    repo.write("src/lib.rs", "pub fn limit() -> u32 {\n    20\n}\n");
    repo.commit("raise the limit");
    assert_eq!(changed(&scan(&repo, &base)), []);

    repo.write(".refquiem.toml", "[changed]\ncompare = \"bodies\"\n");
    assert_eq!(
        changed(&scan(&repo, &base)),
        [(
            "fixture::limit".to_owned(),
            ChangedPart::Body,
            "README.md".to_owned(),
            1
        )]
    );
}

#[test]
fn the_level_fails_the_run_or_turns_the_check_off() {
    let (repo, base) = protocol();
    repo.write("src/version.rs", "pub const PROTOCOL_VERSION: u32 = 2;\n");
    repo.commit("protocol 2");

    repo.write(".refquiem.toml", "[changed]\nlevel = \"deny\"\n");
    let denied = scan(&repo, &base);
    assert_eq!(denied.changed.len(), 1);
    assert!(!denied.is_clean());

    repo.write(".refquiem.toml", "[changed]\nlevel = \"allow\"\n");
    assert_eq!(changed(&scan(&repo, &base)), []);
}

#[test]
fn a_bare_name_two_items_share_is_skipped_and_a_path_still_reports() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub mod client {\n    pub struct Config { pub url: String }\n}\npub mod server {\n    pub struct Config { pub port: u16 }\n}\n",
    );
    repo.write(
        "README.md",
        "Set up the `Config`.\n\nThe server reads `fixture::server::Config`.\n",
    );
    let base = repo.commit("configs");
    repo.write(
        "src/lib.rs",
        "pub mod client {\n    pub struct Config { pub url: String }\n}\npub mod server {\n    pub struct Config { pub port: u32 }\n}\n",
    );
    repo.commit("widen the port");

    assert_eq!(
        changed(&scan(&repo, &base)),
        [(
            "fixture::server::Config".to_owned(),
            ChangedPart::Shape,
            "README.md".to_owned(),
            3
        )]
    );
}

#[test]
fn a_re_exported_path_reports_the_item_it_stands_for() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "mod limits;\npub use limits::Limit;\n");
    repo.write("src/limits.rs", "pub enum Limit { Soft, Hard }\n");
    repo.write("README.md", "See [`fixture::Limit`].\n");
    let base = repo.commit("limits");
    repo.write("src/limits.rs", "pub enum Limit { Soft, Hard, Off }\n");
    repo.commit("add a variant");

    assert_eq!(
        changed(&scan(&repo, &base)),
        [(
            "fixture::limits::Limit".to_owned(),
            ChangedPart::Shape,
            "README.md".to_owned(),
            1
        )]
    );
}

#[test]
fn a_comment_reports_and_a_string_literal_does_not() {
    let (repo, _) = protocol();
    repo.write(
        "src/check.rs",
        "// Compare against PROTOCOL_VERSION.\npub fn check() -> &'static str {\n    \"PROTOCOL_VERSION\"\n}\n",
    );
    repo.write("src/lib.rs", "pub mod check;\npub mod version;\n");
    let base_with_check = repo.commit("check");
    repo.write("src/version.rs", "pub const PROTOCOL_VERSION: u32 = 2;\n");
    repo.commit("protocol 2");

    let found = changed(&scan(&repo, &base_with_check));
    assert_eq!(
        found,
        [
            (
                "fixture::version::PROTOCOL_VERSION".to_owned(),
                ChangedPart::Value,
                "docs/protocol.md".to_owned(),
                3
            ),
            (
                "fixture::version::PROTOCOL_VERSION".to_owned(),
                ChangedPart::Value,
                "src/check.rs".to_owned(),
                1
            ),
        ]
    );
}

/// A crate with a public `a::LIMIT` and a crate-private `b::LIMIT`, `b.rs` commenting on its
/// own, `c.rs` on either, and the README on the public one.
fn shared_name() -> (Repo, String) {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub mod a;\nmod b;\nmod c;\n");
    repo.write("src/a.rs", "pub const LIMIT: u32 = 1;\n");
    repo.write(
        "src/b.rs",
        "// LIMIT bounds the queue.\npub(crate) const LIMIT: u16 = 1;\n",
    );
    repo.write("src/c.rs", "// Check LIMIT first.\npub fn check() {}\n");
    repo.write("README.md", "The `LIMIT` is 1.\n");
    let base = repo.commit("limits");
    (repo, base)
}

#[test]
fn a_bare_name_outside_the_crate_picks_its_only_public_carrier() {
    let (repo, base) = shared_name();
    repo.write("src/a.rs", "pub const LIMIT: u32 = 2;\n");
    repo.commit("raise the public limit");

    assert_eq!(
        changed(&scan(&repo, &base)),
        [(
            "fixture::a::LIMIT".to_owned(),
            ChangedPart::Value,
            "README.md".to_owned(),
            1
        )],
        "inside the crate both carriers count, and b.rs reads its own"
    );
}

#[test]
fn a_changed_crate_private_carrier_stays_ambiguous_in_its_crate_and_hidden_outside() {
    let (repo, base) = shared_name();
    repo.write(
        "src/b.rs",
        "// LIMIT bounds the queue.\npub(crate) const LIMIT: u16 = 2;\n",
    );
    repo.commit("raise the private limit");

    assert_eq!(changed(&scan(&repo, &base)), []);
}
