//! Where mentions sit: doc comment and Markdown structure, links, string literals, and the
//! standard library paths that keep a spelling alive.

mod common;
#[path = "common/vendor.rs"]
mod vendor;

use common::Repo;
use refquiem::{Context, Kind, Source};

const BEFORE: &str =
    "pub fn gone() {}\npub struct Thing;\nimpl Thing {\n    pub fn old(&self) {}\n}\n";

const LIB: &str = "/// Prose names crate::gone here.
/// A span `crate::gone`, a link [`crate::gone`] and [text](crate::gone).
///
/// ```
/// let _ = fixture::gone();
/// ```
///
/// ```ignore
/// fixture::gone();
/// ```
pub fn kept() {}

/**
 * Block docs name `fixture::gone` too.
 */
pub fn other() {}

pub const MESSAGE: &str = \"use fixture::gone, not crate::gone, which strings cannot resolve\";

pub struct Thing;
";

const README: &str = "Prose fixture::gone, span `fixture::gone`.

```rust
fixture::gone();
```

[method](struct.Thing.html#method.old), [docs.rs](https://docs.rs/fixture/latest/fixture/fn.gone.html), [up](../fixture/fn.gone.html), [kept](struct.Thing.html).
";

fn context(source: Source, kind: Kind) -> Context {
    Context { source, kind }
}

#[test]
fn unicode_blank_doc_lines_preserve_reference_offsets() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.commit("public API");
    repo.write(
        "src/lib.rs",
        "///\u{2003}\n///  `fixture::gone`\npub fn kept() {}\n",
    );
    let rows: Vec<_> = found(&repo).into_iter().map(|m| (m.0, m.2, m.3)).collect();
    assert_eq!(rows, [("fixture::gone".to_owned(), 2, 7)]);
}

/// `(removed path, file, line, column, context, text)` for every mention, sorted.
fn found(repo: &Repo) -> Vec<(String, String, usize, usize, Context, String)> {
    let report = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap();
    let mut out: Vec<_> = report
        .groups
        .iter()
        .flat_map(|group| {
            group.mentions.iter().map(|m| {
                (
                    group.path.clone(),
                    m.file.clone(),
                    m.line,
                    m.column,
                    m.context,
                    m.text.clone(),
                )
            })
        })
        .collect();
    out.sort_by(|a, b| (&a.0, &a.1, a.2, a.3).cmp(&(&b.0, &b.1, b.2, b.3)));
    out
}

fn fixture() -> Repo {
    let repo = Repo::new();
    repo.write("src/lib.rs", BEFORE);
    repo.commit("start");
    repo.write("src/lib.rs", LIB);
    repo.write("README.md", README);
    repo.commit("drop gone and old");
    repo
}

/// `(removed path, line, column, context, text)` of the mentions in `file`.
fn in_file<'a>(
    found: &'a [(String, String, usize, usize, Context, String)],
    file: &str,
) -> Vec<(&'a str, usize, usize, Context, &'a str)> {
    found
        .iter()
        .filter(|m| m.1 == file)
        .map(|m| (m.0.as_str(), m.2, m.3, m.4, m.5.as_str()))
        .collect()
}

#[test]
fn doc_comment_mentions_carry_their_markdown_structure() {
    let found = found(&fixture());
    let doc = |kind| context(Source::DocComment, kind);
    let gone = "fixture::gone";

    assert_eq!(
        in_file(&found, "src/lib.rs"),
        [
            (gone, 1, 17, doc(Kind::Text), "crate::gone"),
            (gone, 2, 13, doc(Kind::CodeSpan), "crate::gone"),
            (gone, 2, 36, doc(Kind::Link), "crate::gone"),
            (gone, 2, 61, doc(Kind::Link), "crate::gone"),
            (gone, 5, 13, doc(Kind::CodeBlock { compiled: true }), gone),
            (gone, 9, 5, doc(Kind::CodeBlock { compiled: false }), gone),
            (gone, 14, 21, doc(Kind::CodeSpan), gone),
            (
                gone,
                18,
                32,
                context(Source::StringLiteral, Kind::Text),
                gone
            ),
        ]
    );
}

