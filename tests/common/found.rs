//! The mentions of a scan report with their contexts, as end-to-end tests compare them.

use refquiem::Context;

use crate::common::Repo;

/// `(removed path, file, line, column, context, text)` for every mention, sorted by place.
pub fn found(repo: &Repo) -> Vec<(String, String, usize, usize, Context, String)> {
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
    out.sort_by(|a, b| (&a.1, a.2, a.3).cmp(&(&b.1, b.2, b.3)));
    out
}
