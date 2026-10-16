//! A scan: index the range, subtract head, find mentions, attribute removals.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use gix::ObjectId;

use crate::cache::FileCache;
use crate::config::{self, Config, Filter};
use crate::deps::{self, Removal, Scratch};
use crate::git::{self, Head, Snapshot, TreeSnapshot, WorkTreeSnapshot};
use crate::index::{Index, Indexer};
use crate::intern::{Interner, PathId, Sym};
use crate::parse::SERDE_MARKER;
use crate::stdlib::Vocabulary;
use crate::text::{self, Named, Spelled};
use crate::{
    Context, Error, Group, ItemKind, Kind, Mention, Options, Range, RemovedIn, Report, Source,
    Suggestion, UnusedMarker, entry, stdlib, suggest,
};

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

/// What the range is compared against, as a snapshot.
fn head_snapshot<'r>(
    repo: &'r gix::Repository,
    layout: &Layout,
    commits: &[git::Commit],
    head: &Head,
) -> Box<dyn Snapshot + 'r> {
    match head {
        Head::WorkTree(_) => Box::new(WorkTreeSnapshot::new(
            layout.workdir.clone(),
            vec![layout.target_dir.clone(), layout.cache_dir.clone()],
        )),
        Head::Commit(id) => {
            let tree = commits
                .iter()
                .find(|c| c.id == *id)
                .map(|c| c.tree)
                .expect("the head commit is part of its own range");
            Box::new(TreeSnapshot::new(repo, tree))
        }
    }
}

pub(crate) fn run(options: &Options) -> Result<Report, Error> {
    let mut repo = gix::discover(&options.root).map_err(|source| Error::Discover {
        path: options.root.clone(),
        source,
    })?;
    // Trees are looked up by path once per module file and commit, mostly hitting the same objects.
    repo.object_cache_size_if_unset(64 * 1024 * 1024);
    let layout = layout(options, &repo)?;
    let workspace_dir = layout.workdir.join(&layout.workspace);
    let (config, range) = settings(options, &workspace_dir)?;
    let mut indexer = Indexer::new(FileCache::load(&layout.cache_dir));
    let (commits, head) = git::range(&repo, &range)?;
    let head_snapshot = head_snapshot(&repo, &layout, &commits, &head);
    let scratch = Scratch {
        dir: &layout.cache_dir,
        workspace_dir: &workspace_dir,
        workdir: &layout.workdir,
        workspace: &layout.workspace,
    };
    let dependencies = deps::head(&mut indexer, &*head_snapshot, &scratch)?;
    indexer.use_dependency_macros(&dependencies.macro_rules);
    let head_index = indexer.index(&*head_snapshot, &layout.workspace, true)?;
    indexer.use_head_macros(&head_index);

    let (slots, state_of) = index_range(&mut indexer, &repo, &commits, &layout, &head_index)?;
    let states: Vec<&State> = state_of.iter().map(|&slot| &slots[slot]).collect();

    let mut removed: Vec<PathId> = slots
        .iter()
        .flat_map(|s| s.missing.iter().copied())
        .collect::<HashSet<_>>()
        .into_iter()
        .filter(|&id| !kept_by_macro(&indexer.interner, &head_index, id))
        .collect();
    let removals = removals(&indexer.interner, &removed, &commits, &states);
    let alive = [&head_index.paths, &dependencies.paths];
    let dependency_removals = deps::removed(&mut indexer, &repo, &removals, alive, &scratch)?;
    let dependency_items = dependency_removals.items;
    let interner = &indexer.interner;
    removed.extend(dependency_items.keys().copied());
    removed.sort_by_cached_key(|&id| display(interner, id));

    let entries = Entries::new(interner, &head_index, &removed, &indexer.members);
    let matcher = Matcher::new(
        interner,
        &head_index,
        &dependencies,
        &removed,
        &entries,
        &indexer.crates,
    );
    let filter = filter(
        options,
        &config,
        &entries,
        &workspace_dir,
        &layout.workspace,
    )?;
    let Found { mentions, markers } = find_mentions(
        &*head_snapshot,
        &head_index,
        &matcher,
        &layout.target_prefix,
        &filter,
    )?;
    let history = History {
        commits: &commits,
        states: &states,
        position: commits.iter().enumerate().map(|(i, c)| (c.id, i)).collect(),
        head: &head,
    };
    let (mut groups, suggestable) = groups(
        interner,
        &entries,
        &history,
        removed,
        mentions,
        &dependency_items,
    );
    suggestions(
        &mut indexer,
        &repo,
        &layout.workspace,
        &head_index,
        &suggestable,
        &mut groups,
    )?;
    indexer.save();
    let unused_markers = suppress(&mut groups, markers, &config.allow);
    suggest::replacements(&mut groups, |file| {
        let id = head_snapshot.blob(file).ok()??;
        String::from_utf8(head_snapshot.read(file, id).ok()?).ok()
    });
    Ok(Report {
        groups,
        skipped: dependency_removals.skipped,
        unused_markers,
    })
}

