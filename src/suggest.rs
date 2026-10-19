//! What a removed name most likely became, and the rewrites of its mentions that follow.

use crate::index::Index;
use crate::intern::{Interner, PathId};
use crate::{Change, Group, ItemKind, Kind};

/// The rules' outcome for one removed name.
#[derive(Default)]
pub(crate) struct Outcome {
    /// The only candidate of the first rule that found exactly one.
    pub(crate) suggestion: Option<(PathId, Change)>,
    /// Without a suggestion, the candidates of the earliest rule that found several.
    pub(crate) candidates: Vec<PathId>,
}

/// Applies the rules to head names introduced after the removed name.
pub(crate) fn suggest(
    interner: &Interner,
    removed: PathId,
    head: &Index,
    before: &Index,
    first_seen: &std::collections::HashMap<PathId, usize>,
    first: usize,
) -> Outcome {
    let kind = interner.kind(removed);
    let Some((&name, parent)) = interner.segments(removed).split_last() else {
        return Outcome::default();
    };
    let pool: Vec<PathId> = head
        .paths
        .iter()
        .copied()
        .filter(|&id| {
            interner.kind(id) == kind
                && !before.paths.contains(&id)
                && first_seen.get(&id).is_none_or(|&at| at < first)
        })
        .collect();
    let split = |id: PathId| {
        interner
            .segments(id)
            .split_last()
            .expect("an interned path has segments")
    };
    let moved = |id: PathId| {
        let (&other, other_parent) = split(id);
        !kind.is_manifest_entry() && other == name && other_parent != parent
    };
    let near = |id: PathId| {
        let (&other, other_parent) = split(id);
        other_parent == parent && within_distance(interner.name(name), interner.name(other))
    };
    let signature = before.signatures.get(&removed);
    let same_signature = |id: PathId| {
        matches!(kind, ItemKind::Function | ItemKind::Method)
            && split(id).1 == parent
            && signature.is_some_and(|signature| head.signatures.get(&id) == Some(signature))
    };
    let tables = before.tables.get(&removed).copied().unwrap_or(0);
    let same_table = |id: PathId| {
        kind == ItemKind::Dependency
            && split(id).1 == parent
            && head
                .tables
                .get(&id)
                .is_some_and(|other| other & tables != 0)
    };
    let rules: [(Change, &dyn Fn(PathId) -> bool); 4] = [
        (Change::Moved, &moved),
        (Change::Renamed, &near),
        (Change::Replaced, &same_signature),
        (Change::Replaced, &same_table),
    ];
    let mut outcome = Outcome::default();
    for (change, rule) in rules {
        let matched: Vec<PathId> = pool.iter().copied().filter(|&id| rule(id)).collect();
        if let [only] = matched[..] {
            return Outcome {
                suggestion: Some((only, change)),
                candidates: Vec::new(),
            };
        }
        if outcome.candidates.is_empty() {
            outcome.candidates = matched;
        }
    }
    outcome
}

/// Whether `other` is within rustc's edit distance `max(len, 3) / 3` of `name`, counting an
/// adjacent transposition as one edit.
fn within_distance(name: &str, other: &str) -> bool {
    let a: Vec<char> = name.chars().collect();
    let b: Vec<char> = other.chars().collect();
    let limit = a.len().max(3) / 3;
    if a.len().abs_diff(b.len()) > limit {
        return false;
    }
    // Rows of the optimal string alignment distance, two back for transpositions.
    let mut before: Vec<usize> = Vec::new();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut current = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            current[j] = (previous[j] + 1)
                .min(current[j - 1] + 1)
                .min(previous[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                current[j] = current[j].min(before[j - 2] + 1);
            }
        }
        before = std::mem::replace(&mut previous, current);
    }
    previous[b.len()] <= limit
}

