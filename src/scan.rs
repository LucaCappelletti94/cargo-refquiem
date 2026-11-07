//! A scan: index the range, subtract head, find mentions, attribute removals.

use std::borrow::Cow;
use std::cell::{RefCell, RefMut};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gix::ObjectId;

use crate::cache::FileCache;
use crate::changed::Changed;
use crate::config::{self, Category, Config, Filter, SqlDialects, load_custom};
use crate::definition::Definition;
use crate::deps::{self, Removal, Scratch};
use crate::git::{self, Head, Snapshot, TreeSnapshot, Trees, WorkTreeSnapshot};
use crate::index::{Index, Indexer, Reexports};
use crate::intern::{Interner, PathId, Sym};
use crate::parse::SERDE_MARKER;
use crate::sql::Dialect;
use crate::text::{self, Named, Spelled};
use crate::{
    ChangedItem, Context, Error, Group, HashMap, HashSet, ItemKind, Kind, Level, Mention, Options,
    Position, Range, RefusedSql, RemovedIn, Report, Source, Suggestion, UnusedMarker, Uppercase,
    entry, manifest, sql, stdlib, suggest, vocabulary,
};

/// What the commits of the range declare beyond head, each distinct tree stored once.
#[derive(Default)]
struct States {
    /// The distinct tree of each commit, in walk order, as an index into `complete`.
    slot_of: Vec<usize>,
    /// Whether each distinct tree could be read entirely.
    complete: Vec<bool>,
    /// Every path some tree has and head lacks, with the trees having it as sorted runs of
    /// consecutive slots, first and last included.
    missing: HashMap<PathId, Vec<(u32, u32)>>,
}

impl States {
    /// Adds the next distinct tree, with the paths it has that head lacks, returning its slot.
    fn add_tree(&mut self, missing: impl Iterator<Item = PathId>, complete: bool) -> usize {
        let slot = self.complete.len();
        let at = u32::try_from(slot).expect("fewer than 2^32 trees");
        for id in missing {
            let runs = self.missing.entry(id).or_default();
            match runs.last_mut() {
                Some((_, last)) if *last + 1 == at => *last = at,
                _ => runs.push((at, at)),
            }
        }
        self.complete.push(complete);
        slot
    }

    /// Whether the tree of commit `i` could be read entirely.
    fn complete(&self, i: usize) -> bool {
        self.complete[self.slot_of[i]]
    }

    /// Whether the tree of commit `i` has `id` where head lacks it.
    fn missing(&self, i: usize, id: PathId) -> bool {
        let slot = u32::try_from(self.slot_of[i]).expect("fewer than 2^32 trees");
        self.missing
            .get(&id)
            .is_some_and(|runs| in_runs(runs, slot))
    }

    /// Whether commit `i` has `id` where head lacks it, read from a tree read entirely.
    fn has(&self, i: usize, id: PathId) -> bool {
        self.complete(i) && self.missing(i, id)
    }

    /// Every path some tree has and head lacks.
    fn paths(&self) -> impl Iterator<Item = PathId> + '_ {
        self.missing.keys().copied()
    }
}

/// Whether `slot` lies in the runs of consecutive slots holding a path, first and last slot
/// included.
fn in_runs(runs: &[(u32, u32)], slot: u32) -> bool {
    let at = runs.partition_point(|&(_, last)| last < slot);
    runs.get(at).is_some_and(|&(first, _)| first <= slot)
}

struct IndexedRange {
    states: States,
    first_seen: HashMap<PathId, usize>,
    /// For each parent of a range commit that lies outside the range, the paths its tree has
    /// and head lacks, `None` when the commit is absent or its tree could not be read entirely.
    boundary: HashMap<ObjectId, Option<HashSet<PathId>>>,
    /// What the asked starting revision declares, empty when none was asked.
    start_definitions: HashMap<PathId, Definition>,
}

struct Suggestable {
    group: usize,
    path: PathId,
    tree: ObjectId,
    first: usize,
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
    trees: &Rc<RefCell<Trees>>,
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
            Box::new(TreeSnapshot::new(repo, tree, trees))
        }
    }
}

/// The discovered repository containing `options.root`.
pub(crate) fn repository(options: &Options) -> Result<gix::Repository, Error> {
    gix::discover(&options.root).map_err(|source| Error::Discover {
        path: options.root.clone(),
        source,
    })
}

/// What every walk of a scan shares, read once at head.
struct Shared<'s> {
    repo: &'s gix::Repository,
    options: &'s Options,
    config: &'s Config,
    layout: &'s Layout,
    head: &'s Head,
    head_index: &'s Index,
    dependencies: &'s deps::Layer,
    vocabularies: &'s Vocabularies,
    /// The Rust and Markdown files of head an exclude glob keeps, read once for every walk.
    head_files: &'s [HeadFile],
    /// The names the SQL of head declares.
    sql_names: &'s [String],
    scratch: &'s Scratch<'s>,
}

/// The mentions at head of what a walk removed, and what the rest of the scan takes from it.
struct Walked {
    walk: IndexedRange,
    changed: Changed,
    skipped: Vec<crate::Skipped>,
    groups: Vec<Group>,
    /// The removed path of each group.
    ids: Vec<PathId>,
    suggestable: Vec<Suggestable>,
    changed_mentions: HashMap<PathId, Vec<Mention>>,
    markers: Vec<Marker>,
}

pub(crate) fn run(options: &Options) -> Result<Report, Error> {
    let mut repo = repository(options)?;
    // Trees are looked up by path once per module file and commit, mostly hitting the same objects.
    repo.object_cache_size_if_unset(64 * 1024 * 1024);
    let layout = layout(options, &repo)?;
    let workspace_dir = layout.workdir.join(&layout.workspace);
    let (config_path, config, range) = settings(options, &workspace_dir)?;
    let vocabularies = Vocabularies::new(&config, load_custom(&config_path, &config)?);
    let mut indexer = Indexer::new(FileCache::load(&layout.cache_dir));
    let (commits, head, start) = git::range(&repo, &range)?;
    let head_snapshot = head_snapshot(&repo, &layout, &commits, &head, &indexer.trees);
    let scratch = Scratch {
        dir: &layout.cache_dir,
        workspace_dir: &workspace_dir,
        workdir: &layout.workdir,
        workspace: &layout.workspace,
    };
    let dependencies = deps::head(&mut indexer, &*head_snapshot, &scratch)?;
    let head_index = indexer.index(&*head_snapshot, &layout.workspace, true)?;
    indexer.use_head_macros(&head_index);
    let excludes = Filter::new(&config, &workspace_dir, &layout.workspace, None)?;
    let dialects = SqlDialects::new(&config, &workspace_dir, &layout.workspace)?;
    let HeadText {
        files: head_files,
        names: sql_names,
        refused: refused_sql,
    } = read_head(&*head_snapshot, &excludes, &dialects, &layout.target_prefix)?;
    let shared = Shared {
        repo: &repo,
        options,
        config: &config,
        layout: &layout,
        head: &head,
        head_index: &head_index,
        dependencies: &dependencies,
        vocabularies: &vocabularies,
        head_files: &head_files,
        sql_names: &sql_names,
        scratch: &scratch,
    };

    // Only a range starting from a chosen base has changes the files it left alone may miss.
    let compared = (matches!(range, Range::Base { .. }) && config.changed.level != Level::Allow)
        .then_some(start)
        .flatten();
    let Walked {
        walk,
        changed,
        skipped,
        mut groups,
        ids,
        suggestable,
        changed_mentions,
        markers,
    } = walked(&mut indexer, &shared, &commits, compared)?;
    let scan = Historical {
        repo: &repo,
        workspace: &layout.workspace,
        head: &head_index,
        head_snapshot: &*head_snapshot,
    };
    declarations(&mut indexer, &scan, (&commits, start), &ids, &mut groups)?;
    suggestions(
        &mut indexer,
        &scan,
        &suggestable,
        &walk.first_seen,
        &mut groups,
    )?;
    let mut changed_items = changed_items(&mut indexer, &scan, &changed, changed_mentions)?;
    let mut unused_markers = suppress(&mut groups, &mut changed_items, markers, &config.allow);
    if !unused_markers.is_empty()
        && let Some(unused) = unused_in_full_history(&mut indexer, &shared, start)?
    {
        unused_markers.retain(|marker| unused.contains(marker));
    }
    indexer.save();
    suggest::replacements(&mut groups, |file| scan.head_text(file));
    let mut report = Report {
        base: start.map(|id| id.to_string()),
        head: head_id(&head),
        groups,
        changed: changed_items,
        changed_level: config.changed.level,
        skipped,
        refused_sql,
        unused_markers,
        edited: HashSet::new(),
    };
    report.fill_edited(&repo, &head, &*head_snapshot, &indexer.trees)?;
    Ok(report)
}

