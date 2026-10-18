//! End-to-end behaviour of the stale reference scan over real git histories.

mod common;

use std::path::PathBuf;

use common::Repo;
use refquiem::{Context, ItemKind, Kind, Range, RemovedIn, Source};

/// Scanning a fixture repository.
trait Scan {
    /// Options scanning the full history.
    fn options(&self) -> refquiem::Options;
    /// Runs refquiem over the full history against the working tree.
    fn run(&self) -> refquiem::Report;
    /// Where refquiem caches parsed files for this repository.
    fn cache_dir(&self) -> PathBuf;
}

impl Scan for Repo {
    fn options(&self) -> refquiem::Options {
        refquiem::Options::new(self.path().to_path_buf())
    }

    fn run(&self) -> refquiem::Report {
        refquiem::run(&self.options()).unwrap()
    }

    fn cache_dir(&self) -> PathBuf {
        self.path().join("target").join("refquiem")
    }
}

/// `(removed path, mention text, file, line, column)` for every mention, sorted.
fn mentions(report: &refquiem::Report) -> Vec<(String, String, String, usize, usize)> {
    let mut out: Vec<_> = report
        .groups
        .iter()
        .flat_map(|group| {
            group.mentions.iter().map(|mention| {
                (
                    group.path.clone(),
                    mention.text.clone(),
                    mention.file.clone(),
                    mention.line,
                    mention.column,
                )
            })
        })
        .collect();
    out.sort();
    out
}

/// The removed paths that still have mentions, sorted.
fn removed(report: &refquiem::Report) -> Vec<String> {
    let mut out: Vec<String> = report.groups.iter().map(|g| g.path.clone()).collect();
    out.sort();
    out
}

const PARSER: &str = "pub struct Parser;\n\nimpl Parser {\n    pub fn parse_expr(&self) {}\n    pub fn parse_item(&self) {}\n}\n";

#[test]
fn removed_method_mentioned_in_a_doc_comment_is_reported() {
    let repo = Repo::new();
    repo.write("src/lib.rs", PARSER);
    repo.commit("add parser");
    repo.write(
        "src/lib.rs",
        "pub struct Parser;\n\nimpl Parser {\n    /// Replaces `Parser::parse_expr`.\n    pub fn parse_item(&self) {}\n}\n",
    );
    let deleting = repo.commit("drop parse_expr");

    let report = repo.run();

    assert_eq!(report.groups.len(), 1);
    let group = &report.groups[0];
    assert_eq!(group.path, "fixture::Parser::parse_expr");
    assert_eq!(group.kind, ItemKind::Method);
    assert_eq!(group.removed_in, Some(RemovedIn::Commit(deleting)));
    assert_eq!(group.mentions.len(), 1);
    let mention = &group.mentions[0];
    assert_eq!(mention.file, "src/lib.rs");
    assert_eq!((mention.line, mention.column), (4, 19));
    assert_eq!(mention.text, "Parser::parse_expr");
    assert_eq!(
        mention.context,
        Context {
            source: Source::DocComment,
            kind: Kind::CodeSpan
        }
    );
}

#[test]
fn mentions_of_items_that_still_exist_are_not_reported() {
    let repo = Repo::new();
    repo.write("src/lib.rs", PARSER);
    repo.commit("add parser");
    repo.write(
        "README.md",
        "Use `fixture::Parser::parse_expr` or `Parser::parse_item`.\n",
    );
    repo.commit("docs");

    assert_eq!(removed(&repo.run()), [] as [&str; 0]);
}

#[test]
fn a_moved_item_is_stale_only_under_its_old_module() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub mod a {\n    pub struct Widget;\n}\n");
    repo.commit("widget in a");
    repo.write("src/lib.rs", "pub mod b {\n    pub struct Widget;\n}\n");
    repo.write("README.md", "Old `a::Widget`, new `b::Widget`.\n");
    repo.commit("move widget to b");

    assert_eq!(
        mentions(&repo.run()),
        [(
            "fixture::a::Widget".to_owned(),
            "a::Widget".to_owned(),
            "README.md".to_owned(),
            1,
            6
        )]
    );
}

#[test]
fn an_item_added_and_removed_inside_the_range_is_reported() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn kept() {}\n");
    repo.commit("start");
    repo.write("src/lib.rs", "pub fn kept() {}\npub fn temporary() {}\n");
    repo.write("README.md", "Call `fixture::temporary` first.\n");
    repo.commit("add temporary");
    repo.write("src/lib.rs", "pub fn kept() {}\n");
    let deleting = repo.commit("drop temporary");

    let report = repo.run();

    assert_eq!(removed(&report), ["fixture::temporary"]);
    assert_eq!(report.groups[0].kind, ItemKind::Function);
    assert_eq!(
        report.groups[0].removed_in,
        Some(RemovedIn::Commit(deleting))
    );
}