#[test]
fn markdown_mentions_carry_their_structure_and_rustdoc_page_links() {
    let found = found(&fixture());
    let md = |kind| context(Source::Markdown, kind);
    let gone = "fixture::gone";

    assert_eq!(
        in_file(&found, "README.md"),
        [
            (
                "fixture::Thing::old",
                7,
                10,
                md(Kind::Link),
                "struct.Thing.html#method.old"
            ),
            (gone, 1, 7, md(Kind::Text), gone),
            (gone, 1, 28, md(Kind::CodeSpan), gone),
            (gone, 4, 1, md(Kind::CodeBlock { compiled: false }), gone),
            (
                gone,
                7,
                51,
                md(Kind::Link),
                "https://docs.rs/fixture/latest/fixture/fn.gone.html"
            ),
            (gone, 7, 110, md(Kind::Link), "../fixture/fn.gone.html"),
        ]
    );
}

#[test]
fn intra_doc_disambiguators_and_call_suffixes_are_stripped() {
    let repo = Repo::new();
    repo.write("src/lib.rs", BEFORE);
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "/// See [`fn@crate::gone`] and [call](crate::gone()).\npub struct Thing;\n",
    );
    repo.commit("drop gone");

    let texts: Vec<(usize, String)> = found(&repo).into_iter().map(|m| (m.3, m.5)).collect();
    assert_eq!(
        texts,
        [
            (11, "fn@crate::gone".to_owned()),
            (39, "crate::gone()".to_owned())
        ]
    );
}

#[test]
fn a_standard_library_path_keeps_its_spelling_alive() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub mod io {\n    pub struct Error;\n}\n");
    repo.commit("local io");
    repo.write("src/lib.rs", "");
    repo.write(
        "README.md",
        "`io::Error::new`, `std::io::Error`, `vec::Vec::push` and `fixture::io::Error`.\n",
    );
    repo.commit("drop local io");

    let paths: Vec<String> = found(&repo).into_iter().map(|m| m.5).collect();
    assert_eq!(paths, ["fixture::io::Error"]);
}

#[test]
fn context_names_appear_in_the_report() {
    let repo = Repo::new();
    repo.write("src/lib.rs", BEFORE);
    repo.commit("start");
    repo.write("src/lib.rs", LIB);
    repo.commit("drop gone and old");

    let report = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap();
    let human = report.human();

    for line in [
        "  src/lib.rs:1:17  doc comment\n",
        "  src/lib.rs:2:13  doc comment code span\n",
        "  src/lib.rs:2:36  doc comment link\n",
        "  src/lib.rs:5:13  doc comment code block\n",
        "  src/lib.rs:9:5  doc comment uncompiled code block\n",
        "  src/lib.rs:18:32  string literal\n",
    ] {
        assert!(human.contains(line), "missing {line:?} in:\n{human}");
    }
}

#[test]
fn compile_fail_and_commented_fences_are_compiled_code_blocks() {
    let repo = Repo::new();
    repo.write("src/lib.rs", BEFORE);
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "/// ```compile_fail\n/// let _ = fixture::gone();\n/// ```\n///\n/// ```no_run (needs a network)\n/// let _ = fixture::gone();\n/// ```\n///\n/// ```no_run (a reason) ignore\n/// let _ = fixture::gone();\n/// ```\npub fn kept() {}\n",
    );
    repo.commit("drop gone");

    let got: Vec<Context> = found(&repo).into_iter().map(|m| m.4).collect();
    let compiled = context(Source::DocComment, Kind::CodeBlock { compiled: true });
    let uncompiled = context(Source::DocComment, Kind::CodeBlock { compiled: false });
    assert_eq!(got, [compiled, compiled, uncompiled]);
}