/// The mentions at head of what `commits` removed, grouped by removed path, with head's
/// suppression markers and, from the commit `compared` on, the mentions of changed items.
fn walked(
    indexer: &mut Indexer,
    shared: &Shared<'_>,
    commits: &[git::Commit],
    compared: Option<ObjectId>,
) -> Result<Walked, Error> {
    let Shared {
        repo,
        options,
        config,
        layout,
        head,
        head_index,
        dependencies,
        ..
    } = *shared;
    let walk = index_range(indexer, repo, commits, layout, head_index, compared)?;
    let changed = if compared.is_some() {
        Changed::new(
            &indexer.interner,
            head_index,
            &dependencies.paths,
            &walk.start_definitions,
            config.changed.compare(),
        )
    } else {
        Changed::default()
    };
    let states = &walk.states;

    let mut removed: Vec<PathId> = states
        .paths()
        .filter(|&id| !kept_by_macro(&indexer.interner, head_index, id))
        .collect();
    let removals = removals(&indexer.interner, &removed, commits, states);
    let alive = [&head_index.paths, &dependencies.paths];
    let dependency_removals = deps::removed(indexer, repo, &removals, alive, shared.scratch)?;
    let dependency_items = dependency_removals.items;
    let interner = &indexer.interner;
    let rooted = rooted(interner, &dependency_items, dependencies, &indexer.crates);
    removed.extend(dependency_items.keys().copied());
    removed.sort_by_cached_key(|&id| display(interner, id));
    let history = History::new(commits, states, &walk.boundary, head);
    let ranked = ranked(
        interner,
        &history,
        &removed,
        &dependency_items,
        &indexer.crates,
    );

    let entries = Entries::new(interner, head_index, &removed, &indexer.members);
    let mut matcher = Matcher::new(
        interner,
        shared,
        &ranked,
        &entries,
        &indexer.crates,
        &rooted,
    );
    matcher.add_vocabulary(&config.allow);
    matcher.add_vocabulary(shared.sql_names);
    let filter = filter(
        options,
        config,
        &entries,
        shared.scratch.workspace_dir,
        &layout.workspace,
    )?;
    let start_snapshot = compared
        .and_then(|start| commits.iter().find(|commit| commit.id == start))
        .map(|start| TreeSnapshot::new(repo, start.tree, &indexer.trees));
    let Found {
        mentions,
        changed_mentions,
        markers,
    } = find_mentions(
        shared.head_files,
        head_index,
        &matcher,
        (
            &changed,
            start_snapshot.as_ref().map(|s| s as &dyn Snapshot),
        ),
        &filter,
    )?;
    let (groups, ids, suggestable) = groups(
        interner,
        &entries,
        &history,
        removed,
        mentions,
        &dependency_items,
    );
    Ok(Walked {
        walk,
        changed,
        skipped: dependency_removals.skipped,
        groups,
        ids,
        suggestable,
        changed_mentions,
        markers,
    })
}

/// The markers that suppress nothing in the scan of the full history up to the scanned head,
/// which starts at the config's `since` when it has one, `None` when the range starting at
/// `start` is that history already.
///
/// A range starting after the removal a marker was written for no longer sees the removed name,
/// so each range takes the full history's verdict on its markers.
fn unused_in_full_history(
    indexer: &mut Indexer,
    shared: &Shared<'_>,
    start: Option<ObjectId>,
) -> Result<Option<Vec<UnusedMarker>>, Error> {
    let since = match &shared.config.since {
        Some(since) => Some(git::resolve(shared.repo, since)?),
        None => None,
    };
    if since == start {
        return Ok(None);
    }
    let commits = git::walk(shared.repo, shared.head.tip(), since)?;
    let Walked {
        mut groups,
        markers,
        ..
    } = walked(indexer, shared, &commits, None)?;
    Ok(Some(suppress(
        &mut groups,
        &mut Vec::new(),
        markers,
        &shared.config.allow,
    )))
}

/// The scanned head commit as its full id, the `HEAD` commit for a working tree scan, absent
/// without one.
fn head_id(head: &Head) -> Option<String> {
    match head {
        Head::WorkTree(Some(commit)) | Head::Commit(commit) => Some(commit.to_string()),
        Head::WorkTree(None) => None,
    }
}

impl Report {
    /// Fill `edited`, the mentioned files whose working tree content differs from the `HEAD`
    /// commit, whose positions therefore cannot link to it.
    fn fill_edited(
        &mut self,
        repo: &gix::Repository,
        head: &Head,
        worktree: &dyn Snapshot,
        trees: &Rc<RefCell<Trees>>,
    ) -> Result<(), Error> {
        let Head::WorkTree(Some(commit)) = head else {
            return Ok(());
        };
        let at_head = TreeSnapshot::new(repo, git::commit_tree(repo, *commit)?, trees);
        let mut files = HashSet::new();
        let changed = self.changed.iter().flat_map(|item| {
            let declared = item.declared_at.iter().map(|at| at.file.as_str());
            item.mentions
                .iter()
                .map(|m| m.file.as_str())
                .chain(declared)
        });
        let removed = self.groups.iter().flat_map(|group| &group.mentions);
        files.extend(removed.map(|m| m.file.as_str()).chain(changed));
        for marker in &self.unused_markers {
            files.insert(marker.file.as_str());
        }
        for &file in &files {
            if worktree.blob(file)? != at_head.blob(file)? {
                self.edited.insert(file.to_owned());
            }
        }
        Ok(())
    }
}

/// Groups mentions and identifies the historical indexes needed by suggestion rules, returning
/// the removed path of each group alongside.
fn groups(
    interner: &Interner,
    entries: &Entries<'_>,
    history: &History<'_>,
    removed: Vec<PathId>,
    mut mentions: HashMap<PathId, Vec<Mention>>,
    dependency_items: &HashMap<PathId, PathId>,
) -> (Vec<Group>, Vec<PathId>, Vec<Suggestable>) {
    let mut groups = Vec::new();
    let mut ids = Vec::new();
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
        let dependency = dependency_items.get(&id).copied();
        if dependency.unwrap_or(id) == id
            && let Some((tree, first)) = history.first_tree(id)
        {
            suggestable.push(Suggestable {
                group: groups.len(),
                path: id,
                tree,
                first,
            });
        }
        let removed_in = match dependency {
            Some(entry) => history.blame(&[id, entry]),
            None => history.blame(&[id]),
        };
        ids.push(id);
        groups.push(Group {
            path,
            kind,
            member,
            removed_in,
            declared_at: None,
            suggestion: None,
            candidates: Vec::new(),
            mentions: found,
        });
    }
    (groups, ids, suggestable)
}

/// What the steps reading other revisions of the repository take from the scan.
struct Historical<'a> {
    repo: &'a gix::Repository,
    /// The workspace directory from the repository root.
    workspace: &'a str,
    head: &'a Index,
    head_snapshot: &'a dyn Snapshot,
}

impl Historical<'_> {
    /// The text of `file` at head, absent when head lacks it or it is not UTF-8.
    fn head_text(&self, file: &str) -> Option<String> {
        let id = self.head_snapshot.blob(file).ok()??;
        String::from_utf8(self.head_snapshot.read(file, id).ok()?).ok()
    }
}

/// Sets where the range's starting commit, given by id among `commits`, declares the name of
/// each group, `groups[i]` being the group of `ids[i]`.
fn declarations(
    indexer: &mut Indexer,
    scan: &Historical<'_>,
    (commits, start): (&[git::Commit], Option<ObjectId>),
    ids: &[PathId],
    groups: &mut [Group],
) -> Result<(), Error> {
    let Some(start) = start.and_then(|start| commits.iter().find(|commit| commit.id == start))
    else {
        return Ok(());
    };
    let snapshot = TreeSnapshot::new(scan.repo, start.tree, &indexer.trees);
    let found = positions(indexer, &snapshot, scan.workspace, ids, |source| {
        !scan.head.paths.contains(&source)
    })?;
    for (group, declared_at) in groups.iter_mut().zip(found) {
        group.declared_at = declared_at;
    }
    Ok(())
}

