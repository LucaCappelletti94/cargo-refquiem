//! The removed paths of a scan report that end-to-end tests compare.

/// The removed paths that still have mentions, sorted.
pub fn removed(report: &refquiem::Report) -> Vec<String> {
    let mut out: Vec<String> = report.groups.iter().map(|g| g.path.clone()).collect();
    out.sort();
    out
}