#[test]
fn an_item_moved_into_an_item_macro_stays_alive() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub mod flags {\n    pub struct Flags;\n    pub fn helper() {}\n}\n",
    );
    repo.commit("plain items");
    repo.write(
        "src/lib.rs",
        "macro_rules! declare {\n    ($name:ident) => {\n        pub struct $name;\n    };\n}\n\npub mod flags {\n    declare!(Flags);\n    fn check() {\n        assert!(helper());\n    }\n}\n",
    );
    repo.write("README.md", "See `flags::Flags` and `flags::helper`.\n");
    repo.commit("generate flags");

    assert_eq!(removed(&repo.run()), ["fixture::flags::helper"]);
}

#[test]
fn a_macro_rules_body_keeps_its_generated_names_alive_where_invoked() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub mod api {\n    pub fn generated() {}\n}\n",
    );
    repo.commit("hand written");
    repo.write(
        "src/lib.rs",
        "macro_rules! make {\n    () => {\n        pub fn generated() {}\n    };\n}\n\npub mod api {\n    make!();\n}\n",
    );
    repo.write("README.md", "Call `api::generated`.\n");
    repo.commit("generated");

    assert_eq!(removed(&repo.run()), [] as [&str; 0]);
}

#[test]
fn a_removed_module_covers_paths_below_it() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub mod old {\n    pub fn f() {}\n}\npub fn kept() {}\n",
    );
    repo.commit("old module");
    repo.write("src/lib.rs", "pub fn kept() {}\n");
    repo.write("README.md", "Use `old::f` or `fixture::old::Unknown`.\n");
    repo.commit("drop old");

    let found = mentions(&repo.run());

    assert_eq!(
        found,
        [
            (
                "fixture::old".to_owned(),
                "fixture::old::Unknown".to_owned(),
                "README.md".to_owned(),
                1,
                18
            ),
            (
                "fixture::old::f".to_owned(),
                "old::f".to_owned(),
                "README.md".to_owned(),
                1,
                6
            ),
        ]
    );
}

#[test]
fn the_commit_that_removed_an_item_is_named_rather_than_the_merge() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn doomed() {}\npub fn kept() {}\n");
    repo.commit("start");
    repo.git(&["checkout", "-q", "-b", "topic"]);
    repo.write("src/lib.rs", "pub fn kept() {}\n");
    let deleting = repo.commit("drop doomed");
    repo.git(&["checkout", "-q", "main"]);
    repo.write("README.md", "Call `fixture::doomed`.\n");
    repo.commit("docs on main");
    repo.git(&[
        "merge",
        "-q",
        "--no-ff",
        "-m",
        "Merge pull request #1",
        "topic",
    ]);

    let report = repo.run();

    assert_eq!(removed(&report), ["fixture::doomed"]);
    assert_eq!(
        report.groups[0].removed_in,
        Some(RemovedIn::Commit(deleting))
    );
}

#[test]
fn a_removal_by_uncommitted_changes_is_attributed_to_the_working_tree() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "/// Unlike `crate::gone`.\npub fn kept() {}\npub fn gone() {}\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "/// Unlike `crate::gone`.\npub fn kept() {}\n",
    );

    let report = repo.run();

    assert_eq!(removed(&report), ["fixture::gone"]);
    assert_eq!(report.groups[0].removed_in, Some(RemovedIn::WorkingTree));
}

#[test]
fn crate_self_and_super_resolve_against_the_commenting_file() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub mod outer;\npub fn top() {}\n");
    repo.write("src/outer.rs", "pub mod inner;\npub fn sibling() {}\n");
    repo.write("src/outer/inner.rs", "pub fn leaf() {}\n");
    repo.commit("tree");
    repo.write("src/lib.rs", "pub mod outer;\n");
    repo.write("src/outer.rs", "pub mod inner;\n");
    repo.write(
        "src/outer/inner.rs",
        "// See crate::top, super::sibling and self::leaf.\n",
    );
    repo.commit("drop everything");

    let report = repo.run();

    assert_eq!(
        removed(&report),
        [
            "fixture::outer::inner::leaf",
            "fixture::outer::sibling",
            "fixture::top"
        ]
    );
    assert!(
        report
            .groups
            .iter()
            .flat_map(|g| &g.mentions)
            .all(|m| m.context.source == Source::Comment)
    );
}

