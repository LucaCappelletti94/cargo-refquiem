//! Migrations replayed in order, so the names they drop or rename away stop being alive.

mod common;

use std::fmt::Write as _;

use common::Repo;

/// The names among `names`, removed Rust functions that `README.md` mentions, that a scan
/// reports once head holds `files`.
fn dead(names: &[&str], files: &[(&str, &str)]) -> Vec<String> {
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
    for (file, source) in files {
        repo.write(file, source);
    }
    repo.commit("drop them");
    let report = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap();
    assert_eq!(report.refused_sql.len(), 0, "{:?}", report.refused_sql);
    report
        .groups
        .into_iter()
        .map(|group| group.path.trim_start_matches("fixture::").to_owned())
        .collect()
}

#[test]
fn a_column_a_later_migration_drops_no_longer_keeps_a_renamed_field_alive() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "/// Rows carry `content_state`.\npub struct Photo {\n    pub content_state: u8,\n}\n",
    );
    repo.write(
        "migrations/2024-01-01-000000_photos/up.sql",
        "CREATE TABLE photos (id INT, content_state TEXT);\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "/// Rows carry `content_state`.\npub struct Photo {\n    pub state: u8,\n}\n",
    );
    repo.write(
        "migrations/2024-02-01-000000_drop_state/up.sql",
        "ALTER TABLE photos DROP COLUMN content_state;\n",
    );
    repo.write(
        "migrations/2024-02-01-000000_drop_state/down.sql",
        "ALTER TABLE photos ADD COLUMN content_state TEXT;\n",
    );
    repo.commit("drop the column with the field");
    let report = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap();
    let reported: Vec<String> = report.groups.into_iter().map(|group| group.path).collect();
    assert_eq!(reported, ["fixture::Photo::content_state"]);
}

#[test]
fn a_dropped_table_takes_everything_it_owns_and_a_renamed_one_carries_it() {
    let names = [
        "photos",
        "photo_id",
        "photos_pkey",
        "photos_by_id",
        "photos_audit",
        "photos_policy",
        "shared_col",
        "albums",
        "album_id",
        "albums_by_id",
        "collections",
    ];
    let up = "CREATE TABLE photos (photo_id INT, shared_col INT, CONSTRAINT photos_pkey PRIMARY KEY (photo_id));
CREATE INDEX photos_by_id ON photos (photo_id);
CREATE TRIGGER photos_audit AFTER INSERT ON photos FOR EACH ROW EXECUTE FUNCTION audit();
CREATE POLICY photos_policy ON photos USING (true);
CREATE TABLE albums (album_id INT, shared_col INT);
CREATE INDEX albums_by_id ON albums (album_id);
";
    let down = "DROP TABLE photos;\nALTER TABLE albums RENAME TO collections;\n";
    assert_eq!(
        dead(
            &names,
            &[
                ("migrations/20240101000000_start/up.sql", up),
                ("migrations/20240201000000_next/up.sql", down),
            ],
        ),
        [
            "albums",
            "photo_id",
            "photos",
            "photos_audit",
            "photos_by_id",
            "photos_pkey",
            "photos_policy",
        ]
    );
}

#[test]
fn renamed_columns_and_constraints_keep_only_their_new_names() {
    let names = [
        "old_col",
        "new_col",
        "old_key",
        "new_key",
        "mysql_old",
        "mysql_new",
    ];
    let files = [
        (
            "migrations/postgres/20240101000000_start/up.sql",
            "CREATE TABLE t (old_col INT, CONSTRAINT old_key UNIQUE (old_col));
ALTER TABLE t RENAME COLUMN old_col TO new_col;
ALTER TABLE t RENAME CONSTRAINT old_key TO new_key;
",
        ),
        (
            "migrations/mysql/20240101000000_start/up.sql",
            "CREATE TABLE t (mysql_old INT);\nALTER TABLE t CHANGE mysql_old mysql_new INT;\n",
        ),
    ];
    assert_eq!(dead(&names, &files), ["mysql_old", "old_col", "old_key"]);
}

