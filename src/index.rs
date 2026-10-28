//! Every item path and manifest entry a snapshot declares, assembled from its crates' module
//! trees and manifests.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::hash::{BuildHasher, Hasher};
use std::rc::Rc;

use gix::ObjectId;
use hashbrown::{DefaultHashBuilder, DefaultHasher};

use crate::cache::{Expanded, FileCache};
use crate::git::{ManifestDirs, Snapshot, Trees};
use crate::intern::{Interner, PathId, Sym};
use crate::manifest::{self, Manifests, Member, TargetKind, join};
use crate::parse::{self, FileItems, IncludeBase};
use crate::{Error, HashMap, HashSet, ItemKind, entry, expand};

/// The paths of one snapshot.
#[derive(Default)]
pub(crate) struct Index {
    pub(crate) paths: HashSet<PathId>,
    /// False when a manifest or a reachable file could not be read as Rust or TOML.
    pub(crate) complete: bool,
    /// The module each Rust file of the snapshot was reached as, first reach winning.
    pub(crate) file_modules: HashMap<String, Vec<Sym>>,
    /// Identifiers that item-position macros may turn into items, per module.
    pub(crate) alive: HashMap<PathId, HashSet<Sym>>,
    /// The library crate names.
    pub(crate) libs: Vec<Sym>,
    /// The names no path reaches: parameters, generic parameters and items declared inside a
    /// function body.
    pub(crate) locals: HashSet<Sym>,
    /// The signature of every function, method and trait method, as the parser records it.
    pub(crate) signatures: HashMap<PathId, Rc<str>>,
    /// The dependency tables of every dependency entry, as [`manifest::NORMAL`] bits.
    pub(crate) tables: HashMap<PathId, u8>,
    /// The item-position macros head invokes without expanding them, by name.
    pub(crate) unexpanded: HashSet<Sym>,
    /// Every re-export with the source it stands for.
    pub(crate) reexports: Reexports,
}

/// A named or glob re-export with its module, source, alias and visibility.
type UseDecl = (Vec<Sym>, Vec<Sym>, Option<Sym>, bool);

/// A lend: the module it sits in, the owner reaching the members and their source as written.
type LendDecl = (Vec<Sym>, Vec<Sym>, Vec<Sym>);

/// A named `use` of any visibility: the module it sits in, its source as written and the name.
type Import = (Vec<Sym>, Vec<Sym>, Sym);

/// Re-exports: each path a mention may start with, mapped to the paths it stands for as
/// written.
pub(crate) type Reexports = HashMap<Vec<Sym>, Vec<Vec<Sym>>>;

/// One file, or one macro expansion, read as one module of one crate, reusable across
/// snapshots.
#[derive(Default)]
struct Contribution {
    paths: Vec<PathId>,
    /// The line of each of `paths`, in the same order.
    lines: Vec<u32>,
    /// Paths declared without `pub`, among them the private modules kept when only public
    /// items count.
    hidden: Vec<PathId>,
    /// Paths declared with `pub`, which no hidden declaration of the same path elsewhere hides.
    exposed: Vec<PathId>,
    visits: Vec<Rc<Visit>>,
    uses: Vec<UseDecl>,
    /// The line of each of `uses`, in the same order.
    use_lines: Vec<u32>,
    calls: Vec<Call>,
    macro_rules: Vec<Rules>,
    lends: Vec<LendDecl>,
    traits: Vec<Implemented>,
    declared_traits: Vec<(PathId, bool)>,
    locals: Vec<Sym>,
    signatures: Vec<(PathId, Rc<str>)>,
    failure: Option<String>,
    /// The digest of the fields an alias resolution reads from this contribution.
    digest: [u64; 2],
}

impl Contribution {
    /// Whether `other` carries the same alias inputs, field by field.
    fn alias_inputs_equal(&self, other: &Contribution) -> bool {
        self.paths == other.paths
            && self.hidden == other.hidden
            && self.exposed == other.exposed
            && self.uses == other.uses
            && self.lends == other.lends
            && self.declared_traits == other.declared_traits
            && self.traits == other.traits
    }
}

#[derive(Clone, PartialEq)]
struct Implemented {
    module: Vec<Sym>,
    source: Vec<Sym>,
    members: Vec<PathId>,
}

struct TraitScope<'a> {
    declarations: HashMap<PathId, bool>,
    implementations: Vec<&'a Implemented>,
}

impl<'a> TraitScope<'a> {
    fn new(
        declarations: impl Iterator<Item = (PathId, bool)>,
        implementations: impl Iterator<Item = &'a Implemented>,
    ) -> Self {
        let implementations: Vec<_> = implementations.collect();
        let mut known = HashMap::new();
        if !implementations.is_empty() {
            for (id, public) in declarations {
                *known.entry(id).or_insert(false) |= public;
            }
        }
        Self {
            declarations: known,
            implementations,
        }
    }
}

/// A macro invocation at item or associated-item position.
struct Call {
    module: PathId,
    name: Sym,
    /// The crate a qualified or imported invocation takes its macro from, as `bitflags` for
    /// `bitflags::bitflags!`.
    crate_root: Option<Sym>,
    idents: Vec<Sym>,
    /// For an item-position invocation, what expanding it needs.
    expandable: Option<Expandable>,
}

/// An invocation as expansion reads it.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Expandable {
    /// The inline modules enclosing it in its file.
    inline: Rc<[String]>,
    tokens: Rc<str>,
    /// For an invocation among an impl's items, the impl's header, which wraps the expansion.
    impl_header: Option<Rc<str>>,
}

/// A `macro_rules!` definition.
struct Rules {
    /// The module it is defined in.
    module: PathId,
    name: Sym,
    idents: Vec<Sym>,
    body: Rc<str>,
    exported: bool,
}

/// A `macro_rules!` body with the crate `$crate` names, `None` for the crate being indexed.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct Definition {
    body: Rc<str>,
    home: Option<Sym>,
}

/// A file or expansion `Indexer::reach` read.
#[derive(Clone)]
struct Entry {
    /// The visit it was read for.
    visit: Rc<Visit>,
    contribution: Rc<Contribution>,
    /// How many expansions deep it lies.
    depth: usize,
}

/// The crate roots `Indexer::roots` found.
struct Roots {
    stack: Vec<Rc<Visit>>,
    crates: Vec<Sym>,
    /// The crates of packages outside the workspace.
    outside: HashSet<Sym>,
}

/// What `Indexer::reach` read.
#[derive(Default)]
struct Reached {
    entries: Vec<Entry>,
    /// The invocations expanded by a `macro_rules!` definition, by contribution and index.
    expanded: HashSet<(*const Contribution, usize)>,
    /// The invocations of macros without a known definition, already read as items.
    parsed: HashSet<(*const Contribution, usize)>,
    /// The `macro_rules!` definitions of the entries before `ruled` with their modules, by
    /// crate and name.
    rules: HashMap<(Sym, Sym), Vec<(PathId, Definition)>>,
    ruled: usize,
    /// How many entries have had their invocations looked at.
    examined: usize,
    /// Invocations without a known definition, by entry and invocation index.
    deferred: Vec<(usize, usize)>,
    /// The expansions among the entries, which identical invocations share.
    expansions: HashSet<*const Contribution>,
}

impl Reached {
    fn contributions(&self) -> impl Iterator<Item = &Rc<Contribution>> {
        self.entries.iter().map(|entry| &entry.contribution)
    }

    /// The invocations of every entry that no `macro_rules!` definition expanded.
    fn unexpanded(&self) -> impl Iterator<Item = &Call> {
        self.contributions().flat_map(move |contribution| {
            contribution
                .calls
                .iter()
                .enumerate()
                .filter(move |(k, _)| !self.expanded.contains(&(Rc::as_ptr(contribution), *k)))
                .map(|(_, call)| call)
        })
    }
}

/// How deep expansions may produce further expansions, as rustc's default recursion limit.
const RECURSION_LIMIT: usize = 128;

/// An expansion's visit, invocation and macro definition.
type ExpansionKey = (Rc<Visit>, Expandable, Option<Definition>);

/// A file to read as a module.
#[derive(PartialEq, Eq, Hash)]
struct Visit {
    /// The file read.
    file: String,
    /// Whether `file` is a `mod.rs`-style file owning its directory, as an included file is.
    mod_rs: bool,
    module: PathId,
    /// The directory of the package's `Cargo.toml`, against which `CARGO_MANIFEST_DIR` resolves.
    package: Rc<str>,
    /// Whether only `pub` items and `pub use` re-exports count, as for a dependency.
    public_only: bool,
}

/// The public paths of one library, before re-exports between libraries are resolved.
pub(crate) struct Library {
    paths: HashSet<PathId>,
    hidden: Vec<PathId>,
    exposed: Vec<PathId>,
    uses: Vec<UseDecl>,
    lends: Vec<LendDecl>,
    traits: Vec<Implemented>,
    declared_traits: Vec<(PathId, bool)>,
    /// Identifiers its item-position macros may turn into items, in any module.
    pub(crate) macro_idents: HashSet<Sym>,
    /// Its `#[macro_export]` macros by name.
    pub(crate) macro_rules: Vec<(Sym, Definition)>,
    /// The names of its item-position macro invocations no definition expanded.
    pub(crate) unexpanded: HashSet<Sym>,
    /// The crates those invocations name their macros from, as written or imported.
    pub(crate) macro_crates: HashSet<Sym>,
}

impl Library {
    /// The first segments of the library's `pub use` sources, among them the crates it
    /// re-exports from.
    pub(crate) fn use_roots(&self) -> impl Iterator<Item = Sym> {
        self.uses
            .iter()
            .filter_map(|(_, source, _, _)| source.first().copied())
    }
}