/// Rewrites a path suffix, escaping Rust keywords for item mentions.
fn rewrite(path: &str, suggested: &str, written: &str, kind: ItemKind) -> Option<String> {
    let rust = !kind.is_manifest_entry();
    let mut path = path.rsplit("::");
    let mut count = 0;
    for word in written.rsplit("::") {
        let word = if rust {
            word.strip_prefix("r#").unwrap_or(word)
        } else {
            word
        };
        if path.next() != Some(word) {
            return None;
        }
        count += 1;
    }
    let start = suggested
        .rmatch_indices("::")
        .nth(count - 1)
        .map_or(0, |(at, _)| at + 2);
    let suffix = &suggested[start..];
    if suffix.split("::").eq(written.split("::").map(|word| {
        if rust {
            word.strip_prefix("r#").unwrap_or(word)
        } else {
            word
        }
    })) {
        return None;
    }
    let extra = if rust {
        2 * suffix
            .split("::")
            .filter(|word| crate::text::KEYWORDS.contains(word))
            .count()
    } else {
        0
    };
    let mut new = String::with_capacity(suffix.len() + extra);
    for (at, word) in suffix.split("::").enumerate() {
        if at > 0 {
            new.push_str("::");
        }
        if rust && crate::text::KEYWORDS.contains(&word) {
            new.push_str("r#");
        }
        new.push_str(word);
    }
    Some(new)
}

/// Whether a mention of `kind` in `context` may be rewritten: items and dependencies only,
/// never inside links, and a dependency never inside a code block, its `toml` key.
fn rewritable(kind: ItemKind, context: Kind) -> bool {
    let item = !kind.is_manifest_entry() && kind != ItemKind::Serialized;
    let dependency = kind == ItemKind::Dependency && !matches!(context, Kind::CodeBlock { .. });
    (item || dependency) && context != Kind::Link
}

/// A rewrite of one mention: where it sits among the groups and what it becomes.
struct Rewrite {
    group: usize,
    mention: usize,
    new: String,
}

/// Fills the `replacement` of every rewritable mention with its line, every rewrite on that
/// line applied. `read` gives a file's text at head.
pub(crate) fn replacements(groups: &mut [Group], mut read: impl FnMut(&str) -> Option<String>) {
    let mut rewrites = Vec::new();
    for (g, group) in groups.iter().enumerate() {
        let Some(suggestion) = &group.suggestion else {
            continue;
        };
        for (m, mention) in group.mentions.iter().enumerate() {
            if rewritable(group.kind, mention.context.kind)
                && let Some(new) = rewrite(&group.path, &suggestion.path, &mention.text, group.kind)
            {
                rewrites.push(Rewrite {
                    group: g,
                    mention: m,
                    new,
                });
            }
        }
    }
    let at = |rewrite: &Rewrite| &groups[rewrite.group].mentions[rewrite.mention];
    // By file and line, rightmost first, so each rewrite leaves earlier offsets in place.
    rewrites.sort_by(|a, b| {
        let (a, b) = (at(a), at(b));
        (&a.file, a.line, b.column).cmp(&(&b.file, b.line, a.column))
    });
    let mut text: Option<(String, String)> = None;
    let mut filled = Vec::new();
    for line in rewrites.chunk_by(|a, b| {
        let (a, b) = (at(a), at(b));
        (&a.file, a.line) == (&b.file, b.line)
    }) {
        let first = at(&line[0]);
        if text.as_ref().is_none_or(|(name, _)| *name != first.file) {
            text = read(&first.file).map(|contents| (first.file.clone(), contents));
        }
        let Some(original) = text
            .as_ref()
            .and_then(|(_, contents)| contents.lines().nth(first.line - 1))
        else {
            continue;
        };
        let mut rewritten = original.to_owned();
        let mut applied = Vec::new();
        // Mentions never overlap, and the rightmost goes first, so earlier offsets hold.
        for rewrite in line {
            let mention = at(rewrite);
            let Some((start, _)) = original.char_indices().nth(mention.column - 1) else {
                continue;
            };
            let stop = start + mention.text.len();
            if original.get(start..stop) != Some(mention.text.as_str()) {
                continue;
            }
            rewritten.replace_range(start..stop, &rewrite.new);
            applied.push((rewrite.group, rewrite.mention));
        }
        filled.push((applied, rewritten));
    }
    for (applied, rewritten) in filled {
        for (g, m) in applied {
            groups[g].mentions[m].replacement = Some(rewritten.clone());
        }
    }
}
