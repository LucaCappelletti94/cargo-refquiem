//! Link destinations, rustdoc page URLs and code spans, and where each mention points.

mod common;
#[path = "common/found_kinds.rs"]
mod found_kinds;
#[path = "common/vendor.rs"]
mod vendor;

use common::Repo;
use found_kinds::found;
use refquiem::ItemKind;

fn mention(
    name: &str,
    kind: ItemKind,
    file: &str,
    line: usize,
    column: usize,
    text: &str,
) -> (String, ItemKind, String, usize, usize, String) {
    (
        name.to_owned(),
        kind,
        file.to_owned(),
        line,
        column,
        text.to_owned(),
    )
}

#[test]
fn text_after_an_autolink_is_scanned_like_the_autolink_url() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.commit("start");
    repo.write("src/lib.rs", "");
    repo.write(
        "README.md",
        "See <https://docs.rs/fixture/latest/fixture/fn.gone.html> then fixture::gone.\n",
    );
    repo.commit("drop gone");

    let url = "https://docs.rs/fixture/latest/fixture/fn.gone.html";
    assert_eq!(
        found(&repo),
        [
            mention("fixture::gone", ItemKind::Function, "README.md", 1, 6, url),
            mention(
                "fixture::gone",
                ItemKind::Function,
                "README.md",
                1,
                64,
                "fixture::gone"
            ),
        ]
    );
}

#[test]
fn inline_link_labels_are_scanned_as_running_text() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.commit("start");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "[fixture::gone](crate::gone)\n");
    repo.commit("drop gone");

    assert_eq!(
        found(&repo),
        [
            mention(
                "fixture::gone",
                ItemKind::Function,
                "README.md",
                1,
                2,
                "fixture::gone"
            ),
            mention(
                "fixture::gone",
                ItemKind::Function,
                "README.md",
                1,
                17,
                "crate::gone"
            ),
        ]
    );
}

#[test]
fn double_backtick_spans_start_at_their_content() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.commit("start");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "A span ``fixture::gone`` here.\n");
    repo.commit("drop gone");

    assert_eq!(
        found(&repo),
        [mention(
            "fixture::gone",
            ItemKind::Function,
            "README.md",
            1,
            10,
            "fixture::gone"
        )]
    );
}

#[test]
fn non_lowercase_disambiguators_do_not_open_intra_doc_links() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.commit("start");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "[x](X@fixture::gone)\n");
    repo.commit("drop gone");

    assert_eq!(found(&repo), []);
}

#[test]
fn link_paths_must_start_at_the_destination_start() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.commit("start");
    repo.write("src/lib.rs", "");
    repo.write("README.md", "[x](9fixture::gone)\n");
    repo.commit("drop gone");

    assert_eq!(found(&repo), []);
}

#[test]
fn docs_rs_about_and_release_pages_are_not_crate_links() {
    let repo = Repo::new();
    repo.vendor("releases", "0.1.0", "");
    repo.write("src/lib.rs", "");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
         [dependencies]\nreleases = \"0.1\"\n",
    );
    repo.commit("releases");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    repo.write("README.md", "[docs](https://docs.rs/releases)\n");
    repo.commit("drop releases");

    assert_eq!(found(&repo), []);
}

#[test]
fn published_crate_names_with_hyphens_and_underscores_are_not_dependencies() {
    let repo = Repo::new();
    repo.vendor("gone-crate", "0.1.0", "");
    repo.vendor("under_score", "0.1.0", "");
    repo.write("src/lib.rs", "");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
         [dependencies]\ngone-crate = \"0.1\"\nunder_score = \"0.1\"\n",
    );
    repo.commit("both");
    repo.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    let links = "[a](https://docs.rs/gone-crate) and [b](https://docs.rs/under_score)\n";
    repo.write("README.md", links);
    repo.commit("drop both");

    assert_eq!(found(&repo), []);
    repo.write(
        "README.md",
        &format!(
            "{links}\n```toml\n[dependencies]\ngone-crate = \"0.1\"\nunder_score = \"0.1\"\n```\n"
        ),
    );
    assert_eq!(
        found(&repo),
        [
            mention(
                "gone-crate",
                ItemKind::Dependency,
                "README.md",
                5,
                1,
                "gone-crate"
            ),
            mention(
                "under_score",
                ItemKind::Dependency,
                "README.md",
                6,
                1,
                "under_score"
            ),
        ]
    );
}

#[test]
fn trailing_double_slashes_do_not_break_rustdoc_page_links() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.commit("start");
    repo.write("src/lib.rs", "");
    repo.write(
        "README.md",
        "[x](https://docs.rs/fixture/latest/fixture/fn.gone.html//)\n",
    );
    repo.commit("drop gone");

    let url = "https://docs.rs/fixture/latest/fixture/fn.gone.html//";
    assert_eq!(
        found(&repo),
        [mention(
            "fixture::gone",
            ItemKind::Function,
            "README.md",
            1,
            5,
            url
        )]
    );
}

#[test]
fn unknown_rustdoc_page_kinds_are_not_item_links() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.commit("start");
    repo.write("src/lib.rs", "");
    repo.write(
        "README.md",
        "[x](https://docs.rs/fixture/latest/fixture/widget.gone.html)\n",
    );
    repo.commit("drop gone");

    assert_eq!(found(&repo), []);
}

#[test]
fn a_code_span_after_a_backticked_link_label_is_scanned() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub struct Kept;\npub struct Gone;\n");
    repo.commit("start");
    repo.write("src/lib.rs", "pub struct Kept;\n");
    repo.write("README.md", "See [`Kept`] and `fixture::Gone`.\n");
    repo.commit("drop Gone");

    assert_eq!(
        found(&repo),
        [mention(
            "fixture::Gone",
            ItemKind::Struct,
            "README.md",
            1,
            19,
            "fixture::Gone"
        )]
    );
}

#[test]
fn an_inline_image_destination_names_its_removed_rustdoc_item() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.commit("function");
    repo.write("src/lib.rs", "");
    let url = "https://docs.rs/fixture/latest/fixture/fn.gone.html";
    repo.write("README.md", &format!("![alt]({url})\n"));
    repo.commit("drop the function");

    assert_eq!(
        found(&repo),
        [mention(
            "fixture::gone",
            ItemKind::Function,
            "README.md",
            1,
            8,
            url
        )]
    );
}

#[test]
fn a_rustdoc_url_in_a_plain_comment_stops_before_following_words() {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub fn gone() {}\n");
    repo.commit("function");
    let url = "https://docs.rs/fixture/latest/fixture/fn.gone.html";
    repo.write(
        "src/lib.rs",
        &format!("// see {url} now\npub struct Kept;\n"),
    );
    repo.commit("drop the function");

    assert_eq!(
        found(&repo),
        [mention(
            "fixture::gone",
            ItemKind::Function,
            "src/lib.rs",
            1,
            8,
            url
        )]
    );
}