/// The identifiers the item-position macro invocations of each module of `reached` may turn
/// into items, with the `macro_rules!` bodies they call. Expanded invocations contribute
/// their items instead.
fn macro_idents(interner: &mut Interner, reached: &Reached) -> HashMap<PathId, HashSet<Sym>> {
    let mut bodies: HashMap<Sym, Vec<Sym>> = HashMap::new();
    for rules in reached.contributions().flat_map(|c| &c.macro_rules) {
        bodies.entry(rules.name).or_default().extend(&rules.idents);
    }
    let mut alive: HashMap<PathId, HashSet<Sym>> = HashMap::new();
    for call in reached.unexpanded() {
        let module = alive.entry(call.module).or_default();
        module.extend(&call.idents);
        if let Some(expandable) = &call.expandable {
            let idents = parse::text_idents(&expandable.tokens);
            module.extend(idents.iter().map(|name| interner.sym(name)));
        }
        if let Some(body) = bodies.get(&call.name) {
            module.extend(body);
        }
    }
    alive
}

/// Builds indexes, sharing parsed files and per-module contributions between snapshots.
pub(crate) struct Indexer {
    pub(crate) interner: Interner,
    cache: FileCache,
    memo: HashMap<(Rc<Visit>, ObjectId), Rc<Contribution>>,
    /// Expansions already read into a module, `None` when the invocation did not expand to
    /// items.
    expansions: HashMap<ExpansionKey, Option<Rc<Contribution>>>,
    expander: expand::Expander,
    /// The `#[macro_export]` macros of indexed dependencies by name, expanded wherever the
    /// indexed crate defines no macro of that name.
    dependency_macros: HashMap<Sym, Rc<Vec<Definition>>>,
    /// The item-position macros head invokes without expanding them, by name, which workspace
    /// history leaves unexpanded too, as anything they produced there head may still produce.
    head_unexpanded: HashSet<Sym>,
    /// The package names every member directory had in any indexed snapshot.
    pub(crate) members: BTreeMap<String, BTreeSet<String>>,
    /// The crate names of every workspace target in any indexed snapshot.
    pub(crate) crates: HashSet<Sym>,
    manifest_dirs: ManifestDirs,
    /// The manifests parsed so far, by the blob id of their `Cargo.toml`.
    pub(crate) manifests: Manifests,
    /// The entries of each committed tree parsed so far, by tree id, shared by every snapshot.
    pub(crate) trees: Rc<RefCell<Trees>>,
    /// Recent alias resolutions of snapshots other than head, most recently used first.
    aliases: VecDeque<AliasEntry>,
    /// The hasher of alias keys, one seed for the whole run so equal keys hash equal.
    alias_hasher: DefaultHashBuilder,
    /// The two seeds of a contribution alias digest, one per 64-bit half.
    digest_seeds: [DefaultHashBuilder; 2],
    keywords: Keywords,
    /// The link chain [`Indexer::copy_below`] fills, kept longest prefix first and reused
    /// between its calls.
    copy_links: Vec<(PathId, usize)>,
}

/// The symbols of the path keywords.
struct Keywords {
    krate: Sym,
    self_value: Sym,
    self_type: Sym,
    super_module: Sym,
}

impl Indexer {
    pub(crate) fn new(cache: FileCache) -> Self {
        let mut interner = Interner::default();
        let keywords = Keywords {
            krate: interner.sym("crate"),
            self_value: interner.sym("self"),
            self_type: interner.sym("Self"),
            super_module: interner.sym("super"),
        };
        Self {
            interner,
            cache,
            memo: HashMap::new(),
            expansions: HashMap::new(),
            expander: expand::Expander::default(),
            dependency_macros: HashMap::new(),
            head_unexpanded: HashSet::new(),
            members: BTreeMap::new(),
            crates: HashSet::new(),
            manifest_dirs: ManifestDirs::new(),
            manifests: Manifests::new(),
            trees: Rc::new(RefCell::new(Trees::new())),
            aliases: VecDeque::new(),
            alias_hasher: DefaultHashBuilder::default(),
            digest_seeds: [DefaultHashBuilder::default(), DefaultHashBuilder::default()],
            keywords,
            copy_links: Vec::new(),
        }
    }

    /// Makes the `#[macro_export]` macros `rules` of dependencies expandable in every later
    /// index, returning whether any definition was new.
    pub(crate) fn use_dependency_macros(&mut self, rules: &[(Sym, Definition)]) -> bool {
        let mut added = false;
        for (name, definition) in rules {
            let definitions = Rc::make_mut(self.dependency_macros.entry(*name).or_default());
            if !definitions.contains(definition) {
                definitions.push(definition.clone());
                added = true;
            }
        }
        added
    }

    /// Leaves the macros `head` invokes without expanding them unexpanded in every later
    /// workspace index.
    pub(crate) fn use_head_macros(&mut self, head: &Index) {
        self.head_unexpanded.clone_from(&head.unexpanded);
    }

    /// Interns the manifest entries of `member` into `index`.
    fn entries(&mut self, member: &Member, index: &mut Index) {
        let owner = self.interner.sym(&entry::member(&member.dir));
        let mut add = |interner: &mut Interner, kind: ItemKind, name: &str| {
            let path = [owner, interner.sym(entry::marker(kind)), interner.sym(name)];
            let id = interner.path(&path, kind);
            index.paths.insert(id);
            id
        };
        add(&mut self.interner, ItemKind::Package, &member.package);
        for dependency in &member.dependencies {
            let id = add(&mut self.interner, ItemKind::Dependency, &dependency.key);
            index.tables.insert(id, dependency.tables);
            if let Some(package) = &dependency.package {
                let id = add(&mut self.interner, ItemKind::Dependency, package);
                index.tables.insert(id, dependency.tables);
            }
        }
        for feature in &member.features {
            add(&mut self.interner, ItemKind::Feature, feature);
        }
        for target in &member.targets {
            let kind = match target.kind {
                TargetKind::Bin => ItemKind::Binary,
                TargetKind::Example => ItemKind::Example,
                TargetKind::Lib | TargetKind::Other => continue,
            };
            add(&mut self.interner, kind, &target.name);
        }
    }

    pub(crate) fn save(&self) {
        self.cache.save();
    }

    /// The index of `snapshot` for the workspace in `workspace` and every other package in the
    /// tree.
    ///
    /// At `head`, an unreadable manifest or Rust file of the workspace is an error. Elsewhere it
    /// leaves the index incomplete. Packages outside the workspace that cannot be read are skipped.
    pub(crate) fn index(
        &mut self,
        snapshot: &dyn Snapshot,
        workspace: &str,
        head: bool,
    ) -> Result<Index, Error> {
        let mut index = Index {
            complete: true,
            ..Index::default()
        };
        let Some(Roots {
            stack,
            crates,
            outside,
        }) = self.roots(snapshot, workspace, head, &mut index)?
        else {
            return Ok(index);
        };
        // The paths `roots` interned, before `reach` adds what the contributions declare.
        let roots = (!head).then(|| index.paths.iter().copied().collect::<Vec<_>>());
        let reached = self.reach(snapshot, stack, head, &outside, &mut index)?;
        let uses: Vec<&UseDecl> = reached.contributions().flat_map(|c| &c.uses).collect();
        let lends: Vec<&LendDecl> = reached.contributions().flat_map(|c| &c.lends).collect();
        let traits = TraitScope::new(
            reached
                .contributions()
                .flat_map(|c| c.declared_traits.iter().copied()),
            reached.contributions().flat_map(|c| &c.traits),
        );
        index.reexports = if head {
            let mut hidden = hidden_paths(
                reached
                    .contributions()
                    .map(|c| (c.hidden.as_slice(), c.exposed.as_slice())),
            );
            self.expose_traits(&index.paths, &mut hidden, &uses, &lends, &traits, &crates);
            self.resolve_aliases(&mut index.paths, &mut hidden, &uses, &lends, &crates)
                .0
        } else {
            let inputs = AliasInputs {
                roots: roots
                    .as_deref()
                    .expect("a non-head snapshot carries its roots"),
                entries: &reached.entries,
                uses: &uses,
                lends: &lends,
                traits: &traits,
                crates: &crates,
            };
            self.recalled_aliases(&mut index.paths, &inputs)
        };
        if head {
            index.alive = macro_idents(&mut self.interner, &reached);
            index.unexpanded = reached
                .unexpanded()
                .filter(|call| call.expandable.is_some())
                .map(|call| call.name)
                .collect();
            index.locals = reached
                .contributions()
                .flat_map(|c| c.locals.iter().copied())
                .collect();
        }
        Ok(index)
    }

