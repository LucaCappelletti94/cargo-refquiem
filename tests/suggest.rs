//! Rename, move and replacement suggestions, and the mention rewrites they bring.

mod common;
#[path = "common/vendor.rs"]
mod vendor;

use common::Repo;
use refquiem::{Change, Group, Kind, Options, Report};
use std::fmt::Write as _;

const PACKAGE: &str = "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

fn report(repo: &Repo) -> Report {
    refquiem::run(&Options::new(repo.path().to_path_buf())).unwrap()
}

fn group<'r>(report: &'r Report, path: &str) -> &'r Group {
    report
        .groups
        .iter()
        .find(|group| group.path == path)
        .unwrap_or_else(|| panic!("no group {path}\n{}", report.human()))
}

fn suggested(group: &Group) -> Option<(&str, Change)> {
    group
        .suggestion
        .as_ref()
        .map(|suggestion| (suggestion.path.as_str(), suggestion.change))
}

/// The removed function `colour` mentioned in the README, with `src/lib.rs` written by the caller.
fn colour(repo: &Repo) {
    repo.write("README.md", "Call `fixture::colour`.\n");
}

#[test]
fn a_moved_item_is_suggested_and_its_path_mentions_are_rewritten() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub mod old {\n    pub fn helper() {}\n}\n");
    repo.commit("start");
    repo.write("src/lib.rs", "pub mod new {\n    pub fn helper() {}\n}\n");
    repo.write(
        "README.md",
        "Call `fixture::old::helper` or `old::helper`.\n\nSee [the helper](fixture::old::helper).\n",
    );
    repo.commit("move");

    let report = report(&repo);
    let group = group(&report, "fixture::old::helper");
    assert_eq!(
        suggested(group),
        Some(("fixture::new::helper", Change::Moved))
    );
    assert_eq!(group.candidates, Vec::<String>::new());
    assert!(
        report
            .human()
            .contains(", moved to `fixture::new::helper`\n"),
        "{}",
        report.human()
    );
    let rewritten = "Call `fixture::new::helper` or `new::helper`.";
    let line_one: Vec<Option<&str>> = group
        .mentions
        .iter()
        .filter(|m| m.line == 1)
        .map(|m| m.replacement.as_deref())
        .collect();
    assert_eq!(line_one, [Some(rewritten), Some(rewritten)]);
    let links: Vec<Option<&str>> = group
        .mentions
        .iter()
        .filter(|m| m.context.kind == Kind::Link)
        .map(|m| m.replacement.as_deref())
        .collect();
    assert_eq!(links, [None]);
}

#[test]
fn a_mention_whose_written_segments_a_move_keeps_is_not_rewritten() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub mod old {\n    pub fn helper() {}\n}\n");
    repo.commit("start");
    repo.write("src/lib.rs", "pub mod new {\n    pub fn helper() {}\n}\n");
    repo.write("src/notes.rs", "// Calls `old::helper` through `helper`.\n");
    repo.write("README.md", "`fixture::old::helper`\n");
    repo.commit("move");

    let report = report(&repo);
    let group = group(&report, "fixture::old::helper");
    for mention in &group.mentions {
        let expected = match mention.text.as_str() {
            "fixture::old::helper" => Some("`fixture::new::helper`"),
            "old::helper" => Some("// Calls `new::helper` through `helper`."),
            _ => None,
        };
        assert_eq!(mention.replacement.as_deref(), expected, "{mention:?}");
    }
}

#[test]
fn a_near_name_of_the_same_kind_and_parent_is_a_rename() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn colour() {}\npub mod paint {}\n");
    repo.commit("start");
    // A new function elsewhere is no move, its name differs.
    repo.write(
        "src/lib.rs",
        "pub fn color() {}\npub fn palette() {}\npub mod paint {\n    pub fn mix() {}\n}\n",
    );
    colour(&repo);
    repo.commit("rename");

    let report = report(&repo);
    let group = group(&report, "fixture::colour");
    assert_eq!(suggested(group), Some(("fixture::color", Change::Renamed)));
    assert_eq!(
        group.mentions[0].replacement.as_deref(),
        Some("Call `fixture::color`.")
    );
    assert!(
        report.human().contains(", renamed to `fixture::color`\n"),
        "{}",
        report.human()
    );
    let json: serde_json::Value = serde_json::from_str(&report.json()).unwrap();
    let group = &json["groups"][0];
    assert_eq!(group["suggestion"], "fixture::color");
    assert_eq!(
        group["mentions"][0]["replacement"],
        "Call `fixture::color`."
    );
}

