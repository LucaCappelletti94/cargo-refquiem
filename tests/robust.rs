//! Inputs the crates refquiem reads with fail on, which a scan must survive.

mod common;

use std::fmt::Write;

use common::Repo;
use refquiem::Options;

#[test]
fn deeply_nested_rust_does_not_overflow_the_stack() {
    let repo = Repo::new();
    let depth = 5000;
    repo.write(
        "src/lib.rs",
        &format!(
            "pub fn f() -> u8 {{\n    {}1{}\n}}\n",
            "(".repeat(depth),
            ")".repeat(depth)
        ),
    );
    repo.commit("deep");

    let report = refquiem::run(&Options::new(repo.path().to_path_buf())).unwrap();
    assert!(report.is_clean(), "{}", report.human());
}

#[test]
fn modules_reexporting_each_other_in_a_cycle_are_indexed() {
    let repo = Repo::new();
    let chapters = 12;
    let mut tour = String::new();
    for i in 0..chapters {
        writeln!(tour, "pub mod c{i};").unwrap();
        let previous = match i {
            0 => String::new(),
            _ => format!("pub use super::c{} as previous;\n", i - 1),
        };
        let next = match i + 1 {
            n if n == chapters => String::new(),
            n => format!("pub use super::c{n} as next;\n"),
        };
        repo.write(
            &format!("src/tour/c{i}.rs"),
            &format!(
                "pub fn step{i}() {{}}\npub use crate::tour as table_of_contents;\n{previous}{next}"
            ),
        );
    }
    repo.write("src/tour/mod.rs", &tour);
    repo.write("src/lib.rs", "pub mod tour;\n");
    repo.commit("tour");
    repo.write(
        "src/tour/c0.rs",
        "pub use crate::tour as table_of_contents;\npub use super::c1 as next;\n",
    );
    repo.write("README.md", "`fixture::tour::c0::step0`\n");
    repo.commit("drop step0");

    let report = refquiem::run(&Options::new(repo.path().to_path_buf())).unwrap();
    let groups: Vec<&str> = report.groups.iter().map(|g| g.path.as_str()).collect();
    assert_eq!(groups, ["fixture::tour::c0::step0"]);
}

#[test]
fn markdown_the_parser_panics_on_yields_no_mentions_and_the_scan_goes_on() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub struct Gone;\n");
    repo.commit("start");
    repo.write("src/lib.rs", "");
    // pulldown-cmark 0.13.4 panics on this text, https://github.com/pulldown-cmark/pulldown-cmark/issues/1129
    repo.write("README.md", "-\t[\0]:u\n\x0b");
    repo.write("NOTES.md", "`fixture::Gone`\n");
    repo.commit("drop Gone");

    let report = refquiem::run(&Options::new(repo.path().to_path_buf())).unwrap();
    let mentions: Vec<(&str, &str)> = report
        .groups
        .iter()
        .flat_map(|g| {
            g.mentions
                .iter()
                .map(|m| (g.path.as_str(), m.file.as_str()))
        })
        .collect();
    assert_eq!(mentions, [("fixture::Gone", "NOTES.md")]);
}