#[test]
fn a_base_range_ignores_removals_before_the_base() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn early() {}\npub fn late() {}\n");
    repo.commit("both");
    repo.write("src/lib.rs", "pub fn late() {}\n");
    let base = repo.commit("drop early");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "`fixture::early` and `fixture::late`.\n");
    let head = repo.commit("drop late");

    let mut options = repo.options();
    options.range = Range::Base {
        base: base.clone(),
        head: Some(head),
    };
    assert_eq!(
        removed(&refquiem::run(&options).unwrap()),
        ["fixture::late"]
    );

    options.range = Range::Since(base);
    assert_eq!(
        removed(&refquiem::run(&options).unwrap()),
        ["fixture::late"]
    );
}

#[test]
fn an_explicit_head_commit_is_compared_instead_of_the_working_tree() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn present() {}\n");
    let base = repo.commit("start");
    repo.write("README.md", "`fixture::present`.\n");
    let head = repo.commit("docs");
    repo.write("src/lib.rs", "");

    let mut options = repo.options();
    options.range = Range::Base {
        base,
        head: Some(head),
    };
    assert_eq!(removed(&refquiem::run(&options).unwrap()), [] as [&str; 0]);
}

#[test]
fn a_type_alias_keeps_the_members_of_its_target_alive() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct Located;\nimpl Located {\n    pub fn new() -> Self {\n        Located\n    }\n}\n",
    );
    repo.commit("located");
    repo.write(
        "src/lib.rs",
        "pub struct Spanned;\nimpl Spanned {\n    pub fn new() -> Self {\n        Spanned\n    }\n}\n#[deprecated]\npub type Located = Spanned;\npub type Outside = std::string::String;\n",
    );
    repo.write("README.md", "`Located::new` still works.\n");
    repo.commit("rename behind an alias");

    assert_eq!(removed(&repo.run()), [] as [&str; 0]);
}

#[test]
fn methods_reached_through_deref_stay_alive_on_the_dereferencing_type() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct Raw;\npub struct Container;\nimpl Container {\n    pub fn stderr(&self) {}\n    pub fn gone(&self) {}\n}\n",
    );
    repo.commit("methods on the container");
    repo.write(
        "src/lib.rs",
        "pub struct Raw;\nimpl Raw {\n    pub fn stderr(&self) {}\n}\npub struct Container(Raw);\nimpl core::ops::Deref for Container {\n    type Target = Raw;\n    fn deref(&self) -> &Raw {\n        &self.0\n    }\n}\n",
    );
    repo.write("README.md", "`Container::stderr` and `Container::gone`.\n");
    repo.commit("stderr moves behind Deref");

    assert_eq!(removed(&repo.run()), ["fixture::Container::gone"]);
}

#[test]
fn reexports_keep_an_old_path_alive() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub mod a {\n    pub struct Foo;\n    pub struct Bar;\n}\n",
    );
    repo.commit("a");
    repo.write(
        "src/lib.rs",
        "pub mod b {\n    pub struct Foo;\n}\npub mod c {\n    pub struct Bar;\n}\npub mod a {\n    pub use crate::b::Foo;\n    pub use crate::c::*;\n}\n",
    );
    repo.write("README.md", "`a::Foo` and `a::Bar`.\n");
    repo.commit("move behind re-exports");

    assert_eq!(removed(&repo.run()), [] as [&str; 0]);
}

#[test]
fn a_longer_identifier_sharing_a_prefix_is_not_a_mention() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub struct Foo;\npub struct FooBar;\n");
    repo.commit("both");
    repo.write("src/lib.rs", "pub struct FooBar;\n");
    repo.write("README.md", "`fixture::FooBar` stays.\n");
    repo.commit("drop Foo");

    assert_eq!(removed(&repo.run()), [] as [&str; 0]);
}

#[test]
fn an_item_removed_from_one_cfg_branch_only_stays_alive() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "#[cfg(unix)]\npub fn native() {}\n#[cfg(windows)]\npub fn native() {}\n",
    );
    repo.commit("both platforms");
    repo.write("src/lib.rs", "#[cfg(unix)]\npub fn native() {}\n");
    repo.write("README.md", "`fixture::native`.\n");
    repo.commit("drop windows");

    assert_eq!(removed(&repo.run()), [] as [&str; 0]);
}

#[test]
fn workspace_members_and_path_attributes_are_followed() {
    let repo = Repo::new();
    repo.write("Cargo.toml", "[workspace]\nmembers = [\"crates/*\"]\n");
    repo.write(
        "crates/alpha-core/Cargo.toml",
        "[package]\nname = \"alpha-core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write(
        "crates/alpha-core/src/lib.rs",
        "#[path = \"platform/linux.rs\"]\npub mod platform;\n",
    );
    repo.write(
        "crates/alpha-core/src/platform/linux.rs",
        "pub fn open() {}\npub fn close() {}\n",
    );
    repo.commit("workspace");
    repo.write(
        "crates/alpha-core/src/platform/linux.rs",
        "pub fn open() {}\n",
    );
    repo.write(
        "README.md",
        "`alpha_core::platform::open` and `alpha_core::platform::close`.\n",
    );
    repo.commit("drop close");

    assert_eq!(removed(&repo.run()), ["alpha_core::platform::close"]);
}