#[test]
fn a_renamed_method_rewrites_a_type_qualified_mention() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct Parser;\nimpl Parser {\n    pub fn parse_expr(&self) {}\n}\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "pub struct Parser;\nimpl Parser {\n    pub fn parse_exprs(&self) {}\n}\n",
    );
    repo.write("README.md", "Use `Parser::parse_expr`.\n");
    repo.commit("rename");

    let report = report(&repo);
    let group = group(&report, "fixture::Parser::parse_expr");
    assert_eq!(
        suggested(group),
        Some(("fixture::Parser::parse_exprs", Change::Renamed))
    );
    assert_eq!(
        group.mentions[0].replacement.as_deref(),
        Some("Use `Parser::parse_exprs`.")
    );
}

#[test]
fn the_only_new_sibling_with_the_same_signature_is_a_replacement() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub fn legacy_total(items: &[u32]) -> u64 { 0 }\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "pub fn sum(values : & [ u32 ]) -> u64 { 0 }\npub fn label(id: u32) -> String { String::new() }\n",
    );
    repo.write("README.md", "Call `fixture::legacy_total`.\n");
    repo.commit("replace");

    let report = report(&repo);
    let group = group(&report, "fixture::legacy_total");
    assert_eq!(suggested(group), Some(("fixture::sum", Change::Replaced)));
    assert!(
        report.human().contains(", replaced by `fixture::sum`\n"),
        "{}",
        report.human()
    );
}

#[test]
fn tied_candidates_are_listed_without_a_suggestion() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn colour() {}\n");
    repo.commit("start");
    // The earliest rule finding several names is the one listed.
    repo.write(
        "src/lib.rs",
        "pub fn color() {}\npub fn colours() {}\npub fn shade() {}\n",
    );
    colour(&repo);
    repo.commit("rename twice");

    let report = report(&repo);
    let group = group(&report, "fixture::colour");
    assert_eq!(suggested(group), None);
    assert_eq!(group.candidates, ["fixture::color", "fixture::colours"]);
    assert_eq!(group.mentions[0].replacement, None);
    assert!(
        report
            .human()
            .contains(", perhaps `fixture::color` or `fixture::colours`\n"),
        "{}",
        report.human()
    );
}

#[test]
fn a_name_older_than_the_removed_one_is_no_candidate() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn color() {}\n");
    repo.commit("start");
    repo.write("src/lib.rs", "pub fn color() {}\npub fn colour() {}\n");
    repo.commit("add colour");
    repo.write("src/lib.rs", "pub fn color() {}\n");
    colour(&repo);
    repo.commit("drop colour");

    let report = report(&repo);
    let group = group(&report, "fixture::colour");
    assert_eq!(suggested(group), None);
    assert_eq!(group.candidates, Vec::<String>::new());
}

#[test]
fn a_replacement_added_before_the_removal_is_found() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn colour() {}\n");
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "#[deprecated]\npub fn colour() {}\npub fn color() {}\n",
    );
    repo.commit("deprecate colour");
    repo.write("src/lib.rs", "pub fn color() {}\n");
    colour(&repo);
    repo.commit("drop colour");

    let report = report(&repo);
    assert_eq!(
        suggested(group(&report, "fixture::colour")),
        Some(("fixture::color", Change::Renamed))
    );
}

#[test]
fn a_dependency_is_replaced_by_the_only_crate_added_to_a_table_it_shared() {
    let repo = Repo::new();
    for name in ["structopt", "clap", "insta"] {
        repo.vendor(name, "1.0.0", "");
    }
    repo.write("src/lib.rs", "");
    repo.write(
        "Cargo.toml",
        &format!(
            "{PACKAGE}\n[dependencies]\nstructopt = \"1\"\n\n[dev-dependencies]\nstructopt = \"1\"\n"
        ),
    );
    repo.commit("start");
    repo.write(
        "Cargo.toml",
        &format!(
            "{PACKAGE}\n[dev-dependencies]\nclap = \"1\"\n\n[build-dependencies]\ninsta = \"1\"\n"
        ),
    );
    repo.write(
        "README.md",
        "Run `cargo add structopt`.\n\n[docs](https://docs.rs/structopt)\n\n```toml\n[dependencies]\nstructopt = \"1\"\n```\n",
    );
    repo.commit("swap the parser");

    let report = report(&repo);
    let group = group(&report, "structopt");
    assert_eq!(suggested(group), Some(("clap", Change::Replaced)));
    let replacements: Vec<(Kind, Option<&str>)> = group
        .mentions
        .iter()
        .map(|m| (m.context.kind, m.replacement.as_deref()))
        .collect();
    assert_eq!(
        replacements,
        [
            (Kind::CodeSpan, Some("Run `cargo add clap`.")),
            (Kind::CodeBlock { compiled: false }, None),
        ]
    );
}