#[test]
fn uncompiled_code_blocks_only_count_paths_naming_a_workspace_crate() {
    let repo = Repo::new();
    repo.vendor(
        "redis",
        "0.25.0",
        "pub struct Client;\nimpl Client {\n    pub fn open() {}\n}\n",
    );
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nredis = \"0.25\"\n",
    );
    repo.write(
        "src/lib.rs",
        "pub mod images {\n    pub mod postgres {\n        pub struct Postgres;\n    }\n}\n",
    );
    repo.commit("bundled images");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("src/lib.rs", "");
    repo.write(
        "README.md",
        "Prose names `postgres::Postgres`.

```rust
let c = redis::Client::open(\"redis://localhost\");
let p = postgres::Postgres::default();
let q = fixture::images::postgres::Postgres::default();
```
",
    );
    repo.commit("images moved to another crate");

    let found: Vec<(usize, usize, String)> =
        found(&repo).into_iter().map(|m| (m.2, m.3, m.5)).collect();
    assert_eq!(
        found,
        [
            (1, 14, "postgres::Postgres".to_owned()),
            (
                6,
                9,
                "fixture::images::postgres::Postgres::default".to_owned()
            ),
        ]
    );
}

const MANIFEST: &str = "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

#[test]
fn inner_docs_followed_by_outer_docs_do_not_share_a_code_fence() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "//! names fixture::gone\n//! ```\n/// fence line names fixture::gone\nfn kept() {}\n",
    );
    repo.commit("drop gone");

    let text = context(Source::DocComment, Kind::Text);
    assert_eq!(
        found(&repo),
        [
            (
                "fixture::gone".to_owned(),
                "src/lib.rs".to_owned(),
                1,
                11,
                text,
                "fixture::gone".to_owned()
            ),
            (
                "fixture::gone".to_owned(),
                "src/lib.rs".to_owned(),
                3,
                22,
                text,
                "fixture::gone".to_owned()
            ),
        ]
    );
}

#[test]
fn a_doc_comment_run_stops_at_the_next_attribute() {
    let repo = Repo::new();
    repo.vendor("gone_feature", "0.3.0", "");
    repo.write(
        "Cargo.toml",
        &format!("{MANIFEST}\n[dependencies]\ngone_feature = \"0.3\"\n"),
    );
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.commit("start");
    repo.write("Cargo.toml", MANIFEST);
    repo.write(
        "src/lib.rs",
        "/// names fixture::gone\n\
         /// line two\n\
         #[doc = \"see gone_feature\"]\n\
         fn kept() {}\n",
    );
    repo.commit("drop gone and gone_feature");

    let text = context(Source::DocComment, Kind::Text);
    assert_eq!(
        found(&repo),
        [(
            "fixture::gone".to_owned(),
            "src/lib.rs".to_owned(),
            1,
            11,
            text,
            "fixture::gone".to_owned()
        )]
    );
}

#[test]
fn string_literals_inside_hash_attributes_are_macro_arguments() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub fn gone_a() {}\npub fn gone_b() {}\npub fn gone_c() {}\npub fn gone_d() {}\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "#![my_inner(\"see gone_a\")]\n\
         \n\
         #[doc = \"note\"]\n\
         static B: &str = \"fixture::gone_b\";\n\
         \n\
         static A: [&str; 1] = [\"fixture::gone_c\"];\n\
         \n\
         #[my_attr([x] \"see gone_d\")]\n\
         fn f() {}\n",
    );
    repo.commit("drop gone_a through gone_d");

    let literal = context(Source::StringLiteral, Kind::Text);
    assert_eq!(
        found(&repo),
        [
            (
                "fixture::gone_b".to_owned(),
                "src/lib.rs".to_owned(),
                4,
                19,
                literal,
                "fixture::gone_b".to_owned()
            ),
            (
                "fixture::gone_c".to_owned(),
                "src/lib.rs".to_owned(),
                6,
                25,
                literal,
                "fixture::gone_c".to_owned()
            )
        ]
    );
}

#[test]
fn a_decorated_block_doc_fences_its_content_as_a_code_block() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "pub fn a() {}\n\n/**\n * ```\n * names fixture::gone\n */\npub fn kept() {}\n",
    );
    repo.commit("drop gone");

    let block = context(Source::DocComment, Kind::CodeBlock { compiled: true });
    assert_eq!(
        found(&repo),
        [(
            "fixture::gone".to_owned(),
            "src/lib.rs".to_owned(),
            5,
            10,
            block,
            "fixture::gone".to_owned()
        )]
    );
}