    /// The crate roots of the workspace in `workspace` and every other package of `snapshot`,
    /// interning their manifest entries into `index`. `None` when a snapshot other than head has
    /// no readable workspace manifest, which leaves `index` incomplete.
    fn roots(
        &mut self,
        snapshot: &dyn Snapshot,
        workspace: &str,
        head: bool,
        index: &mut Index,
    ) -> Result<Option<Roots>, Error> {
        let members = match manifest::workspace(snapshot, workspace, &mut self.manifests) {
            Ok(members) => members,
            Err(Error::Manifest { .. }) if !head => {
                index.complete = false;
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let dirs = snapshot.manifest_dirs(&mut self.manifest_dirs)?;
        let outside_members = manifest::packages(snapshot, &dirs, &members, &mut self.manifests);
        let mut workspace_crates = HashSet::new();
        let mut outside = HashSet::new();
        let mut crates = Vec::new();
        let mut stack = Vec::new();
        let workspace_members = members.len();
        for (n, member) in members.into_iter().chain(outside_members).enumerate() {
            self.entries(&member, index);
            let package: Rc<str> = Rc::from(member.dir.as_str());
            self.members
                .entry(member.dir)
                .or_default()
                .insert(member.package);
            for target in member.targets {
                let crate_sym = self.interner.sym(&target.crate_name);
                if n < workspace_members {
                    workspace_crates.insert(crate_sym);
                } else {
                    outside.insert(crate_sym);
                }
                crates.push(crate_sym);
                self.crates.insert(crate_sym);
                if target.kind == TargetKind::Lib {
                    index.libs.push(crate_sym);
                }
                let module = self.interner.path(&[crate_sym], ItemKind::Module);
                index.paths.insert(module);
                stack.push(Rc::new(Visit {
                    file: target.root,
                    mod_rs: true,
                    module,
                    package: package.clone(),
                    public_only: false,
                }));
            }
        }
        // A crate name a workspace member also has stays strict at head.
        outside.retain(|krate| !workspace_crates.contains(krate));
        Ok(Some(Roots {
            stack,
            crates,
            outside,
        }))
    }

    /// The file and line where `snapshot` declares each of `wanted` that it declares in a file.
    /// A path standing behind a re-export points at its source's declaration when `gone` says
    /// head lacks the source, else at the `use` creating it.
    pub(crate) fn locate(
        &mut self,
        snapshot: &dyn Snapshot,
        workspace: &str,
        wanted: &[PathId],
        gone: impl Fn(PathId) -> bool,
    ) -> Result<HashMap<PathId, (String, u32)>, Error> {
        let mut index = Index {
            complete: true,
            ..Index::default()
        };
        let mut found = HashMap::new();
        let Some(Roots {
            stack,
            crates,
            outside,
        }) = self.roots(snapshot, workspace, false, &mut index)?
        else {
            return Ok(found);
        };
        let reached = self.reach(snapshot, stack, false, &outside, &mut index)?;
        // Lines of an expansion count from its own text, so only files place a declaration.
        let mut declared: HashMap<PathId, (&str, u32)> = HashMap::new();
        let mut placed_uses = Vec::new();
        for entry in reached.entries.iter().filter(|entry| entry.depth == 0) {
            let file = entry.visit.file.as_str();
            let contribution = &entry.contribution;
            for (&id, &line) in contribution.paths.iter().zip(&contribution.lines) {
                declared.entry(id).or_insert((file, line));
            }
            for (decl, &line) in contribution.uses.iter().zip(&contribution.use_lines) {
                placed_uses.push((decl, file, line));
            }
        }
        let uses: Vec<&UseDecl> = reached.contributions().flat_map(|c| &c.uses).collect();
        let lends: Vec<&LendDecl> = reached.contributions().flat_map(|c| &c.lends).collect();
        let mut hidden = hidden_paths(
            reached
                .contributions()
                .map(|c| (c.hidden.as_slice(), c.exposed.as_slice())),
        );
        let (_, origins) =
            self.resolve_aliases(&mut index.paths, &mut hidden, &uses, &lends, &crates);
        for &id in wanted {
            let at = match declared.get(&id) {
                Some(&at) => Some(at),
                None => match origins.get(&id) {
                    Some(&source) if gone(source) && declared.contains_key(&source) => {
                        Some(declared[&source])
                    }
                    Some(&source) => {
                        self.reexporting(id, source, &placed_uses, &origins, &crates, &index.paths)
                    }
                    None => None,
                },
            };
            if let Some((file, line)) = at {
                found.insert(id, (file.to_owned(), line));
            }
        }
        Ok(found)
    }

    /// The file and line of the `use` among `uses` through which `id` stands for `source`.
    fn reexporting<'f>(
        &mut self,
        id: PathId,
        source: PathId,
        uses: &[(&UseDecl, &'f str, u32)],
        origins: &HashMap<PathId, PathId>,
        crates: &[Sym],
        paths: &HashSet<PathId>,
    ) -> Option<(&'f str, u32)> {
        let segments = self.interner.segments(id).to_vec();
        for &((module, written, name, _), file, line) in uses {
            let mut base = module.clone();
            base.extend(*name);
            let Some(rest) = segments.strip_prefix(base.as_slice()) else {
                continue;
            };
            let Some((mut target, _)) = self.resolve_use(module, written, crates, paths) else {
                continue;
            };
            target.extend_from_slice(rest);
            if self
                .interner
                .find_path(&target)
                .is_some_and(|target| origin(origins, target) == source)
            {
                return Some((file, line));
            }
        }
        None
    }

    /// Reads every file reachable from `stack` into `index`, with the items its item-position
    /// macro invocations expand to, returning their contributions.
    ///
    /// At `head`, an unreadable Rust file is an error. Elsewhere it leaves the index incomplete.
    /// An unreadable file of an `outside` crate is skipped.
    fn reach(
        &mut self,
        snapshot: &dyn Snapshot,
        mut stack: Vec<Rc<Visit>>,
        head: bool,
        outside: &HashSet<Sym>,
        index: &mut Index,
    ) -> Result<Reached, Error> {
        let mut seen = HashSet::new();
        let mut reached = Reached::default();
        loop {
            while let Some(visit) = stack.pop() {
                if !seen.insert(Rc::clone(&visit)) {
                    continue;
                }
                let Some(id) = snapshot.blob(&visit.file)? else {
                    continue;
                };
                let contribution = self.contribution(snapshot, &visit, id)?;
                if let Some(message) = &contribution.failure {
                    if outside.contains(&self.interner.segments(visit.module)[0]) {
                        continue;
                    }
                    if head {
                        return Err(Error::Parse {
                            path: visit.file.clone(),
                            message: message.clone(),
                        });
                    }
                    index.complete = false;
                    continue;
                }
                if head && !index.file_modules.contains_key(&visit.file) {
                    let module = self.interner.segments(visit.module).to_vec();
                    index.file_modules.insert(visit.file.clone(), module);
                }
                absorb(&contribution, index, &mut stack);
                reached.entries.push(Entry {
                    visit,
                    contribution,
                    depth: 0,
                });
            }
            if !self.expand_calls(&mut reached, index, &mut stack) {
                return Ok(reached);
            }
        }
    }

    /// Expands the item-position invocations of `reached` whose `macro_rules!` definition in
    /// the same crate, or among head's dependencies, is known. When none expands, reads the
    /// invocations of macros without a definition as items. Returns whether anything was added.
    fn expand_calls(
        &mut self,
        reached: &mut Reached,
        index: &mut Index,
        stack: &mut Vec<Rc<Visit>>,
    ) -> bool {
        let mut new_rules = false;
        for entry in &reached.entries[reached.ruled..] {
            let krate = self.interner.segments(entry.visit.module)[0];
            for rule in &entry.contribution.macro_rules {
                let definition = Definition {
                    body: rule.body.clone(),
                    home: None,
                };
                let definitions = reached.rules.entry((krate, rule.name)).or_default();
                if !definitions.contains(&(rule.module, definition.clone())) {
                    definitions.push((rule.module, definition));
                    new_rules = true;
                }
            }
        }
        reached.ruled = reached.entries.len();
        let mut work: Vec<(usize, usize)> = Vec::new();
        for (i, entry) in reached.entries.iter().enumerate().skip(reached.examined) {
            let expandable = entry.contribution.calls.iter().enumerate();
            work.extend(
                expandable
                    .filter(|(_, c)| c.expandable.is_some())
                    .map(|(k, _)| (i, k)),
            );
        }
        reached.examined = reached.entries.len();
        if new_rules {
            work.append(&mut reached.deferred);
        }
        let mut grew = false;
        for (i, k) in work {
            let entry = &reached.entries[i];
            let krate = self.interner.segments(entry.visit.module)[0];
            let call = &entry.contribution.calls[k];
            let unexpandable =
                !entry.visit.public_only && self.head_unexpanded.contains(&call.name);
            let definitions = match reached.rules.get(&(krate, call.name)) {
                _ if unexpandable => None,
                Some(local) => Some(Rc::new(self.visible(local, call.module))),
                None => self.dependency_macros.get(&call.name).cloned(),
            };
            match definitions {
                Some(definitions) => {
                    let found = definitions.iter().map(Some);
                    grew |= self.expand_call(reached, i, k, found, index, stack);
                }
                None => reached.deferred.push((i, k)),
            }
        }
        if !grew {
            for (i, k) in reached.deferred.clone() {
                let key = (Rc::as_ptr(&reached.entries[i].contribution), k);
                if reached.parsed.insert(key) {
                    grew |= self.expand_call(reached, i, k, [None], index, stack);
                }
            }
        }
        grew
    }

    /// The definitions among `all` an invocation in `module` sees, those of the nearest
    /// enclosing module that has any, else all of them, as `#[macro_use]` lends them anywhere.
    fn visible(&self, all: &[(PathId, Definition)], module: PathId) -> Vec<Definition> {
        let segments = self.interner.segments(module);
        for (_, enclosing) in self.interner.prefixes(segments) {
            let found: Vec<Definition> = all
                .iter()
                .filter(|(defined_in, _)| *defined_in == enclosing)
                .map(|(_, definition)| definition.clone())
                .collect();
            if !found.is_empty() {
                return found;
            }
        }
        all.iter()
            .map(|(_, definition)| definition.clone())
            .collect()
    }

    /// Adds what invocation `k` of entry `i` expands to under each of `definitions`, `None`
    /// reading its tokens as items. Returns whether anything was added.
    fn expand_call<'d>(
        &mut self,
        reached: &mut Reached,
        i: usize,
        k: usize,
        definitions: impl IntoIterator<Item = Option<&'d Definition>>,
        index: &mut Index,
        stack: &mut Vec<Rc<Visit>>,
    ) -> bool {
        let entry = reached.entries[i].clone();
        let call = &entry.contribution.calls[k];
        let Some(expandable) = &call.expandable else {
            return false;
        };
        if entry.depth >= RECURSION_LIMIT {
            return false;
        }
        let mut grew = false;
        for definition in definitions {
            let Some(expansion) = self.expansion(&entry.visit, expandable, definition) else {
                continue;
            };
            if definition.is_some() {
                reached
                    .expanded
                    .insert((Rc::as_ptr(&entry.contribution), k));
            }
            if !reached.expansions.insert(Rc::as_ptr(&expansion)) {
                continue;
            }
            absorb(&expansion, index, stack);
            reached.entries.push(Entry {
                visit: Rc::clone(&entry.visit),
                contribution: expansion,
                depth: entry.depth + 1,
            });
            grew = true;
        }
        grew
    }

    /// The items the invocation `expandable` of `visit`'s file expands to under `definition`,
    /// or reads as without one.
    fn expansion(
        &mut self,
        visit: &Rc<Visit>,
        expandable: &Expandable,
        definition: Option<&Definition>,
    ) -> Option<Rc<Contribution>> {
        let key = (Rc::clone(visit), expandable.clone(), definition.cloned());
        if let Some(known) = self.expansions.get(&key) {
            return known.clone();
        }
        let home = definition.map(|definition| {
            definition
                .home
                .map_or("crate", |home| self.interner.name(home))
        });
        let body = definition.map(|definition| &*definition.body);
        let header = expandable.impl_header.as_deref();
        let stored = expansion_key([body, home, Some(&*expandable.tokens), header]);
        let recalled = stored.and_then(|key| self.cache.expansion(key));
        let expanded = if let Some(expanded) = recalled {
            expanded
        } else {
            let text = match (definition, home) {
                (Some(definition), Some(home)) => {
                    self.expander
                        .expand(&definition.body, &expandable.tokens, home)
                }
                _ => Some(expandable.tokens.to_string()),
            };
            let text = match header {
                Some(header) => text.map(|text| format!("{header} {{ {text} }}")),
                None => text,
            };
            let expanded = Expanded::from(text.and_then(|text| parse::parse_expansion(&text).ok()));
            if let Some(key) = stored {
                self.cache.store_expansion(key, &expanded);
            }
            expanded
        };
        let built = expanded.items().map(|mut items| {
            items.nest(&expandable.inline);
            Rc::new(self.build(visit, &items))
        });
        self.expansions.insert(key, built.clone());
        built
    }

    /// The public items of the library whose root file is `root` in `snapshot`, rooted at
    /// `crate_name`, of the package in directory `package`.
    pub(crate) fn library(
        &mut self,
        snapshot: &dyn Snapshot,
        package: &str,
        root: &str,
        crate_name: &str,
    ) -> Result<Library, Error> {
        let crate_sym = self.interner.sym(crate_name);
        let module = self.interner.path(&[crate_sym], ItemKind::Module);
        let mut index = Index::default();
        index.paths.insert(module);
        let visit = Rc::new(Visit {
            file: root.to_owned(),
            mod_rs: true,
            module,
            package: Rc::from(package),
            public_only: true,
        });
        let reached = self.reach(snapshot, vec![visit], false, &HashSet::new(), &mut index)?;
        Ok(Library {
            macro_idents: macro_idents(&mut self.interner, &reached)
                .into_values()
                .flatten()
                .collect(),
            macro_rules: reached
                .contributions()
                .flat_map(|c| &c.macro_rules)
                .filter(|rules| rules.exported)
                .map(|rules| {
                    let definition = Definition {
                        body: rules.body.clone(),
                        home: Some(crate_sym),
                    };
                    (rules.name, definition)
                })
                .collect(),
            unexpanded: reached
                .unexpanded()
                .filter(|call| call.expandable.is_some())
                .map(|call| call.name)
                .collect(),
            macro_crates: reached
                .unexpanded()
                .filter(|call| call.expandable.is_some())
                .filter_map(|call| call.crate_root)
                .collect(),
            paths: index.paths,
            hidden: reached
                .contributions()
                .flat_map(|c| c.hidden.iter().copied())
                .collect(),
            exposed: reached
                .contributions()
                .flat_map(|c| c.exposed.iter().copied())
                .collect(),
            uses: reached
                .contributions()
                .flat_map(|c| c.uses.iter().cloned())
                .collect(),
            lends: reached
                .contributions()
                .flat_map(|c| c.lends.iter().cloned())
                .collect(),
            traits: reached
                .contributions()
                .flat_map(|c| c.traits.iter().cloned())
                .collect(),
            declared_traits: reached
                .contributions()
                .flat_map(|c| c.declared_traits.iter().copied())
                .collect(),
        })
    }

    /// Publicly reachable paths from `roots` after resolving library re-exports.
    pub(crate) fn public_paths(&mut self, libraries: &[Library], roots: &[Sym]) -> HashSet<PathId> {
        let mut paths: HashSet<PathId> = libraries
            .iter()
            .flat_map(|library| library.paths.iter().copied())
            .collect();
        let crates: Vec<Sym> = paths
            .iter()
            .map(|&id| self.interner.segments(id)[0])
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let uses: Vec<&UseDecl> = libraries.iter().flat_map(|l| &l.uses).collect();
        let lends: Vec<&LendDecl> = libraries.iter().flat_map(|l| &l.lends).collect();
        let traits = TraitScope::new(
            libraries
                .iter()
                .flat_map(|l| l.declared_traits.iter().copied()),
            libraries.iter().flat_map(|l| &l.traits),
        );
        let mut hidden = hidden_paths(
            libraries
                .iter()
                .map(|library| (library.hidden.as_slice(), library.exposed.as_slice())),
        );
        self.expose_traits(&paths, &mut hidden, &uses, &lends, &traits, &crates);
        // What lies below a private module goes even where a `pub use` shares the module's path.
        let private = hidden.clone();
        self.resolve_aliases(&mut paths, &mut hidden, &uses, &lends, &crates);
        paths.retain(|&id| {
            let segments = self.interner.segments(id);
            roots.contains(&segments[0]) && public_path(id, &self.interner, &private, &hidden)
        });
        paths
    }

    fn contribution(
        &mut self,
        snapshot: &dyn Snapshot,
        visit: &Rc<Visit>,
        id: ObjectId,
    ) -> Result<Rc<Contribution>, Error> {
        let key = (Rc::clone(visit), id);
        if let Some(contribution) = self.memo.get(&key) {
            return Ok(Rc::clone(contribution));
        }
        let parsed = self
            .cache
            .get_or_parse(id, || snapshot.read(&visit.file, id))?;
        let contribution = Rc::new(match &*parsed {
            Ok(items) => self.build(visit, items),
            Err(message) => Contribution {
                failure: Some(message.clone()),
                ..Contribution::default()
            },
        });
        self.memo.insert(key, Rc::clone(&contribution));
        Ok(contribution)
    }

    fn build(&mut self, visit: &Visit, items: &FileItems) -> Contribution {
        let module: Vec<Sym> = self.interner.segments(visit.module).to_vec();
        let counts = |public: bool| public || !visit.public_only;
        let mut out = Contribution::default();
        for decl in &items.items {
            let path = self.within(&module, &decl.path);
            let id = self.interner.path(&path, decl.kind);
            if decl.kind == ItemKind::Trait {
                out.declared_traits.push((id, decl.public));
            }
            if decl.public {
                out.exposed.push(id);
            } else {
                out.hidden.push(id);
            }
            if counts(decl.public) || decl.kind == ItemKind::Module {
                out.paths.push(id);
                out.lines.push(decl.line);
                out.signatures
                    .extend(decl.signature.as_deref().map(|s| (id, Rc::from(s))));
            }
        }
        let imports = self.imports(&module, items, &out.paths);
        self.implementations(&module, items, &imports, visit.public_only, &mut out);
        for rules in items.macro_rules.iter().filter(|rules| rules.exported) {
            let name = self.interner.sym(&rules.name);
            let id = self.interner.path(&[module[0], name], ItemKind::Macro);
            out.paths.push(id);
            out.lines.push(rules.line);
            out.exposed.push(id);
        }
        out.visits = self.visits(visit, &module, items);
        for u in items.uses.iter().filter(|u| counts(u.public)) {
            let name = u.name.as_deref().map(|n| self.interner.sym(n));
            let within = self.within(&module, &u.module);
            let source = imported(&imports, &within, self.syms(&u.source));
            out.uses.push((within, source, name, u.public));
            out.use_lines.push(u.line);
        }
        out.calls = self.calls(&module, items, &imports);
        out.macro_rules = self.rules(&module, items);
        out.lends = items
            .lends
            .iter()
            .map(|d| {
                let within = self.within(&module, &d.module);
                let owner = imported(&imports, &within, self.syms(&d.owner));
                let source = imported(&imports, &within, self.syms(&d.source));
                (within, owner, source)
            })
            .collect();
        out.locals = self.syms(&items.locals);
        out.digest = self.alias_digest(&out);
        out
    }

    /// The digest of `contribution`'s alias inputs, each field framed by a tag and a length.
    fn alias_digest(&self, contribution: &Contribution) -> [u64; 2] {
        let mut digest = Digest::new(&self.digest_seeds);
        digest.list(1, &contribution.paths);
        digest.list(2, &contribution.hidden);
        digest.list(3, &contribution.exposed);
        for (module, source, name, public) in &contribution.uses {
            digest.word(4);
            digest.list(5, module);
            digest.list(6, source);
            digest.word(u32::from(name.is_some()));
            digest.word(name.unwrap_or(0));
            digest.word(u32::from(*public));
        }
        for (module, owner, source) in &contribution.lends {
            digest.word(7);
            digest.list(8, module);
            digest.list(9, owner);
            digest.list(10, source);
        }
        for &(id, public) in &contribution.declared_traits {
            digest.word(11);
            digest.word(id);
            digest.word(u32::from(public));
        }
        for implementation in &contribution.traits {
            digest.word(12);
            digest.list(13, &implementation.module);
            digest.list(14, &implementation.source);
            digest.list(15, &implementation.members);
        }
        digest.finish()
    }

    fn implementations(
        &mut self,
        module: &[Sym],
        items: &FileItems,
        imports: &[Import],
        public_only: bool,
        out: &mut Contribution,
    ) {
        for block in &items.impls {
            let within = self.within(module, &block.module);
            let self_ty = imported(imports, &within, self.syms(&block.self_ty));
            let Some(mut base) = self.resolve(&within, &self_ty, module[0]) else {
                continue;
            };
            let source = block
                .trait_path
                .as_ref()
                .map(|path| imported(imports, &within, self.syms(path)));
            let mut members = Vec::new();
            for (name, kind, public, signature, line) in &block.items {
                if *public || !public_only {
                    base.push(self.interner.sym(name));
                    let id = self.interner.path(&base, *kind);
                    out.paths.push(id);
                    out.lines.push(*line);
                    out.signatures
                        .extend(signature.as_deref().map(|s| (id, Rc::from(s))));
                    if *public && source.is_none() {
                        out.exposed.push(id);
                    } else {
                        out.hidden.push(id);
                    }
                    if source.is_some() {
                        members.push(id);
                    }
                    base.pop();
                }
            }
            if let Some(source) = source {
                out.traits.push(Implemented {
                    module: within,
                    source,
                    members,
                });
            }
        }
    }

    /// The macro invocations of `items`, read as the module `module` with its `imports`.
    fn calls(&mut self, module: &[Sym], items: &FileItems, imports: &[Import]) -> Vec<Call> {
        items
            .invocations
            .iter()
            .map(|inv| {
                let within = self.within(module, &inv.module);
                let path = imported(imports, &within, self.syms(&inv.path));
                let crate_root = (path.len() > 1).then(|| path[0]).filter(|&root| {
                    !matches!(
                        self.interner.name(root),
                        "crate" | "self" | "super" | "$crate"
                    )
                });
                Call {
                    module: self.interner.path(&within, ItemKind::Module),
                    name: self.interner.sym(&inv.name),
                    crate_root,
                    idents: self.syms(&inv.idents),
                    expandable: inv.tokens.as_deref().map(|tokens| Expandable {
                        inline: Rc::from(inv.module.as_slice()),
                        tokens: Rc::from(tokens),
                        impl_header: inv.impl_header.as_deref().map(Rc::from),
                    }),
                }
            })
            .collect()
    }

    /// The `macro_rules!` definitions of `items`, read as the module `module`.
    fn rules(&mut self, module: &[Sym], items: &FileItems) -> Vec<Rules> {
        items
            .macro_rules
            .iter()
            .map(|rules| {
                let within = self.within(module, &rules.module);
                Rules {
                    module: self.interner.path(&within, ItemKind::Module),
                    name: self.interner.sym(&rules.name),
                    idents: self.syms(&rules.idents),
                    body: Rc::from(rules.body.as_str()),
                    exported: rules.exported,
                }
            })
            .collect()
    }

    /// The files `mod` declarations and `include!` invocations of `items` pull into the crate.
    fn visits(&mut self, visit: &Visit, module: &[Sym], items: &FileItems) -> Vec<Rc<Visit>> {
        let mut visits = Vec::new();
        for decl in &items.mods {
            let mut child = self.within(module, &decl.module);
            child.push(self.interner.sym(&decl.name));
            let child = self.interner.path(&child, ItemKind::Module);
            let base = owned_dir(&visit.file, visit.mod_rs, &decl.module);
            for path in decl.paths.iter().chain(&decl.conditional_paths) {
                let file = if decl.module.is_empty() {
                    join(parent(&visit.file), path)
                } else {
                    join(&base, path)
                };
                visits.push(Rc::new(Visit {
                    file,
                    mod_rs: true,
                    module: child,
                    package: visit.package.clone(),
                    public_only: visit.public_only,
                }));
            }
            if decl.paths.is_empty() {
                for (file, mod_rs) in [
                    (join(&base, &format!("{}.rs", decl.name)), false),
                    (join(&base, &format!("{}/mod.rs", decl.name)), true),
                ] {
                    visits.push(Rc::new(Visit {
                        file,
                        mod_rs,
                        module: child,
                        package: visit.package.clone(),
                        public_only: visit.public_only,
                    }));
                }
            }
        }
        for include in &items.includes {
            let within = self.within(module, &include.module);
            let base = match include.base {
                IncludeBase::File => parent(&visit.file),
                IncludeBase::Package => &visit.package,
            };
            // As in rustc, an included file's `mod`s resolve against its own directory.
            visits.push(Rc::new(Visit {
                file: join(base, &include.path),
                mod_rs: true,
                module: self.interner.path(&within, ItemKind::Module),
                package: visit.package.clone(),
                public_only: visit.public_only,
            }));
        }
        visits
    }

    fn imports(&mut self, module: &[Sym], items: &FileItems, paths: &[PathId]) -> Vec<Import> {
        let declared: HashSet<PathId> = paths.iter().copied().collect();
        items
            .uses
            .iter()
            .chain(&items.imports)
            .filter_map(|u| {
                let name = self.interner.sym(u.name.as_deref()?);
                let mut within = self.within(module, &u.module);
                within.push(name);
                if self
                    .interner
                    .find_path(&within)
                    .is_some_and(|id| declared.contains(&id))
                {
                    return None;
                }
                within.pop();
                Some((within, self.syms(&u.source), name))
            })
            .collect()
    }

    fn syms(&mut self, names: &[String]) -> Vec<Sym> {
        names.iter().map(|name| self.interner.sym(name)).collect()
    }

    fn within(&mut self, module: &[Sym], inline: &[String]) -> Vec<Sym> {
        let mut path = module.to_vec();
        path.extend(inline.iter().map(|s| self.interner.sym(s)));
        path
    }

    /// `segments` as written inside `module`, resolved syntactically.
    fn resolve(&self, module: &[Sym], segments: &[Sym], crate_sym: Sym) -> Option<Vec<Sym>> {
        let Keywords {
            krate,
            self_value,
            self_type,
            super_module,
        } = self.keywords;
        let first = *segments.first()?;
        if first == self_type {
            return None;
        }
        if first == krate {
            let mut path = vec![crate_sym];
            path.extend_from_slice(&segments[1..]);
            return Some(path);
        }
        let mut base = module.to_vec();
        let mut rest = if first == self_value {
            &segments[1..]
        } else {
            segments
        };
        while rest.first() == Some(&super_module) {
            if base.len() <= 1 {
                return None;
            }
            base.pop();
            rest = &rest[1..];
        }
        base.extend_from_slice(rest);
        Some(base)
    }

    fn expose_traits(
        &mut self,
        paths: &HashSet<PathId>,
        hidden: &mut HashSet<PathId>,
        uses: &[&UseDecl],
        lends: &[&LendDecl],
        traits: &TraitScope<'_>,
        crates: &[Sym],
    ) {
        if traits.implementations.is_empty() {
            return;
        }
        if traits.declarations.is_empty() {
            for implementation in &traits.implementations {
                for id in &implementation.members {
                    hidden.remove(id);
                }
            }
            return;
        }
        let mut resolved = paths.clone();
        let mut visibility = hidden.clone();
        let (_, origins) =
            self.resolve_aliases(&mut resolved, &mut visibility, uses, lends, crates);
        let visible: HashSet<PathId> = resolved
            .iter()
            .copied()
            .filter_map(|id| {
                let stands_for = origin(&origins, id);
                (traits.declarations.get(&stands_for) == Some(&true)
                    && public_path(id, &self.interner, hidden, &visibility))
                .then_some(stands_for)
            })
            .collect();
        for implementation in &traits.implementations {
            let source = self
                .resolve_use(
                    &implementation.module,
                    &implementation.source,
                    crates,
                    &resolved,
                )
                .map(|(source, _)| source)
                .or_else(|| {
                    self.resolve(
                        &implementation.module,
                        &implementation.source,
                        implementation.module[0],
                    )
                });
            let known = source
                .and_then(|source| self.interner.find_path(&source))
                .map(|id| origin(&origins, id))
                .filter(|id| traits.declarations.contains_key(id));
            if known.is_none_or(|id| visible.contains(&id)) {
                for id in &implementation.members {
                    hidden.remove(id);
                }
            }
        }
    }

    /// Adds re-export paths and, for sources inside the workspace, everything below them, then
    /// the members a `Deref` target or a supertrait lends. Paths only a non-`pub` re-export or a
    /// copy of a hidden path creates join `hidden`. Returns every re-export with its source.
    fn resolve_aliases(
        &mut self,
        paths: &mut HashSet<PathId>,
        hidden: &mut HashSet<PathId>,
        uses: &[&UseDecl],
        lends: &[&LendDecl],
        crates: &[Sym],
    ) -> (Reexports, HashMap<PathId, PathId>) {
        const ROUNDS: usize = 8;
        let mut origins = HashMap::new();
        let mut tree = self.tree(paths);
        let mut use_sites: Vec<UseSite> = uses.iter().map(|_| UseSite::default()).collect();
        let mut lend_sites: Vec<LendSite> = lends
            .iter()
            .map(|(module, owner, source)| LendSite {
                owner: self.resolve(module, owner, module[0]),
                source: self.resolve(module, source, module[0]),
                target: None,
                owner_known: false,
                made: None,
            })
            .collect();
        let mut public_aliases = HashSet::new();
        // One path stands for every namespace, so a public re-export of a name keeps it public.
        for ((module, _, name, public), site) in uses.iter().zip(&mut use_sites) {
            if *public && let Some(name) = name {
                let (id, _) = named(&mut site.alias, &mut self.interner, module, *name);
                hidden.remove(&id);
                public_aliases.insert(id);
            }
        }
        for _ in 0..ROUNDS {
            let before = paths.len();
            for ((module, source, name, public), site) in uses.iter().zip(&mut use_sites) {
                let UseSite {
                    alias,
                    target: settled,
                    made,
                } = site;
                let target = settled.or_else(|| {
                    let (resolved, fixed) = self.resolve_use(module, source, crates, paths)?;
                    let target = self
                        .interner
                        .find_path(&resolved)
                        .filter(|id| paths.contains(id));
                    if fixed {
                        *settled = target;
                    }
                    target
                });
                if let Some(name) = name {
                    let (id, alias) = named(alias, &mut self.interner, module, *name);
                    let hide = !public && !public_aliases.contains(&id);
                    let added = paths.insert(id);
                    if added {
                        tree.pending.push(id);
                        if hide {
                            hidden.insert(id);
                        }
                    }
                    if let Some(target) = target {
                        let stands_for = origin(&origins, target);
                        let interner = &self.interner;
                        tree.changes
                            .stand_for(interner, &mut origins, id, stands_for, added);
                        if !self.self_named_copy(target, alias) {
                            let copy = (target, alias, hide);
                            self.copy_below(copy, made, &mut tree, paths, &mut origins, hidden);
                        }
                    }
                } else if let Some(target) = target
                    && !self.self_named_copy(target, module.as_slice())
                {
                    let copy = (target, module.as_slice(), !public);
                    self.copy_below(copy, made, &mut tree, paths, &mut origins, hidden);
                }
            }
            self.copy_lent(&mut lend_sites, &mut tree, paths, &mut origins, hidden);
            if paths.len() == before {
                break;
            }
            self.grow(&mut tree);
        }
        // A source among the paths reaches mentions only through the copies above, unless it is
        // itself a re-export of an outside source, whose members no copy carries.
        let mut reexports = Reexports::new();
        let mut inside = Vec::new();
        for ((module, source, name, _), site) in uses.iter().zip(&use_sites) {
            let target = site.target.or_else(|| {
                self.resolve_use(module, source, crates, paths)
                    .and_then(|(resolved, _)| self.interner.find_path(&resolved))
                    .filter(|id| paths.contains(id))
            });
            let mut alias = module.clone();
            alias.extend(*name);
            match target {
                Some(target) if name.is_some() => inside.push((alias, source, target)),
                Some(_) => {}
                None => reexports.entry(alias).or_default().push(source.clone()),
            }
        }
        while let Some(at) = inside
            .iter()
            .position(|(_, _, target)| reexports.contains_key(self.interner.segments(*target)))
        {
            let (alias, source, _) = inside.swap_remove(at);
            reexports.entry(alias).or_default().push(source.clone());
        }
        (reexports, origins)
    }

    /// Copies below each lend's owner the members its source has, once both are among `paths`.
    fn copy_lent(
        &mut self,
        sites: &mut [LendSite],
        tree: &mut Tree,
        paths: &mut HashSet<PathId>,
        origins: &mut HashMap<PathId, PathId>,
        hidden: &mut HashSet<PathId>,
    ) {
        for site in sites {
            let LendSite {
                owner: Some(owner),
                source,
                target,
                owner_known,
                made,
            } = site
            else {
                continue;
            };
            *target = target.or_else(|| {
                let found = self.interner.find_path(source.as_deref()?)?;
                paths.contains(&found).then_some(found)
            });
            *owner_known = *owner_known
                || self
                    .interner
                    .find_path(owner)
                    .is_some_and(|id| paths.contains(&id));
            if let Some(target) = *target
                && *owner_known
            {
                let copy = (target, owner.as_slice(), false);
                self.copy_below(copy, made, tree, paths, origins, hidden);
            }
        }
    }

    /// [`Self::resolve_aliases`] for a snapshot other than head, reusing the result of a recent
    /// snapshot whose inputs were identical. A digest match is only a candidate, verified
    /// field by field, so a collision costs a comparison, never a wrong result. A resolution
    /// reads nothing but its inputs and the paths it interns, which stay interned, so the
    /// reused result is the one it would compute.
    fn recalled_aliases(
        &mut self,
        paths: &mut HashSet<PathId>,
        inputs: &AliasInputs<'_>,
    ) -> Reexports {
        let key = alias_key(inputs);
        let hash = self.alias_hasher.hash_one(&key);
        let found = self
            .aliases
            .iter()
            .position(|entry| reusable(entry, hash, &key, inputs));
        if let Some(entry) = found.and_then(|at| self.aliases.remove(at)) {
            paths.extend(&entry.added);
            let reexports = entry.reexports.clone();
            self.aliases.push_front(entry);
            return reexports;
        }
        let mut hidden = hidden_paths(inputs.entries.iter().map(|entry| {
            (
                entry.contribution.hidden.as_slice(),
                entry.contribution.exposed.as_slice(),
            )
        }));
        let mut before: Vec<PathId> = paths.iter().copied().collect();
        before.sort_unstable();
        self.expose_traits(
            paths,
            &mut hidden,
            inputs.uses,
            inputs.lends,
            inputs.traits,
            inputs.crates,
        );
        let reexports = self
            .resolve_aliases(paths, &mut hidden, inputs.uses, inputs.lends, inputs.crates)
            .0;
        let added = paths
            .iter()
            .copied()
            .filter(|id| before.binary_search(id).is_err())
            .collect();
        self.aliases.truncate(RECALLED_ALIASES.saturating_sub(1));
        self.aliases.push_front(AliasEntry {
            hash,
            key,
            contributions: inputs
                .entries
                .iter()
                .map(|entry| Rc::clone(&entry.contribution))
                .collect(),
            added,
            reexports: reexports.clone(),
        });
        reexports
    }

    /// The path `source` names when used inside `module`, and whether no path joining `paths`
    /// changes it.
    fn resolve_use(
        &self,
        module: &[Sym],
        source: &[Sym],
        crates: &[Sym],
        paths: &HashSet<PathId>,
    ) -> Option<(Vec<Sym>, bool)> {
        let first = *source.first()?;
        let Keywords {
            krate,
            self_value,
            super_module,
            ..
        } = self.keywords;
        if [krate, self_value, super_module].contains(&first) {
            return self
                .resolve(module, source, module[0])
                .map(|path| (path, true));
        }
        let mut local = module.to_vec();
        local.push(first);
        if self
            .interner
            .find_path(&local)
            .is_some_and(|id| paths.contains(&id))
        {
            local.extend_from_slice(&source[1..]);
            return Some((local, true));
        }
        crates.contains(&first).then(|| (source.to_vec(), false))
    }

    /// The interned prefixes of `to`, longest first, as links in `self.copy_links` with the
    /// origin each stands for, plus their ids and the id of `to` itself when interned.
    fn prefix_chain(
        &mut self,
        to: &[Sym],
        origins: &HashMap<PathId, PathId>,
    ) -> (Option<PathId>, usize, Vec<PathId>) {
        let mut prefixes = Vec::new();
        let mut whole = None;
        self.copy_links.clear();
        for (len, id) in self.interner.prefixes(to) {
            if whole.is_none() {
                whole = (len == to.len()).then_some(id);
            }
            prefixes.push(id);
            let before = self.copy_links.len().checked_sub(1).unwrap_or(NO_LINK);
            self.copy_links.push((origin(origins, id), before));
        }
        (
            whole,
            self.copy_links.len().checked_sub(1).unwrap_or(NO_LINK),
            prefixes,
        )
    }

    /// Whether the copy of `from` at `to` would stand a crate's own root below the crate's
    /// own name, where the root already stands, adding no spelling a reader writes.
    fn self_named_copy(&self, from: PathId, to: &[Sym]) -> bool {
        let segments = self.interner.segments(from);
        to.len() == 2 && to[0] == to[1] && segments.len() == 1 && segments[0] == to[0]
    }

    /// Re-roots every path strictly below `from` under `to`, never through a module the copy
    /// already passes, so that re-exports forming a cycle stay finite.
    ///
    /// `origins` maps every alias and copied path to the declared path it stands for, and the
    /// paths the copy adds join `tree.pending`. A copy into another crate skips the `hidden`
    /// paths with everything below them, and a new copy of a hidden path, or any new copy when
    /// `hide` is set, joins them.
    ///
    /// `last` is the copy the same use or lend made before, whose destination and hiding are
    /// this copy's, as both follow from the declaration alone. Made again from the same source
    /// with the same chain of origins above `to`, the copy skips every path at and below which
    /// nothing changed since, as the copy made then already holds everything below it. The
    /// stored chain is reused without re-walking the prefixes while no path was interned and
    /// every prefix still stands for the origin it held when the copy was made.
    fn copy_below(
        &mut self,
        (from, to, hide): (PathId, &[Sym], bool),
        last: &mut Option<Made>,
        tree: &mut Tree,
        paths: &mut HashSet<PathId>,
        origins: &mut HashMap<PathId, PathId>,
        hidden: &mut HashSet<PathId>,
    ) {
        let Tree {
            children,
            pending,
            changes,
            ..
        } = tree;
        changes.clock += 1;
        let across = self.interner.segments(from)[0] != to[0];
        // The origins on each copy's chain, as links to the link before them.
        let (whole, tip, since) = match last {
            Some(made)
                if made.from == from
                    && made.count == self.interner.path_count()
                    && made
                        .prefixes
                        .iter()
                        .zip(&made.links)
                        .all(|(p, link)| origin(origins, *p) == link.0) =>
            {
                let since = std::mem::replace(&mut made.tick, changes.clock);
                self.copy_links.clear();
                self.copy_links.extend_from_slice(&made.links);
                (
                    made.whole,
                    made.links.len().checked_sub(1).unwrap_or(NO_LINK),
                    Some(since),
                )
            }
            _ => {
                let (whole, tip, prefixes) = self.prefix_chain(to, origins);
                let since = match last {
                    Some(made) if made.from == from && made.links == self.copy_links[..] => {
                        made.prefixes = prefixes;
                        made.count = self.interner.path_count();
                        Some(std::mem::replace(&mut made.tick, changes.clock))
                    }
                    _ => {
                        *last = Some(Made {
                            from,
                            links: self.copy_links[..].to_vec(),
                            prefixes,
                            whole,
                            count: self.interner.path_count(),
                            tick: changes.clock,
                        });
                        None
                    }
                };
                (whole, tip, since)
            }
        };
        // Each entry is a source, the copy it lands at (`None` for `to` when never interned),
        // the tip of its chain and, while the chain is the one of the last copy, its tick.
        let mut stack = vec![(from, whole, tip, since)];
        while let Some((source, prefix, tip, since)) = stack.pop() {
            for &child in children.get(&source).into_iter().flatten() {
                if since.is_some_and(|tick| changes.below(child) < tick)
                    || across && hidden.contains(&child)
                {
                    continue;
                }
                let last = *self
                    .interner
                    .segments(child)
                    .last()
                    .expect("children are non-empty paths");
                let kind = self.interner.kind(child);
                let id = if let Some(parent) = prefix {
                    self.interner.child(parent, last, kind)
                } else {
                    let path: Vec<Sym> = to.iter().copied().chain([last]).collect();
                    self.interner.path(&path, kind)
                };
                let added = paths.insert(id);
                if added {
                    pending.push(id);
                    if hide || hidden.contains(&child) {
                        hidden.insert(id);
                    }
                }
                let child_origin = origin(origins, child);
                changes.stand_for(&self.interner, origins, id, child_origin, added);
                if !on_chain(&self.copy_links, tip, child_origin) {
                    self.copy_links.push((child_origin, tip));
                    let since = since.filter(|&tick| changes.renewed(child) < tick);
                    stack.push((child, Some(id), self.copy_links.len() - 1, since));
                }
            }
        }
    }

    /// The parent and child links among `paths`.
    fn tree(&self, paths: &HashSet<PathId>) -> Tree {
        let mut tree = Tree::default();
        for &id in paths {
            self.link(&mut tree, id);
        }
        for list in tree.children.values_mut() {
            list.sort_unstable();
        }
        tree
    }

    /// Adds the pending paths to `tree`, adopting the paths whose parent one of them is and
    /// was not interned before.
    fn grow(&self, tree: &mut Tree) {
        let mut touched = HashSet::new();
        let mut pending = std::mem::take(&mut tree.pending);
        for &id in &pending {
            if let Some(parent) = self.link(tree, id) {
                touched.insert(parent);
            }
            if let Some(adopted) = tree.orphans.remove(self.interner.segments(id)) {
                tree.children.entry(id).or_default().extend(adopted);
                touched.insert(id);
            }
            tree.changes.renew(&self.interner, id);
        }
        for parent in touched {
            if let Some(list) = tree.children.get_mut(&parent) {
                list.sort_unstable();
            }
        }
        pending.clear();
        tree.pending = pending;
    }

    /// Files `id` under its parent in `tree`, or among the orphans while the parent is not
    /// interned. A parent outside the set keeps its children unused until it joins. Returns
    /// the parent it was filed under.
    fn link(&self, tree: &mut Tree, id: PathId) -> Option<PathId> {
        if let Some(found) = self.interner.parent(id) {
            tree.children.entry(found).or_default().push(id);
            return Some(found);
        }
        let segments = self.interner.segments(id);
        let (_, parent) = segments.split_last()?;
        if !parent.is_empty() {
            tree.orphans.entry(parent.into()).or_default().push(id);
        }
        None
    }
}

/// A forest of unique path-parent edges.
#[derive(Default)]
struct Tree {
    /// The children of every path, sorted.
    children: HashMap<PathId, Vec<PathId>>,
    /// The paths whose parent was not interned yet, by the parent's segments.
    orphans: HashMap<Box<[Sym]>, Vec<PathId>>,
    /// The paths added since the tree last grew.
    pending: Vec<PathId>,
    changes: Changes,
}

/// When the paths of a [`Tree`] last changed, on a clock that ticks once per copy.
#[derive(Default)]
struct Changes {
    clock: u32,
    /// The last tick each path joined the tree or took an origin, and the last tick that
    /// happened to it or to a path below it.
    ticks: HashMap<PathId, (u32, u32)>,
}

impl Changes {
    /// The last tick `id` joined the tree or took an origin.
    fn renewed(&self, id: PathId) -> u32 {
        self.ticks.get(&id).map_or(0, |&(renewed, _)| renewed)
    }

    /// The last tick `id` or a path below it was renewed.
    fn below(&self, id: PathId) -> u32 {
        self.ticks.get(&id).map_or(0, |&(_, below)| below)
    }

    /// Marks `id` renewed now, and changed below every path above it.
    fn renew(&mut self, interner: &Interner, id: PathId) {
        let tick = self.clock;
        let ticks = self.ticks.entry(id).or_default();
        *ticks = (tick, tick);
        for (_, above) in interner.ancestors(id) {
            let below = &mut self.ticks.entry(above).or_default().1;
            if *below == tick {
                break;
            }
            *below = tick;
        }
    }

    /// Records that `id` stands for `declared` unless it already stands for a path. A path
    /// that was in the tree before is renewed, as copies passing it may now stop elsewhere.
    fn stand_for(
        &mut self,
        interner: &Interner,
        origins: &mut HashMap<PathId, PathId>,
        id: PathId,
        declared: PathId,
        added: bool,
    ) {
        if let hashbrown::hash_map::Entry::Vacant(slot) = origins.entry(id) {
            slot.insert(declared);
            if !added {
                self.renew(interner, id);
            }
        }
    }
}

/// What a use settled on in one resolution, each part kept once no later round can change it.
#[derive(Default)]
struct UseSite {
    /// The alias a named use declares, with its id.
    alias: Option<(PathId, Box<[Sym]>)>,
    /// The source among the paths.
    target: Option<PathId>,
    made: Option<Made>,
}

/// What a lend settled on in one resolution, each part kept once no later round can change it.
struct LendSite {
    /// The owner, resolved syntactically.
    owner: Option<Vec<Sym>>,
    /// The source, resolved syntactically.
    source: Option<Vec<Sym>>,
    /// The source among the paths.
    target: Option<PathId>,
    /// Whether the owner is among the paths.
    owner_known: bool,
    made: Option<Made>,
}

/// The alias `name` a use declares inside `module`, interned when `slot` first asks for it.
fn named<'a>(
    slot: &'a mut Option<(PathId, Box<[Sym]>)>,
    interner: &mut Interner,
    module: &[Sym],
    name: Sym,
) -> (PathId, &'a [Sym]) {
    let (id, alias) = slot.get_or_insert_with(|| {
        let alias: Box<[Sym]> = module.iter().copied().chain([name]).collect();
        (interner.path(&alias, ItemKind::Reexport), alias)
    });
    (*id, alias)
}

/// The copy a use or lend last made, with the tick it was made at.
struct Made {
    from: PathId,
    /// The chain of origins above the destination, longest prefix first, as links to the link
    /// before them.
    links: Vec<(PathId, usize)>,
    /// The ids of the interned prefixes of the destination, longest first, whose origins are
    /// checked against `links` before the chain is reused.
    prefixes: Vec<PathId>,
    /// The id of the destination itself when interned.
    whole: Option<PathId>,
    /// The number of interned paths when the copy was made.
    count: usize,
    tick: u32,
}

/// The number of historical alias inputs retained.
const RECALLED_ALIASES: usize = 64;

/// One alias resolution with the exact inputs it ran on.
struct AliasEntry {
    hash: u64,
    key: Vec<u64>,
    /// The contributions the resolution ran on, in reach order.
    contributions: Vec<Rc<Contribution>>,
    /// The paths the resolution added.
    added: Vec<PathId>,
    reexports: Reexports,
}

/// The inputs of one alias resolution.
struct AliasInputs<'a> {
    /// The paths the roots interned.
    roots: &'a [PathId],
    /// The contributions in reach order.
    entries: &'a [Entry],
    uses: &'a [&'a UseDecl],
    lends: &'a [&'a LendDecl],
    traits: &'a TraitScope<'a>,
    crates: &'a [Sym],
}