/// The walked commits and their states, children first.
struct History<'a> {
    commits: &'a [git::Commit],
    states: &'a [&'a State],
    position: HashMap<ObjectId, usize>,
    head: &'a Head,
}

/// Where a path was removed, ordered from the latest change to the earliest.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum RemovedAt {
    WorkingTree,
    /// The commit at this position of the walk, children first.
    Commit(usize),
}

impl History<'_> {
    /// The latest of the changes that removed one of `ids`, so a name several removals took
    /// away is blamed on the one after which it exists nowhere.
    fn blame(&self, ids: &[PathId]) -> Option<RemovedIn> {
        let latest = ids.iter().filter_map(|&id| self.removal(id)).min()?;
        Some(match latest {
            RemovedAt::WorkingTree => RemovedIn::WorkingTree,
            RemovedAt::Commit(i) => RemovedIn::Commit(self.commits[i].id.to_string()),
        })
    }

    /// The change that removed `id`: the uncommitted changes when `HEAD` has it, else the
    /// commit closest to head that lacks it while every parent has it.
    fn removal(&self, id: PathId) -> Option<RemovedAt> {
        let has = |i: usize| self.states[i].complete && self.states[i].missing.contains(&id);
        if let Head::WorkTree(Some(head_commit)) = self.head
            && self.position.get(head_commit).is_some_and(|&i| has(i))
        {
            return Some(RemovedAt::WorkingTree);
        }
        self.commits
            .iter()
            .enumerate()
            .position(|(i, commit)| {
                let lacks = self.states[i].complete && !self.states[i].missing.contains(&id);
                lacks
                    && !commit.parents.is_empty()
                    && commit
                        .parents
                        .iter()
                        .all(|parent| self.position.get(parent).is_some_and(|&p| has(p)))
            })
            .map(RemovedAt::Commit)
    }

    /// The tree of the oldest commit that has `id`, where it first appeared.
    fn first_tree(&self, id: PathId) -> Option<ObjectId> {
        let at = self
            .states
            .iter()
            .rposition(|state| state.missing.contains(&id))?;
        Some(self.commits[at].tree)
    }
}

/// A group per removed path still mentioned, and each group the suggestion rules apply to,
/// by index with its removed path and the tree where that path first appeared.
fn groups(
    interner: &Interner,
    entries: &Entries<'_>,
    history: &History<'_>,
    removed: Vec<PathId>,
    mut mentions: HashMap<PathId, Vec<Mention>>,
    dependency_items: &HashMap<PathId, PathId>,
) -> (Vec<Group>, Vec<(usize, PathId, ObjectId)>) {
    let mut groups = Vec::new();
    let mut suggestable = Vec::new();
    for id in removed {
        let Some(mut found) = mentions.remove(&id) else {
            continue;
        };
        found.sort_by(|a, b| (&a.file, a.line, a.column).cmp(&(&b.file, b.line, b.column)));
        found.dedup_by(|a, b| (&a.file, a.line, a.column) == (&b.file, b.line, b.column));
        let kind = interner.kind(id);
        let (path, member) = if kind.is_manifest_entry() {
            let (dir, name) = entries.decode(id);
            (name.to_owned(), entries.member_label(dir))
        } else {
            (display(interner, id), None)
        };
        let removal = dependency_items.get(&id).copied().unwrap_or(id);
        if removal == id
            && let Some(tree) = history.first_tree(id)
        {
            suggestable.push((groups.len(), id, tree));
        }
        groups.push(Group {
            path,
            kind,
            member,
            removed_in: history.blame(&[id, removal]),
            suggestion: None,
            candidates: Vec::new(),
            mentions: found,
        });
    }
    (groups, suggestable)
}