#[test]
fn a_file_that_does_not_parse_at_head_is_an_error() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn fine() {}\n");
    repo.commit("start");
    repo.write("src/lib.rs", "pub fn broken( {}\n");

    let error = refquiem::run(&repo.options()).unwrap_err();

    assert!(error.to_string().contains("src/lib.rs"), "{error}");
}

#[test]
fn a_corrupt_cache_is_rebuilt_with_the_same_findings() {
    let repo = Repo::new();
    repo.write("src/lib.rs", PARSER);
    repo.commit("add parser");
    repo.write("src/lib.rs", "pub struct Parser;\n");
    repo.write("README.md", "`Parser::parse_expr`.\n");
    repo.commit("drop methods");

    let first = mentions(&repo.run());
    for entry in walk(&repo.cache_dir()) {
        std::fs::write(entry, b"{ not json").unwrap();
    }
    let second = mentions(&repo.run());

    assert_eq!(first.len(), 1);
    assert_eq!(first, second);
}

#[test]
fn a_cache_entry_that_no_longer_decodes_is_parsed_again() {
    let repo = Repo::new();
    repo.write("src/lib.rs", PARSER);
    repo.commit("add parser");
    repo.write("src/lib.rs", "pub struct Parser;\n");
    repo.write("README.md", "`Parser::parse_expr`.\n");
    repo.commit("drop methods");

    let first = mentions(&repo.run());
    let parsed = walk(&repo.cache_dir()).into_iter().filter(|path| {
        path.file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("files-"))
    });
    for path in parsed {
        let text = std::fs::read_to_string(&path).unwrap();
        let mut entries: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&text).unwrap();
        assert!(!entries.is_empty());
        for entry in entries.values_mut() {
            *entry = serde_json::Value::Null;
        }
        std::fs::write(&path, serde_json::to_string(&entries).unwrap()).unwrap();
    }
    let second = mentions(&repo.run());

    assert_eq!(first.len(), 1);
    assert_eq!(first, second);
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    assert!(!files.is_empty(), "no cache files under {}", dir.display());
    files
}

#[test]
fn files_inside_the_git_directory_are_never_read() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub struct Gone;\n");
    repo.commit("start");
    repo.write("src/lib.rs", "");
    repo.commit("drop Gone");
    std::fs::write(repo.path().join(".git/notes.md"), "`fixture::Gone`\n").unwrap();

    let report = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap();
    assert!(report.is_clean(), "{}", report.human());
}

#[test]
fn a_bare_name_goes_to_the_latest_of_the_removed_items_it_ends() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub mod client {\n    pub trait Connect {}\n}\npub enum Method {\n    Connect,\n    Get,\n}\n",
    );
    repo.commit("both");
    repo.write(
        "src/lib.rs",
        "pub mod client {\n    pub trait Connect {}\n}\npub enum Method {\n    Get,\n}\n",
    );
    repo.commit("drop the variant");
    repo.write("src/lib.rs", "pub enum Method {\n    Get,\n}\n");
    repo.write("README.md", "The `Connect` trait.\n");
    repo.commit("drop the trait");

    assert_eq!(removed(&repo.run()), ["fixture::client::Connect"]);
}

#[test]
fn a_bare_name_goes_to_the_item_of_the_mentioning_crate_first() {
    let repo = Repo::new();
    repo.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"alpha\", \"beta\"]\n",
    );
    for member in ["alpha", "beta"] {
        repo.write(
            &format!("{member}/Cargo.toml"),
            &format!("[package]\nname = \"{member}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"),
        );
        repo.write(&format!("{member}/src/lib.rs"), "pub struct Shared;\n");
    }
    repo.commit("both share");
    repo.write("beta/src/lib.rs", "/// `Shared`\npub struct Kept;\n");
    repo.commit("beta drops Shared");
    repo.write("alpha/src/lib.rs", "");
    repo.write("beta/README.md", "`Shared`\n");
    repo.commit("alpha drops Shared");

    let groups: Vec<(String, Vec<String>)> = repo
        .run()
        .groups
        .iter()
        .map(|g| {
            (
                g.path.clone(),
                g.mentions.iter().map(|m| m.file.clone()).collect(),
            )
        })
        .collect();
    assert_eq!(
        groups,
        [(
            "beta::Shared".to_owned(),
            vec!["beta/README.md".to_owned(), "beta/src/lib.rs".to_owned()]
        )]
    );
}
