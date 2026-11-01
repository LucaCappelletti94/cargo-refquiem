//! The mentions of a scan report with the kinds of their removed names, as end-to-end tests
//! compare them.

use refquiem::ItemKind;

use crate::common::Repo;

/// `(removed path, kind, file, line, column, text)` for one mention.
pub fn mention(
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

/// `(removed path, kind, file, line, column, text)` for every mention, sorted by place.
pub fn found(repo: &Repo) -> Vec<(String, ItemKind, String, usize, usize, String)> {
    let report = refquiem::run(&refquiem::Options::new(repo.path().to_path_buf())).unwrap();
    let mut out: Vec<_> = report
        .groups
        .iter()
        .flat_map(|group| {
            group.mentions.iter().map(|m| {
                (
                    group.path.clone(),
                    group.kind,
                    m.file.clone(),
                    m.line,
                    m.column,
                    m.text.clone(),
                )
            })
        })
        .collect();
    out.sort_by(|a, b| (&a.2, a.3, a.4).cmp(&(&b.2, b.3, b.4)));
    out
}
