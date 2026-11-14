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

    repo.write(".refquiem.toml", "[changed]\ncompare = \"shapes\"\n");
    assert_eq!(changed(&scan(&repo, &base)), [], "a value is no shape");
}

/// `fixture::client::conf::Config` and `fixture::server::conf::Config`, mentioned by every
/// spelling, committed as the base.
fn two_configs() -> (Repo, String) {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub mod client {\n    pub mod conf {\n        pub struct Config { pub url: String }\n    }\n}\npub mod server {\n    pub mod conf {\n        pub struct Config { pub port: u16 }\n    }\n}\n",
    );
    repo.write(
        "README.md",
        "Set up the `Config`.\n\nRead `conf::Config`.\n\nThe server reads `fixture::server::conf::Config`.\n\nThe client reads `fixture::client::conf::Config`.\n\nBoth hold a `std::vec::Vec` and no `nothing::here`.\n",
    );
    let base = repo.commit("configs");
    (repo, base)
}

#[test]
fn a_spelling_two_items_share_is_skipped_and_a_full_path_still_reports() {
    let (repo, base) = two_configs();
    repo.write(
        "src/lib.rs",
        "pub mod client {\n    pub mod conf {\n        pub struct Config { pub url: String }\n    }\n}\npub mod server {\n    pub mod conf {\n        pub struct Config { pub port: u32 }\n    }\n}\n",
    );
    repo.commit("widen the port");

    assert_eq!(
        changed(&scan(&repo, &base)),
        [(
            "fixture::server::conf::Config".to_owned(),
            ChangedPart::Shape,
            "README.md".to_owned(),
            5
        )]
    );
}

#[test]
fn two_changed_items_sharing_a_spelling_report_only_by_full_path() {
    let (repo, base) = two_configs();
    repo.write(
        "src/lib.rs",
        "pub mod client {\n    pub mod conf {\n        pub struct Config { pub uri: String }\n    }\n}\npub mod server {\n    pub mod conf {\n        pub struct Config { pub port: u32 }\n    }\n}\n",
    );
    repo.commit("change both");

    let lines: Vec<(String, usize)> = changed(&scan(&repo, &base))
        .into_iter()
        .map(|(path, _, _, line)| (path, line))
        .collect();
    assert_eq!(
        lines,
        [
            ("fixture::client::conf::Config".to_owned(), 7),
            ("fixture::server::conf::Config".to_owned(), 5),
        ]
    );
}

#[test]
fn a_bare_name_running_text_claims_reports_only_as_code() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn budget(n: u32) {}\n");
    repo.write("README.md", "Mind the budget.\n\nCall `budget` first.\n");
    let base = repo.commit("budget");
    repo.write("src/lib.rs", "pub fn budget(n: u64) {}\n");
    repo.commit("widen the budget");

    let report = scan(&repo, &base);
    assert_eq!(
        changed(&report),
        [(
            "fixture::budget".to_owned(),
            ChangedPart::Shape,
            "README.md".to_owned(),
            3
        )]
    );
    assert!(
        report
            .json()
            .contains("\"headline\":\"`fixture::budget` (function), signature changed\""),
        "{}",
        report.json()
    );
}

#[test]
fn a_rust_comment_resolves_a_bare_name_through_its_module_first() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub mod a;\npub mod b;\npub mod c;\n");
    repo.write("src/a.rs", "pub const MAX_DEPTH: u32 = 1;\n");
    repo.write(
        "src/b.rs",
        "// MAX_DEPTH caps the queue.\npub use crate::a::MAX_DEPTH;\n",
    );
    repo.write("src/c.rs", "pub const MAX_DEPTH: u8 = 1;\n");
    repo.write("README.md", "The `MAX_DEPTH` is 1.\n");
    let base = repo.commit("depths");
    repo.write("src/a.rs", "pub const MAX_DEPTH: u32 = 2;\n");
    repo.commit("raise a");

    assert_eq!(
        changed(&scan(&repo, &base)),
        [(
            "fixture::a::MAX_DEPTH".to_owned(),
            ChangedPart::Value,
            "src/b.rs".to_owned(),
            1
        )],
        "two public carriers leave the README's bare name ambiguous"
    );
}

