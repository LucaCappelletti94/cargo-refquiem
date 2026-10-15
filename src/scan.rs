//! A scan: index the range, subtract head, find mentions, attribute removals.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use gix::ObjectId;

use crate::cache::FileCache;
use crate::git::{self, Head, Snapshot, TreeSnapshot, WorkTreeSnapshot};
use crate::index::{Index, Indexer};
use crate::intern::{Interner, PathId, Sym};
use crate::text::{self, Named, Spelled};
use crate::{
    Error, Group, ItemKind, Kind, Mention, Options, RemovedIn, Report, Source, entry, stdlib,
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

    let entries = Entries::new(interner, &head_index, &removed, &indexer.members);
    let matcher = Matcher::new(interner, &head_index, &removed, &entries);
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
        found.dedup_by(|a, b| (&a.file, a.line, a.column) == (&b.file, b.line, b.column));
        let kind = interner.kind(id);
        let (path, member) = if kind.is_manifest_entry() {
            let (dir, name) = entries.decode(id);
            (name.to_owned(), entries.member_label(dir))
        } else {
            (interner.display(id), None)
        };
        groups.push(Group {
            path,
            kind,
            member,
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
            text::rust(source)
        } else {
            text::markdown_file(source)
        };
        if spelled.is_empty() {
            continue;
        }
        let starts = text::line_starts(source);
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

/// Decides which spelled paths and manifest names name something removed.
struct Matcher<'a> {
    interner: &'a Interner,
    alive: HashSet<Box<[Sym]>>,
    removed: HashMap<Box<[Sym]>, PathId>,
    /// The library crate names of head.
    libs: Vec<&'a str>,
    std: [stdlib::Layer; 4],
    entries: &'a Entries<'a>,
}

impl<'a> Matcher<'a> {
    fn new(
        interner: &'a Interner,
        head: &'a Index,
        removed: &[PathId],
        entries: &'a Entries<'a>,
    ) -> Self {
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
            libs: head.libs.iter().map(|&lib| interner.name(lib)).collect(),
            std: stdlib::layers(),
            entries,
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
        }
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
            .is_some_and(|sym| self.interner.find_path(&[sym]).is_some())
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