/// Fills the suggestion or candidates of each group in `suggestable`, given by index with its
/// removed path and the tree where that path first appeared.
fn suggestions(
    indexer: &mut Indexer,
    repo: &gix::Repository,
    workspace: &str,
    head: &Index,
    suggestable: &[(usize, PathId, ObjectId)],
    groups: &mut [Group],
) -> Result<(), Error> {
    let mut befores: HashMap<ObjectId, Index> = HashMap::new();
    for &(_, _, tree) in suggestable {
        if let std::collections::hash_map::Entry::Vacant(slot) = befores.entry(tree) {
            slot.insert(indexer.index(&TreeSnapshot::new(repo, tree), workspace, false)?);
        }
    }
    let interner = &indexer.interner;
    for &(at, id, tree) in suggestable {
        let outcome = suggest::suggest(interner, id, head, &befores[&tree]);
        let group = &mut groups[at];
        group.suggestion = outcome.suggestion.map(|(path, change)| Suggestion {
            path: reported(interner, path),
            change,
        });
        group.candidates = outcome
            .candidates
            .into_iter()
            .map(|path| reported(interner, path))
            .collect();
        group.candidates.sort();
    }
    Ok(())
}

/// How path `id` reads in a group: its name for a manifest entry, else its full path.
fn reported(interner: &Interner, id: PathId) -> String {
    if interner.kind(id).is_manifest_entry() {
        let name = interner.segments(id).last().copied();
        name.map(|sym| interner.name(sym).to_owned())
            .unwrap_or_default()
    } else {
        display(interner, id)
    }
}

/// The config of the scan and its range, `since` from the config starting a full one.
fn settings(options: &Options, workspace_dir: &Path) -> Result<(Config, Range), Error> {
    let path = options
        .config
        .clone()
        .unwrap_or_else(|| workspace_dir.join(config::FILE_NAME));
    let config = config::load(&path, options.config.is_some())?;
    let range = match (&options.range, &config.since) {
        (Range::Full, Some(since)) => Range::Since(since.clone()),
        (range, _) => range.clone(),
    };
    Ok((config, range))
}

/// The files the scan reads for mentions, inside the `--package` member when one is named.
fn filter(
    options: &Options,
    config: &Config,
    entries: &Entries<'_>,
    workspace_dir: &Path,
    workspace: &str,
) -> Result<Filter, Error> {
    let package_dir = match &options.package {
        Some(name) => Some(
            entries
                .package_dir(name)
                .ok_or_else(|| Error::UnknownPackage(name.clone()))?
                .to_owned(),
        ),
        None => None,
    };
    Filter::new(config, workspace_dir, workspace, package_dir)
}

/// Indexes each distinct tree of the range once, returning the states and each commit's state.
fn index_range(
    indexer: &mut Indexer,
    repo: &gix::Repository,
    commits: &[git::Commit],
    layout: &Layout,
    head_index: &Index,
) -> Result<(Vec<State>, Vec<usize>), Error> {
    let mut slots: Vec<State> = Vec::new();
    let mut state_of = Vec::with_capacity(commits.len());
    let mut by_tree: HashMap<ObjectId, usize> = HashMap::new();
    for commit in commits {
        if let Some(&slot) = by_tree.get(&commit.tree) {
            state_of.push(slot);
            continue;
        }
        let snapshot = TreeSnapshot::new(repo, commit.tree);
        let index = indexer.index(&snapshot, &layout.workspace, false)?;
        by_tree.insert(commit.tree, slots.len());
        state_of.push(slots.len());
        slots.push(State {
            missing: index.paths.difference(&head_index.paths).copied().collect(),
            complete: index.complete,
        });
    }
    Ok((slots, state_of))
}