#[test]
fn signatures_are_read_after_a_shebang() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "#!/usr/bin/env run-cargo-script\npub fn legacy_total(items: &[u32]) -> u64 { 0 }\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "#!/usr/bin/env run-cargo-script\npub fn sum(values: &[u32]) -> u64 { 0 }\npub fn label(id: u32) -> String { String::new() }\n",
    );
    repo.write("README.md", "Call `fixture::legacy_total`.\n");
    repo.commit("replace");

    let report = report(&repo);
    assert_eq!(
        suggested(group(&report, "fixture::legacy_total")),
        Some(("fixture::sum", Change::Replaced))
    );
}

#[test]
fn trait_methods_are_matched_by_their_own_signatures() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub trait Store {\n    fn get(&self, key: u32) -> u32;\n    fn put(&mut self, value: String);\n}\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "pub trait Store {\n    fn fetch(&self, key: u32) -> u32 { key }\n    fn put(&mut self, value: String);\n}\n",
    );
    repo.write("README.md", "Call `Store::get`.\n");
    repo.commit("replace get");

    let report = report(&repo);
    assert_eq!(
        suggested(group(&report, "fixture::Store::get")),
        Some(("fixture::Store::fetch", Change::Replaced))
    );
}

#[test]
fn receivers_tell_signatures_apart() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct Store;\nimpl Store {\n    pub fn count(&self) -> u32 { 0 }\n    pub fn bump(&mut self) {}\n    pub fn into_inner(self: Box<Self>) {}\n}\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "pub struct Store;\nimpl Store {\n    pub fn tally(&self) -> u32 { 0 }\n    pub fn consume(self) -> u32 { 0 }\n    pub fn grow(&mut self) {}\n    pub fn drain(self) {}\n    pub fn unwrap_box(self: Box<Self>) {}\n}\n",
    );
    repo.write(
        "README.md",
        "`Store::count`, `Store::bump` and `Store::into_inner`\n",
    );
    repo.commit("replace them");

    let report = report(&repo);
    for (removed, replacement) in [
        ("fixture::Store::count", "fixture::Store::tally"),
        ("fixture::Store::bump", "fixture::Store::grow"),
        ("fixture::Store::into_inner", "fixture::Store::unwrap_box"),
    ] {
        assert_eq!(
            suggested(group(&report, removed)),
            Some((replacement, Change::Replaced)),
            "{removed}"
        );
    }
}

/// Each pair sits alone in its module, with signatures that differ so only rule 2 can fit.
#[test]
fn the_edit_distance_counts_transpositions_and_stops_at_a_third_of_the_name() {
    let repo = Repo::new();
    let pairs = [("ebe", "bee"), ("dad", "ada"), ("dbb", "bb")];
    let modules = |new: bool| {
        let mut out = String::new();
        for (i, (old, fresh)) in pairs.iter().enumerate() {
            let (name, ty) = if new { (fresh, "u16") } else { (old, "u8") };
            let _ = writeln!(
                out,
                "pub mod m{i} {{\n    pub fn {name}(value: {ty}) {{}}\n}}"
            );
        }
        out
    };
    repo.write("src/lib.rs", &modules(false));
    repo.commit("start");
    repo.write("src/lib.rs", &modules(true));
    repo.write("README.md", "`m0::ebe`, `m1::dad` and `m2::dbb`\n");
    repo.commit("rename");

    let report = report(&repo);
    assert_eq!(
        suggested(group(&report, "fixture::m0::ebe")),
        Some(("fixture::m0::bee", Change::Renamed))
    );
    assert_eq!(suggested(group(&report, "fixture::m1::dad")), None);
    assert_eq!(
        suggested(group(&report, "fixture::m2::dbb")),
        Some(("fixture::m2::bb", Change::Renamed))
    );
}

#[test]
fn a_feature_is_renamed_within_its_member_and_its_flags_stay() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "");
    repo.write(
        "Cargo.toml",
        &format!("{PACKAGE}\n[features]\nserde-support = []\n"),
    );
    repo.commit("start");
    repo.write(
        "Cargo.toml",
        &format!("{PACKAGE}\n[features]\nserde_support = []\n"),
    );
    repo.write("README.md", "`--features serde-support`\n");
    repo.commit("rename the feature");

    let report = report(&repo);
    let group = group(&report, "serde-support");
    assert_eq!(suggested(group), Some(("serde_support", Change::Renamed)));
    assert!(group.mentions.iter().all(|m| m.replacement.is_none()));
}