/// The key of a resolution's inputs: the crate list, the roots' paths sorted and the
/// contributions' digests in reach order.
fn alias_key(inputs: &AliasInputs<'_>) -> Vec<u64> {
    let mut key = Vec::new();
    key.push(u64::try_from(inputs.crates.len()).expect("fewer than 2^64 crates"));
    key.extend(inputs.crates.iter().copied().map(u64::from));
    let mut sorted: Vec<u64> = inputs.roots.iter().copied().map(u64::from).collect();
    sorted.sort_unstable();
    key.push(u64::try_from(sorted.len()).expect("fewer than 2^64 roots"));
    key.extend(sorted);
    key.push(u64::try_from(inputs.entries.len()).expect("fewer than 2^64 entries"));
    for entry in inputs.entries {
        key.extend(entry.contribution.digest);
    }
    key
}

/// Whether `entry` is the resolution of `inputs`, whose result it may return.
fn reusable(entry: &AliasEntry, hash: u64, key: &[u64], inputs: &AliasInputs<'_>) -> bool {
    entry.hash == hash
        && entry.key == *key
        && entry.contributions.len() == inputs.entries.len()
        && entry
            .contributions
            .iter()
            .zip(inputs.entries.iter().map(|e| &e.contribution))
            .all(|(stored, fresh)| stored.alias_inputs_equal(fresh))
}

