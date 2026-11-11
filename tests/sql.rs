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
        "owner_role",
        "float_range",
        "app_user",
        "one_fn",
        "c_fn",
        "e_fn",
        "e_table",
        "e_col",
        "q_fn",
        "q_table",
        "orders_amount_key",
        "orders_self_fk",
        "no_overlap",
        "orders_key",
        "orders_unique",
        "orders_uniq",
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
    CONSTRAINT orders_amount_check CHECK (amount > 0),
    CONSTRAINT orders_amount_key UNIQUE (amount),
    CONSTRAINT orders_self_fk FOREIGN KEY (order_id) REFERENCES app_schema.orders (order_id),
    CONSTRAINT no_overlap EXCLUDE USING gist (amount WITH &&)
);
CREATE VIEW order_view (view_total) AS SELECT order_id FROM app_schema.orders;
CREATE INDEX orders_by_id ON app_schema.orders (order_id);
CREATE INDEX ON app_schema.orders (amount);
CREATE FUNCTION order_total(total_param INT) RETURNS INT LANGUAGE sql AS $$ SELECT total_param $$;
CREATE FUNCTION one_fn(INT) RETURNS INT LANGUAGE sql RETURN 1;
CREATE FUNCTION c_fn() RETURNS INT AS 'obj', 'sym' LANGUAGE c;
CREATE FUNCTION e_fn() RETURNS void LANGUAGE sql AS E'CREATE TABLE e_table (e_col INT)';
CREATE FUNCTION q_fn() RETURNS void LANGUAGE sql AS 'CREATE TABLE q_table (q INT)';
CREATE TRIGGER orders_audit AFTER INSERT ON app_schema.orders FOR EACH ROW EXECUTE FUNCTION order_total();
CREATE ROLE order_reader;
CREATE USER app_user;
CREATE SCHEMA AUTHORIZATION owner_role;
CREATE TYPE float_range AS RANGE (subtype = float8);
CREATE POLICY orders_policy ON app_schema.orders USING (true);
ALTER TABLE app_schema.orders ADD COLUMN placed_at INT;
ALTER TABLE app_schema.orders RENAME COLUMN placed_at TO placed_on;
ALTER TABLE app_schema.orders ADD CONSTRAINT orders_key PRIMARY KEY USING INDEX orders_by_id;
ALTER TABLE app_schema.orders ADD CONSTRAINT orders_unique UNIQUE USING INDEX orders_by_id;
ALTER TABLE app_schema.orders RENAME CONSTRAINT orders_unique TO orders_uniq;
ALTER TABLE app_schema.orders DROP COLUMN amount;
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
fn mysql_sqlite_and_generic_files_declare_names_in_their_own_syntax() {
    let declared = [
        "shops",
        "shop_id",
        "shops_uk",
        "shops_ix",
        "shops_ft",
        "shops_fk",
        "store_id",
        "docs_index",
        "gen_proc",
        "proc_param",
    ];
    let repo = removed_functions(&declared);
    repo.write(
        "db/mysql/schema.sql",
        "# MySQL alone reads this comment
CREATE TABLE `shops` (
    shop_id INT,
    UNIQUE KEY shops_uk (shop_id),
    INDEX shops_ix (shop_id),
    FULLTEXT shops_ft (shop_id),
    CONSTRAINT shops_fk FOREIGN KEY (shop_id) REFERENCES owners (owner_id)
);
ALTER TABLE shops CHANGE COLUMN shop_id store_id INT;
",
    );
    repo.write(
        "db/sqlite/schema.sql",
        "CREATE VIRTUAL TABLE docs_index USING fts5(body);\n",
    );
    repo.write(
        "db/procedures.sql",
        "CREATE PROCEDURE gen_proc (proc_param INT) AS BEGIN SELECT 1; END;\n",
    );
    repo.commit("schemas");
    let report = scan(&repo);
    assert_eq!(refused(&report), []);
    assert_eq!(report.groups.len(), 0);
}

