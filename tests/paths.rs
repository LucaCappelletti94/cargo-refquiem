//! Paths in prose and code spans, with generic arguments, raw identifiers and boundaries.

mod common;
#[path = "common/found.rs"]
mod found;

use common::Repo;
use found::found;
use refquiem::{Context, Kind, Source};

fn markdown(kind: Kind) -> Context {
    Context {
        source: Source::Markdown,
        kind,
    }
}

#[test]
fn a_leading_colon_path_at_the_start_of_text_is_found() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub mod gone {\n    pub struct Thing;\n}\n");
    repo.commit("start");
    repo.write("src/lib.rs", "pub struct Kept;\n");
    repo.write("README.md", "::gone::Thing here.\n");
    repo.commit("drop gone");

    assert_eq!(
        found(&repo),
        [(
            "fixture::gone::Thing".to_owned(),
            "README.md".to_owned(),
            1,
            1,
            markdown(Kind::Text),
            "::gone::Thing".to_owned(),
        )]
    );
}

#[test]
fn a_path_glued_to_a_hash_is_not_found() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub mod gone {\n    pub struct Thing;\n}\n");
    repo.commit("start");
    repo.write("src/lib.rs", "pub struct Kept;\n");
    repo.write("README.md", "Jump to #gone::Thing for details.\n");
    repo.commit("drop gone");

    assert_eq!(found(&repo), []);
}

#[test]
fn a_path_with_generic_arguments_between_segments_is_found() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub enum Pick {\n    None,\n}\n");
    repo.commit("start");
    repo.write("src/lib.rs", "pub struct Kept;\n");
    repo.write("README.md", "The `Pick<T>::None` variant.\n");
    repo.commit("drop Pick");

    assert_eq!(
        found(&repo),
        [(
            "fixture::Pick::None".to_owned(),
            "README.md".to_owned(),
            1,
            6,
            markdown(Kind::CodeSpan),
            "Pick<T>::None".to_owned(),
        )]
    );
}

#[test]
fn an_angle_with_parentheses_breaks_a_path() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub mod gone {\n    pub mod mid {\n        pub struct end;\n    }\n}\n",
    );
    repo.commit("start");
    repo.write("src/lib.rs", "pub struct Kept;\n");
    repo.write("README.md", "See `gone<f()>::mid::end`.\n");
    repo.commit("drop gone");

    assert_eq!(
        found(&repo),
        [(
            "fixture::gone::mid::end".to_owned(),
            "README.md".to_owned(),
            1,
            15,
            markdown(Kind::CodeSpan),
            "::mid::end".to_owned(),
        )]
    );
}

#[test]
fn a_path_spelled_with_a_raw_identifier_is_found() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub mod m {\n    pub struct r#loop;\n}\n");
    repo.commit("start");
    repo.write("src/lib.rs", "pub struct Kept;\n");
    repo.write("README.md", "The `m::r#loop` item.\n");
    repo.commit("drop loop");

    assert_eq!(
        found(&repo),
        [(
            "fixture::m::loop".to_owned(),
            "README.md".to_owned(),
            1,
            6,
            markdown(Kind::CodeSpan),
            "m::r#loop".to_owned(),
        )]
    );
}

#[test]
fn a_path_with_a_turbofish_between_segments_is_found() {
    let repo = Repo::new();
    repo.write(
        "src/lib.rs",
        "pub struct Holder;\nimpl Holder {\n    pub fn make() {}\n}\n",
    );
    repo.commit("start");
    repo.write("src/lib.rs", "pub struct Holder;\n");
    repo.write(
        "README.md",
        "`fixture::Holder::<u8>::make()` and `Holder::<u8>::make`\n",
    );
    repo.commit("drop make");

    let row = |column, text: &str| {
        (
            "fixture::Holder::make".to_owned(),
            "README.md".to_owned(),
            1,
            column,
            markdown(Kind::CodeSpan),
            text.to_owned(),
        )
    };
    assert_eq!(
        found(&repo),
        [
            row(2, "fixture::Holder::<u8>::make"),
            row(38, "Holder::<u8>::make")
        ]
    );
}

#[test]
fn a_path_from_the_crate_root_of_a_code_block_names_the_example_itself() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub mod gone {\n    pub struct Thing;\n}\n");
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        "/// `crate::gone::Thing`\n///\n/// ```\n/// crate::gone::Thing;\n/// fixture::gone::Thing;\n/// ```\n///\n/// ```ignore\n/// self::gone::Thing;\n/// ```\npub struct Kept;\n",
    );
    repo.write("README.md", "```rust\ncrate::gone::Thing;\n```\n");
    repo.commit("drop gone");

    let rows: Vec<(String, String, usize, usize, String)> = found(&repo)
        .into_iter()
        .map(|m| (m.0, m.1, m.2, m.3, m.5))
        .collect();
    let row = |line, column, text: &str| {
        (
            "fixture::gone::Thing".to_owned(),
            "src/lib.rs".to_owned(),
            line,
            column,
            text.to_owned(),
        )
    };
    assert_eq!(
        rows,
        [
            row(1, 6, "crate::gone::Thing"),
            row(5, 5, "fixture::gone::Thing")
        ]
    );
}

#[test]
fn a_path_from_a_generic_parameter_of_a_compiled_example_names_nothing() {
    let repo = Repo::new();
    let svc = "pub trait Svc {\n    type Future;\n    type Output;\n}\n";
    repo.write(
        "src/lib.rs",
        &format!("{svc}pub struct S;\nimpl Svc for S {{\n    type Future = ();\n    type Output = ();\n}}\npub struct F;\nimpl Svc for F {{\n    type Future = ();\n    type Output = ();\n}}\n"),
    );
    repo.commit("start");
    repo.write(
        "src/lib.rs",
        &format!("{svc}/// ```\n/// fn run<I, S>(io: I, svc: S) where S::Future: Send {{}}\n/// impl<F> Executor<F> for Tokio where F::Output: Send {{}}\n/// ```\n///\n/// `S::Future` names the type.\n///\n/// ```\n/// let x: S::Future = todo();\n/// ```\npub struct Kept;\n"),
    );
    repo.commit("drop S and F");

    let rows: Vec<(String, usize, usize, String)> = found(&repo)
        .into_iter()
        .map(|m| (m.0, m.2, m.3, m.5))
        .collect();
    let row = |line, column| {
        (
            "fixture::S::Future".to_owned(),
            line,
            column,
            "S::Future".to_owned(),
        )
    };
    assert_eq!(rows, [row(10, 6), row(13, 12)]);
}
