//! Mentions in the comments of `.sql` files and in their `COMMENT ON … IS` documentation.

mod common;
#[path = "common/found.rs"]
mod found;

use common::Repo;
use found::found;
use refquiem::{Context, Kind, Source};

const SQL_TEXT: Context = Context {
    source: Source::SqlComment,
    kind: Kind::Text,
};

/// A repository whose `fixture::gone_fn` was removed, `src/lib.rs` keeping `kept_fn`.
fn gone() -> Repo {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone_fn() {}\npub fn kept_fn() {}\n");
    repo.commit("start");
    repo.write("src/lib.rs", "pub fn kept_fn() {}\n");
    repo.commit("drop gone_fn");
    repo
}

/// `(file, line, column, context, text)` of every mention.
fn mentions(repo: &Repo) -> Vec<(String, usize, usize, Context, String)> {
    found(repo)
        .into_iter()
        .map(|(_, file, line, column, context, text)| (file, line, column, context, text))
        .collect()
}

fn at(
    file: &str,
    line: usize,
    column: usize,
    text: &str,
) -> (String, usize, usize, Context, String) {
    (file.to_owned(), line, column, SQL_TEXT, text.to_owned())
}

#[test]
fn sql_comments_and_comments_inside_bodies_are_scanned_and_strings_are_not() {
    let repo = gone();
    repo.write(
        "postgres/schema.sql",
        "-- Apply fixture::gone_fn first.
CREATE TABLE t (a TEXT DEFAULT 'x -- fixture::gone_fn');
/* block fixture::gone_fn */
CREATE FUNCTION f() RETURNS INT LANGUAGE sql AS $body$
    SELECT '-- fixture::gone_fn' -- body fixture::gone_fn
$body$;
",
    );
    repo.commit("schema");
    assert_eq!(
        mentions(&repo),
        [
            at("postgres/schema.sql", 1, 10, "fixture::gone_fn"),
            at("postgres/schema.sql", 3, 10, "fixture::gone_fn"),
            at("postgres/schema.sql", 5, 42, "fixture::gone_fn"),
        ]
    );
}

#[test]
fn a_comment_run_that_is_sql_is_skipped_unless_commented_code_is_scanned() {
    let repo = gone();
    repo.write(
        "schema.sql",
        "-- CREATE TABLE gone_fn (id INT);
-- SELECT gone_fn FROM kept;
CREATE TABLE kept (id INT);

-- Older deployments still hold gone_fn:
-- CREATE TABLE gone_fn (id INT);
",
    );
    repo.commit("schema");
    assert_eq!(
        mentions(&repo),
        [
            at("schema.sql", 5, 33, "gone_fn"),
            at("schema.sql", 6, 17, "gone_fn"),
        ]
    );

    repo.write(".refquiem.toml", "scan-commented-code = true\n");
    repo.commit("scan code");
    assert_eq!(mentions(&repo).len(), 4);
}

#[test]
fn a_file_sqlparser_refuses_yields_no_mentions() {
    let repo = gone();
    repo.write(
        "schema.sql",
        "-- Apply fixture::gone_fn first.\nCREATE TABLE ( ;\n",
    );
    repo.commit("broken");
    assert_eq!(mentions(&repo), []);
}

#[test]
fn comment_on_documentation_is_scanned() {
    let repo = gone();
    repo.write(
        "postgres/schema.sql",
        "CREATE TABLE t (a INT);\nCOMMENT ON TABLE t IS 'Filled by fixture::gone_fn.';\n",
    );
    repo.commit("documented");
    assert_eq!(
        mentions(&repo),
        [at("postgres/schema.sql", 2, 34, "fixture::gone_fn")]
    );
}

#[test]
fn markers_in_sql_comments_suppress_and_unused_ones_are_reported() {
    let repo = gone();
    repo.write(
        "schema.sql",
        "-- refquiem: allow(fixture::gone_fn)\n-- Apply fixture::gone_fn first.\nCREATE TABLE t (a INT);\n\n-- refquiem: allow(Nothing)\nCREATE TABLE u (a INT);\n",
    );
    repo.commit("markers");
    let report = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap();
    assert_eq!(report.groups.len(), 0);
    let unused: Vec<(&str, usize, &str)> = report
        .unused_markers
        .iter()
        .map(|m| (m.file.as_str(), m.line, m.name.as_str()))
        .collect();
    assert_eq!(unused, [("schema.sql", 5, "Nothing")]);
}

#[test]
fn a_sql_files_dialect_vocabulary_claims_its_words_there_alone() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub fn set_config() {}\npub fn json_extract() {}\npub fn kept() {}\n",
    );
    repo.commit("start");
    repo.write("src/lib.rs", "pub fn kept() {}\n");
    repo.write(
        "postgres/up.sql",
        "-- Calls set_config and json_extract.\nCREATE TABLE t (a INT);\n",
    );
    repo.write(
        "sqlite/up.sql",
        "-- Calls set_config and json_extract.\nCREATE TABLE t (a INT);\n",
    );
    repo.write("README.md", "Calls set_config and json_extract.\n");
    repo.commit("drop both");
    let rows: Vec<(String, String)> = found(&repo)
        .into_iter()
        .map(|(path, file, ..)| (file, path))
        .collect();
    assert_eq!(
        rows,
        [
            ("README.md".to_owned(), "fixture::set_config".to_owned()),
            ("README.md".to_owned(), "fixture::json_extract".to_owned()),
            (
                "postgres/up.sql".to_owned(),
                "fixture::json_extract".to_owned()
            ),
            ("sqlite/up.sql".to_owned(), "fixture::set_config".to_owned()),
        ]
    );
}

#[test]
fn a_sql_comment_reads_as_sql_comment_in_reports() {
    assert_eq!(SQL_TEXT.describe(), "SQL comment");
    assert_eq!(
        serde_json::to_value(SQL_TEXT).unwrap(),
        serde_json::json!({"source": "sql_comment", "kind": "text"})
    );
}