#[test]
fn plpgsql_blocks_branches_cases_and_loops_declare_names_and_dynamic_sql_does_not() {
    let declared = [
        "nested_t",
        "if_t",
        "elsif_t",
        "else_t",
        "case_t",
        "case_else_t",
        "loop_t",
    ];
    let mut names = declared.to_vec();
    names.push("dynamic_t");
    let repo = removed_functions(&names);
    repo.write(
        "postgres/up.sql",
        "DO $$
BEGIN
    BEGIN CREATE TABLE nested_t (a INT); END;
    IF true THEN CREATE TABLE if_t (a INT);
    ELSIF false THEN CREATE TABLE elsif_t (a INT);
    ELSE CREATE TABLE else_t (a INT);
    END IF;
    CASE WHEN true THEN CREATE TABLE case_t (a INT); ELSE CREATE TABLE case_else_t (a INT); END CASE;
    LOOP CREATE TABLE loop_t (a INT); EXIT; END LOOP;
    EXECUTE 'CREATE TABLE dynamic_t (a INT)';
END
$$;
",
    );
    repo.commit("control flow");
    let report = scan(&repo);
    assert_eq!(refused(&report), []);
    assert_eq!(reported(&repo), paths(&["dynamic_t"]));
}

#[test]
fn a_do_block_is_read_in_the_language_it_names() {
    let repo = removed_functions(&["named_t", "literal_t", "escaped_t", "python_t"]);
    repo.write(
        "postgres/up.sql",
        "DO LANGUAGE plpgsql $$ BEGIN CREATE TABLE named_t (a INT); END $$;
DO LANGUAGE 'plpgsql' $$ BEGIN CREATE TABLE literal_t (a INT); END $$;
DO LANGUAGE E'plpgsql' $$ BEGIN CREATE TABLE escaped_t (a INT); END $$;
DO LANGUAGE plpython3u $$ plpy.execute('CREATE TABLE python_t (a INT)') $$;
DO LANGUAGE plpgsql;
",
    );
    repo.commit("languages");
    let report = scan(&repo);
    assert_eq!(refused(&report), []);
    assert_eq!(reported(&repo), paths(&["python_t"]));
}

#[test]
fn tokenizer_errors_deep_nesting_and_refused_sql_bodies_are_warnings_at_their_place() {
    let repo = removed_functions(&["gone"]);
    repo.write("a.sql", "CREATE TABLE t (a TEXT DEFAULT 'unterminated\n");
    repo.write(
        "b.sql",
        &format!("SELECT {}1{};\n", "(".repeat(200), ")".repeat(200)),
    );
    repo.write(
        "postgres/c.sql",
        "CREATE TABLE kept (id INT);\nCREATE FUNCTION f() RETURNS INT LANGUAGE sql AS $$ SELECT ( $$;\n",
    );
    let mut nested = String::from("BEGIN NULL; END");
    for level in 0..9 {
        nested = format!("BEGIN DO $l{level}$ {nested} $l{level}$; END");
    }
    repo.write("postgres/d.sql", &format!("\nDO $top$ {nested} $top$;\n"));
    repo.commit("refusals");
    let report = scan(&repo);
    let refusals: Vec<(&str, usize, usize, &str)> = report
        .refused_sql
        .iter()
        .map(|r| (r.file.as_str(), r.line, r.column, r.error.as_str()))
        .collect();
    assert_eq!(refusals[0], ("a.sql", 1, 32, "Unterminated string literal"));
    assert_eq!(refusals[1], ("b.sql", 1, 1, "recursion limit exceeded"));
    let (file, line, column, error) = refusals[2];
    assert_eq!((file, line, column), ("postgres/c.sql", 2, 49));
    assert!(error.starts_with("Expected: "), "{error}");
    assert_eq!(
        refusals[3],
        ("postgres/d.sql", 2, 4, "bodies nest deeper than 8")
    );
    assert_eq!(refusals.len(), 4);
}

