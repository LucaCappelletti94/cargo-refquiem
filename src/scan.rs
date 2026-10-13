//! A scan: index the range, subtract head, find mentions, attribute removals.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use gix::ObjectId;

use crate::cache::FileCache;
use crate::git::{self, Head, Snapshot, TreeSnapshot, WorkTreeSnapshot};
use crate::index::{Index, Indexer};
use crate::intern::{Interner, PathId, Sym};
use crate::text::{self, Spelled};
use crate::{Context, Error, Group, Mention, Options, RemovedIn, Report};

/// What one commit of the range declares beyond head.
struct State {
    /// Paths of the commit that head lacks.
    missing: HashSet<PathId>,
    complete: bool,
}

/// Where the scanned workspace sits inside the repository.
struct Layout {
    workdir: PathBuf,
    /// The workspace directory from the repository root, empty at the root.
    workspace: String,
    cache_dir: PathBuf,
    target_dir: PathBuf,
    /// The workspace `target` directory from the repository root.
    target_prefix: String,
}

pub(crate) fn run(options: &Options) -> Result<Report, Error> {
    let mut repo = gix::discover(&options.root).map_err(|source| Error::Discover {
        path: options.root.clone(),
        source,
    })?;
    // Trees are looked up by path once per module file and commit, mostly hitting the same objects.
    repo.object_cache_size_if_unset(64 * 1024 * 1024);
    let layout = layout(options, &repo)?;
    let mut indexer = Indexer::new(FileCache::load(&layout.cache_dir));
    let (commits, head) = git::range(&repo, &options.range)?;
    let head_snapshot: Box<dyn Snapshot + '_> = match head {
        Head::WorkTree(_) => Box::new(WorkTreeSnapshot::new(
            layout.workdir,
            vec![layout.target_dir, layout.cache_dir],
        )),
        Head::Commit(id) => {
            let tree = commits
                .iter()
                .find(|c| c.id == id)
                .map(|c| c.tree)
                .expect("the head commit is part of its own range");
            Box::new(TreeSnapshot::new(&repo, tree))
        }
    };
    let head_index = indexer.index(&*head_snapshot, &layout.workspace, true)?;

    let mut slots: Vec<State> = Vec::new();
    let mut state_of = Vec::with_capacity(commits.len());
    let mut by_tree: HashMap<ObjectId, usize> = HashMap::new();
    for commit in &commits {
        if let Some(&slot) = by_tree.get(&commit.tree) {
            state_of.push(slot);
            continue;
        }
        let snapshot = TreeSnapshot::new(&repo, commit.tree);
        let index = indexer.index(&snapshot, &layout.workspace, false)?;
        by_tree.insert(commit.tree, slots.len());
        state_of.push(slots.len());
        slots.push(State {
            missing: index.paths.difference(&head_index.paths).copied().collect(),
            complete: index.complete,
        });
    }
    indexer.save();
    let states: Vec<&State> = state_of.iter().map(|&slot| &slots[slot]).collect();

    let interner = &indexer.interner;
    let mut removed: Vec<PathId> = slots
        .iter()
        .flat_map(|s| s.missing.iter().copied())
        .collect::<HashSet<_>>()
        .into_iter()
        .filter(|&id| !kept_by_macro(interner, &head_index, id))
        .collect();
    removed.sort_by_cached_key(|&id| interner.display(id));

    let matcher = Matcher::new(interner, &head_index, &removed);
    let mut mentions = find_mentions(
        &*head_snapshot,
        &head_index,
        &matcher,
        &layout.target_prefix,
    )?;
    let position: HashMap<ObjectId, usize> =
        commits.iter().enumerate().map(|(i, c)| (c.id, i)).collect();
    let mut groups = Vec::new();
    for id in removed {
        let Some(mut found) = mentions.remove(&id) else {
            continue;
        };
        found.sort_by(|a, b| (&a.file, a.line, a.column).cmp(&(&b.file, b.line, b.column)));
        groups.push(Group {
            path: interner.display(id),
            kind: interner.kind(id),
            removed_in: blame(id, &commits, &states, &position, &head),
            mentions: found,
        });
    }
    Ok(Report { groups })
}