/// One contribution alias digest, its two 64-bit halves written together.
struct Digest {
    halves: [DefaultHasher; 2],
}

impl Digest {
    fn new(seeds: &[DefaultHashBuilder; 2]) -> Self {
        Self {
            halves: [seeds[0].build_hasher(), seeds[1].build_hasher()],
        }
    }

    /// One word to both halves.
    fn word(&mut self, value: u32) {
        for half in &mut self.halves {
            half.write_u32(value);
        }
    }

    /// A length-prefixed list under `tag`.
    fn list(&mut self, tag: u32, items: &[u32]) {
        self.word(tag);
        self.word(u32::try_from(items.len()).expect("fewer than 2^32 items"));
        for &value in items {
            self.word(value);
        }
    }

    /// The finished halves.
    fn finish(self) -> [u64; 2] {
        [self.halves[0].finish(), self.halves[1].finish()]
    }
}

/// The key an expansion's items are stored under, the SHA-1 of its macro body, home crate name,
/// invocation tokens and impl header, each marked present or absent and prefixed by its length.
/// `None` when the hasher detects a collision attack.
fn expansion_key(parts: [Option<&str>; 4]) -> Option<ObjectId> {
    let mut hasher = gix::hash::hasher(gix::hash::Kind::Sha1);
    for part in parts {
        match part {
            Some(text) => {
                let len = u64::try_from(text.len()).expect("lengths fit in u64");
                hasher.update(&[1]);
                hasher.update(&len.to_le_bytes());
                hasher.update(text.as_bytes());
            }
            None => hasher.update(&[0]),
        }
    }
    hasher.try_finalize().ok()
}