/// A name in a suppression marker of head.
struct Marker {
    file: String,
    line: usize,
    column: usize,
    name: String,
    used: bool,
}

/// Drops the groups `allow` names and the mentions a marker names, returning the unused markers.
fn suppress(
    groups: &mut Vec<Group>,
    mut markers: Vec<Marker>,
    allow: &[String],
) -> Vec<UnusedMarker> {
    groups.retain(|group| !allow.iter().any(|entry| names(&group.path, entry)));
    for group in groups.iter_mut() {
        group.mentions.retain(|mention| {
            let mut kept = true;
            for marker in markers.iter_mut().filter(|marker| {
                marker.file == mention.file
                    && (marker.line == mention.line || marker.line + 1 == mention.line)
            }) {
                if names(&group.path, &marker.name) || mention.text == marker.name {
                    marker.used = true;
                    kept = false;
                }
            }
            kept
        });
    }
    groups.retain(|group| !group.mentions.is_empty());
    markers
        .into_iter()
        .filter(|marker| !marker.used)
        .map(|marker| UnusedMarker {
            file: marker.file,
            line: marker.line,
            column: marker.column,
            name: marker.name,
        })
        .collect()
}

/// Whether `name` spells the group `path` or ends it at a `::` boundary.
fn names(path: &str, name: &str) -> bool {
    path.strip_suffix(name)
        .is_some_and(|rest| rest.is_empty() || rest.ends_with("::"))
}

/// The removed dependency entries with the tree closest to head that still has each, those
/// removed closest to head first, so an item several of them provide maps to the removal
/// after which it exists nowhere.
fn removals(
    interner: &Interner,
    removed: &[PathId],
    commits: &[git::Commit],
    states: &[&State],
) -> Vec<Removal> {
    let mut found: Vec<(usize, String, Removal)> = removed
        .iter()
        .filter(|&&id| interner.kind(id) == ItemKind::Dependency)
        .filter_map(|&id| {
            let at = (0..commits.len())
                .find(|&i| states[i].complete && states[i].missing.contains(&id))?;
            let segments = interner.segments(id);
            let removal = Removal {
                entry: id,
                tree: commits[at].tree,
                member_dir: entry::member_dir(interner.name(segments[0]))?.to_owned(),
                key: interner.name(segments[2]).to_owned(),
            };
            Some((at, display(interner, id), removal))
        })
        .collect();
    found.sort_by(|(a, a_name, _), (b, b_name, _)| (a, a_name).cmp(&(b, b_name)));
    found.into_iter().map(|(_, _, removal)| removal).collect()
}

