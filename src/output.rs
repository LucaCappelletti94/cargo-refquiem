//! The report as JSON, GitHub workflow annotations and a GitHub job summary.

use std::fmt::Write as _;

use crate::{Change, RemovedIn, Report};

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
            out.push_str(
                "Findings and replacement suggestions require verification before editing.\n\n",
            );
            out.push_str(
                "| Removed name | Kind | Removed in | Status | Suggested action | Mentions |\n|---|---|---|---|---|---|\n",
            );
            for group in &self.groups {
                let mut name = String::new();
                span(&mut name, &group.path);
                if let Some(member) = &group.member {
                    name.push_str(" of ");
                    span(&mut name, member);
                }
                let removed_in = match &group.removed_in {
                    Some(RemovedIn::Commit(id)) => id.get(..7).unwrap_or(id),
                    Some(RemovedIn::WorkingTree) => "working tree",
                    None => "",
                };
                let status = match group
                    .suggestion
                    .as_ref()
                    .map(|suggestion| suggestion.change)
                {
                    Some(Change::Moved) => "Possible move",
                    Some(Change::Renamed) => "Possible rename",
                    Some(Change::Replaced) => "Possible replacement",
                    None if group.candidates.is_empty() => "Removed (no replacement found)",
                    None => "Ambiguous replacement",
                };
                let mut mentions = String::new();
                for (i, mention) in group.mentions.iter().enumerate() {
                    if i > 0 {
                        mentions.push_str("<br>");
                    }
                    span_position(&mut mentions, &mention.file, mention.line, mention.column);
                }
                let _ = write!(
                    out,
                    "| {} | {} | {removed_in} | {status} | ",
                    cell(&name),
                    group.kind.describe(),
                );
                if let Some(suggestion) = &group.suggestion {
                    let mut path = String::new();
                    span(&mut path, &suggestion.path);
                    let _ = write!(out, "Verify and update the reference to {}", cell(&path));
                } else if let Some(candidates) = group.became() {
                    let _ = write!(
                        out,
                        "Review candidates before changing this reference ({})",
                        cell(&candidates)
                    );
                } else {
                    out.push_str("Review the mention and remove or rewrite it if stale");
                }
                let _ = writeln!(out, " | {} |", cell(&mentions));
            }
        }
        if !self.unused_markers.is_empty() {
            out.push_str("\n### Unused suppression markers\n\n");
            for marker in &self.unused_markers {
                out.push_str("- ");
                span_position(&mut out, &marker.file, marker.line, marker.column);
                out.push_str(" `refquiem: allow(");
                out.push_str(&marker.name);
                out.push_str(")`\n");
            }
        }
        if !self.skipped.is_empty() {
            out.push_str("\n### Skipped dependencies\n\n");
            for skipped in &self.skipped {
                out.push_str("- ");
                span(&mut out, &skipped.dependency);
                out.push_str(" could not be resolved\n");
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

/// Writes a code span with padding that preserves backtick and space edges.
fn span(out: &mut String, text: &str) {
    let edge = |c: Option<char>| c.map(|c| if matches!(c, '\n' | '\r') { ' ' } else { c });
    let first = edge(text.chars().next());
    let last = edge(text.chars().next_back());
    let all_space = text.chars().all(|c| matches!(c, ' ' | '\n' | '\r'));
    let pad = !all_space
        && (first.is_some_and(|c| c == '`')
            || last.is_some_and(|c| c == '`')
            || (first.is_some_and(|c| c == ' ') && last.is_some_and(|c| c == ' ')));
    let run = text
        .split(|c: char| c != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    for _ in 0..=run {
        out.push('`');
    }
    if pad {
        out.push(' ');
    }
    for c in text.chars() {
        out.push(if matches!(c, '\n' | '\r') { ' ' } else { c });
    }
    if pad {
        out.push(' ');
    }
    for _ in 0..=run {
        out.push('`');
    }
}

/// The numeric suffix prevents the location from ending in a backtick.
fn span_position(out: &mut String, file: &str, line: usize, column: usize) {
    let first = file
        .chars()
        .next()
        .map(|c| if matches!(c, '\n' | '\r') { ' ' } else { c });
    let pad = first.is_some_and(|c| c == '`');
    let run = file
        .split(|c: char| c != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    for _ in 0..=run {
        out.push('`');
    }
    if pad {
        out.push(' ');
    }
    for c in file.chars() {
        out.push(if matches!(c, '\n' | '\r') { ' ' } else { c });
    }
    let _ = write!(out, ":{line}:{column}");
    if pad {
        out.push(' ');
    }
    for _ in 0..=run {
        out.push('`');
    }
}

/// A Markdown table cell, whose `|` would end it.
fn cell(text: &str) -> String {
    text.replace('|', "\\|")
}
