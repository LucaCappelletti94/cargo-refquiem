//! Where mentions sit: doc comment and Markdown structure, links, string literals, and the
//! standard library paths that keep a spelling alive.

mod common;

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

pub const MESSAGE: &str = \"use crate::gone instead\";

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
                "crate::gone"
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