#[test]
fn overlapping_globs_of_one_dialect_agree_and_other_files_keep_their_directory() {
    let repo = removed_functions(&["app_reader", "lite_table"]);
    repo.write(
        ".refquiem.toml",
        "[sql-dialects]\n\"db/*.sql\" = \"postgres\"\n\"db/roles.sql\" = \"pg\"\n",
    );
    repo.write(
        "db/roles.sql",
        "DO $$ BEGIN CREATE ROLE app_reader; END $$;\n",
    );
    repo.write("sqlite/lite.sql", "CREATE TABLE lite_table (id INT);\n");
    repo.commit("configured");
    let report = scan(&repo);
    assert_eq!(refused(&report), []);
    assert_eq!(report.groups.len(), 0);
}

#[test]
fn an_invalid_dialect_glob_is_an_error() {
    let repo = removed_functions(&["gone"]);
    repo.write(
        ".refquiem.toml",
        "[sql-dialects]\n\"db/{a,b.sql\" = \"postgres\"\n",
    );
    repo.commit("bad glob");
    let error = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap_err();
    assert!(
        matches!(error, refquiem::Error::SqlDialectGlob(_)),
        "{error}"
    );
}

#[test]
fn sql_outside_a_nested_workspace_still_declares_names() {
    let repo = Repo::new();
    std::fs::remove_file(repo.path().join("Cargo.toml")).unwrap();
    repo.write("app/Cargo.toml", common::PACKAGE);
    repo.write("app/src/lib.rs", "pub fn shared_table() {}\n");
    repo.commit("start");
    repo.write("app/src/lib.rs", "");
    repo.write("app/README.md", "Call `shared_table`.\n");
    repo.write(".refquiem.toml", "[sql-dialects]\n\"*.sql\" = \"mysql\"\n");
    repo.write(
        "db/postgres/schema.sql",
        "CREATE TABLE shared_table (id INT);\n",
    );
    repo.commit("schema outside the workspace");
    let report = refquiem::run(&refquiem::Options::new(repo.path().join("app"))).unwrap();
    assert_eq!(refused(&report), []);
    assert_eq!(report.groups.len(), 0);
}

#[test]
fn a_sql_file_that_is_not_utf8_is_skipped_without_a_warning() {
    let repo = removed_functions(&["latin_table"]);
    std::fs::write(
        repo.path().join("latin.sql"),
        b"-- caf\xe9\nCREATE TABLE latin_table (id INT);\n",
    )
    .unwrap();
    let report = scan(&repo);
    assert_eq!(refused(&report), []);
    assert_eq!(reported(&repo), paths(&["latin_table"]));
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

/// A repository whose functions `names` were removed from `src/old.rs` while `README.md` names
/// each of them, leaving `src/lib.rs` free for the test's own code.
fn removed_from_old(names: &[&str]) -> Repo {
    let repo = Repo::new();
    let mut old = String::new();
    let mut readme = String::new();
    for name in names {
        let _ = writeln!(old, "pub fn {name}() {{}}");
        let _ = writeln!(readme, "Call `{name}`.");
    }
    repo.write("src/lib.rs", "pub mod old;\n");
    repo.write("src/old.rs", &old);
    repo.commit("start");
    repo.write("src/old.rs", "");
    repo.write("README.md", &readme);
    repo.commit("drop them");
    repo
}

#[test]
fn rust_string_literals_holding_ddl_declare_names_escapes_and_raw_strings_included() {
    let repo = removed_from_old(&[
        "connetto_epoch",
        "singleton",
        "system_identifier",
        "raw_table",
        "raw_col",
        "select_only",
    ]);
    repo.write(
        "src/lib.rs",
        "pub mod old;
pub const EPOCH_DDL: &str = \"CREATE TABLE IF NOT EXISTS connetto_epoch (\\
    singleton BOOLEAN PRIMARY KEY, \\
    system_identifier TEXT NOT NULL)\";
pub const RAW: &str = r#\"create table \"raw_table\" (raw_col INT)\"#;
pub const QUERY: &str = \"SELECT select_only FROM t\";
",
    );
    repo.commit("ddl in strings");
    let report = scan(&repo);
    assert_eq!(refused(&report), []);
    assert_eq!(reported(&repo), paths(&["old::select_only"]));
}