#[test]
fn an_object_is_its_folded_name_as_written() {
    let names = ["photos", "albums", "quoted", "Quoted"];
    let up = "CREATE TABLE photos (id INT);
CREATE TABLE Albums (id INT);
CREATE TABLE \"Quoted\" (id INT);
CREATE TABLE quoted (id INT);
DROP TABLE public.photos;
DROP TABLE ALBUMS;
DROP TABLE \"quoted\";
";
    assert_eq!(
        dead(&names, &[("migrations/20240101000000_start/up.sql", up)]),
        ["albums", "quoted"]
    );
}

#[test]
fn every_kind_of_drop_removes_its_object_and_what_it_owns() {
    let names = [
        "app_schema",
        "trgm_ext",
        "mood_kind",
        "happy_label",
        "pair_kind",
        "left_part",
        "amount_domain",
        "order_seq",
        "order_view",
        "view_total",
        "order_role",
        "order_user",
        "orders",
        "orders_key",
        "amount_col",
        "loose_index",
        "kept_table",
        "pg_trigger",
        "kept_policy",
    ];
    let up = "CREATE SCHEMA app_schema;
CREATE EXTENSION trgm_ext;
CREATE TYPE mood_kind AS ENUM ('happy_label');
CREATE TYPE pair_kind AS (left_part INT);
CREATE DOMAIN amount_domain AS INT;
CREATE SEQUENCE order_seq;
CREATE VIEW order_view (view_total) AS SELECT 1;
CREATE ROLE order_role;
CREATE USER order_user;
CREATE TABLE orders (amount_col INT, CONSTRAINT orders_key UNIQUE (amount_col));
CREATE TABLE kept_table (id INT);
CREATE INDEX loose_index ON kept_table (id);
DROP SCHEMA app_schema;
DROP EXTENSION trgm_ext;
DROP TYPE mood_kind, pair_kind;
DROP DOMAIN amount_domain;
DROP SEQUENCE order_seq;
DROP VIEW order_view;
DROP ROLE order_role;
DROP USER order_user;
ALTER TABLE orders DROP CONSTRAINT orders_key;
ALTER TABLE orders DROP COLUMN amount_col;
DROP INDEX loose_index;
CREATE TRIGGER pg_trigger AFTER INSERT ON kept_table FOR EACH ROW EXECUTE FUNCTION audit();
CREATE POLICY kept_policy ON kept_table USING (true);
DROP TRIGGER pg_trigger ON kept_table;
DROP POLICY kept_policy ON kept_table;
DROP DATABASE other_db;
ALTER TABLE kept_table ALTER COLUMN id SET NOT NULL;
";
    assert_eq!(
        dead(
            &names,
            &[("migrations/postgres/20240101000000_start/up.sql", up)],
        ),
        [
            "amount_col",
            "amount_domain",
            "app_schema",
            "happy_label",
            "kept_policy",
            "left_part",
            "loose_index",
            "mood_kind",
            "order_role",
            "order_seq",
            "order_user",
            "order_view",
            "orders_key",
            "pair_kind",
            "pg_trigger",
            "trgm_ext",
            "view_total",
        ]
    );
}

#[test]
fn mysql_and_sqlite_drop_keys_by_index_and_triggers_by_schema_name() {
    let names = [
        "my_index",
        "k2_key",
        "my_trigger",
        "lite_trigger",
        "moved",
        "moved_col",
        "moved_too",
    ];
    let mysql = "CREATE TABLE kept_table (id INT, UNIQUE KEY my_index (id));
ALTER TABLE kept_table DROP INDEX my_index;
CREATE TABLE k2 (id INT, UNIQUE KEY k2_key (id));
DROP INDEX k2_key ON k2;
CREATE TRIGGER my_trigger BEFORE INSERT ON kept_table FOR EACH ROW SET NEW.id = 1;
DROP TRIGGER my_trigger;
CREATE TABLE moved (moved_col INT);
ALTER TABLE moved RENAME TO archive.moved_too;
";
    let sqlite = "CREATE TABLE s (id INT);
CREATE TRIGGER lite_trigger AFTER INSERT ON s BEGIN SELECT 1; END;
DROP TRIGGER lite_trigger;
";
    assert_eq!(
        dead(
            &names,
            &[
                ("migrations/mysql/20240101000000_start/up.sql", mysql),
                ("migrations/sqlite/20240101000000_start/up.sql", sqlite),
            ],
        ),
        ["k2_key", "lite_trigger", "moved", "my_index", "my_trigger"]
    );
}