/// The changed items `found` mentions, by path, each where head declares it.
fn changed_items(
    indexer: &mut Indexer,
    scan: &Historical<'_>,
    changed: &Changed,
    found: HashMap<PathId, Vec<Mention>>,
) -> Result<Vec<ChangedItem>, Error> {
    let mut found: Vec<(PathId, Vec<Mention>)> = found.into_iter().collect();
    found.sort_by_cached_key(|&(id, _)| display(&indexer.interner, id));
    let ids: Vec<PathId> = found.iter().map(|&(id, _)| id).collect();
    let places = positions(indexer, scan.head_snapshot, scan.workspace, &ids, |_| false)?;
    let interner = &indexer.interner;
    Ok(found
        .into_iter()
        .zip(places)
        .map(|((id, mut mentions), declared_at)| {
            mentions.sort_by(|a, b| (&a.file, a.line, a.column).cmp(&(&b.file, b.line, b.column)));
            mentions.dedup_by(|a, b| (&a.file, a.line, a.column) == (&b.file, b.line, b.column));
            ChangedItem {
                path: display(interner, id),
                kind: interner.kind(id),
                part: changed.parts[&id],
                declared_at,
                mentions,
            }
        })
        .collect())
}

/// Where `snapshot` declares each of `ids`, a manifest entry at its key and an item as
/// [`Indexer::locate`] places it given `gone`.
fn positions(
    indexer: &mut Indexer,
    snapshot: &dyn Snapshot,
    workspace: &str,
    ids: &[PathId],
    gone: impl Fn(PathId) -> bool,
) -> Result<Vec<Option<Position>>, Error> {
    let found = if ids.is_empty() {
        HashMap::new()
    } else {
        indexer.locate(snapshot, workspace, ids, gone)?
    };
    ids.iter()
        .map(|&id| {
            if indexer.interner.kind(id).is_manifest_entry() {
                manifest_entry_line(&indexer.interner, snapshot, id)
            } else {
                Ok(found.get(&id).map(|(file, line)| Position {
                    file: file.clone(),
                    line: usize::try_from(*line).expect("lines fit in usize"),
                }))
            }
        })
        .collect()
}

/// Where `snapshot` declares the manifest entry `id` in its member's `Cargo.toml`.
fn manifest_entry_line(
    interner: &Interner,
    snapshot: &dyn Snapshot,
    id: PathId,
) -> Result<Option<Position>, Error> {
    let segments = interner.segments(id);
    let Some(dir) = entry::member_dir(interner.name(segments[0])) else {
        return Ok(None);
    };
    let file = manifest::join(dir, "Cargo.toml");
    let Some(blob) = snapshot.blob(&file)? else {
        return Ok(None);
    };
    let text = String::from_utf8_lossy(&snapshot.read(&file, blob)?).into_owned();
    let line = manifest::entry_line(&text, interner.kind(id), interner.name(segments[2]));
    Ok(line.map(|line| Position { file, line }))
}