fn layout(options: &Options, repo: &gix::Repository) -> Result<Layout, Error> {
    let workdir = repo
        .workdir()
        .ok_or_else(|| Error::Bare(options.root.clone()))?;
    let workdir = canonical(workdir)?;
    let workspace_dir = find_workspace(&canonical(&options.root)?, &workdir)?;
    let target_dir = workspace_dir.join("target");
    Ok(Layout {
        workspace: git::relative(&workdir, &workspace_dir).unwrap_or_default(),
        cache_dir: options
            .cache_dir
            .clone()
            .unwrap_or_else(|| target_dir.join("refquiem")),
        target_prefix: git::relative(&workdir, &target_dir).unwrap_or_default(),
        target_dir,
        workdir,
    })
}

/// Mentions of removed items in the comments and Markdown files of `snapshot`.
fn find_mentions(
    snapshot: &dyn Snapshot,
    head: &Index,
    matcher: &Matcher<'_>,
    target_prefix: &str,
) -> Result<HashMap<PathId, Vec<Mention>>, Error> {
    let mut mentions: HashMap<PathId, Vec<Mention>> = HashMap::new();
    for file in snapshot.files()? {
        if !target_prefix.is_empty() && file.starts_with(&format!("{target_prefix}/")) {
            continue;
        }
        let is_rust = has_extension(&file, &["rs"]);
        if !is_rust && !has_extension(&file, &["md", "markdown"]) {
            continue;
        }
        let Some(id) = snapshot.blob(&file)? else {
            continue;
        };
        let bytes = snapshot.read(&file, id)?;
        let Ok(source) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let spelled: Vec<Spelled> = if is_rust {
            text::rust_comments(source)
                .into_iter()
                .flat_map(|(offset, comment, context)| text::paths(comment, offset, context))
                .collect()
        } else {
            text::paths(source, 0, Context::Markdown)
        };
        if spelled.is_empty() {
            continue;
        }
        let starts = text::line_starts(source);
        let module = head.file_modules.get(&file).map(Vec::as_slice);
        for path in spelled {
            let Some(removed) = matcher.stale(&path, is_rust, module) else {
                continue;
            };
            let (line, column) = text::line_column(&starts, source, path.start);
            mentions.entry(removed).or_default().push(Mention {
                file: file.clone(),
                line,
                column,
                text: source[path.start..path.end].to_owned(),
                context: path.context,
            });
        }
    }
    Ok(mentions)
}

fn canonical(path: &Path) -> Result<PathBuf, Error> {
    path.canonicalize().map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// The outermost `[workspace]` manifest directory between `root` and `workdir`, else the
/// nearest directory with a `Cargo.toml`.
fn find_workspace(root: &Path, workdir: &Path) -> Result<PathBuf, Error> {
    let mut nearest = None;
    let mut outermost_workspace = None;
    for dir in root.ancestors() {
        let manifest = dir.join("Cargo.toml");
        if manifest.is_file() {
            nearest.get_or_insert_with(|| dir.to_path_buf());
            let text = std::fs::read_to_string(&manifest).map_err(|source| Error::Io {
                path: manifest.clone(),
                source,
            })?;
            let table: toml::Table = text.parse().map_err(|source| Error::Manifest {
                path: manifest.display().to_string(),
                source: Box::new(source),
            })?;
            if table.contains_key("workspace") {
                outermost_workspace = Some(dir.to_path_buf());
            }
        }
        if dir == workdir {
            break;
        }
    }
    outermost_workspace
        .or(nearest)
        .ok_or_else(|| Error::NoManifest(root.to_path_buf()))
}

fn has_extension(file: &str, extensions: &[&str]) -> bool {
    file.rsplit_once('.')
        .is_some_and(|(_, ext)| extensions.iter().any(|e| ext.eq_ignore_ascii_case(e)))
}

/// Whether an item-position macro of head may still generate `id`.
fn kept_by_macro(interner: &Interner, head: &Index, id: PathId) -> bool {
    let segments = interner.segments(id);
    for module_len in (1..segments.len()).rev() {
        let prefix = &segments[..module_len];
        let is_module = module_len == 1
            || interner
                .find_path(prefix)
                .is_some_and(|p| interner.kind(p) == crate::ItemKind::Module);
        if is_module {
            let Some(module) = interner.find_path(prefix) else {
                return false;
            };
            return head
                .alive
                .get(&module)
                .is_some_and(|alive| alive.contains(&segments[module_len]));
        }
    }
    false
}

/// The change that removed `id`: the uncommitted changes when `HEAD` has it, else the commit
/// closest to head that lacks it while every parent has it.
fn blame(
    id: PathId,
    commits: &[git::Commit],
    states: &[&State],
    position: &HashMap<ObjectId, usize>,
    head: &Head,
) -> Option<RemovedIn> {
    let has = |i: usize| states[i].complete && states[i].missing.contains(&id);
    if let Head::WorkTree(Some(head_commit)) = head
        && position.get(head_commit).is_some_and(|&i| has(i))
    {
        return Some(RemovedIn::WorkingTree);
    }
    commits.iter().enumerate().find_map(|(i, commit)| {
        let lacks = states[i].complete && !states[i].missing.contains(&id);
        let removed_here = lacks
            && !commit.parents.is_empty()
            && commit
                .parents
                .iter()
                .all(|parent| position.get(parent).is_some_and(|&p| has(p)));
        removed_here.then(|| RemovedIn::Commit(commit.id.to_string()))
    })
}

/// Decides which spelled paths name a removed item.
struct Matcher<'a> {
    interner: &'a Interner,
    alive: HashSet<Box<[Sym]>>,
    removed: HashMap<Box<[Sym]>, PathId>,
    libs: &'a [Sym],
}