#[test]
fn a_routine_drop_removes_one_signature_or_every_overload_and_their_parameters() {
    let names = [
        "overloaded",
        "text_arg",
        "int_arg",
        "single",
        "single_arg",
        "proc_one",
        "proc_param",
    ];
    let up = "CREATE FUNCTION overloaded(int_arg INT) RETURNS INT LANGUAGE sql RETURN 1;
CREATE FUNCTION overloaded(text_arg TEXT) RETURNS INT LANGUAGE sql RETURN 1;
CREATE FUNCTION single(single_arg INT) RETURNS INT LANGUAGE sql RETURN 1;
DROP FUNCTION overloaded(INT);
DROP FUNCTION single;
";
    let procedures = "CREATE PROCEDURE proc_one (proc_param INT) AS BEGIN SELECT 1; END;\nDROP PROCEDURE proc_one;\n";
    assert_eq!(
        dead(
            &names,
            &[
                ("migrations/postgres/20240101000000_start/up.sql", up),
                ("migrations/20240101000000_procedures/up.sql", procedures),
            ]
        ),
        ["int_arg", "proc_one", "proc_param", "single", "single_arg"]
    );
}

#[test]
fn body_statements_declare_objects_and_remove_nothing() {
    let names = ["body_type", "kept_col", "body_col", "body_renamed"];
    let up = "CREATE TABLE t (kept_col INT);
DO $$ BEGIN
  IF NOT EXISTS (SELECT 1) THEN
    CREATE TYPE body_type AS ENUM ('a');
    ALTER TABLE t DROP COLUMN kept_col;
    ALTER TABLE t RENAME COLUMN kept_col TO body_renamed;
    CREATE TABLE body_table (body_col INT);
  END IF;
END $$;
";
    let later = "DROP TYPE body_type;\n";
    assert_eq!(
        dead(
            &names,
            &[
                ("migrations/postgres/20240101000000_start/up.sql", up),
                ("migrations/postgres/20240201000000_later/up.sql", later),
            ],
        ),
        ["body_type"]
    );
}

#[test]
fn sqlx_and_refinery_migrations_replay_in_numeric_version_order() {
    let names = ["sqlx_col", "sqlx_up_col", "refinery_col"];
    let files = [
        (
            "db/migrations/2_create.sql",
            "CREATE TABLE s (sqlx_col INT);\n",
        ),
        (
            "db/migrations/10_drop.sql",
            "ALTER TABLE s DROP COLUMN sqlx_col;\n",
        ),
        (
            "up/migrations/3_create.up.sql",
            "CREATE TABLE u (sqlx_up_col INT);\n",
        ),
        (
            "up/migrations/11_drop.up.sql",
            "ALTER TABLE u DROP COLUMN sqlx_up_col;\n",
        ),
        (
            "up/migrations/11_drop.down.sql",
            "ALTER TABLE u ADD COLUMN sqlx_up_col INT;\n",
        ),
        ("sql/V2__create.sql", "CREATE TABLE r (refinery_col INT);\n"),
        (
            "sql/V10__drop.sql",
            "ALTER TABLE r DROP COLUMN refinery_col;\n",
        ),
    ];
    assert_eq!(
        dead(&names, &files),
        ["refinery_col", "sqlx_col", "sqlx_up_col"]
    );
}

#[test]
fn sqlx_shaped_files_outside_a_migrations_directory_stay_cumulative() {
    let names = ["loose_col"];
    let files = [
        ("schema/1_create.sql", "CREATE TABLE l (loose_col INT);\n"),
        (
            "schema/2_drop.sql",
            "ALTER TABLE l DROP COLUMN loose_col;\n",
        ),
    ];
    assert_eq!(dead(&names, &files), Vec::<String>::new());
}