/// The link before the first one of a chain in [`Indexer::copy_below`].
const NO_LINK: usize = usize::MAX;

/// Whether `origin` is on the chain of `links` ending at `tip`.
fn on_chain(links: &[(PathId, usize)], mut tip: usize, origin: PathId) -> bool {
    while let Some(&(on, before)) = links.get(tip) {
        if on == origin {
            return true;
        }
        tip = before;
    }
    false
}

/// The declared path `id` stands for, given the alias and copy `origins`.
fn origin(origins: &HashMap<PathId, PathId>, id: PathId) -> PathId {
    origins.get(&id).copied().unwrap_or(id)
}

/// Paths hidden by declarations and exposed by none.
fn hidden_paths<'a>(lists: impl Iterator<Item = (&'a [PathId], &'a [PathId])>) -> HashSet<PathId> {
    let mut hidden = HashSet::new();
    let mut exposed = HashSet::new();
    for (hides, exposes) in lists {
        hidden.extend(hides.iter().copied());
        exposed.extend(exposes.iter().copied());
    }
    hidden.retain(|id| !exposed.contains(id));
    hidden
}

/// Resolves a written first segment through the named imports of its module.
fn imported(imports: &[Import], module: &[Sym], mut path: Vec<Sym>) -> Vec<Sym> {
    for _ in 0..imports.len() {
        let Some((_, source, _)) = imports.iter().find(|(within, source, name)| {
            within == module && path.first() == Some(name) && source.first() != Some(name)
        }) else {
            break;
        };
        path.splice(..1, source.iter().copied());
    }
    path
}

