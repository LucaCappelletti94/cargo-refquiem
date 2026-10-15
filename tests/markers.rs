//! `refquiem: allow(...)` suppression markers and the markers that suppress nothing.

mod common;

use common::Repo;
use refquiem::{Options, UnusedMarker};

/// A repository whose `fixture::Gone` and `fixture::Other` were removed.
fn removed() -> Repo {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub struct Gone;\npub struct Other;\n");
    repo.commit("start");
    repo.write("src/lib.rs", "");
    repo
}

/// `(file, line)` of every mention still reported, sorted.
fn reported(report: &refquiem::Report) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = report
        .groups
        .iter()
        .flat_map(|g| g.mentions.iter().map(|m| (m.file.clone(), m.line)))
        .collect();
    out.sort();
    out
}

fn unused(file: &str, line: usize, column: usize, name: &str) -> UnusedMarker {
    UnusedMarker {
        file: file.to_owned(),
        line,
        column,
        name: name.to_owned(),
    }
}

#[test]
fn a_marker_covers_its_own_line_and_the_next() {
    let repo = removed();
    repo.write(
        "src/notes.rs",
        "// refquiem: allow(Gone)\n// `fixture::Gone`\n// `fixture::Gone`\n/// `fixture::Gone` refquiem: allow(fixture::Gone)\npub struct Kept;\n",
    );
    repo.write(
        "README.md",
        "<!-- refquiem: allow(Gone, Other) -->\n`Gone` and `fixture::Other`\n\n`fixture::Gone`\n\n<!-- refquiem: allow(Gone) -->\n`fixture::Gone`\n",
    );
    repo.commit("drop them");

    let report = refquiem::run(&Options::new(repo.path().to_path_buf())).unwrap();
    assert_eq!(
        reported(&report),
        [("README.md".to_owned(), 4), ("src/notes.rs".to_owned(), 3)]
    );
    assert_eq!(report.unused_markers, []);
}

#[test]
fn a_marker_name_that_suppresses_nothing_is_reported() {
    let repo = removed();
    repo.write(
        "src/notes.rs",
        "// refquiem: allow(Gone, Missing)\n// `fixture::Gone`\n\n// refquiem: allow(Other)\npub struct Kept;\n",
    );
    repo.commit("drop them");

    let report = refquiem::run(&Options::new(repo.path().to_path_buf())).unwrap();
    assert_eq!(reported(&report), []);
    assert_eq!(
        report.unused_markers,
        [
            unused("src/notes.rs", 1, 26, "Missing"),
            unused("src/notes.rs", 4, 20, "Other")
        ]
    );
    assert!(!report.is_clean());
    assert!(
        report
            .human()
            .contains("src/notes.rs:4:20  refquiem: allow(Other) suppresses nothing"),
        "{}",
        report.human()
    );
}

#[test]
fn a_marker_in_a_string_literal_or_plain_markdown_text_is_not_a_marker() {
    let repo = removed();
    repo.write(
        "src/lib.rs",
        "pub const NOTE: &str = \"refquiem: allow(Gone)\";\n// `fixture::Gone`\n",
    );
    repo.write("README.md", "refquiem: allow(Gone)\n`fixture::Gone`\n");
    repo.commit("drop them");

    let report = refquiem::run(&Options::new(repo.path().to_path_buf())).unwrap();
    assert_eq!(
        reported(&report),
        [("README.md".to_owned(), 2), ("src/lib.rs".to_owned(), 2)]
    );
    assert_eq!(report.unused_markers, []);
}

#[test]
fn a_marker_holding_anything_but_names_or_paths_is_not_a_marker() {
    let repo = removed();
    repo.write(
        "src/lib.rs",
        "//! Writes `refquiem: allow(...)` markers.\n// refquiem: allow(Gone, the rest)\n// `fixture::Gone`\n",
    );
    repo.write(
        "README.md",
        "<!-- refquiem: allow(...) -->\n`fixture::Gone`\n",
    );
    repo.commit("drop them");

    let report = refquiem::run(&Options::new(repo.path().to_path_buf())).unwrap();
    assert_eq!(
        reported(&report),
        [("README.md".to_owned(), 2), ("src/lib.rs".to_owned(), 3)]
    );
    assert_eq!(report.unused_markers, []);
}
