//! `refquiem: allow(...)` suppression markers and the markers that suppress nothing.

mod common;

use common::Repo;
use refquiem::{Options, Range, UnusedMarker};

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

#[test]
fn marker_text_inside_markdown_code_is_not_a_suppression() {
    let repo = removed();
    repo.write(
        "README.md",
        "`<!-- refquiem: allow(Gone) -->`\n`fixture::Gone`\n\n```markdown\n<!-- refquiem: allow(Other) -->\nfixture::Other\n```\n",
    );
    let report = refquiem::run(&Options::new(repo.path().to_path_buf())).unwrap();
    assert_eq!(
        reported(&report),
        [("README.md".to_owned(), 2), ("README.md".to_owned(), 6)]
    );
    assert_eq!(report.unused_markers, []);
}

#[test]
fn a_nested_html_comment_preserves_mentions_after_the_comment() {
    let repo = removed();
    repo.write("README.md", "<!-- a <!-- b -->\n\nUse `fixture::Gone`.\n");
    repo.commit("drop the structures");

    let report = refquiem::run(&Options::new(repo.path().to_path_buf())).unwrap();
    assert_eq!(reported(&report), [("README.md".to_owned(), 3)]);
    assert_eq!(report.groups[0].path, "fixture::Gone");
    assert_eq!(report.unused_markers, []);
}

/// The commits that add `fixture::Gone`, drop it beside the notes that mention it under a
/// marker, and change nothing about either.
fn dropped_then_untouched() -> (Repo, [String; 3]) {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub struct Gone;\n");
    let added = repo.commit("add Gone");
    repo.write("src/lib.rs", "pub struct Kept;\n");
    repo.write(
        "src/notes.rs",
        "// refquiem: allow(Gone)\n// `fixture::Gone`\n\n// refquiem: allow(Gone, Missing)\npub struct Other;\n",
    );
    let dropped = repo.commit("drop Gone");
    repo.write("README.md", "Unrelated.\n");
    let untouched = repo.commit("add a readme");
    (repo, [added, dropped, untouched])
}

/// The scan of `range` in `repo`.
fn ranged(repo: &Repo, range: Range) -> Options {
    let mut options = Options::new(repo.path().to_path_buf());
    options.range = range;
    options
}

fn base(base: &str, head: &str) -> Range {
    Range::Base {
        base: base.to_owned(),
        head: Some(head.to_owned()),
    }
}

#[test]
fn every_range_gives_a_marker_the_verdict_of_the_full_history() {
    let (repo, [added, dropped, untouched]) = dropped_then_untouched();
    let ranges = [
        Range::Full,
        base(&added, &dropped),
        base(&dropped, &untouched),
        Range::Since(dropped),
    ];
    for range in ranges {
        let options = ranged(&repo, range);
        let report = refquiem::run(&options).unwrap();
        assert_eq!(reported(&report), [], "{:?}", options.range);
        assert_eq!(
            report.unused_markers,
            [
                unused("src/notes.rs", 4, 20, "Gone"),
                unused("src/notes.rs", 4, 26, "Missing")
            ],
            "{:?}",
            options.range
        );
    }
}

#[test]
fn the_full_history_judging_markers_starts_at_the_config_since() {
    let (repo, [_, dropped, untouched]) = dropped_then_untouched();
    repo.write(".refquiem.toml", &format!("since = \"{untouched}\"\n"));
    for range in [Range::Full, base(&dropped, &untouched)] {
        let options = ranged(&repo, range);
        let report = refquiem::run(&options).unwrap();
        assert_eq!(
            report.unused_markers,
            [
                unused("src/notes.rs", 1, 20, "Gone"),
                unused("src/notes.rs", 4, 20, "Gone"),
                unused("src/notes.rs", 4, 26, "Missing")
            ],
            "{:?}",
            options.range
        );
    }
}
