//! The mentions of a scan report grouped by removed name, as end-to-end tests compare them.

use refquiem::Context;

use crate::common::Repo;
use crate::found::found;

/// `(removed path, file, line, column, context, text)` for every mention, sorted by removed
/// path, then by place.
pub fn found_by_path(repo: &Repo) -> Vec<(String, String, usize, usize, Context, String)> {
    let mut out = found(repo);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}