#[test]
fn an_indented_block_doc_line_keeps_its_indent() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "/**\n    names fixture::gone\n */\npub fn kept() {}\n",
    );
    repo.commit("drop gone");

    let text = context(Source::DocComment, Kind::Text);
    assert_eq!(
        found(&repo),
        [(
            "fixture::gone".to_owned(),
            "src/lib.rs".to_owned(),
            2,
            11,
            text,
            "fixture::gone".to_owned()
        )]
    );
}

#[test]
fn the_doc_indent_is_the_shortest_line_indent() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "/**\n        fixture::gone\n    ab\n */\npub fn kept() {}\n",
    );
    repo.commit("drop gone");

    let block = context(Source::DocComment, Kind::CodeBlock { compiled: true });
    assert_eq!(
        found(&repo),
        [(
            "fixture::gone".to_owned(),
            "src/lib.rs".to_owned(),
            2,
            9,
            block,
            "fixture::gone".to_owned()
        )]
    );
}

#[test]
fn an_uneven_doc_line_keeps_the_mention_out_of_code() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "/**\n       fixture::gone\n    ab\n */\npub fn kept() {}\n",
    );
    repo.commit("drop gone");

    let text = context(Source::DocComment, Kind::Text);
    assert_eq!(
        found(&repo),
        [(
            "fixture::gone".to_owned(),
            "src/lib.rs".to_owned(),
            2,
            8,
            text,
            "fixture::gone".to_owned()
        )]
    );
}

#[test]
fn rustdoc_fence_attributes_mark_the_block_compiled() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "/// ```rust\n/// fixture::gone();\n/// ```\n///\n/// ```rust,E0432\n/// fixture::gone();\n/// ```\n///\n/// ```rust,{x}\n/// fixture::gone();\n/// ```\n///\n/// ```rust,Ex\n/// fixture::gone();\n/// ```\npub fn kept() {}\n",
    );
    repo.commit("drop gone");

    let compiled = context(Source::DocComment, Kind::CodeBlock { compiled: true });
    let uncompiled = context(Source::DocComment, Kind::CodeBlock { compiled: false });
    assert_eq!(
        found(&repo),
        [
            (
                "fixture::gone".to_owned(),
                "src/lib.rs".to_owned(),
                2,
                5,
                compiled,
                "fixture::gone".to_owned()
            ),
            (
                "fixture::gone".to_owned(),
                "src/lib.rs".to_owned(),
                6,
                5,
                compiled,
                "fixture::gone".to_owned()
            ),
            (
                "fixture::gone".to_owned(),
                "src/lib.rs".to_owned(),
                10,
                5,
                compiled,
                "fixture::gone".to_owned()
            ),
            (
                "fixture::gone".to_owned(),
                "src/lib.rs".to_owned(),
                14,
                5,
                uncompiled,
                "fixture::gone".to_owned()
            ),
        ]
    );
}

#[test]
fn commented_out_code_is_skipped_unless_the_config_asks_for_it() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone_fn() {}\npub fn kept() {}\n");
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "/* fixture::gone_fn();\n   let x = gone_fn(); */\n// let y = fixture::gone_fn();\n// gone_fn();\n\n// fn old() { fixture::gone_fn(); }\n\n// let total = fixture::gone_fn(\n//     1,\n// );\n\n// Prose names fixture::gone_fn here.\n/// Docs name `fixture::gone_fn`.\npub fn kept() {}\n",
    );
    repo.commit("drop gone_fn");
    let rows = |repo: &Repo| -> Vec<(usize, usize, Source, String)> {
        found(repo)
            .into_iter()
            .map(|m| (m.2, m.3, m.4.source, m.5))
            .collect()
    };
    let row = |line, column, source, text: &str| (line, column, source, text.to_owned());
    let path = "fixture::gone_fn";
    let (comment, doc) = (Source::Comment, Source::DocComment);

    assert_eq!(
        rows(&repo),
        [row(12, 16, comment, path), row(13, 16, doc, path)]
    );

    repo.write(".refquiem.toml", "scan-commented-code = true\n");
    assert_eq!(
        rows(&repo),
        [
            row(1, 4, comment, path),
            row(2, 12, comment, "gone_fn"),
            row(3, 12, comment, path),
            row(4, 4, comment, "gone_fn"),
            row(6, 15, comment, path),
            row(8, 16, comment, path),
            row(12, 16, comment, path),
            row(13, 16, doc, path),
        ]
    );
}