/// The directory owned by `file` for `mod` declarations nested in the inline modules `inline`.
fn owned_dir(file: &str, mod_rs: bool, inline: &[String]) -> String {
    let mut dir = parent(file).to_owned();
    if !mod_rs {
        let name = file.rsplit('/').next().unwrap_or(file);
        dir = join(&dir, name.strip_suffix(".rs").unwrap_or(name));
    }
    for segment in inline {
        dir = join(&dir, segment);
    }
    dir
}

fn parent(file: &str) -> &str {
    file.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// Adds the paths, signatures and files of `contribution` to `index` and `stack`.
fn absorb(contribution: &Contribution, index: &mut Index, stack: &mut Vec<Rc<Visit>>) {
    index.paths.extend(contribution.paths.iter().copied());
    index.signatures.extend(
        contribution
            .signatures
            .iter()
            .map(|(id, signature)| (*id, Rc::clone(signature))),
    );
    stack.extend(contribution.visits.iter().map(Rc::clone));
}

fn public_path(
    id: PathId,
    interner: &Interner,
    private: &HashSet<PathId>,
    hidden: &HashSet<PathId>,
) -> bool {
    !hidden.contains(&id)
        && !interner
            .ancestors(id)
            .take_while(|&(len, _)| len >= 2)
            .any(|(_, prefix)| private.contains(&prefix) || hidden.contains(&prefix))
}

#[cfg(test)]
mod tests {
    use super::{
        AliasEntry, AliasInputs, BuildHasher, Contribution, Entry, FileCache, Implemented, Indexer,
        LendDecl, PathId, Reexports, Sym, TraitScope, UseDecl, Visit, alias_key, reusable,
    };
    use std::rc::Rc;

    /// A contribution with every alias field non-empty.
    fn contrib(mutate: impl FnOnce(&mut Contribution)) -> Contribution {
        let mut contribution = Contribution {
            paths: vec![1, 2],
            hidden: vec![3],
            exposed: vec![4],
            uses: vec![(vec![5], vec![6], Some(7), true)],
            lends: vec![(vec![8], vec![9], vec![10])],
            declared_traits: vec![(11, true)],
            traits: vec![Implemented {
                module: vec![12],
                source: vec![13],
                members: vec![14],
            }],
            ..Contribution::default()
        };
        mutate(&mut contribution);
        contribution
    }

    /// One entry reading `contribution` at depth zero.
    fn entry(contribution: Contribution) -> Entry {
        Entry {
            visit: Rc::new(Visit {
                file: "lib.rs".into(),
                mod_rs: false,
                module: 0,
                package: Rc::from(""),
                public_only: false,
            }),
            contribution: Rc::new(contribution),
            depth: 0,
        }
    }

    /// Runs `f` on the alias inputs of `entries` under `crates` and `roots`.
    fn with_inputs(
        crates: &[Sym],
        roots: &[PathId],
        entries: &[Entry],
        f: impl FnOnce(&AliasInputs<'_>),
    ) {
        let uses: Vec<&UseDecl> = entries.iter().flat_map(|e| &e.contribution.uses).collect();
        let lends: Vec<&LendDecl> = entries.iter().flat_map(|e| &e.contribution.lends).collect();
        let traits = TraitScope::new(
            entries
                .iter()
                .flat_map(|e| e.contribution.declared_traits.iter().copied()),
            entries.iter().flat_map(|e| &e.contribution.traits),
        );
        f(&AliasInputs {
            roots,
            entries,
            uses: &uses,
            lends: &lends,
            traits: &traits,
            crates,
        });
    }

    /// The key of the contributions' digests under `crates` and `roots`.
    fn key_of(crates: [Sym; 2], roots: [PathId; 1], digests: &[(u64, u64)]) -> Vec<u64> {
        let entries: Vec<Entry> = digests
            .iter()
            .map(|&(half, low)| entry(contrib(|c| c.digest = [half, low])))
            .collect();
        let mut key = Vec::new();
        with_inputs(&crates, &roots, &entries, |inputs| {
            key = alias_key(inputs);
        });
        key
    }

    /// The one-field variants of [`contrib`].
    fn variants() -> Vec<Contribution> {
        let mutates: [fn(&mut Contribution); 14] = [
            |c| {
                c.paths.pop();
            },
            |c| {
                c.hidden.pop();
            },
            |c| {
                c.exposed.pop();
            },
            |c| c.uses[0].2 = None,
            |c| c.uses[0].2 = Some(99),
            |c| c.uses[0].3 = false,
            |c| {
                c.lends[0].0.pop();
            },
            |c| {
                c.lends[0].1.pop();
            },
            |c| {
                c.lends[0].2.pop();
            },
            |c| c.declared_traits[0].0 = 99,
            |c| c.declared_traits[0].1 = false,
            |c| {
                c.traits[0].module.pop();
            },
            |c| {
                c.traits[0].source.pop();
            },
            |c| {
                c.traits[0].members.pop();
            },
        ];
        mutates.into_iter().map(|mutate| contrib(mutate)).collect()
    }

    /// An indexer over an empty cache in a scratch directory.
    fn indexer() -> (tempfile::TempDir, Indexer) {
        let dir = tempfile::tempdir().unwrap();
        let indexer = Indexer::new(FileCache::load(dir.path()));
        (dir, indexer)
    }

    #[test]
    fn equal_contributions_carry_the_equal_alias_inputs() {
        assert!(contrib(|_| {}).alias_inputs_equal(&contrib(|_| {})));
        let base = contrib(|_| {});
        for variant in variants() {
            assert!(!base.alias_inputs_equal(&variant), "one field must matter");
        }
    }

    #[test]
    fn equal_contributions_give_the_equal_digest() {
        let (_dir, indexer) = indexer();
        assert_eq!(
            indexer.alias_digest(&contrib(|_| {})),
            indexer.alias_digest(&contrib(|_| {}))
        );
    }

    #[test]
    fn a_field_variant_gives_the_different_digest() {
        let (_dir, indexer) = indexer();
        let base = indexer.alias_digest(&contrib(|_| {}));
        for variant in variants() {
            assert_ne!(
                base,
                indexer.alias_digest(&variant),
                "one field must matter"
            );
        }
        let moved = contrib(|c| {
            c.paths.pop();
            c.hidden.insert(0, 2);
        });
        assert_ne!(
            base,
            indexer.alias_digest(&moved),
            "an item moved between neighbouring lists must matter"
        );
    }

    #[test]
    fn equal_inputs_give_the_equal_key() {
        assert_eq!(
            key_of([1, 2], [7], &[(3, 4)]),
            key_of([1, 2], [7], &[(3, 4)])
        );
    }

    #[test]
    fn a_changed_input_gives_the_different_key() {
        let base = key_of([1, 2], [7], &[(3, 4)]);
        assert_ne!(
            key_of([1, 3], [7], &[(3, 4)]),
            base,
            "the crate list must matter"
        );
        assert_ne!(
            key_of([1, 2], [8], &[(3, 4)]),
            base,
            "the roots must matter"
        );
        assert_ne!(
            key_of([1, 2], [7], &[(3, 5)]),
            base,
            "a contribution digest must matter"
        );
        assert_ne!(
            key_of([1, 2], [7], &[(3, 4), (9, 8)]),
            key_of([1, 2], [7], &[(9, 8), (3, 4)]),
            "the contribution order must matter"
        );
    }

    #[test]
    fn reusable_needs_the_same_hash_key_and_contributions() {
        let (_dir, indexer) = indexer();
        with_inputs(&[1], &[7], &[entry(contrib(|_| {}))], |inputs| {
            let key = alias_key(inputs);
            let hash = indexer.alias_hasher.hash_one(&key);
            let entry = AliasEntry {
                hash,
                key: key.clone(),
                contributions: inputs
                    .entries
                    .iter()
                    .map(|e| Rc::clone(&e.contribution))
                    .collect(),
                added: vec![],
                reexports: Reexports::new(),
            };
            assert!(reusable(&entry, hash, &key, inputs));
            assert!(
                !reusable(&entry, hash.wrapping_add(1), &key, inputs),
                "the hash must matter"
            );
            let other_key = {
                let mut other_key = key.clone();
                *other_key.last_mut().unwrap() += 1;
                other_key
            };
            assert!(
                !reusable(&entry, hash, &other_key, inputs),
                "the key must matter"
            );
            let more = AliasEntry {
                hash,
                key: key.clone(),
                contributions: vec![
                    Rc::clone(&entry.contributions[0]),
                    Rc::clone(&entry.contributions[0]),
                ],
                added: vec![],
                reexports: Reexports::new(),
            };
            assert!(
                !reusable(&more, hash, &key, inputs),
                "the contribution count must matter"
            );
            let drifted = AliasEntry {
                hash,
                key: key.clone(),
                contributions: vec![Rc::new(contrib(|c| {
                    c.paths.pop();
                }))],
                added: vec![],
                reexports: Reexports::new(),
            };
            assert!(
                !reusable(&drifted, hash, &key, inputs),
                "a contribution field must matter"
            );
        });
    }
}
