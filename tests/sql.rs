//! Names SQL declares at head, which keep a removed Rust item of the same name alive, and the
//! SQL that could not be read.

mod common;

use std::fmt::Write as _;

use common::Repo;

/// The scan of `repo` at its working tree.
fn scan(repo: &Repo) -> refquiem::Report {
    refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap()
}

/// The removed paths a scan of `repo` reports.
fn reported(repo: &Repo) -> Vec<String> {
    scan(repo)
        .groups
        .into_iter()
        .map(|group| group.path)
        .collect()
}

/// `(file, line, column)` of every piece of SQL the scan of `repo` could not read.
fn refused(report: &refquiem::Report) -> Vec<(String, usize, usize)> {
    report
        .refused_sql
        .iter()
        .map(|refused| (refused.file.clone(), refused.line, refused.column))
        .collect()
}

/// A repository whose functions `names` were removed while `README.md` names each of them.
fn removed_functions(names: &[&str]) -> Repo {
    let repo = Repo::new();
    let mut lib = String::new();
    let mut readme = String::new();
    for name in names {
        let _ = writeln!(lib, "pub fn {name}() {{}}");
        let _ = writeln!(readme, "Call `{name}`.");
    }
    repo.write("src/lib.rs", &lib);
    repo.commit("start");
    repo.write("src/lib.rs", "");
    repo.write("README.md", &readme);
    repo.commit("drop them");
    repo
}

fn paths(names: &[&str]) -> Vec<String> {
    names
        .iter()
        .map(|name| format!("fixture::{name}"))
        .collect()
}

#[test]
fn a_column_sql_declares_keeps_the_name_of_a_renamed_field_alive() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct Photo {\n    pub content_state: u8,\n}\n",
    );
    repo.commit("start");
    repo.write("src/lib.rs", "pub struct Photo {\n    pub state: u8,\n}\n");
    repo.write("README.md", "Rows carry `content_state`.\n");
    repo.commit("rename the field");
    assert_eq!(reported(&repo), ["fixture::Photo::content_state"]);

    repo.write(
        "schema.sql",
        "CREATE TABLE photos (content_state TEXT NOT NULL);\n",
    );
    repo.commit("the column");
    assert_eq!(reported(&repo), Vec::<String>::new());
    assert_eq!(refused(&scan(&repo)), []);
}

#[test]
fn every_declaring_statement_keeps_its_names_alive() {
    let declared = [
        "app_schema",
        "pg_trgm_ext",
        "mood_kind",
        "happy_label",
        "point_pair",
        "left_point",
        "positive_amount",
        "order_seq",
        "orders",
        "order_id",
        "orders_pkey",
        "orders_amount_check",
        "order_view",
        "view_total",
        "orders_by_id",
        "order_total",
        "total_param",
        "orders_audit",
        "order_reader",
        "orders_policy",
        "placed_at",
        "placed_on",
        "purchases",
    ];
    let mut names = declared.to_vec();
    names.push("not_declared");
    let repo = removed_functions(&names);
    repo.write(
        "db/postgres/schema.sql",
        "CREATE SCHEMA app_schema;
CREATE EXTENSION pg_trgm_ext;
CREATE TYPE mood_kind AS ENUM ('happy_label');
CREATE TYPE point_pair AS (left_point INT, right_point INT);
CREATE DOMAIN positive_amount AS INT CHECK (VALUE > 0);
CREATE SEQUENCE order_seq;
CREATE TABLE app_schema.orders (
    order_id INT,
    amount INT,
    CONSTRAINT orders_pkey PRIMARY KEY (order_id),
    CONSTRAINT orders_amount_check CHECK (amount > 0)
);
CREATE VIEW order_view (view_total) AS SELECT order_id FROM app_schema.orders;
CREATE INDEX orders_by_id ON app_schema.orders (order_id);
CREATE FUNCTION order_total(total_param INT) RETURNS INT LANGUAGE sql AS $$ SELECT total_param $$;
CREATE TRIGGER orders_audit AFTER INSERT ON app_schema.orders FOR EACH ROW EXECUTE FUNCTION order_total();
CREATE ROLE order_reader;
CREATE POLICY orders_policy ON app_schema.orders USING (true);
ALTER TABLE app_schema.orders ADD COLUMN placed_at INT;
ALTER TABLE app_schema.orders RENAME COLUMN placed_at TO placed_on;
ALTER TABLE app_schema.orders RENAME TO purchases;
",
    );
    repo.commit("schema");

    let report = scan(&repo);
    assert_eq!(refused(&report), []);
    let reported: Vec<String> = report.groups.into_iter().map(|group| group.path).collect();
    assert_eq!(reported, paths(&["not_declared"]));
}

#[test]
fn an_unquoted_name_is_alive_as_written_and_lowercased_and_a_quoted_one_as_written() {
    let repo = removed_functions(&["Photos", "photos", "Caption", "caption", "Quoted", "quoted"]);
    repo.write(
        "schema.sql",
        "CREATE TABLE Photos (Caption TEXT, \"Quoted\" TEXT);\n",
    );
    repo.commit("schema");
    assert_eq!(reported(&repo), paths(&["quoted"]));
}