/// How removed path `id` reads in reports, a serialized spelling quoted after its owner.
fn display(interner: &Interner, id: PathId) -> String {
    let segments = interner.segments(id);
    let Some(marker) = segments
        .iter()
        .position(|&sym| interner.name(sym) == SERDE_MARKER)
    else {
        return interner.display(id);
    };
    let owner: Vec<&str> = segments[..marker - 1]
        .iter()
        .map(|&sym| interner.name(sym))
        .collect();
    let spelling: Vec<&str> = segments[marker + 1..]
        .iter()
        .map(|&sym| interner.name(sym))
        .collect();
    format!("{}::\"{}\"", owner.join("::"), spelling.join("::"))
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

/// What the files a scan reads hold.
struct Found {
    mentions: HashMap<PathId, Vec<Mention>>,
    markers: Vec<Marker>,
}

/// The mentions and suppression markers in the comments and Markdown `filter` keeps.
fn find_mentions(
    snapshot: &dyn Snapshot,
    head: &Index,
    matcher: &Matcher<'_>,
    target_prefix: &str,
    filter: &Filter,
) -> Result<Found, Error> {
    let mut mentions: HashMap<PathId, Vec<Mention>> = HashMap::new();
    let mut markers = Vec::new();
    for file in snapshot.files()? {
        if !target_prefix.is_empty() && file.starts_with(&format!("{target_prefix}/")) {
            continue;
        }
        let is_rust = has_extension(&file, &["rs"]);
        if !is_rust && !has_extension(&file, &["md", "markdown"]) || filter.skips_path(&file) {
            continue;
        }
        let Some(id) = snapshot.blob(&file)? else {
            continue;
        };
        let bytes = snapshot.read(&file, id)?;
        let Ok(source) = std::str::from_utf8(&bytes) else {
            continue;
        };
        if filter.skips_text(source) {
            continue;
        }
        let starts = text::line_starts(source);
        for marker in text::markers(source, is_rust) {
            let (line, column) = text::line_column(&starts, source, marker.start);
            markers.push(Marker {
                file: file.clone(),
                line,
                column,
                name: marker.name,
                used: false,
            });
        }
        let spelled: Vec<Spelled> = if is_rust {
            text::rust(source)
        } else {
            text::markdown_file(source)
        };
        let module = head.file_modules.get(&file).map(Vec::as_slice);
        for path in spelled {
            let Some(removed) = matcher.stale(&path, &file, is_rust, module) else {
                continue;
            };
            let (line, column) = text::line_column(&starts, source, path.start);
            mentions.entry(removed).or_default().push(Mention {
                file: file.clone(),
                line,
                column,
                text: source[path.start..path.end].to_owned(),
                context: path.context,
                replacement: None,
            });
        }
    }
    Ok(Found { mentions, markers })
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

/// Decides which spelled paths, bare names and manifest names name something removed.
struct Matcher<'a> {
    interner: &'a Interner,
    alive: HashSet<Box<[Sym]>>,
    removed: HashMap<Box<[Sym]>, PathId>,
    /// The last segment of every item of head and its dependencies.
    names: HashSet<Sym>,
    /// The identifiers item-position macros of head may turn into items, in any module.
    macro_idents: HashSet<Sym>,
    /// Removed items other than serialized spellings, by last segment.
    removed_names: HashMap<Sym, PathId>,
    /// Removed serialized spellings, by spelling.
    removed_spellings: HashMap<Sym, PathId>,
    /// The library crate names of head.
    libs: Vec<&'a str>,
    std: [stdlib::Layer; 4],
    vocabulary: Vocabulary,
    entries: &'a Entries<'a>,
    /// The crate names of every workspace target in any indexed snapshot.
    crates: &'a HashSet<Sym>,
}

impl<'a> Matcher<'a> {
    fn new(
        interner: &'a Interner,
        head: &'a Index,
        dependencies: &deps::Layer,
        removed: &[PathId],
        entries: &'a Entries<'a>,
        crates: &'a HashSet<Sym>,
    ) -> Self {
        let mut alive = HashSet::new();
        let mut names = head.params.clone();
        for &id in head.paths.iter().chain(&dependencies.paths) {
            let segments = interner.segments(id);
            alive.extend(suffixes(segments).map(Box::from));
            if !interner.kind(id).is_manifest_entry() {
                names.extend(segments.last().copied());
            }
        }
        let mut removed_suffixes = HashMap::new();
        let mut removed_names = HashMap::new();
        let mut removed_spellings = HashMap::new();
        for &id in removed {
            let segments = interner.segments(id);
            for suffix in suffixes(segments) {
                removed_suffixes.entry(Box::from(suffix)).or_insert(id);
            }
            let by_name = match interner.kind(id) {
                kind if kind.is_manifest_entry() => continue,
                ItemKind::Serialized => &mut removed_spellings,
                _ => &mut removed_names,
            };
            if let [_, .., last] = segments {
                by_name.entry(*last).or_insert(id);
            }
        }
        Self {
            interner,
            alive,
            removed: removed_suffixes,
            names,
            macro_idents: head
                .alive
                .values()
                .flatten()
                .chain(&dependencies.macro_idents)
                .copied()
                .collect(),
            removed_names,
            removed_spellings,
            libs: head.libs.iter().map(|&lib| interner.name(lib)).collect(),
            std: stdlib::layers(),
            vocabulary: Vocabulary::new(),
            entries,
            crates,
        }
    }

    /// The removed item or manifest entry `path`, found in `file`, names.
    fn stale(
        &self,
        path: &Spelled,
        file: &str,
        in_rust: bool,
        module: Option<&[Sym]>,
    ) -> Option<PathId> {
        let scope = self.entries.scope(file);
        match &path.named {
            Named::Path => self.stale_path(path, scope, in_rust, module),
            Named::Crate(name) => {
                let kinds = [ItemKind::Package, ItemKind::Dependency];
                self.entries.stale(&kinds, name, scope)
            }
            Named::Feature { package, name } => {
                let scope = match package {
                    Some(package) => Some(self.entries.package_dir(package)?),
                    None => scope,
                };
                self.entries.stale(&[ItemKind::Feature], name, scope)
            }
            Named::Binary(name) => self.entries.stale(&[ItemKind::Binary], name, scope),
            Named::Example(name) => self.entries.stale(&[ItemKind::Example], name, scope),
            Named::Name { word, data } => self.stale_name(word, *data, path.context, scope),
        }
    }

    /// The removed item, serialized spelling or crate the bare name `word` names, unless
    /// something live or a vocabulary of `context` claims the name. A word in a data example
    /// only names serialized spellings.
    fn stale_name(
        &self,
        word: &str,
        data: bool,
        context: Context,
        scope: Option<&str>,
    ) -> Option<PathId> {
        let sym = self.interner.find_sym(word);
        let spelling_only = data || word.contains('-');
        let item = sym.and_then(|sym| {
            let name = if spelling_only {
                None
            } else {
                self.removed_names.get(&sym)
            };
            name.or_else(|| self.removed_spellings.get(&sym)).copied()
        });
        let found = match item {
            Some(item) => item,
            None if data => return None,
            None => {
                let kinds = [ItemKind::Package, ItemKind::Dependency];
                self.entries.stale(&kinds, word, scope)?
            }
        };
        let claimed = sym
            .is_some_and(|sym| self.names.contains(&sym) || self.macro_idents.contains(&sym))
            || self.std.iter().any(|layer| layer.has_name(word))
            || (context.kind == Kind::Text && self.vocabulary.claims(word));
        (!claimed).then_some(found)
    }

    /// The removed item every reading of `path` names, else the removed dependency its first
    /// segment names when no reading is alive.
    fn stale_path(
        &self,
        path: &Spelled,
        scope: Option<&str>,
        in_rust: bool,
        module: Option<&[Sym]>,
    ) -> Option<PathId> {
        // Uncompiled examples freely use crates the workspace never depends on, so there only
        // paths spelled from a workspace crate root count.
        let example = path.context.kind == Kind::CodeBlock { compiled: false };
        let mut readings = self.readings(path, in_rust, module);
        if example {
            readings.retain(|reading| self.is_crate(reading[0]));
            if readings.is_empty() {
                return None;
            }
        }
        let mut found = None;
        let mut all_removed = !readings.is_empty();
        for reading in &readings {
            match self.match_removed(reading) {
                Hit::Alive => return None,
                Hit::Removed(id) => {
                    found.get_or_insert(id);
                }
                Hit::Unknown => all_removed = false,
            }
        }
        if all_removed {
            return found;
        }
        if example {
            return None;
        }
        let first = match path.url_item {
            Some(0) => return None,
            _ => path.segments.first()?.as_str(),
        };
        let known = matches!(first, "crate" | "self" | "super" | "Self")
            || self.libs.contains(&first)
            || self.std.iter().any(|layer| layer.root == first);
        if known {
            return None;
        }
        self.entries.stale(&[ItemKind::Dependency], first, scope)
    }

    /// Whether `name` was the crate name of a workspace target in some indexed snapshot.
    fn is_crate(&self, name: &str) -> bool {
        self.interner
            .find_sym(name)
            .is_some_and(|sym| self.crates.contains(&sym))
    }

    /// The full paths `path` may mean.
    fn readings<'s>(
        &'s self,
        path: &'s Spelled,
        in_rust: bool,
        module: Option<&[Sym]>,
    ) -> Vec<Vec<&'s str>> {
        let segments: Vec<&str> = path.segments.iter().map(String::as_str).collect();
        if segments.contains(&"Self") {
            return Vec::new();
        }
        if let Some(item) = path.url_item {
            let root = segments[..item].iter().position(|dir| {
                self.libs.contains(dir) || self.std.iter().any(|layer| layer.root == *dir)
            });
            let reading = segments[root.unwrap_or(item)..].to_vec();
            return if reading.len() >= 2 {
                vec![reading]
            } else {
                Vec::new()
            };
        }
        let first = segments[0];
        if path.leading_colon || !matches!(first, "crate" | "self" | "super") {
            return vec![segments];
        }
        if path.context.source == Source::StringLiteral {
            // A string is data, not code, so `crate`, `self` and `super` have nothing to refer to.
            return Vec::new();
        }
        if first == "crate" {
            let crates = match (in_rust, module) {
                (true, Some(module)) => vec![self.interner.name(module[0])],
                _ => self.libs.clone(),
            };
            return crates
                .into_iter()
                .map(|name| {
                    std::iter::once(name)
                        .chain(segments[1..].iter().copied())
                        .collect()
                })
                .collect();
        }
        let Some(module) = module.filter(|_| in_rust) else {
            return Vec::new();
        };
        let mut base: Vec<&str> = module.iter().map(|&sym| self.interner.name(sym)).collect();
        let mut from = usize::from(first == "self");
        while segments.get(from) == Some(&"super") {
            if base.len() <= 1 {
                return Vec::new();
            }
            base.pop();
            from += 1;
        }
        base.extend_from_slice(&segments[from..]);
        vec![base]
    }

    /// What the longest prefix of `path` that is alive in head or the standard library, or
    /// removed, says about it.
    fn match_removed(&self, path: &[&str]) -> Hit {
        for len in (2..=path.len()).rev() {
            let prefix = &path[..len];
            let joined = prefix.join("::");
            if self.std.iter().any(|layer| layer.ends_a_path(&joined)) {
                return Hit::Alive;
            }
            let Some(syms) = prefix
                .iter()
                .map(|name| self.interner.find_sym(name))
                .collect::<Option<Vec<Sym>>>()
            else {
                continue;
            };
            if self.alive.contains(syms.as_slice()) {
                return Hit::Alive;
            }
            if let Some(&removed) = self.removed.get(syms.as_slice()) {
                return Hit::Removed(removed);
            }
        }
        Hit::Unknown
    }
}

