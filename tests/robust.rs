//! Inputs the crates refquiem reads with fail on, which a scan must survive.

mod common;

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
fn a_markdown_reader_failure_aborts_the_scan() {
    for (file, body) in [
        ("README.md", "`fixture::Gone`\n\n- [o]:u\n    \t"),
        ("src/lib.rs", "/// - [o]:u\n///     \t\npub struct Kept;\n"),
        ("src/lib.rs", "/**- [o]:u\n    \t*/\npub struct Kept;\n"),
    ] {
        let repo = Repo::new();
        repo.write("src/lib.rs", "pub struct Gone;\n");
        repo.commit("public API");
        repo.write("src/lib.rs", "");
        repo.write(file, body);
        repo.write("NOTES.md", "`fixture::Gone`\n");
        repo.commit("remove API");
        let Err(error) = refquiem::run(&Options::new(repo.path().to_path_buf())) else {
            panic!("incomplete scan succeeded for {file}");
        };
        assert!(error.to_string().contains(file), "{error}");
    }
}
