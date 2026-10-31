//! The report as JSON, GitHub workflow annotations and a GitHub job summary.

use std::fmt::Write as _;

use crate::{Change, Group, Position, RemovedIn, Report, web};

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
            let message = format!("{}, is still mentioned here", group.headline());
            for mention in &group.mentions {
                let message = match &mention.uppercase {
                    Some(hint) => escape(&format!(
                        "{message}, write `{}` to mark it as `{}`",
                        hint.spelling, hint.vocabulary
                    )),
                    None => escape(&message),
                };
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
                escape(&marker.message())
            );
        }
        for skipped in &self.skipped {
            let _ = writeln!(
                out,
                "::warning title=refquiem::{}",
                escape(&skipped.message())
            );
        }
        out
    }

    /// The report as a Markdown job summary, every mention listed, the places linked to the
    /// repository at `root` when the report names a head commit.
    #[must_use]
    pub fn job_summary(&self, root: Option<&str>) -> String {
        self.summary(root, None)
    }

    /// The job summary for a GitHub step, which hides a summary that passes `limit` bytes, so
    /// the table stops before it and a closing line names the left-out rows.
    #[must_use]
    pub fn github_summary(&self, root: Option<&str>, limit: usize) -> String {
        self.summary(root, Some(limit))
    }

    /// The table with one row per group, the closing when every row fits, the notice when
    /// `limit` stops the table earlier.
    fn summary(&self, root: Option<&str>, limit: Option<usize>) -> String {
        let mut out = self.prefix();
        let mut kept = 0;
        let closing = self.closing(root);
        let tail = closing.len().max(notice(self.groups.len()).len());
        for (i, group) in self.groups.iter().enumerate() {
            let row = self.row(group, root);
            let separator = usize::from(i > 0);
            if limit.is_some_and(|limit| out.len() + separator + row.len() + 1 + tail > limit) {
                break;
            }
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&row);
            kept += 1;
        }
        if kept > 0 {
            out.push('\n');
        }
        if kept == self.groups.len() {
            out.push_str(&closing);
        } else {
            out.push_str(&notice(self.groups.len() - kept));
        }
        out
    }

    /// The heading and the table's heading, or the notice when nothing is stale.
    fn prefix(&self) -> String {
        if self.groups.is_empty() {
            return String::from("## refquiem\n\nNo stale mentions\n");
        }
        format!(
            "## refquiem\n\n{}\n\nFindings and replacement suggestions require verification before editing.\n\n| Removed name | Kind | Removed in | Status | Suggested action | Mentions |\n|---|---|---|---|---|---|\n",
            self.tally()
        )
    }

    /// One table row of the job summary.
    fn row(&self, group: &Group, root: Option<&str>) -> String {
        let mut name = String::new();
        span(&mut name, &group.path);
        if let Some(member) = &group.member {
            name.push_str(" of ");
            span(&mut name, member);
        }
        let name = if let (Some(root), Some(base), Some(at)) =
            (root, self.base.as_deref(), group.declared_at.as_ref())
        {
            format!("[{name}]({})", web::blob(root, base, &at.file, at.line))
        } else {
            name
        };
        let removed_in = match &group.removed_in {
            Some(RemovedIn::Commit(id)) => {
                let short = id.get(..7).unwrap_or(id);
                root.map_or_else(
                    || short.to_owned(),
                    |root| format!("[{short}]({})", web::commit_page(root, id)),
                )
            }
            Some(RemovedIn::WorkingTree) => "working tree".to_owned(),
            None => String::new(),
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
        let mut action = String::new();
        if let Some(suggestion) = &group.suggestion {
            let mut path = String::new();
            span(&mut path, &suggestion.path);
            let path = self.suggested(&path, suggestion.declared_at.as_ref(), root);
            let _ = write!(action, "Verify and update the reference to {path}");
        } else if let Some(candidates) = group.became() {
            let candidates = if group.candidates.len() > 3 {
                folded_candidates(&group.candidates)
            } else {
                candidates
            };
            let _ = write!(
                action,
                "Review candidates before changing this reference ({})",
                cell(&candidates)
            );
        } else {
            action.push_str("Review the mention and remove or rewrite it if stale");
        }
        let mentions = group
            .mentions
            .iter()
            .map(|mention| {
                let mut place = self.position(&mention.file, mention.line, mention.column, root);
                if let Some(hint) = &mention.uppercase {
                    place.push_str(", write ");
                    span(&mut place, &hint.spelling);
                    place.push_str(" to mark it as ");
                    span(&mut place, &hint.vocabulary);
                }
                place
            })
            .collect::<Vec<_>>();
        let mentions = folded(&mentions);
        format!(
            "| {} | {} | {removed_in} | {status} | {action} | {} |",
            cell(&name),
            group.kind.describe(),
            cell(&mentions)
        )
    }

    /// The suggested name, linked to its declaration at the head commit when the report places
    /// it there.
    fn suggested(&self, path: &str, declared_at: Option<&Position>, root: Option<&str>) -> String {
        if let (Some(root), Some(head), Some(at)) = (root, self.head.as_deref(), declared_at) {
            format!("[{path}]({})", web::blob(root, head, &at.file, at.line))
        } else {
            path.to_owned()
        }
    }

    /// The code span of a mention or marker, linked to its line at the head commit when it can
    /// be.
    fn position(&self, file: &str, line: usize, column: usize, root: Option<&str>) -> String {
        let text = span_position(file, line, column);
        match self.head_link(root, file, line) {
            Some(url) => format!("[{text}]({url})"),
            None => text,
        }
    }

    /// The URL of line `line` of `file` at the head commit, allowed by the web root, the head
    /// commit and an unedited file.
    fn head_link(&self, root: Option<&str>, file: &str, line: usize) -> Option<String> {
        let head = self.head.as_deref()?;
        if self.edited.contains(file) {
            return None;
        }
        Some(web::blob(root?, head, file, line))
    }

    /// The sections after the table, the unused suppression markers and the skipped
    /// dependencies.
    fn closing(&self, root: Option<&str>) -> String {
        let mut out = String::new();
        if !self.unused_markers.is_empty() {
            out.push_str("\n### Unused suppression markers\n\n");
            for marker in &self.unused_markers {
                out.push_str("- ");
                out.push_str(&self.position(&marker.file, marker.line, marker.column, root));
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
                out.push_str(" could not be resolved: ");
                span(&mut out, skipped.cause());
                out.push('\n');
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

/// The code span of `file:line:column`, whose numeric suffix prevents the location from ending
/// in a backtick.
fn span_position(file: &str, line: usize, column: usize) -> String {
    let mut out = String::new();
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
    out
}

/// A Markdown table cell, whose `|` would end it.
fn cell(text: &str) -> String {
    text.replace('|', "\\|")
}

/// Up to three of `items` joined with `<br>`, the rest inside a `<details>` element.
fn folded(items: &[String]) -> String {
    if items.len() <= 3 {
        return items.join("<br>");
    }
    let (visible, rest) = items.split_at(3);
    format!(
        "{}<br>and <details><summary>{} more</summary>{}</details>",
        visible.join("<br>"),
        rest.len(),
        rest.join("<br>")
    )
}

/// The first three of `names` with the rest inside a `<details>` element, in the prose a
/// candidates cell uses.
fn folded_candidates(names: &[String]) -> String {
    let (visible, rest) = names.split_at(3);
    let span = |name: &String| format!("`{name}`");
    format!(
        "perhaps {} and <details><summary>{} more</summary>{}</details>",
        visible.iter().map(span).collect::<Vec<_>>().join(", "),
        rest.len(),
        rest.iter().map(span).collect::<Vec<_>>().join("<br>")
    )
}

/// The closing line of a capped summary, naming the rows left out.
fn notice(count: usize) -> String {
    let (verb, object) = if count == 1 {
        ("is", "it")
    } else {
        ("are", "them")
    };
    format!(
        "{} {verb} not shown. `--json` and the annotations list {object}.",
        crate::plural(count, "removed name", "removed names")
    )
}

#[cfg(test)]
mod tests {
    use super::{Report, notice};
    use crate::{Context, Group, HashSet, ItemKind, Kind, Mention, RemovedIn, Source};

    fn mention(file: &str, line: usize) -> Mention {
        Mention {
            file: file.to_owned(),
            line,
            column: 7,
            text: String::new(),
            context: Context {
                source: Source::Comment,
                kind: Kind::Text,
            },
            replacement: None,
            uppercase: None,
        }
    }

    fn group(path: &str, mentions: usize) -> Group {
        Group {
            path: path.to_owned(),
            kind: ItemKind::Function,
            member: None,
            removed_in: Some(RemovedIn::Commit("a".repeat(40))),
            declared_at: None,
            suggestion: None,
            candidates: vec![],
            mentions: (1..=mentions)
                .map(|line| mention("docs.md", line))
                .collect(),
        }
    }

    fn report(groups: Vec<Group>) -> Report {
        Report {
            base: None,
            head: Some("b".repeat(40)),
            groups,
            skipped: vec![],
            unused_markers: vec![],
            edited: HashSet::new(),
        }
    }

    #[test]
    fn the_github_summary_equals_the_job_summary_when_everything_fits() {
        let report = report(vec![group("a::one", 2), group("a::two", 3)]);
        assert_eq!(
            report.github_summary(Some("https://github.com/o/r"), 1_048_576),
            report.job_summary(Some("https://github.com/o/r"))
        );
    }

    #[test]
    fn the_github_summary_names_the_rows_the_cap_leaves_out() {
        let report = report(vec![
            group("a::one", 1),
            group("a::two", 1),
            group("a::three", 1),
        ]);
        let root = Some("https://github.com/o/r");
        let limit =
            report.prefix().len() + report.row(&report.groups[0], root).len() + 1 + notice(2).len();
        let summary = report.github_summary(root, limit);
        assert!(summary.contains("`a::one`"), "{summary}");
        assert!(!summary.contains("`a::two`"), "{summary}");
        assert!(summary.ends_with(&format!("\n{}", notice(2))), "{summary}");
    }

    #[test]
    fn the_github_summary_keeps_no_rows_when_none_fit() {
        let report = report(vec![group("a::one", 1), group("a::two", 1)]);
        let summary = report.github_summary(None, 1);
        assert_eq!(summary, format!("{}{}", report.prefix(), notice(2)));
        assert!(!summary.contains("`a::one`"));
    }

    #[test]
    fn the_notice_counts_the_left_out_names() {
        assert_eq!(
            notice(1),
            "1 removed name is not shown. `--json` and the annotations list it."
        );
        assert_eq!(
            notice(2),
            "2 removed names are not shown. `--json` and the annotations list them."
        );
    }

    #[test]
    fn the_cell_folds_the_mentions_beyond_the_third() {
        let report = report(vec![group("a::gone", 5)]);
        let row = report.row(&report.groups[0], None);
        assert!(
            row.contains(
                "`docs.md:1:7`<br>`docs.md:2:7`<br>`docs.md:3:7`<br>and <details><summary>2 more</summary>`docs.md:4:7`<br>`docs.md:5:7`</details>"
            ),
            "{row}"
        );
    }

    #[test]
    fn the_cell_folds_the_candidates_beyond_the_third() {
        let mut group = group("a::gone", 1);
        group.candidates = ["a::one", "a::two", "a::three", "a::four"]
            .map(str::to_owned)
            .to_vec();
        let report = report(vec![group]);
        let row = report.row(&report.groups[0], None);
        assert!(
            row.contains(
                "(perhaps `a::one`, `a::two`, `a::three` and <details><summary>1 more</summary>`a::four`</details>)"
            ),
            "{row}"
        );
    }
    #[test]
    fn the_removed_in_cell_is_empty_without_a_removal_commit() {
        let mention = Mention {
            file: "docs.md".to_owned(),
            line: 1,
            column: 7,
            text: String::new(),
            context: Context {
                source: Source::Comment,
                kind: Kind::Text,
            },
            replacement: None,
            uppercase: None,
        };
        let report = Report {
            base: None,
            head: None,
            groups: vec![Group {
                path: "a::gone".to_owned(),
                kind: ItemKind::Function,
                member: None,
                removed_in: None,
                declared_at: None,
                suggestion: None,
                candidates: vec![],
                mentions: vec![mention],
            }],
            skipped: vec![],
            unused_markers: vec![],
            edited: HashSet::new(),
        };
        assert!(
            report
                .job_summary(None)
                .contains("| function |  | Removed (no replacement found) |")
        );
    }

    #[test]
    fn the_job_summary_lists_one_row_per_group() {
        let mention = Mention {
            file: "docs.md".to_owned(),
            line: 1,
            column: 7,
            text: String::new(),
            context: Context {
                source: Source::Comment,
                kind: Kind::Text,
            },
            replacement: None,
            uppercase: None,
        };
        let group = |path: &str| Group {
            path: path.to_owned(),
            kind: ItemKind::Function,
            member: None,
            removed_in: None,
            declared_at: None,
            suggestion: None,
            candidates: vec![],
            mentions: vec![mention.clone()],
        };
        let report = Report {
            base: None,
            head: None,
            groups: vec![group("a::one"), group("a::two")],
            skipped: vec![],
            unused_markers: vec![],
            edited: HashSet::new(),
        };
        let mut expected = report.prefix();
        expected.push_str(&report.row(&report.groups[0], None));
        expected.push('\n');
        expected.push_str(&report.row(&report.groups[1], None));
        expected.push('\n');
        assert_eq!(report.job_summary(None), expected);
    }

    #[test]
    fn the_cap_keeps_exactly_the_rows_the_limit_covers() {
        let report = report(vec![
            group("a::one", 1),
            group("b::two", 2),
            group("c::three", 3),
        ]);
        let prefix = report.prefix();
        let tail = report.closing(None).len().max(notice(3).len());
        let rows = [
            report.row(&report.groups[0], None),
            report.row(&report.groups[1], None),
            report.row(&report.groups[2], None),
        ];
        let both = prefix.len() + rows[0].len() + 1 + rows[1].len() + 1 + tail;
        assert_eq!(
            report.github_summary(None, both),
            format!("{}{}\n{}\n{}", prefix, rows[0], rows[1], notice(1))
        );
        assert_eq!(
            report.github_summary(None, both - 1),
            format!("{}{}\n{}", prefix, rows[0], notice(2))
        );
        let first = prefix.len() + rows[0].len() + 1 + tail;
        assert_eq!(
            report.github_summary(None, first),
            format!("{}{}\n{}", prefix, rows[0], notice(2))
        );
        assert_eq!(
            report.github_summary(None, first - 1),
            format!("{}{}", prefix, notice(3))
        );
    }

    #[test]
    fn the_candidates_prose_lists_exactly_three_without_folding() {
        let mut group = group("a::gone", 1);
        group.candidates = ["a::one", "a::two", "a::three"].map(str::to_owned).to_vec();
        let report = report(vec![group]);
        let row = report.row(&report.groups[0], None);
        assert!(
            row.contains(
                "Review candidates before changing this reference (perhaps `a::one`, `a::two` or `a::three`)"
            ),
            "{row}"
        );
    }
}