#[test]
fn a_string_literal_holding_one_identifier_is_data() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub const EXCEPT_TABLE: u8 = 0;\npub fn seq_shuffle_100() {}\npub fn gone_fn() {}\npub mod old {\n    pub struct Item;\n}\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "pub const ARG: &str = \"EXCEPT_TABLE\";\npub fn bench(c: &mut Bencher) { c.bench_function(\"seq_shuffle_100\", |b| b.iter(|| ())); }\npub const PATH: &str = \"fixture::old::Item\";\npub const HINT: &str = \"use gone_fn instead\";\n",
    );
    repo.commit("drop them");

    let rows: Vec<(usize, usize, String)> =
        found(&repo).into_iter().map(|m| (m.2, m.3, m.5)).collect();
    assert_eq!(
        rows,
        [
            (4, 29, "gone_fn".to_owned()),
            (3, 25, "fixture::old::Item".to_owned())
        ]
    );
}

#[test]
fn a_doc_comment_runs_across_doc_attributes_plain_comments_and_attributes() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone_fn() {}\npub fn gone_too() {}\n");
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "//! Crate docs.\n#![doc = \"Inner names `fixture::gone_too`.\"]\n\n/// ```\n/// let a = 1;\n#[cfg_attr(not(doctest), doc = \"     ...\")]\n/// let b = 2;\n/// ```\n///\n/// Prose names gone_fn.\npub fn one() {}\n\n/// ```text\n/// error output\n//    --> src/main.rs\n//  note: gone_too was here\n/// ```\n///\n/// Prose names gone_too here.\npub fn two() {}\n\n#[doc = \"Docs name `fixture::gone_fn` too.\"]\n#[doc = r\"Raw names `fixture::gone_too`.\"]\n#[doc = \"First line.\nSecond names `fixture::gone_fn`.\"]\npub fn three() {}\n\n/// Docs.\n#[deprecated(note = \"use fixture::gone_fn\")]\npub fn four() {}\n",
    );
    repo.commit("drop them");

    let rows: Vec<(usize, usize, Context, String)> = found(&repo)
        .into_iter()
        .map(|m| (m.2, m.3, m.4, m.5))
        .collect();
    let doc = |kind| context(Source::DocComment, kind);
    let text = |source| context(source, Kind::Text);
    let row = |line, column, context, mention: &str| (line, column, context, mention.to_owned());
    let (gone_fn, gone_too) = ("fixture::gone_fn", "fixture::gone_too");
    assert_eq!(
        rows,
        [
            row(10, 17, doc(Kind::Text), "gone_fn"),
            row(22, 21, doc(Kind::CodeSpan), gone_fn),
            row(25, 15, doc(Kind::CodeSpan), gone_fn),
            row(29, 26, text(Source::StringLiteral), gone_fn),
            row(2, 24, doc(Kind::CodeSpan), gone_too),
            row(16, 11, text(Source::Comment), "gone_too"),
            row(19, 17, doc(Kind::Text), "gone_too"),
            row(23, 22, doc(Kind::CodeSpan), gone_too),
        ]
    );
}