/// A feature in another member is neither a move nor a rename.
#[test]
fn a_feature_renamed_in_another_member_is_no_candidate() {
    let repo = Repo::new();
    repo.write("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n");
    for member in ["alpha", "beta"] {
        repo.write(
            &format!("crates/{member}/Cargo.toml"),
            &format!("[package]\nname = \"{member}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        );
        repo.write(&format!("crates/{member}/src/lib.rs"), "");
    }
    repo.write(
        "crates/alpha/Cargo.toml",
        "[package]\nname = \"alpha\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\nserde-support = []\n",
    );
    repo.write("crates/alpha/README.md", "`--features serde-support`\n");
    repo.commit("alpha's feature");
    repo.write(
        "crates/alpha/Cargo.toml",
        "[package]\nname = \"alpha\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write(
        "crates/beta/Cargo.toml",
        "[package]\nname = \"beta\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[features]\nserde_support = []\n",
    );
    repo.commit("drop alpha's feature, add beta's");

    let report = report(&repo);
    let group = group(&report, "serde-support");
    assert_eq!(suggested(group), None);
    assert_eq!(group.candidates, Vec::<String>::new());
}

/// A dependency in another member is no replacement.
#[test]
fn a_dependency_added_to_another_member_is_no_replacement() {
    let repo = Repo::new();
    for name in ["structopt", "clap"] {
        repo.vendor(name, "1.0.0", "");
    }
    repo.write("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n");
    repo.write(
        "crates/alpha/Cargo.toml",
        "[package]\nname = \"alpha\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nstructopt = \"1\"\n",
    );
    repo.write("crates/alpha/src/lib.rs", "");
    repo.write(
        "crates/alpha/README.md",
        "```toml\n[dependencies]\nstructopt = \"1\"\n```\n",
    );
    repo.write(
        "crates/beta/Cargo.toml",
        "[package]\nname = \"beta\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("crates/beta/src/lib.rs", "");
    repo.commit("alpha depends on structopt");
    repo.write(
        "crates/alpha/Cargo.toml",
        "[package]\nname = \"alpha\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write(
        "crates/beta/Cargo.toml",
        "[package]\nname = \"beta\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nclap = \"1\"\n",
    );
    repo.commit("alpha drops structopt, beta takes clap");

    let report = report(&repo);
    let group = group(&report, "structopt");
    assert_eq!(suggested(group), None);
    assert_eq!(group.candidates, Vec::<String>::new());
}

/// Incomplete history cannot establish a suggestion's introduction order.
#[test]
fn an_incomplete_first_tree_gets_no_suggestion() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "mod broken;\npub fn colour() {}\n");
    repo.write("src/broken.rs", "pub fn color() {\n");
    repo.commit("colour, with a broken file");
    repo.write("src/lib.rs", "pub fn color() {}\n");
    colour(&repo);
    repo.commit("drop colour");

    let report = report(&repo);
    let group = group(&report, "fixture::colour");
    assert_eq!(suggested(group), None);
    assert_eq!(group.candidates, Vec::<String>::new());
}

#[test]
fn a_restored_name_older_than_the_removed_name_is_no_candidate() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn color() {}\n");
    colour(&repo);
    repo.commit("older name");
    repo.write("src/lib.rs", "");
    repo.commit("older name absent");
    repo.write("src/lib.rs", "pub fn colour() {}\n");
    repo.commit("removed name");
    repo.write("src/lib.rs", "pub fn color() {}\n");
    repo.commit("restore older name");

    let report = report(&repo);
    let group = group(&report, "fixture::colour");
    assert_eq!(suggested(group), None);
    assert_eq!(group.candidates, Vec::<String>::new());
}

#[test]
fn a_keyword_replacement_is_written_as_a_raw_rust_identifier() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn typo() {}\n");
    repo.commit("original name");
    repo.write("src/lib.rs", "pub fn r#type() {}\n");
    repo.write("README.md", "`fixture::typo`\n`fixture::r#typo`\n");
    repo.commit("keyword name");

    let report = report(&repo);
    let group = group(&report, "fixture::typo");
    assert_eq!(suggested(group), Some(("fixture::type", Change::Renamed)));
    assert_eq!(
        group
            .mentions
            .iter()
            .map(|mention| mention.replacement.as_deref())
            .collect::<Vec<_>>(),
        [Some("`fixture::r#type`"), Some("`fixture::r#type`")]
    );
}
