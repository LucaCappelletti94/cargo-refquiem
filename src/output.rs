//! The report as JSON, GitHub workflow annotations and a GitHub job summary.

use std::fmt::Write as _;

use crate::{RemovedIn, Report};

impl Report {
    /// The report as JSON, with `snake_case` keys and kinds.
    ///
    /// # Panics
    ///
    /// Never, as a report holds only strings, numbers and options of them.
    #[must_use]
    pub fn json(&self) -> String {
        serde_json::to_string(self).expect("a report always serializes")
    }

    /// One workflow annotation per finding, at `error` level or `warning` when the run cannot fail.
    #[must_use]
    pub fn annotations(&self, no_fail: bool) -> String {
        let level = if no_fail { "warning" } else { "error" };
        let mut out = String::new();
        for group in &self.groups {
            let message = escape(&format!("{}, is still mentioned here", group.headline()));
            for mention in &group.mentions {
                let _ = writeln!(
                    out,
                    "::{level} file={},line={},col={},title=refquiem::{message}",
                    escape_property(&mention.file),
                    mention.line,
                    mention.column
                );
            }
        }
        for marker in &self.unused_markers {
            let _ = writeln!(
                out,
                "::{level} file={},line={},col={},title=refquiem::{}",
                escape_property(&marker.file),
                marker.line,
                marker.column,
                escape(&format!(
                    "refquiem: allow({}) suppresses nothing",
                    marker.name
                ))
            );
        }
        for skipped in &self.skipped {
            let _ = writeln!(
                out,
                "::warning title=refquiem::{}",
                escape(&format!(
                    "removed dependency `{}` could not be resolved, so its items were not checked",
                    skipped.dependency
                ))
            );
        }
        out
    }

    /// The report as a Markdown job summary, every mention listed.
    #[must_use]
    pub fn job_summary(&self) -> String {
        let mut out = String::from("## refquiem\n\n");
        if self.groups.is_empty() {
            out.push_str("No stale mentions\n");
        } else {
            let _ = writeln!(out, "{}\n", self.tally());
            out.push_str("| Removed name | Kind | Removed in | Mentions |\n|---|---|---|---|\n");
            for group in &self.groups {
                let mut name = format!("`{}`", group.path);
                if let Some(member) = &group.member {
                    let _ = write!(name, " of `{member}`");
                }
                let removed_in = match &group.removed_in {
                    Some(RemovedIn::Commit(id)) => id.get(..7).unwrap_or(id),
                    Some(RemovedIn::WorkingTree) => "working tree",
                    None => "",
                };
                let mentions: Vec<String> = group
                    .mentions
                    .iter()
                    .map(|m| format!("`{}:{}:{}`", m.file, m.line, m.column))
                    .collect();
                let _ = writeln!(
                    out,
                    "| {} | {} | {removed_in} | {} |",
                    cell(&name),
                    group.kind.describe(),
                    cell(&mentions.join("<br>"))
                );
            }
        }
        if !self.unused_markers.is_empty() {
            out.push_str("\n### Unused suppression markers\n\n");
            for marker in &self.unused_markers {
                let _ = writeln!(
                    out,
                    "- `{}:{}:{}` `refquiem: allow({})`",
                    marker.file, marker.line, marker.column, marker.name
                );
            }
        }
        if !self.skipped.is_empty() {
            out.push_str("\n### Skipped dependencies\n\n");
            for skipped in &self.skipped {
                let _ = writeln!(out, "- `{}` could not be resolved", skipped.dependency);
            }
        }
        out
    }
}

/// A workflow command message, with the characters that would end it percent encoded.
fn escape(text: &str) -> String {
    text.replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// A workflow command property value, which a `,` or `:` would also end.
fn escape_property(text: &str) -> String {
    escape(text).replace(':', "%3A").replace(',', "%2C")
}

/// A Markdown table cell, whose `|` would end it.
fn cell(text: &str) -> String {
    text.replace('|', "\\|")
}