#[test]
fn diesel_toml_names_a_migrations_directory_of_any_entry_names_relative_to_itself() {
    let cases = [
        (
            "app",
            "[migrations_directory]\ndir = \"schema_changes\"\n",
            "app/schema_changes",
            "custom_col",
        ),
        (
            "dot",
            "[migrations_directory]\ndir = \"./changes/\"\n",
            "dot/changes",
            "dot_col",
        ),
        (
            "nested/app",
            "[migrations_directory]\ndir = \"../shared\"\n",
            "nested/shared",
            "parent_col",
        ),
        (
            "abs",
            "[migrations_directory]\ndir = \"/srv/changes\"\n",
            "abs/srv/changes",
            "abs_col",
        ),
        (
            "escape",
            "[migrations_directory]\ndir = \"../../out\"\n",
            "out",
            "escape_col",
        ),
        (
            "plain",
            "[print_schema]\nfile = \"src/schema.rs\"\n",
            "plain/migrations",
            "plain_col",
        ),
    ];
    let mut files = Vec::new();
    let mut names = Vec::new();
    for (dir, toml, migrations, column) in cases {
        files.push((format!("{dir}/diesel.toml"), toml.to_owned()));
        files.push((
            format!("{migrations}/a_create/up.sql"),
            format!("CREATE TABLE c (id INT, {column} INT);\n"),
        ));
        files.push((
            format!("{migrations}/b_drop/up.sql"),
            format!("ALTER TABLE c DROP COLUMN {column};\n"),
        ));
        names.push(column);
    }
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(file, source)| (file.as_str(), source.as_str()))
        .collect();
    assert_eq!(
        dead(&names, &files),
        ["custom_col", "dot_col", "parent_col"]
    );
}

#[test]
fn a_name_alive_in_another_tree_or_a_plain_sql_file_stays_alive() {
    let names = ["tree_col", "plain_col", "gone_col"];
    let files = [
        (
            "migrations/postgres/20240101000000_start/up.sql",
            "CREATE TABLE t (tree_col INT, plain_col INT, gone_col INT);
ALTER TABLE t DROP COLUMN tree_col;
ALTER TABLE t DROP COLUMN plain_col;
ALTER TABLE t DROP COLUMN gone_col;
",
        ),
        (
            "migrations/sqlite/20240101000000_start/up.sql",
            "CREATE TABLE t (tree_col INT);\n",
        ),
        ("schema.sql", "CREATE TABLE p (plain_col INT);\n"),
    ];
    assert_eq!(dead(&names, &files), ["gone_col"]);
}

#[test]
fn down_migrations_declare_nothing() {
    let names = ["down_only", "sqlx_down_only"];
    let files = [
        (
            "migrations/20240101000000_start/up.sql",
            "CREATE TABLE t (id INT);\n",
        ),
        (
            "migrations/20240101000000_start/down.sql",
            "CREATE TABLE down_only (id INT);\n",
        ),
        ("db/migrations/1_start.up.sql", "CREATE TABLE s (id INT);\n"),
        (
            "db/migrations/1_start.down.sql",
            "CREATE TABLE sqlx_down_only (id INT);\n",
        ),
    ];
    assert_eq!(dead(&names, &files), ["down_only", "sqlx_down_only"]);
}

#[test]
fn a_refused_migration_is_skipped_and_the_replay_goes_on() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub fn kept_col() {}\npub fn later_col() {}\n",
    );
    repo.commit("start");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "Call `kept_col` and `later_col`.\n");
    repo.write(
        "migrations/20240101000000_start/up.sql",
        "CREATE TABLE t (kept_col INT, later_col INT);\n",
    );
    repo.write(
        "migrations/20240201000000_broken/up.sql",
        "ALTER TABLE t DROP COLUMN kept_col;\nCREATE TABLE (;\n",
    );
    repo.write(
        "migrations/20240301000000_later/up.sql",
        "ALTER TABLE t DROP COLUMN later_col;\n",
    );
    repo.commit("drop them");
    let report = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap();
    let reported: Vec<String> = report.groups.into_iter().map(|group| group.path).collect();
    assert_eq!(reported, ["fixture::later_col"]);
    let refused: Vec<&str> = report
        .refused_sql
        .iter()
        .map(|refused| refused.file.as_str())
        .collect();
    assert_eq!(refused, ["migrations/20240201000000_broken/up.sql"]);
}