/// Fills the suggestion or candidates of each group in `suggestable`, given by index with its
/// removed path and the tree where that path first appeared, and where head declares each
/// suggestion.
fn suggestions(
    indexer: &mut Indexer,
    scan: &Historical<'_>,
    suggestable: &[Suggestable],
    first_seen: &HashMap<PathId, usize>,
    groups: &mut [Group],
) -> Result<(), Error> {
    let mut befores: HashMap<ObjectId, Index> = HashMap::new();
    for input in suggestable {
        if let hashbrown::hash_map::Entry::Vacant(slot) = befores.entry(input.tree) {
            let before = TreeSnapshot::new(scan.repo, input.tree, &indexer.trees);
            slot.insert(indexer.index(&before, scan.workspace, false)?);
        }
    }
    let interner = &indexer.interner;
    let mut suggested: Vec<(usize, PathId)> = Vec::new();
    for input in suggestable {
        let outcome = suggest::suggest(
            interner,
            input.path,
            scan.head,
            &befores[&input.tree],
            first_seen,
            input.first,
        );
        let group = &mut groups[input.group];
        group.suggestion = outcome.suggestion.map(|(path, change)| {
            suggested.push((input.group, path));
            Suggestion {
                path: reported(interner, path),
                change,
                declared_at: None,
            }
        });
        group.candidates = outcome
            .candidates
            .into_iter()
            .map(|path| reported(interner, path))
            .collect();
        group.candidates.sort();
    }
    let ids: Vec<PathId> = suggested.iter().map(|&(_, id)| id).collect();
    // Head has every suggestion, so a re-exported one points at the item it names.
    let found = positions(indexer, scan.head_snapshot, scan.workspace, &ids, |_| true)?;
    for ((group, _), declared_at) in suggested.into_iter().zip(found) {
        if let Some(suggestion) = &mut groups[group].suggestion {
            suggestion.declared_at = declared_at;
        }
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

/// The config of the scan and the path selected for it, including the absent default.
pub(crate) fn selected_config(
    options: &Options,
    workspace_dir: &Path,
) -> Result<(PathBuf, Config), Error> {
    let path = options
        .config
        .clone()
        .unwrap_or_else(|| workspace_dir.join(config::FILE_NAME));
    let config = config::load(&path, options.config.is_some())?;
    Ok((path, config))
}

/// The config of the scan, the path selected for it and its range, `since` from the config
/// starting a full one.
fn settings(options: &Options, workspace_dir: &Path) -> Result<(PathBuf, Config, Range), Error> {
    let (path, config) = selected_config(options, workspace_dir)?;
    let range = match (&options.range, &config.since) {
        (Range::Full, Some(since)) => Range::Since(since.clone()),
        (range, _) => range.clone(),
    };
    Ok((path, config, range))
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

/// Indexes each distinct tree of the range once, returning what each commit declares beyond head
/// and what the commit `start` declares.
fn index_range(
    indexer: &mut Indexer,
    repo: &gix::Repository,
    commits: &[git::Commit],
    layout: &Layout,
    head_index: &Index,
    start: Option<ObjectId>,
) -> Result<IndexedRange, Error> {
    let start_tree = start
        .and_then(|start| commits.iter().find(|commit| commit.id == start))
        .map(|commit| commit.tree);
    let mut start_definitions = HashMap::new();
    let mut states = States::default();
    let mut first_seen: HashMap<PathId, usize> = HashMap::new();
    // Head's paths by table position, an id interned after head reading as absent.
    let head_at =
        |id: PathId| usize::try_from(id).expect("u32 ids fit in usize on supported targets");
    let mut in_head = vec![false; indexer.interner.path_count()];
    for &id in &head_index.paths {
        in_head[head_at(id)] = true;
    }
    let mut by_tree: HashMap<ObjectId, (Option<usize>, usize)> = commits
        .iter()
        .enumerate()
        .map(|(at, commit)| (commit.tree, (None, at)))
        .collect();
    for commit in commits {
        let (slot, oldest) = by_tree
            .get_mut(&commit.tree)
            .expect("every tree was recorded");
        if let Some(slot) = *slot {
            states.slot_of.push(slot);
            continue;
        }
        let oldest = *oldest;
        let snapshot = TreeSnapshot::new(repo, commit.tree, &indexer.trees);
        let mut index = indexer.index(&snapshot, &layout.workspace, false)?;
        if start_tree == Some(commit.tree) {
            start_definitions = std::mem::take(&mut index.definitions);
        }
        let missing = index.paths.iter().filter_map(|&path| {
            if in_head.get(head_at(path)) == Some(&true) {
                first_seen
                    .entry(path)
                    .and_modify(|first| *first = (*first).max(oldest))
                    .or_insert(oldest);
                None
            } else {
                Some(path)
            }
        });
        let next = states.add_tree(missing, index.complete);
        *slot = Some(next);
        states.slot_of.push(next);
    }
    // A parent the walk hid tells blame what the range commit above it took away. The
    // starting commit carries no parents, so removals before the range stay unblamed.
    let walked: HashSet<ObjectId> = commits.iter().map(|commit| commit.id).collect();
    let mut boundary = HashMap::new();
    for &parent in commits.iter().flat_map(|commit| &commit.parents) {
        if walked.contains(&parent) || boundary.contains_key(&parent) {
            continue;
        }
        // A shallow clone names the parents of its oldest commits without holding them.
        if !repo.has_object(parent) {
            boundary.insert(parent, None);
            continue;
        }
        let snapshot = TreeSnapshot::new(repo, git::commit_tree(repo, parent)?, &indexer.trees);
        let index = indexer.index(&snapshot, &layout.workspace, false)?;
        let missing = index.complete.then(|| {
            index
                .paths
                .into_iter()
                .filter(|path| in_head.get(head_at(*path)) != Some(&true))
                .collect()
        });
        boundary.insert(parent, missing);
    }
    Ok(IndexedRange {
        states,
        first_seen,
        boundary,
        start_definitions,
    })
}

/// A name in a suppression marker of head.
struct Marker {
    file: String,
    line: usize,
    column: usize,
    name: String,
    used: bool,
}

/// Drops the removed names and changed items `allow` names and the mentions a marker names,
/// returning the unused markers.
fn suppress(
    groups: &mut Vec<Group>,
    changed: &mut Vec<ChangedItem>,
    mut markers: Vec<Marker>,
    allow: &[String],
) -> Vec<UnusedMarker> {
    let allowed = |path: &str| allow.iter().any(|entry| names(path, entry));
    groups.retain(|group| !allowed(&group.path));
    changed.retain(|item| !allowed(&item.path));
    let found = groups
        .iter_mut()
        .map(|group| (&group.path, &mut group.mentions))
        .chain(
            changed
                .iter_mut()
                .map(|item| (&item.path, &mut item.mentions)),
        );
    for (path, mentions) in found {
        mentions.retain(|mention| {
            let mut kept = true;
            for marker in markers.iter_mut().filter(|marker| {
                marker.file == mention.file
                    && (marker.line == mention.line || marker.line + 1 == mention.line)
            }) {
                if names(path, &marker.name) || mention.text == marker.name {
                    marker.used = true;
                    kept = false;
                }
            }
            kept
        });
    }
    groups.retain(|group| !group.mentions.is_empty());
    changed.retain(|item| !item.mentions.is_empty());
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

/// Removed dependency entries, closest-to-head removals first.
fn removals(
    interner: &Interner,
    removed: &[PathId],
    commits: &[git::Commit],
    states: &States,
) -> Vec<Removal> {
    let mut found: Vec<(usize, String, Removal)> = removed
        .iter()
        .filter(|&&id| interner.kind(id) == ItemKind::Dependency)
        .filter_map(|&id| {
            let at = (0..commits.len()).find(|&i| states.has(i, id))?;
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

/// The workdir and the workspace directory `options.root` resolves to.
pub(crate) fn workspace(
    options: &Options,
    repo: &gix::Repository,
) -> Result<(PathBuf, PathBuf), Error> {
    let workdir = repo
        .workdir()
        .ok_or_else(|| Error::Bare(options.root.clone()))?;
    let workdir = canonical(workdir)?;
    let workspace_dir = find_workspace(&canonical(&options.root)?, &workdir)?;
    Ok((workdir, workspace_dir))
}

fn layout(options: &Options, repo: &gix::Repository) -> Result<Layout, Error> {
    let (workdir, workspace_dir) = workspace(options, repo)?;
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
    /// The mentions of changed items, by item.
    changed_mentions: HashMap<PathId, Vec<Mention>>,
    markers: Vec<Marker>,
}

/// A Rust or Markdown file of head, read once for the SQL it holds and for its mentions.
struct HeadFile {
    file: String,
    id: ObjectId,
    source: String,
    is_rust: bool,
    /// The dialect of a `.sql` file, whose vocabularies apply inside it.
    dialect: Option<Dialect>,
    read: Result<text::Read, text::MarkdownError>,
}

/// The files of head, the names their SQL declares and the SQL among it that could not be read.
struct HeadText {
    files: Vec<HeadFile>,
    names: Vec<String>,
    refused: Vec<RefusedSql>,
}

/// Reads the `.rs`, `.md` and `.sql` files of `snapshot` outside its target directory that no
/// exclude glob leaves out. Every file counts whatever member the scan is limited to, since a
/// name lives wherever it is declared.
fn read_head(
    snapshot: &dyn Snapshot,
    filter: &Filter,
    dialects: &SqlDialects,
    target_prefix: &str,
) -> Result<HeadText, Error> {
    let mut head = HeadText {
        files: Vec::new(),
        names: Vec::new(),
        refused: Vec::new(),
    };
    for file in snapshot.files()? {
        let is_rust = has_extension(&file, &["rs"]);
        let is_sql = has_extension(&file, &["sql"]);
        if !(is_rust || is_sql || has_extension(&file, &["md", "markdown"]))
            || in_target(&file, target_prefix)
            || filter.excluded(&file)
        {
            continue;
        }
        let Some((id, source)) = utf8_text(snapshot, &file)? else {
            continue;
        };
        if is_sql {
            let dialect = dialects.of(&file)?;
            let (declared, comments) = sql::read(&source, dialect);
            head.names.extend(declared.names);
            head.refused
                .extend(declared.refused.into_iter().map(|refusal| RefusedSql {
                    file: file.clone(),
                    line: refusal.line,
                    column: refusal.column,
                    error: refusal.error,
                }));
            let read = text::sql_file(&source, &comments, filter.scan_commented_code);
            head.files.push(HeadFile {
                file,
                id,
                source,
                is_rust: false,
                dialect: Some(dialect),
                read: Ok(read),
            });
            continue;
        }
        let read = if is_rust {
            text::rust(&source, filter.scan_commented_code)
        } else {
            text::markdown_file(&source)
        };
        if let Ok(read) = &read {
            let starts = if read.fences.is_empty() {
                Vec::new()
            } else {
                text::line_starts(&source)
            };
            for fence in &read.fences {
                let dialect = match fence.dialect {
                    Some(dialect) => dialect,
                    None => dialects.of(&file)?,
                };
                let declared = sql::declared(&fence.text, dialect);
                head.names.extend(declared.names);
                for refusal in declared.refused {
                    let at = sql::offset(&fence.text, refusal.line, refusal.column);
                    let (line, column) = text::line_column(&starts, &source, fence.file_offset(at));
                    head.refused.push(RefusedSql {
                        file: file.clone(),
                        line,
                        column,
                        error: refusal.error,
                    });
                }
            }
            // A string literal makes no claim to be SQL, so one sqlparser refuses is no SQL.
            if !read.literals.is_empty() {
                let dialect = dialects.of(&file)?;
                for literal in &read.literals {
                    head.names.extend(sql::declared(literal, dialect).names);
                }
            }
        }
        head.files.push(HeadFile {
            file,
            id,
            source,
            is_rust,
            dialect: None,
            read,
        });
    }
    head.names.sort_unstable();
    head.names.dedup();
    Ok(head)
}

/// Whether `file` lies in the scanned workspace's target directory at `target_prefix`.
fn in_target(file: &str, target_prefix: &str) -> bool {
    !target_prefix.is_empty()
        && file
            .strip_prefix(target_prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// The blob id and text of `file` in `snapshot`, absent when it is no regular file or not
/// UTF-8.
fn utf8_text(snapshot: &dyn Snapshot, file: &str) -> Result<Option<(ObjectId, String)>, Error> {
    let Some(id) = snapshot.blob(file)? else {
        return Ok(None);
    };
    let text = String::from_utf8(snapshot.read(file, id)?).ok();
    Ok(text.map(|text| (id, text)))
}

/// The mentions and suppression markers in the comments and Markdown `filter` keeps, with the
/// mentions of `changed` items in files the same in `start` as at head.
fn find_mentions(
    files: &[HeadFile],
    head: &Index,
    matcher: &Matcher<'_>,
    (changed, start): (&Changed, Option<&dyn Snapshot>),
    filter: &Filter,
) -> Result<Found, Error> {
    let mut mentions: HashMap<PathId, Vec<Mention>> = HashMap::new();
    let mut changed_mentions: HashMap<PathId, Vec<Mention>> = HashMap::new();
    let mut markers = Vec::new();
    for HeadFile {
        file,
        id,
        source,
        is_rust,
        dialect,
        read,
    } in files
    {
        let (id, source, is_rust, dialect) = (*id, source.as_str(), *is_rust, *dialect);
        if filter.skips_path(file) || filter.skips_text(source) {
            continue;
        }
        let read = read
            .as_ref()
            .map_err(|_| Error::Markdown { path: file.clone() })?;
        let starts = text::line_starts(source);
        for marker in &read.markers {
            let (line, column) = text::line_column(&starts, source, marker.start);
            markers.push(Marker {
                file: file.clone(),
                line,
                column,
                name: marker.name.clone(),
                used: false,
            });
        }
        if read.mentions.is_empty() {
            continue;
        }
        // A file the range edits had its author's eyes on it.
        let unedited = match start {
            Some(start) if !changed.is_empty() => start.blob(file)? == Some(id),
            _ => false,
        };
        let module = head.file_modules.get(file).map(Vec::as_slice);
        for path in &read.mentions {
            let mention = |uppercase| {
                let (line, column) = text::line_column(&starts, source, path.start);
                Mention {
                    file: file.clone(),
                    line,
                    column,
                    text: source[path.start..path.end].to_owned(),
                    context: path.context,
                    replacement: None,
                    uppercase,
                }
            };
            if let Some(removed) = matcher.stale(path, file, (is_rust, dialect), module) {
                let uppercase = matcher.uppercase(path, removed);
                mentions
                    .entry(removed)
                    .or_default()
                    .push(mention(uppercase));
            } else if unedited
                && path.context.source != Source::StringLiteral
                && let Some(item) = matcher.changed(changed, path, (is_rust, dialect), module)
            {
                changed_mentions
                    .entry(item)
                    .or_default()
                    .push(mention(None));
            }
        }
    }
    Ok(Found {
        mentions,
        changed_mentions,
        markers,
    })
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

/// The removed dependency `items` whose crate is neither among head's `dependencies` nor a
/// workspace crate of the `crates`, which match only as whole paths from that crate.
fn rooted(
    interner: &Interner,
    items: &HashMap<PathId, PathId>,
    dependencies: &deps::Layer,
    crates: &HashSet<Sym>,
) -> HashSet<PathId> {
    items
        .keys()
        .copied()
        .filter(|&id| {
            let root = interner.segments(id)[0];
            !dependencies.roots.contains(&root) && !crates.contains(&root)
        })
        .collect()
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

/// Where a path was removed, ordered from the latest change to the earliest.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum RemovedAt {
    WorkingTree,
    /// The commit at this position of the walk, children first.
    Commit(usize),
}

/// A parent of a range commit, its walk index when in the range, its id just below it.
#[derive(Clone, Copy)]
enum Parent {
    /// In the range, at this walk index.
    InRange(usize),
    /// Just below the range, looked up in `History::boundary` by this id.
    OutOfRange(ObjectId),
}

/// The word index of tree slot `slot`, one bit per slot. A lossless widening, `usize`
/// holding at least 32 bits.
fn word_of(slot: u32) -> usize {
    (slot >> 6) as usize
}

/// The tree slots holding one path, one bit per distinct slot, so a removal scan tests
/// membership without searching the path's runs at every commit. The buffer is reused, and
/// the words are cleared before each path is set.
struct PathBits {
    inner: RefCell<PathBitsInner>,
}

struct PathBitsInner {
    /// One bit per distinct tree slot.
    words: Vec<u64>,
}

impl PathBits {
    /// Sizes the words for `slots` distinct tree slots.
    fn with_slots(slots: usize) -> Self {
        let words = vec![0; slots.div_ceil(64)];
        Self {
            inner: RefCell::new(PathBitsInner { words }),
        }
    }

    /// Clears the bits set for the previous path and sets the slots of `runs`, returning the
    /// words for the scan to test.
    fn set(&self, runs: Option<&[(u32, u32)]>) -> RefMut<'_, PathBitsInner> {
        let mut inner = self.inner.borrow_mut();
        inner.words.fill(0);
        if let Some(runs) = runs {
            for &(first, last) in runs {
                let lo = word_of(first);
                let hi = word_of(last);
                let lo_mask = u64::MAX << (first & 63);
                let hi_mask = u64::MAX >> (63 - (last & 63));
                if lo == hi {
                    inner.words[lo] |= lo_mask & hi_mask;
                } else {
                    inner.words[lo] |= lo_mask;
                    inner.words[hi] |= hi_mask;
                    inner.words[lo + 1..hi].fill(u64::MAX);
                }
            }
        }
        inner
    }
}

/// The walked commits and their states, children first.
struct History<'a> {
    commits: &'a [git::Commit],
    states: &'a States,
    /// The tree slot of each commit, in walk order.
    slots: Vec<u32>,
    /// Whether the tree of each commit could be read entirely, in walk order.
    complete: Vec<bool>,
    /// The walk index of the HEAD commit, when head is the working tree and the range holds it.
    head_at: Option<usize>,
    /// Every commit's parents, commit-major, in-range ones by walk index.
    parents: Vec<Parent>,
    /// The first parent of each commit in `parents`, the final entry closing the last.
    parent_at: Vec<usize>,
    /// The paths of each parent outside the range, `None` when unreadable.
    boundary: &'a HashMap<ObjectId, Option<HashSet<PathId>>>,
    /// The bits `removal` reuses to test which tree slots hold a path.
    bits: PathBits,
}

impl<'a> History<'a> {
    fn new(
        commits: &'a [git::Commit],
        states: &'a States,
        boundary: &'a HashMap<ObjectId, Option<HashSet<PathId>>>,
        head: &'a Head,
    ) -> Self {
        let slots = states
            .slot_of
            .iter()
            .map(|&slot| u32::try_from(slot).expect("fewer than 2^32 trees"))
            .collect();
        let complete = states
            .slot_of
            .iter()
            .map(|&slot| states.complete[slot])
            .collect();
        let position: HashMap<ObjectId, usize> = commits
            .iter()
            .enumerate()
            .map(|(i, commit)| (commit.id, i))
            .collect();
        let mut parents =
            Vec::with_capacity(commits.iter().map(|commit| commit.parents.len()).sum());
        let mut parent_at = Vec::with_capacity(commits.len() + 1);
        parent_at.push(0);
        for commit in commits {
            for &parent in &commit.parents {
                parents.push(match position.get(&parent) {
                    Some(&i) => Parent::InRange(i),
                    None => Parent::OutOfRange(parent),
                });
            }
            parent_at.push(parents.len());
        }
        let head_at = match head {
            Head::WorkTree(Some(commit)) => position.get(commit).copied(),
            _ => None,
        };
        let bits = PathBits::with_slots(states.complete.len());
        Self {
            commits,
            states,
            slots,
            complete,
            head_at,
            parents,
            parent_at,
            boundary,
            bits,
        }
    }
}

impl History<'_> {
    /// The latest of the changes that removed one of `ids`, so a name several removals took
    /// away is blamed on the one after which it exists nowhere.
    fn blame(&self, ids: &[PathId]) -> Option<RemovedIn> {
        Some(match self.latest(ids)? {
            RemovedAt::WorkingTree => RemovedIn::WorkingTree,
            RemovedAt::Commit(i) => RemovedIn::Commit(self.commits[i].id.to_string()),
        })
    }

    /// The latest of the changes that removed one of `ids`.
    fn latest(&self, ids: &[PathId]) -> Option<RemovedAt> {
        ids.iter().filter_map(|&id| self.removal(id)).min()
    }

    /// The change that removed `id`: the uncommitted changes when `HEAD` has it, else the
    /// commit closest to head that lacks it while every parent, in the range or just below it,
    /// has it.
    fn removal(&self, id: PathId) -> Option<RemovedAt> {
        let runs = self.states.missing.get(&id);
        let bits = self.bits.set(runs.map(Vec::as_slice));
        let has = |slot: u32| bits.words[word_of(slot)] >> (slot & 63) & 1 != 0;
        if let Some(head) = self.head_at
            && self.complete[head]
            && has(self.slots[head])
        {
            return Some(RemovedAt::WorkingTree);
        }
        (0..self.commits.len())
            .position(|i| {
                let slot = self.slots[i];
                if !self.complete[i] || has(slot) {
                    return false;
                }
                let parents = &self.parents[self.parent_at[i]..self.parent_at[i + 1]];
                !parents.is_empty()
                    && parents.iter().all(|parent| match parent {
                        Parent::InRange(p) => self.complete[*p] && has(self.slots[*p]),
                        Parent::OutOfRange(below) => self
                            .boundary
                            .get(below)
                            .and_then(Option::as_ref)
                            .is_some_and(|paths| paths.contains(&id)),
                    })
            })
            .map(RemovedAt::Commit)
    }

    /// The first known introduction, when that tree and all older states are complete.
    fn first_tree(&self, id: PathId) -> Option<(ObjectId, usize)> {
        let at = (0..self.commits.len())
            .rev()
            .find(|&i| self.states.missing(i, id))?;
        (at..self.commits.len())
            .all(|i| self.states.complete(i))
            .then_some((self.commits[at].tree, at))
    }
}

/// The `removed` paths in the order a bare name or suffix several of them end picks from:
/// workspace items before items of dependencies, which are those outside the workspace
/// `crates`, later removals before earlier ones, then by display path.
fn ranked(
    interner: &Interner,
    history: &History<'_>,
    removed: &[PathId],
    dependency_items: &HashMap<PathId, PathId>,
    crates: &HashSet<Sym>,
) -> Vec<PathId> {
    let mut by_entry = HashMap::new();
    let mut keyed: Vec<_> = removed
        .iter()
        .map(|&id| {
            let workspace = crates.contains(&interner.segments(id)[0]);
            let removal = match dependency_items.get(&id) {
                Some(&entry) if !workspace => *by_entry
                    .entry(entry)
                    .or_insert_with(|| history.removal(entry)),
                Some(&entry) => history.latest(&[id, entry]),
                None => history.removal(id),
            };
            let last = RemovedAt::Commit(usize::MAX);
            (
                (!workspace, removal.unwrap_or(last), display(interner, id)),
                id,
            )
        })
        .collect();
    keyed.sort();
    keyed.into_iter().map(|(_, id)| id).collect()
}

/// Words under one case rule, each with its category and the first selected file holding it.
#[derive(Default)]
struct VocabularyWords {
    entries: HashMap<String, (Category, usize)>,
    technical: bool,
}

impl VocabularyWords {
    fn insert(&mut self, word: String, category: Category, file: usize) {
        self.technical |= category == Category::Technical;
        self.entries
            .entry(word)
            .and_modify(|(kind, _)| {
                if category == Category::Technical {
                    *kind = Category::Technical;
                }
            })
            .or_insert((category, file));
    }

    fn applies(&self, text: bool) -> bool {
        !self.entries.is_empty() && (text || self.technical)
    }

    /// The file holding `word` when its category applies to the context.
    fn source(&self, word: &str, text: bool) -> Option<usize> {
        self.entries
            .get(word)
            .filter(|(category, _)| text || *category == Category::Technical)
            .map(|&(_, file)| file)
    }

    fn claims(&self, word: &str, text: bool) -> bool {
        self.source(word, text).is_some()
    }
}

/// The exact-case technical built-ins, in catalog order, with their word lists.
const TECHNICAL: [(config::Builtin, fn() -> stdlib::Vocabulary); 12] = [
    (config::Builtin::Sql2023, stdlib::Vocabulary::sql_2023),
    (
        config::Builtin::WasiPreview1,
        stdlib::Vocabulary::wasi_preview1,
    ),
    (config::Builtin::Wasm3, stdlib::Vocabulary::wasm_3),
    (config::Builtin::Android36, stdlib::Vocabulary::android_36),
    (config::Builtin::Uikit27_1, stdlib::Vocabulary::uikit_27_1),
    (config::Builtin::Posix2017, stdlib::Vocabulary::posix_2017),
    (
        config::Builtin::WebApi2026_10,
        stdlib::Vocabulary::web_api_2026_10,
    ),
    (config::Builtin::Python3_15, stdlib::Vocabulary::python_3_15),
    (
        config::Builtin::Javascript2026_10,
        stdlib::Vocabulary::javascript_2026_10,
    ),
    (config::Builtin::Svg2026_10, stdlib::Vocabulary::svg_2026_10),
    (config::Builtin::Postgres18, stdlib::Vocabulary::postgres_18),
    (config::Builtin::Sqlite3_54, stdlib::Vocabulary::sqlite_3_54),
];

/// The selected optional words, stored once under each declared case rule.
#[derive(Default)]
struct Vocabularies {
    english: Option<stdlib::Vocabulary>,
    /// The selected exact-case technical built-ins, in catalog order.
    technical: Vec<(config::Builtin, stdlib::Vocabulary)>,
    sensitive: VocabularyWords,
    ascii: VocabularyWords,
    unicode: VocabularyWords,
    /// The canonical paths of the selected custom files, by the index the words keep.
    files: Vec<String>,
    /// The words a SQL document's own dialect claims inside it whatever the selection, the
    /// key words of SQL:2023 in every dialect.
    sql: Vec<(Option<Dialect>, stdlib::Vocabulary)>,
}

impl Vocabularies {
    fn new(config: &config::Config, files: Vec<config::Custom>) -> Self {
        let mut selected = Self {
            english: config
                .selects(config::Builtin::English)
                .then(stdlib::Vocabulary::english),
            technical: TECHNICAL
                .iter()
                .filter(|(builtin, _)| config.selects(*builtin))
                .map(|&(builtin, words)| (builtin, words()))
                .collect(),
            sql: vec![
                (None, stdlib::Vocabulary::sql_2023()),
                (Some(Dialect::Postgres), stdlib::Vocabulary::postgres_18()),
                (Some(Dialect::Sqlite), stdlib::Vocabulary::sqlite_3_54()),
            ],
            ..Self::default()
        };
        for file in files {
            let index = selected.files.len();
            selected.files.push(file.path.display().to_string());
            let words = match file.case {
                config::Case::Sensitive => &mut selected.sensitive,
                config::Case::AsciiInsensitive => &mut selected.ascii,
                config::Case::UnicodeLowercase => &mut selected.unicode,
            };
            for mut word in file.words {
                match file.case {
                    config::Case::Sensitive => {}
                    config::Case::AsciiInsensitive => word.make_ascii_lowercase(),
                    config::Case::UnicodeLowercase => {
                        if needs_lowercase(&word) {
                            word = word.to_lowercase();
                        }
                    }
                }
                words.insert(word, file.kind, index);
            }
        }
        selected
    }

    /// The exact-case entry `word` would match in uppercase where `kind` lets it apply.
    fn uppercase(&self, word: &str, kind: Kind) -> Option<Uppercase> {
        let text = kind == Kind::Text;
        if !text && kind != Kind::CodeSpan {
            return None;
        }
        let spelling = word.to_uppercase();
        if spelling == word {
            return None;
        }
        let vocabulary = match self
            .technical
            .iter()
            .find(|(_, words)| words.contains(&spelling))
        {
            Some((builtin, _)) => vocabulary::id(*builtin).to_owned(),
            None => self.files[self.sensitive.source(&spelling, text)?].clone(),
        };
        Some(Uppercase {
            spelling,
            vocabulary,
        })
    }

    /// Whether a vocabulary claims `word` where `kind` lets it apply, in a SQL document of
    /// `dialect` when there is one.
    fn claims_in(&self, word: &str, kind: Kind, dialect: Option<Dialect>) -> bool {
        let sql = dialect.is_some_and(|dialect| {
            kind == Kind::Text
                && self.sql.iter().any(|(only, words)| {
                    only.is_none_or(|only| only == dialect) && words.contains(word)
                })
        });
        sql || self.claims(word, kind)
    }

    fn claims(&self, word: &str, kind: Kind) -> bool {
        let text = kind == Kind::Text;
        if !text && kind != Kind::CodeSpan {
            return false;
        }
        if self.sensitive.claims(word, text)
            || self.technical.iter().any(|(_, words)| words.contains(word))
        {
            return true;
        }
        let english = self.english.as_ref().filter(|_| text);
        let lowercase = (self.unicode.applies(text) || english.is_some()).then(|| {
            if needs_lowercase(word) {
                Cow::Owned(word.to_lowercase())
            } else {
                Cow::Borrowed(word)
            }
        });
        if let Some(lowercase) = &lowercase
            && (english.is_some_and(|english| english.contains(lowercase))
                || self.unicode.claims(lowercase, text))
        {
            return true;
        }
        if !self.ascii.applies(text) {
            return false;
        }
        let ascii = if word.is_ascii() { lowercase } else { None };
        let ascii = ascii.unwrap_or_else(|| {
            if word.bytes().any(|byte| byte.is_ascii_uppercase()) {
                Cow::Owned(word.to_ascii_lowercase())
            } else {
                Cow::Borrowed(word)
            }
        });
        self.ascii.claims(&ascii, text)
    }
}

fn needs_lowercase(word: &str) -> bool {
    word.chars()
        .any(|character| character.to_lowercase().ne(std::iter::once(character)))
}

/// Decides which spelled paths, bare names and manifest names name something removed, and
/// which name a changed item.
struct Matcher<'a> {
    interner: &'a Interner,
    alive: HashSet<Box<[Sym]>>,
    /// Removed paths by each suffix, in the order [`ranked`] gives.
    removed: HashMap<Box<[Sym]>, Vec<PathId>>,
    /// The last segment of head items, dependency items and allowed bare names.
    names: HashSet<Sym>,
    /// The identifiers item-position macros of head may turn into items, in any module.
    macro_idents: HashSet<Sym>,
    /// The names of head no path reaches.
    locals: &'a HashSet<Sym>,
    /// Removed items other than serialized spellings, by last segment, ranked.
    removed_names: HashMap<Sym, Vec<PathId>>,
    /// Removed serialized spellings, by spelling, ranked.
    removed_spellings: HashMap<Sym, Vec<PathId>>,
    /// The library crate names of head.
    libs: Vec<&'a str>,
    std: [stdlib::Layer; 4],
    vocabularies: &'a Vocabularies,
    entries: &'a Entries<'a>,
    /// The crate names of every workspace target in any indexed snapshot.
    crates: &'a HashSet<Sym>,
    /// The re-exports of head with the sources they stand for.
    reexports: &'a Reexports,
}

impl<'a> Matcher<'a> {
    fn add_vocabulary(&mut self, names: &[String]) {
        let interner = self.interner;
        self.names.extend(
            names
                .iter()
                .filter(|name| !name.contains("::"))
                .filter_map(|name| interner.find_sym(name)),
        );
    }
    /// Builds the matcher of head and its dependencies in `shared` from the `removed` paths in
    /// their [`ranked`] order, where the `rooted` among them match only as whole paths from
    /// their crate root.
    fn new(
        interner: &'a Interner,
        shared: &Shared<'a>,
        removed: &[PathId],
        entries: &'a Entries<'a>,
        crates: &'a HashSet<Sym>,
        rooted: &HashSet<PathId>,
    ) -> Self {
        let head = shared.head_index;
        let dependencies = shared.dependencies;
        let mut alive = HashSet::new();
        let mut names = head.locals.clone();
        for &id in head.paths.iter().chain(&dependencies.paths) {
            let segments = interner.segments(id);
            alive.extend(suffixes(segments).map(Box::from));
            let kind = interner.kind(id);
            if kind == ItemKind::Feature {
                // Binaries, examples and packages claim theirs through their crate roots.
                names.insert(segments[2]);
            } else if !kind.is_manifest_entry() {
                names.extend(segments.last().copied());
            }
        }
        let mut removed_suffixes: HashMap<Box<[Sym]>, Vec<PathId>> = HashMap::new();
        let mut removed_names: HashMap<Sym, Vec<PathId>> = HashMap::new();
        let mut removed_spellings: HashMap<Sym, Vec<PathId>> = HashMap::new();
        for &id in removed {
            let segments = interner.segments(id);
            if rooted.contains(&id) {
                removed_suffixes
                    .entry(Box::from(segments))
                    .or_default()
                    .push(id);
                continue;
            }
            for suffix in suffixes(segments) {
                removed_suffixes
                    .entry(Box::from(suffix))
                    .or_default()
                    .push(id);
            }
            let by_name = match interner.kind(id) {
                kind if kind.is_manifest_entry() => continue,
                ItemKind::Serialized => &mut removed_spellings,
                _ => &mut removed_names,
            };
            if let [_, .., last] = segments {
                by_name.entry(*last).or_default().push(id);
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
            locals: &head.locals,
            removed_names,
            removed_spellings,
            libs: head.libs.iter().map(|&lib| interner.name(lib)).collect(),
            std: stdlib::layers(),
            vocabularies: shared.vocabularies,
            entries,
            crates,
            reexports: &head.reexports,
        }
    }

    /// The removed item or manifest entry `path`, found in `file`, names, `dialect` being the
    /// dialect of a `.sql` file.
    fn stale(
        &self,
        path: &Spelled,
        file: &str,
        (in_rust, dialect): (bool, Option<Dialect>),
        module: Option<&[Sym]>,
    ) -> Option<PathId> {
        let scope = self.entries.scope(file);
        // The crate a Rust file belongs to, else the package of the member holding the file.
        let own = match module {
            Some(module) => module.first().copied(),
            None => scope
                .and_then(|dir| self.entries.package(dir))
                .and_then(|package| self.interner.find_sym(&package.replace('-', "_"))),
        };
        match &path.named {
            Named::Path => self.stale_path(path, scope, in_rust, module, own),
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
            Named::Name { word, data } => {
                self.stale_name(word, *data, (path.context, dialect), (own, scope))
            }
        }
    }

    /// The uppercase hint of a bare name `path` that names the removed item `removed`.
    fn uppercase(&self, path: &Spelled, removed: PathId) -> Option<Uppercase> {
        match &path.named {
            Named::Name { word, data: false }
                if self.interner.kind(removed) != ItemKind::Serialized =>
            {
                self.vocabularies.uppercase(word, path.context.kind)
            }
            _ => None,
        }
    }

    /// The changed item `path`, found in a file read as `module`, names and no other item does.
    fn changed(
        &self,
        changed: &Changed,
        path: &Spelled,
        (in_rust, dialect): (bool, Option<Dialect>),
        module: Option<&[Sym]>,
    ) -> Option<PathId> {
        match &path.named {
            Named::Path => self.changed_path(changed, path, in_rust, module),
            Named::Name { word, data: false } if !word.contains('-') => self.changed_name(
                changed,
                word,
                (path.context, dialect),
                module.filter(|_| in_rust),
            ),
            _ => None,
        }
    }

    /// The changed item the bare `word` names: the item of that name in a Rust file's
    /// `module`, else the item [`Changed::named_bare`] picks, unless something else claims it.
    fn changed_name(
        &self,
        changed: &Changed,
        word: &str,
        (context, dialect): (Context, Option<Dialect>),
        module: Option<&[Sym]>,
    ) -> Option<PathId> {
        let sym = self.interner.find_sym(word)?;
        let claimed = self.locals.contains(&sym)
            || self.macro_idents.contains(&sym)
            || self.std.iter().any(|layer| layer.has_name(word))
            || self.vocabularies.claims_in(word, context.kind, dialect);
        if claimed {
            return None;
        }
        if let Some(module) = module {
            let mut path = module.to_vec();
            path.push(sym);
            if self.alive.contains(path.as_slice()) {
                return changed.named(&path);
            }
        }
        changed.named_bare(sym, module.and_then(|module| module.first().copied()))
    }

    /// The changed item every reading of `path` that names a live item names.
    fn changed_path(
        &self,
        changed: &Changed,
        path: &Spelled,
        in_rust: bool,
        module: Option<&[Sym]>,
    ) -> Option<PathId> {
        let mut readings = self.readings(path, in_rust, module);
        if path.context.kind == (Kind::CodeBlock { compiled: false }) {
            readings.retain(|reading| self.is_crate(reading[0]));
        }
        let mut found = None;
        for reading in &readings {
            match self.live_prefix(changed, reading) {
                Live::Changed(item) if found.is_none_or(|found| found == item) => {
                    found = Some(item);
                }
                Live::Changed(_) | Live::Other => return None,
                Live::Unknown => {}
            }
        }
        found
    }

    /// What the longest prefix of `path` alive in head, a dependency or the standard library
    /// names among the items of `changed`.
    fn live_prefix(&self, changed: &Changed, path: &[&str]) -> Live {
        if self.reexports_alive(path) {
            return Live::Other;
        }
        for len in (2..=path.len()).rev() {
            let prefix = &path[..len];
            if self
                .std
                .iter()
                .any(|layer| layer.ends_a_path(&prefix.join("::")))
            {
                return Live::Other;
            }
            let Some(syms) = prefix
                .iter()
                .map(|name| self.interner.find_sym(name))
                .collect::<Option<Vec<Sym>>>()
            else {
                continue;
            };
            if self.alive.contains(syms.as_slice()) {
                return changed.named(&syms).map_or(Live::Other, Live::Changed);
            }
        }
        Live::Unknown
    }

    /// Resolves a bare item or serialized name, subject to live names and vocabulary ambiguity.
    /// A serialized spelling counts only in a member whose code can name its type's crate, the
    /// crate `own` mentions belong to coming first.
    fn stale_name(
        &self,
        word: &str,
        data: bool,
        (context, dialect): (Context, Option<Dialect>),
        (own, scope): (Option<Sym>, Option<&str>),
    ) -> Option<PathId> {
        let sym = self.interner.find_sym(word)?;
        let claimed = self.names.contains(&sym)
            || self.macro_idents.contains(&sym)
            || self.std.iter().any(|layer| layer.has_name(word));
        if claimed {
            return None;
        }
        let spelling_only = data || word.contains('-');
        let name = if spelling_only {
            None
        } else {
            self.removed_names.get(&sym)
        };
        if let Some(name) =
            name.filter(|_| !self.vocabularies.claims_in(word, context.kind, dialect))
        {
            return Some(self.pick(name, own));
        }
        let reachable: Vec<PathId> = self
            .removed_spellings
            .get(&sym)?
            .iter()
            .copied()
            .filter(|&id| {
                scope.is_none_or(|dir| {
                    let krate = self.interner.name(self.interner.segments(id)[0]);
                    self.entries.reaches(dir, krate)
                })
            })
            .collect();
        (!reachable.is_empty()).then(|| self.pick(&reachable, own))
    }

    /// The removed item every reading of `path` names, preferring an item of crate `own`, else
    /// the removed dependency its first segment names when no reading is alive.
    fn stale_path(
        &self,
        path: &Spelled,
        scope: Option<&str>,
        in_rust: bool,
        module: Option<&[Sym]>,
        own: Option<Sym>,
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
            match self.match_removed(reading, own) {
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
        if matches!(path.context.kind, Kind::CodeBlock { .. }) {
            // Rustdoc compiles every example as a crate of its own.
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
    /// removed, says about it, after a prefix re-exporting a live dependency path, preferring
    /// a removed item of crate `own`.
    fn match_removed(&self, path: &[&str], own: Option<Sym>) -> Hit {
        if self.reexports_alive(path) {
            return Hit::Alive;
        }
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
            if let Some(removed) = self.removed.get(syms.as_slice()) {
                return Hit::Removed(self.pick(removed, own));
            }
        }
        Hit::Unknown
    }

    /// The first of the ranked `candidates` in crate `own`, else the first of them.
    fn pick(&self, candidates: &[PathId], own: Option<Sym>) -> PathId {
        own.and_then(|own| {
            candidates
                .iter()
                .find(|&&id| self.interner.segments(id)[0] == own)
        })
        .unwrap_or(&candidates[0])
        .to_owned()
    }

    /// Whether a prefix of `path` re-exports a source under which the rest of `path` is alive,
    /// which the copies below re-exports miss for a source outside the workspace.
    fn reexports_alive(&self, path: &[&str]) -> bool {
        let Some(syms) = path
            .iter()
            .map(|name| self.interner.find_sym(name))
            .collect::<Option<Vec<Sym>>>()
        else {
            return false;
        };
        (1..syms.len()).any(|len| {
            self.reexports.get(&syms[..len]).is_some_and(|sources| {
                sources.iter().any(|source| {
                    let target: Vec<Sym> = source.iter().chain(&syms[len..]).copied().collect();
                    self.alive.contains(target.as_slice())
                })
            })
        })
    }
}

/// What a reading of a path names.
enum Hit {
    Alive,
    Removed(PathId),
    Unknown,
}

/// What the longest live prefix of a reading names among the changed items.
enum Live {
    /// No prefix is alive.
    Unknown,
    /// An unchanged item, several items, or one outside the workspace.
    Other,
    /// That changed item alone.
    Changed(PathId),
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

    /// The package of member `dir`.
    fn package(&self, dir: &str) -> Option<&'a str> {
        self.labels.get(dir).copied()
    }

    /// The member whose directory holds `file`, none when no member does.
    fn scope(&self, file: &str) -> Option<&'a str> {
        self.dirs
            .iter()
            .copied()
            .find(|dir| config::below(dir, file))
    }

    /// The member directory of package `name`.
    fn package_dir(&self, name: &str) -> Option<&'a str> {
        self.packages
            .get(&comparable(ItemKind::Package, name))
            .copied()
    }

    /// The removed entry of one of `kinds` called `name` as seen from `scope`, when no entry of
    /// those kinds keeps the name alive there and no package of head is a dependency of that name.
    fn stale(&self, kinds: &[ItemKind], name: &str, scope: Option<&str>) -> Option<PathId> {
        let alive = kinds.iter().any(|&kind| {
            let name = comparable(kind, name);
            match scope {
                Some(dir) => self.head.contains(&(kind, name, dir)),
                None => self.head_any.contains(&(kind, name)),
            }
        });
        let package = (ItemKind::Package, comparable(ItemKind::Package, name));
        if alive || (kinds.contains(&ItemKind::Dependency) && self.head_any.contains(&package)) {
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

    /// Whether code of member `dir` can name items of crate `name`, as its own package or a
    /// dependency it declares at head or declared during the range.
    fn reaches(&self, dir: &str, name: &str) -> bool {
        let dependency = comparable(ItemKind::Dependency, name);
        self.package(dir)
            .is_some_and(|own| comparable(ItemKind::Package, own) == dependency)
            || self
                .head
                .contains(&(ItemKind::Dependency, dependency.clone(), dir))
            || self
                .removed
                .get(&(ItemKind::Dependency, dependency))
                .is_some_and(|removed| removed.iter().any(|&(d, _)| d == dir))
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

#[cfg(test)]
mod tests {
    use super::{PathBits, States, in_runs, word_of};

    /// The states of one commit per tree, each tree read entirely and missing `trees[i]`.
    fn states(trees: &[&[u32]]) -> States {
        let mut states = States::default();
        for tree in trees {
            let slot = states.add_tree(tree.iter().copied(), true);
            states.slot_of.push(slot);
        }
        states
    }

    #[test]
    fn a_path_is_missing_exactly_in_the_trees_of_its_runs() {
        let states = states(&[&[7], &[7, 8], &[7], &[8], &[7], &[]]);
        let seven: Vec<bool> = (0..6).map(|i| states.missing(i, 7)).collect();
        assert_eq!(seven, [true, true, true, false, true, false]);
        let eight: Vec<bool> = (0..6).map(|i| states.missing(i, 8)).collect();
        assert_eq!(eight, [false, true, false, true, false, false]);
        assert!(!states.missing(0, 9));
    }

    #[test]
    fn a_commit_repeating_an_earlier_tree_reads_that_tree() {
        let mut states = states(&[&[7], &[]]);
        states.slot_of.push(0);
        assert!(states.missing(2, 7));
        assert!(!states.missing(1, 7));
    }

    #[test]
    fn an_incomplete_tree_has_no_path_it_lists() {
        let mut states = States::default();
        let slot = states.add_tree([7].into_iter(), false);
        states.slot_of.push(slot);
        assert!(states.missing(0, 7));
        assert!(!states.has(0, 7));
    }

    #[test]
    fn the_slot_bitset_marks_exactly_the_slots_of_the_runs() {
        let runs: [(u32, u32); 3] = [(0, 40), (100, 250), (280, 290)];
        let bits = PathBits::with_slots(300);
        {
            let words = bits.set(Some(&runs));
            for slot in 0..300u32 {
                let marked = words.words[word_of(slot)] >> (slot & 63) & 1 != 0;
                assert_eq!(marked, in_runs(&runs, slot), "slot {slot}");
            }
        }
        {
            let words = bits.set(Some(&[(63, 64)]));
            for slot in 0..300u32 {
                let marked = words.words[word_of(slot)] >> (slot & 63) & 1 != 0;
                assert_eq!(marked, in_runs(&[(63, 64)], slot), "slot {slot}");
            }
        }
        let words = bits.set(None);
        assert!(words.words.iter().all(|&word| word == 0));
    }
}