#[test]
fn a_string_literal_takes_the_dialect_of_its_file_path() {
    let repo = removed_from_old(&["string_role"]);
    repo.write("src/lib.rs", "pub mod old;\npub mod pg;\n");
    repo.write(
        "src/pg/mod.rs",
        "pub const ROLES: &str = \"DO $$ BEGIN CREATE ROLE string_role; END $$\";\n",
    );
    repo.commit("roles");
    let report = scan(&repo);
    assert_eq!(refused(&report), []);
    assert_eq!(report.groups.len(), 0);
}

#[test]
fn a_string_literal_sqlparser_refuses_declares_nothing_and_warns_about_nothing() {
    let repo = removed_from_old(&["lost_col", "byte_col", "escape_col"]);
    repo.write(
        "src/lib.rs",
        "pub mod old;
pub const TEMPLATE: &str = \"CREATE TABLE {table} (lost_col INT)\";
pub const PROSE: &str = \"create a key before reading it\";
pub const BYTES: &[u8] = b\"CREATE TABLE byte_table (byte_col INT)\";
",
    );
    repo.write(
        "notes/snippet.rs",
        "const ESCAPE: &str = \"CREATE TABLE escape_table (escape_col INT) \\q\";\n",
    );
    repo.commit("not sql");
    let report = scan(&repo);
    assert_eq!(refused(&report), []);
    assert_eq!(
        reported(&repo),
        paths(&["old::byte_col", "old::escape_col", "old::lost_col"])
    );
}

#[test]
fn a_refused_fence_in_a_blockquote_warns_at_its_line_and_column_in_the_file() {
    let repo = removed_from_old(&["gone"]);
    repo.write(
        "docs/schema.md",
        "Intro.\n\n> ```sql\n> CREATE TABLE t (\n>   a INT,\n>   ( ;\n> ```\n",
    );
    repo.commit("bad fence");
    assert_eq!(refused(&scan(&repo)), [("docs/schema.md".to_owned(), 6, 5)]);
}

#[test]
fn sql_fences_in_markdown_and_doc_comments_declare_names() {
    let repo = removed_from_old(&[
        "fenced_table",
        "fenced_col",
        "fence_role",
        "quoted_fence",
        "doc_table",
        "doc_col",
    ]);
    repo.write(
        "docs/schema.md",
        "```sql
CREATE TABLE fenced_table (fenced_col INT);
```

```postgres
DO $$ BEGIN CREATE ROLE fence_role; END $$;
```

> ```sql
> CREATE TABLE quoted_fence (q INT);
> ```
",
    );
    repo.write(
        "src/lib.rs",
        "pub mod old;
/// ```sql
/// CREATE TABLE doc_table (doc_col INT);
/// ```
pub fn kept() {}
",
    );
    repo.commit("fences");
    let report = scan(&repo);
    assert_eq!(refused(&report), []);
    assert_eq!(report.groups.len(), 0);
}

#[test]
fn a_refused_fence_in_a_doc_comment_warns_at_its_place_in_the_file() {
    let repo = removed_from_old(&["gone"]);
    repo.write(
        "src/lib.rs",
        "pub mod old;\n/// ```sql\n/// CREATE TABLE ( ;\n/// ```\npub fn kept() {}\n",
    );
    repo.commit("bad fence");
    assert_eq!(refused(&scan(&repo)), [("src/lib.rs".to_owned(), 3, 18)]);
}

#[test]
fn strings_and_fences_of_an_excluded_file_declare_nothing() {
    let repo = removed_from_old(&["hidden_table"]);
    repo.write(
        "src/lib.rs",
        "pub mod old;\npub const DDL: &str = \"CREATE TABLE hidden_table (id INT)\";\n",
    );
    repo.write(".refquiem.toml", "exclude = [\"src/lib.rs\"]\n");
    repo.commit("excluded");
    assert_eq!(reported(&repo), paths(&["old::hidden_table"]));
}
