//! Running text that states a removed name's removal, which mentions it as gone.

mod common;

use common::Repo;
use refquiem::Options;

/// A repository whose `fixture::Gone` and `fixture::Other` were removed.
fn removed() -> Repo {
    let repo = Repo::new();
    repo.write("src/lib.rs", "pub struct Gone;\npub struct Other;\n");
    repo.commit("start");
    repo.write("src/lib.rs", "pub struct Kept;\n");
    repo
}

/// `(file, line, column)` of every mention still reported, sorted.
fn reported(repo: &Repo) -> Vec<(String, usize, usize)> {
    let report = refquiem::run(&Options::new(repo.path().to_path_buf())).unwrap();
    let mut out: Vec<_> = report
        .groups
        .iter()
        .flat_map(|g| {
            g.mentions
                .iter()
                .map(|m| (m.file.clone(), m.line, m.column))
        })
        .collect();
    out.sort();
    out
}

fn at(file: &str, line: usize, column: usize) -> (String, usize, usize) {
    (file.to_owned(), line, column)
}

#[test]
fn markdown_that_states_the_removal_is_not_reported() {
    let repo = removed();
    repo.write(
        "PLAN.md",
        "3. Delete `fixture::Gone` and `fixture::Other`, and move the test onto the code.\n\
         4. No classification reads a message, `Gone` and `Other` do not exist, and the chapter says so.\n\n\
         When the classification becomes code-driven, `fixture::Gone` is deleted, and R106 owns it.\n\n\
         A read still ends at once with `Gone` gone.\n\n\
         The removal of `fixture::Other` closes the phase. `Gone`, `Other`, and `Kept` have been removed.\n\n\
         ## Removing `fixture::Gone`\n\n\
         ## `fixture::Other` removed\n\n\
         **`fixture::Other` no longer exists.** `Gone` doesn\u{2019}t exist either.\n",
    );
    repo.commit("record the removal");

    assert_eq!(reported(&repo), []);
}

#[test]
fn markdown_that_describes_the_name_as_current_is_reported() {
    let repo = removed();
    repo.write(
        "PLAN.md",
        "`fixture::Gone` recognises a timeout by the words in the message.\n\n\
         Call `fixture::Gone`, then delete the cache.\n\n\
         Remove the entry with `fixture::Other`.\n\n\
         `fixture::Gone` is not removed, and `fixture::Other` isn't deleted.\n\n\
         Delete the cache and keep `fixture::Gone`.\n\n\
         `fixture::Other`\n\n\
         is deleted.\n\n\
         Delete [`Other`](crate::Other).\n\n\
         `fixture::Gone` removed the stale rows.\n",
    );
    repo.commit("describe it");

    assert_eq!(
        reported(&repo),
        [
            at("PLAN.md", 1, 2),
            at("PLAN.md", 3, 7),
            at("PLAN.md", 5, 24),
            at("PLAN.md", 7, 2),
            at("PLAN.md", 7, 38),
            at("PLAN.md", 9, 28),
            at("PLAN.md", 11, 2),
            at("PLAN.md", 15, 18),
            at("PLAN.md", 17, 2),
        ]
    );
}

#[test]
fn comments_and_doc_comments_that_state_the_removal_are_not_reported() {
    let repo = removed();
    repo.write(
        "src/notes.rs",
        "// `fixture::Gone` is\n// deleted, so nothing calls it.\n\
         /// Deleting `fixture::Other` left this.\n\
         /* fixture::Other was removed */\n\
         pub fn kept() -> &'static str {\n    \"fixture::Gone no longer exists\"\n}\n\
         // fixture::Gone answers.\n//\n// Removed in R106.\n\
         /// `fixture::Other` recognises it.\npub struct Kept;\n",
    );
    repo.commit("record it in comments");

    assert_eq!(
        reported(&repo),
        [at("src/notes.rs", 8, 4), at("src/notes.rs", 11, 6)]
    );
}

#[test]
fn markdown_that_records_a_rename_or_replacement_is_not_reported() {
    let repo = removed();
    repo.write(
        "PLAN.md",
        "1. Rename `fixture::Gone` to `fixture::Kept`.\n\
         2. Supersede `fixture::Other`.\n\n\
         `fixture::Other` was renamed `fixture::Kept`. `Gone` is replaced by `Kept`.\n\n\
         `fixture::Gone` becomes `fixture::Kept`, with `fixture::Other` becoming `Kept`.\n\n\
         `Kept` carries the fields in place of `fixture::Other`, and `Kept` replaces `fixture::Gone`.\n\n\
         `fixture::Kept` (renamed from `fixture::Gone`) is the type.\n\n\
         `fixture::Other` (`src/lib.rs`) is superseded by `Kept`.\n\n\
         `Kept` therefore loses `fixture::Gone` and `fixture::Other`.\n\n\
         `fixture::Gone` must go.\n\n\
         ## `fixture::Other` renamed to `fixture::Kept`\n",
    );
    repo.commit("record the renames");

    assert_eq!(reported(&repo), []);
}

#[test]
fn markdown_that_describes_a_renamed_name_as_current_is_reported() {
    let repo = removed();
    repo.write(
        "PLAN.md",
        "`fixture::Gone` becomes internal.\n\n\
         `fixture::Other` becomes the value of the field.\n\n\
         The type was renamed `fixture::Gone`.\n\n\
         `fixture::Gone` replaced `fixture::Other` in the handler.\n\n\
         `fixture::Other` goes into the cache.\n\n\
         `fixture::Gone` was not renamed.\n",
    );
    repo.commit("describe them");

    assert_eq!(
        reported(&repo),
        [
            at("PLAN.md", 1, 2),
            at("PLAN.md", 3, 2),
            at("PLAN.md", 5, 23),
            at("PLAN.md", 7, 2),
            at("PLAN.md", 9, 2),
            at("PLAN.md", 11, 2),
        ]
    );
}