#[test]
fn identifiers_in_attribute_arguments_of_a_compiled_example_are_not_names() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct VariadicArgs {\n    pub argument_count: u8,\n}\npub struct Container {\n    pub rename_all: u8,\n}\npub struct RemovedDerive;\npub struct GoneType;\npub fn gone_attr() {}\npub fn gone_inner() {}\npub fn gone_arg() {}\npub fn gone_lint() {}\npub fn gone_idx() {}\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "/// ```\n/// #![allow(gone_inner)]\n/// #[variadic(argument_count)]\n/// #[serde(rename_all = \"camelCase\")]\n/// #[cfg_attr(test, derive(RemovedDerive), gone_arg)]\n/// #[cfg_attr(test, allow(gone_lint))]\n/// #[gone_attr]\n/// struct S { field: GoneType }\n/// let x = data[(gone_idx)];\n/// ```\npub struct Kept;\n",
    );
    repo.commit("drop them");

    let rows: Vec<(usize, usize, String)> =
        found(&repo).into_iter().map(|m| (m.2, m.3, m.5)).collect();
    assert_eq!(
        rows,
        [
            (8, 23, "GoneType".to_owned()),
            (5, 29, "RemovedDerive".to_owned()),
            (7, 7, "gone_attr".to_owned()),
            (9, 19, "gone_idx".to_owned())
        ]
    );
}

#[test]
fn a_rust_example_in_a_plain_comment_declares_its_own_names() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct GoneAlias;\nimpl GoneAlias {\n    pub fn gone_assoc() {}\n}\npub fn gone_call() {}\npub struct GoneOther;\npub struct GoneText;\n",
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "// Implement HasTable for tuples.\n//\n// For example:\n// ```rust,ignore\n// type GoneAlias = <(a, b) as HasTable>::Table;\n// let t = GoneAlias::gone_assoc();\n// ::GoneAlias::gone_assoc();\n// gone_call();\n// ```\n//\n// A `GoneOther` outside the example counts, and so does a text example:\n// ```text\n// type GoneText = u8;\n// ```\npub struct Kept;\n",
    );
    repo.commit("drop them");

    let rows: Vec<(String, usize)> = found(&repo).into_iter().map(|m| (m.0, m.2)).collect();
    assert_eq!(
        rows,
        [
            ("fixture::GoneAlias::gone_assoc".to_owned(), 7),
            ("fixture::GoneOther".to_owned(), 11),
            ("fixture::GoneText".to_owned(), 13),
            ("fixture::gone_call".to_owned(), 8)
        ]
    );
}

#[test]
fn attribute_boundaries_preserve_string_literal_mentions() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct Data { #[serde(rename = \"old-alias-842\")] pub value: u8 }\npub struct GoneThing;\npub struct ArrayThing;\n",
    );
    repo.commit("structure and serialized field");
    repo.write(
        "src/lib.rs",
        "#![allow(dead_code, reason = \"old-alias-842\")]\n#[attr([inner] \"GoneThing went away\")]\npub struct Kept;\npub const DIRECT: &str = \"GoneThing went away\";\npub const ARRAY: [&str; 1] = [\"ArrayThing went away\"];\n",
    );
    repo.commit("drop the structures");

    let mentions: Vec<_> = found(&repo)
        .into_iter()
        .map(|row| (row.0, row.2, row.5))
        .collect();
    assert_eq!(
        mentions,
        [
            ("fixture::ArrayThing".to_owned(), 5, "ArrayThing".to_owned()),
            ("fixture::GoneThing".to_owned(), 4, "GoneThing".to_owned()),
        ]
    );
}

#[test]
fn a_doc_attribute_keeps_its_example_bindings_local() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub fn retired_probe_842() {}\npub struct RetiredOutside842;\n",
    );
    repo.commit("function and structure");
    repo.write(
        "src/lib.rs",
        "#[doc = r\"```\nlet retired_probe_842 = 1;\nretired_probe_842;\n```\n\nUse `RetiredOutside842`.\n\"]\npub struct Kept;\n",
    );
    repo.commit("drop the items");

    let mentions: Vec<_> = found(&repo).into_iter().map(|row| (row.0, row.5)).collect();
    assert_eq!(
        mentions,
        [(
            "fixture::RetiredOutside842".to_owned(),
            "RetiredOutside842".to_owned()
        )]
    );
}