impl<'a> Matcher<'a> {
    fn new(interner: &'a Interner, head: &'a Index, removed: &[PathId]) -> Self {
        let mut alive = HashSet::new();
        for &id in &head.paths {
            let segments = interner.segments(id);
            alive.extend(suffixes(segments).map(Box::from));
        }
        let mut removed_suffixes = HashMap::new();
        for &id in removed {
            let segments = interner.segments(id);
            for suffix in suffixes(segments) {
                removed_suffixes.entry(Box::from(suffix)).or_insert(id);
            }
        }
        Self {
            interner,
            alive,
            removed: removed_suffixes,
            libs: &head.libs,
        }
    }

    /// The removed item `path` names, when every reading of it names one.
    fn stale(&self, path: &Spelled, in_rust: bool, module: Option<&[Sym]>) -> Option<PathId> {
        let mut found = None;
        for candidate in self.readings(path, in_rust, module) {
            let hit = self.match_removed(&candidate)?;
            found.get_or_insert(hit);
        }
        found
    }

    /// The full paths `path` may mean, unknown identifiers as `None`.
    fn readings(
        &self,
        path: &Spelled,
        in_rust: bool,
        module: Option<&[Sym]>,
    ) -> Vec<Vec<Option<Sym>>> {
        let segments = &path.segments;
        if segments.iter().any(|s| s == "Self") {
            return Vec::new();
        }
        let rest = |from: usize| segments[from..].iter().map(|s| self.interner.find_sym(s));
        let first = segments[0].as_str();
        if path.leading_colon || !matches!(first, "crate" | "self" | "super") {
            return vec![rest(0).collect()];
        }
        if first == "crate" {
            return match (in_rust, module) {
                (true, Some(module)) => {
                    vec![std::iter::once(Some(module[0])).chain(rest(1)).collect()]
                }
                _ => self
                    .libs
                    .iter()
                    .map(|&lib| std::iter::once(Some(lib)).chain(rest(1)).collect())
                    .collect(),
            };
        }
        let Some(module) = module.filter(|_| in_rust) else {
            return Vec::new();
        };
        let mut base = module.to_vec();
        let mut from = usize::from(first == "self");
        while segments.get(from).is_some_and(|s| s == "super") {
            if base.len() <= 1 {
                return Vec::new();
            }
            base.pop();
            from += 1;
        }
        vec![base.into_iter().map(Some).chain(rest(from)).collect()]
    }

    /// The removed item named by the longest prefix of `path` that is not alive.
    fn match_removed(&self, path: &[Option<Sym>]) -> Option<PathId> {
        for len in (2..=path.len()).rev() {
            let Some(prefix) = path[..len].iter().copied().collect::<Option<Vec<Sym>>>() else {
                continue;
            };
            if self.alive.contains(prefix.as_slice()) {
                return None;
            }
            if let Some(&removed) = self.removed.get(prefix.as_slice()) {
                return Some(removed);
            }
        }
        None
    }
}

/// The suffixes of `segments` with at least two segments, longest first.
fn suffixes(segments: &[Sym]) -> impl Iterator<Item = &[Sym]> {
    std::iter::successors(Some(segments), |rest| rest.get(1..)).take_while(|rest| rest.len() >= 2)
}