/// What a reading of a path names.
enum Hit {
    Alive,
    Removed(PathId),
    Unknown,
}

/// Removed and live manifest entries, by kind, name and member directory.
struct Entries<'a> {
    interner: &'a Interner,
    removed: HashMap<(ItemKind, String), Vec<(&'a str, PathId)>>,
    head: HashSet<(ItemKind, String, &'a str)>,
    head_any: HashSet<(ItemKind, String)>,
    /// The member directories of head, longest first.
    dirs: Vec<&'a str>,
    /// Whether the range ever had more than one member.
    several: bool,
    /// The member directory of every package name seen in the range.
    packages: HashMap<String, &'a str>,
    /// The package name of each member directory, head's first.
    labels: HashMap<&'a str, &'a str>,
}

impl<'a> Entries<'a> {
    fn new(
        interner: &'a Interner,
        head: &Index,
        removed: &[PathId],
        members: &'a BTreeMap<String, BTreeSet<String>>,
    ) -> Self {
        let mut entries = Self {
            interner,
            removed: HashMap::new(),
            head: HashSet::new(),
            head_any: HashSet::new(),
            dirs: Vec::new(),
            several: members.len() > 1,
            packages: HashMap::new(),
            labels: HashMap::new(),
        };
        for &id in &head.paths {
            let kind = interner.kind(id);
            if !kind.is_manifest_entry() {
                continue;
            }
            let (dir, name) = entries.decode(id);
            if kind == ItemKind::Package {
                entries.labels.insert(dir, name);
                entries.dirs.push(dir);
            }
            entries.head.insert((kind, comparable(kind, name), dir));
            entries.head_any.insert((kind, comparable(kind, name)));
        }
        for &id in removed {
            let kind = interner.kind(id);
            if kind.is_manifest_entry() {
                let (dir, name) = entries.decode(id);
                entries
                    .removed
                    .entry((kind, comparable(kind, name)))
                    .or_default()
                    .push((dir, id));
            }
        }
        for (dir, names) in members {
            for name in names {
                entries
                    .packages
                    .insert(comparable(ItemKind::Package, name), dir.as_str());
                entries.labels.entry(dir.as_str()).or_insert(name.as_str());
            }
        }
        // A package at head keeps its head directory, whatever member held its name before.
        for &dir in &entries.dirs {
            if let Some(&name) = entries.labels.get(dir) {
                entries
                    .packages
                    .insert(comparable(ItemKind::Package, name), dir);
            }
        }
        entries.dirs.sort_by_key(|dir| std::cmp::Reverse(dir.len()));
        entries
    }

    /// The member directory and name of entry `id`.
    fn decode(&self, id: PathId) -> (&'a str, &'a str) {
        let segments = self.interner.segments(id);
        let dir = entry::member_dir(self.interner.name(segments[0])).unwrap_or_default();
        (dir, self.interner.name(segments[2]))
    }

    /// The package naming member `dir` in reports, when the workspace has several members.
    fn member_label(&self, dir: &str) -> Option<String> {
        self.several
            .then(|| self.labels.get(dir).map(|&name| name.to_owned()))
            .flatten()
    }

    /// The member whose directory holds `file`, none when no member does.
    fn scope(&self, file: &str) -> Option<&'a str> {
        self.dirs.iter().copied().find(|dir| {
            dir.is_empty()
                || file
                    .strip_prefix(*dir)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
    }

    /// The member directory of package `name`.
    fn package_dir(&self, name: &str) -> Option<&'a str> {
        self.packages
            .get(&comparable(ItemKind::Package, name))
            .copied()
    }

    /// The removed entry of one of `kinds` called `name` as seen from `scope`, when no entry of
    /// those kinds keeps the name alive there.
    fn stale(&self, kinds: &[ItemKind], name: &str, scope: Option<&str>) -> Option<PathId> {
        let alive = kinds.iter().any(|&kind| {
            let name = comparable(kind, name);
            match scope {
                Some(dir) => self.head.contains(&(kind, name, dir)),
                None => self.head_any.contains(&(kind, name)),
            }
        });
        if alive {
            return None;
        }
        kinds.iter().find_map(|&kind| {
            let removed = self.removed.get(&(kind, comparable(kind, name)))?;
            match scope {
                Some(dir) => removed.iter().find(|(d, _)| *d == dir).map(|&(_, id)| id),
                None => removed.first().map(|&(_, id)| id),
            }
        })
    }
}

/// `name` as compared for `kind`: package and dependency names treat `-` and `_` alike.
fn comparable(kind: ItemKind, name: &str) -> String {
    match kind {
        ItemKind::Package | ItemKind::Dependency => name.replace('_', "-"),
        _ => name.to_owned(),
    }
}

/// The suffixes of `segments` with at least two segments, longest first.
fn suffixes(segments: &[Sym]) -> impl Iterator<Item = &[Sym]> {
    std::iter::successors(Some(segments), |rest| rest.get(1..)).take_while(|rest| rest.len() >= 2)
}