#[test]
fn a_directory_or_the_config_names_the_dialect_a_file_is_read_in() {
    let repo = removed_functions(&["app_reader"]);
    let roles = "CREATE TABLE audit (id INT);\nDO $$ BEGIN CREATE ROLE app_reader; END $$;\n";
    repo.write("db/roles.sql", roles);
    repo.commit("generic roles");
    let report = scan(&repo);
    assert_eq!(
        report
            .groups
            .iter()
            .map(|g| g.path.as_str())
            .collect::<Vec<_>>(),
        ["fixture::app_reader"],
        "GenericDialect refuses DO"
    );
    assert_eq!(refused(&report), [("db/roles.sql".to_owned(), 2, 1)]);

    repo.write(
        ".refquiem.toml",
        "[sql-dialects]\n\"db/*.sql\" = \"postgres\"\n",
    );
    repo.commit("configured");
    let report = scan(&repo);
    assert_eq!(report.groups.len(), 0);
    assert_eq!(refused(&report), []);

    std::fs::remove_file(repo.path().join(".refquiem.toml")).unwrap();
    std::fs::rename(repo.path().join("db"), repo.path().join("postgres")).unwrap();
    repo.commit("by directory");
    let report = scan(&repo);
    assert_eq!(report.groups.len(), 0);
    assert_eq!(refused(&report), []);
}

#[test]
fn globs_naming_two_dialects_for_one_file_are_an_error() {
    let repo = removed_functions(&["gone"]);
    repo.write("db/schema.sql", "CREATE TABLE t (id INT);\n");
    repo.write(
        ".refquiem.toml",
        "[sql-dialects]\n\"db/*.sql\" = \"postgres\"\n\"*.sql\" = \"mysql\"\n",
    );
    repo.commit("conflict");
    let error = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap_err();
    assert_eq!(
        error.to_string(),
        "the [sql-dialects] globs `*.sql` and `db/*.sql` give db/schema.sql two dialects"
    );
}

#[test]
fn an_unknown_dialect_is_a_config_error() {
    let repo = removed_functions(&["gone"]);
    repo.write(".refquiem.toml", "[sql-dialects]\n\"*.sql\" = \"oracle\"\n");
    repo.commit("unknown");
    let error = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap_err();
    assert!(matches!(error, refquiem::Error::Config { .. }), "{error}");
}

#[test]
fn statements_inside_plpgsql_bodies_declare_names_exception_handlers_included() {
    let repo = removed_functions(&["make_log", "audit_log", "entry", "audit_fallback"]);
    repo.write(
        "postgres/up.sql",
        "CREATE FUNCTION make_log() RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    CREATE TABLE audit_log (entry TEXT);
EXCEPTION WHEN duplicate_table THEN
    CREATE TABLE audit_fallback (entry TEXT);
END
$$;
",
    );
    repo.commit("function");
    let report = scan(&repo);
    assert_eq!(refused(&report), []);
    assert_eq!(report.groups.len(), 0);
}

#[test]
fn a_refused_body_loses_only_its_own_names_and_warns_at_the_body() {
    let repo = removed_functions(&["kept_table", "body_table"]);
    repo.write(
        "postgres/up.sql",
        "CREATE TABLE kept_table (id INT);\nDO $$ BEGIN CREATE TABLE body_table ( ; END $$;\n",
    );
    repo.commit("bad body");
    let report = scan(&repo);
    assert_eq!(refused(&report), [("postgres/up.sql".to_owned(), 2, 4)]);
    let reported: Vec<&str> = report.groups.iter().map(|g| g.path.as_str()).collect();
    assert_eq!(reported, ["fixture::body_table"]);
}

#[test]
fn a_file_sqlparser_refuses_declares_nothing_and_is_a_warning_in_every_output() {
    let repo = removed_functions(&["kept_name"]);
    repo.write(
        "db/broken.sql",
        "CREATE TABLE kept_name (id INT);\nCREATE TABLE ( ;\n",
    );
    repo.commit("broken");
    let report = scan(&repo);
    assert_eq!(reported(&repo), paths(&["kept_name"]));
    assert_eq!(refused(&report), [("db/broken.sql".to_owned(), 2, 14)]);
    let error = &report.refused_sql[0].error;
    assert!(
        !error.is_empty() && !error.contains(" at Line: "),
        "{error}"
    );

    let message =
        format!("this SQL could not be read, so the names it declares are not known: {error}");
    assert!(
        report
            .human()
            .contains(&format!("warning: db/broken.sql:2:14  {message}\n")),
        "{}",
        report.human()
    );
    assert!(
        report.annotations(false).contains(&format!(
            "::warning file=db/broken.sql,line=2,col=14,title=refquiem::{message}\n"
        )),
        "{}",
        report.annotations(false)
    );
    let json: serde_json::Value = serde_json::from_str(&report.json()).unwrap();
    assert_eq!(
        json["refused_sql"],
        serde_json::json!([{"file": "db/broken.sql", "line": 2, "column": 14, "error": error}])
    );
    assert!(
        report.job_summary(None).contains(&format!(
            "\n### Unread SQL\n\n- `db/broken.sql:2:14` `{error}`\n"
        )),
        "{}",
        report.job_summary(None)
    );
}

#[test]
fn an_excluded_sql_file_is_neither_read_nor_reported() {
    let repo = removed_functions(&["kept_name"]);
    repo.write(
        "template.sql",
        "CREATE TABLE {table_name} (kept_name INT);\n",
    );
    repo.write(".refquiem.toml", "exclude = [\"template.sql\"]\n");
    repo.commit("template");
    let report = scan(&repo);
    assert_eq!(refused(&report), []);
    assert_eq!(report.groups.len(), 1);
}