#[test]
fn a_working_tree_scan_compares_uncommitted_definitions() {
    let (repo, _) = protocol();
    repo.write("src/version.rs", "pub const PROTOCOL_VERSION: u32 = 2;\n");
    let mut options = refquiem::Options::new(repo.path().to_path_buf());
    options.range = Range::Base {
        base: "HEAD".to_owned(),
        head: None,
    };

    assert_eq!(
        changed(&refquiem::run(&options).unwrap()),
        [(
            "fixture::version::PROTOCOL_VERSION".to_owned(),
            ChangedPart::Value,
            "docs/protocol.md".to_owned(),
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
    repo.write("src/limits.rs", "pub enum Limit { Soft, Hard(u8) }\n");
    repo.commit("bound the hard limit");

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

/// `(item path, file, line)` for every changed item mention, sorted.
fn lines(report: &refquiem::Report) -> Vec<(String, String, usize)> {
    changed(report)
        .into_iter()
        .map(|(path, _, file, line)| (path, file, line))
        .collect()
}

/// A crate whose `Refusal` is `Empty` alone, described in a guide and in the docs of `parse`
/// and `check`, of which only the guide's second section and `check` name the variant.
fn refusals() -> (Repo, String) {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub mod parse;\n\n/// What parsing can refuse.\npub enum Refusal {\n    /// The input is empty.\n    Empty,\n}\n",
    );
    repo.write(
        "src/parse.rs",
        "use crate::Refusal;\n\
         \n\
         /// Parses `text`.\n\
         ///\n\
         /// # Errors\n\
         ///\n\
         /// Returns a [`Refusal`] when `text` is unusable.\n\
         pub fn parse(text: &str) -> Result<(), Refusal> {\n    check(text)\n}\n\
         \n\
         /// Checks `text`.\n\
         ///\n\
         /// # Errors\n\
         ///\n\
         /// Returns a [`Refusal`], [`Refusal::Empty`] when `text` holds nothing.\n\
         pub fn check(text: &str) -> Result<(), Refusal> {\n    if text.is_empty() { Err(Refusal::Empty) } else { Ok(()) }\n}\n",
    );
    repo.write(
        "docs/guide.md",
        "# Parsing\n\nEvery refusal of `parse` is a `Refusal`.\n\n## Kinds\n\nA `Refusal` is `Empty` when the input holds nothing.\n",
    );
    let base = repo.commit("refusals");
    (repo, base)
}

#[test]
fn an_added_variant_reports_only_sections_naming_a_variant() {
    let (repo, base) = refusals();
    repo.write(
        "src/lib.rs",
        "pub mod parse;\n\n/// What parsing can refuse.\npub enum Refusal {\n    /// The input is empty.\n    Empty,\n    /// The input holds a character outside the grammar.\n    Unexpected(char),\n}\n",
    );
    repo.commit("refuse unexpected characters");

    assert_eq!(
        lines(&scan(&repo, &base)),
        [
            ("fixture::Refusal".to_owned(), "docs/guide.md".to_owned(), 7),
            ("fixture::Refusal".to_owned(), "src/parse.rs".to_owned(), 16),
        ]
    );
}

#[test]
fn a_changed_variant_beside_an_added_one_reports_every_mention() {
    let (repo, base) = refusals();
    repo.write(
        "src/lib.rs",
        "pub mod parse;\n\n/// What parsing can refuse.\npub enum Refusal {\n    /// The input is empty.\n    Empty { at: usize },\n    Unexpected(char),\n}\n",
    );
    repo.commit("place empty inputs");

    assert_eq!(
        lines(&scan(&repo, &base)),
        [
            ("fixture::Refusal".to_owned(), "docs/guide.md".to_owned(), 3),
            ("fixture::Refusal".to_owned(), "docs/guide.md".to_owned(), 7),
            ("fixture::Refusal".to_owned(), "src/parse.rs".to_owned(), 7),
            ("fixture::Refusal".to_owned(), "src/parse.rs".to_owned(), 16),
            (
                "fixture::Refusal::Empty".to_owned(),
                "src/parse.rs".to_owned(),
                16
            ),
        ]
    );
}

/// A crate whose `Kind` enum has `variants`, and a README naming `Kind`.
fn kinds(variants: &str) -> (Repo, String) {
    let repo = Repo::new();
    repo.write("src/lib.rs", &format!("pub enum Kind {{ {variants} }}\n"));
    repo.write("README.md", "Every value has a `Kind`.\n");
    let base = repo.commit("kinds");
    (repo, base)
}

/// What the scan from `base` reports after `Kind` becomes `variants`.
fn rekinded(repo: &Repo, base: &str, variants: &str) -> Vec<(String, String, usize)> {
    repo.write("src/lib.rs", &format!("pub enum Kind {{ {variants} }}\n"));
    repo.commit(variants);
    lines(&scan(repo, base))
}

#[test]
fn a_variant_inserted_where_no_discriminant_shows_is_an_addition() {
    let (repo, base) = kinds("Text(String), Number(i64)");
    assert_eq!(
        rekinded(&repo, &base, "Text(String), Flag(bool), Number(i64)"),
        []
    );
}

#[test]
fn a_variant_inserted_before_a_castable_one_reports_and_an_appended_one_does_not() {
    let reported = [("fixture::Kind".to_owned(), "README.md".to_owned(), 1)];
    let (repo, base) = kinds("Text, Number");
    assert_eq!(rekinded(&repo, &base, "Text, Number, Flag"), []);
    assert_eq!(rekinded(&repo, &base, "Text, Flag, Number"), reported);

    let (repo, base) = kinds("Text(String), Number = 4");
    assert_eq!(
        rekinded(&repo, &base, "Text(String), Flag, Number = 4"),
        reported
    );
}

#[test]
fn reordered_variants_report_though_one_was_added() {
    let (repo, base) = kinds("Text(String), Number(i64)");
    assert_eq!(
        rekinded(&repo, &base, "Number(i64), Text(String), Flag(bool)"),
        [("fixture::Kind".to_owned(), "README.md".to_owned(), 1)]
    );
}

#[test]
fn an_added_struct_field_reports_only_sections_naming_a_field() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct Config {\n    pub name: String,\n}\n",
    );
    repo.write(
        "README.md",
        "# Setup\n\nBuild a `Config` first.\n\n## Fields\n\nA `Config` holds its `name`.\n",
    );
    let base = repo.commit("config");
    repo.write(
        "src/lib.rs",
        "pub struct Config {\n    pub port: u16,\n    pub name: String,\n}\n",
    );
    repo.commit("add a port");

    assert_eq!(
        lines(&scan(&repo, &base)),
        [("fixture::Config".to_owned(), "README.md".to_owned(), 7)]
    );
}

#[test]
fn a_compiled_doctest_naming_a_changed_item_is_left_to_rustdoc() {
    let (repo, _) = protocol();
    repo.write(
        "src/check.rs",
        "//! Checks.\n\
         //!\n\
         //! ```\n\
         //! assert_eq!(fixture::version::PROTOCOL_VERSION, 1);\n\
         //! ```\n\
         //!\n\
         //! ```ignore\n\
         //! let version = fixture::version::PROTOCOL_VERSION;\n\
         //! ```\n\
         pub fn check() {}\n",
    );
    repo.write("src/lib.rs", "pub mod check;\npub mod version;\n");
    let base = repo.commit("check");
    repo.write("src/version.rs", "pub const PROTOCOL_VERSION: u32 = 2;\n");
    repo.commit("protocol 2");

    let path = "fixture::version::PROTOCOL_VERSION".to_owned();
    assert_eq!(
        lines(&scan(&repo, &base)),
        [
            (path.clone(), "docs/protocol.md".to_owned(), 3),
            (path, "src/check.rs".to_owned(), 8),
        ]
    );
}
