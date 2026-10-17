//! Every item path and manifest entry a snapshot declares, assembled from its crates' module
//! trees and manifests.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use gix::ObjectId;

use crate::cache::FileCache;
use crate::git::{ManifestDirs, Snapshot};
use crate::intern::{Interner, PathId, Sym};
use crate::manifest::{self, Member, TargetKind, join};
use crate::parse::{self, FileItems};
use crate::{Error, ItemKind, entry, expand};

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
    /// The parameter names of every function, method and trait method.
    pub(crate) params: HashSet<Sym>,
    /// The signature of every function, method and trait method, as the parser records it.
    pub(crate) signatures: HashMap<PathId, Rc<str>>,
    /// The dependency tables of every dependency entry, as [`manifest::NORMAL`] bits.
    pub(crate) tables: HashMap<PathId, u8>,
    /// The item-position macros head invokes without expanding them, by name.
    pub(crate) unexpanded: HashSet<Sym>,
    /// Every re-export with the source it stands for.
    pub(crate) reexports: Reexports,
}

/// A `use` creating a path: the module it sits in, the imported path, the created name or
/// `None` for a glob, and whether it is `pub`.
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
struct Contribution {
    paths: Vec<PathId>,
    /// Paths declared without `pub`, among them the private modules kept when only public
    /// items count.
    hidden: Vec<PathId>,
    /// Paths declared with `pub`, which no hidden declaration of the same path elsewhere hides.
    exposed: Vec<PathId>,
    visits: Vec<Rc<Visit>>,
    uses: Vec<UseDecl>,
    calls: Vec<Call>,
    macro_rules: Vec<Rules>,
    lends: Vec<LendDecl>,
    params: Vec<Sym>,
    signatures: Vec<(PathId, Rc<str>)>,
    failure: Option<String>,
}

/// A macro invocation at item or associated-item position.
struct Call {
    module: PathId,
    name: Sym,
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
    /// Identifiers its item-position macros may turn into items, in any module.
    pub(crate) macro_idents: HashSet<Sym>,
    /// Its `#[macro_export]` macros by name.
    pub(crate) macro_rules: Vec<(Sym, Definition)>,
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
    /// The `#[macro_export]` macros of head's dependencies by name, expanded wherever the
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
}

impl Indexer {
    pub(crate) fn new(cache: FileCache) -> Self {
        Self {
            interner: Interner::default(),
            cache,
            memo: HashMap::new(),
            expansions: HashMap::new(),
            expander: expand::Expander::default(),
            dependency_macros: HashMap::new(),
            head_unexpanded: HashSet::new(),
            members: BTreeMap::new(),
            crates: HashSet::new(),
            manifest_dirs: ManifestDirs::new(),
        }
    }

    /// Makes the `#[macro_export]` macros `rules` of head's dependencies expandable in every
    /// later index.
    pub(crate) fn use_dependency_macros(&mut self, rules: &[(Sym, Definition)]) {
        for (name, definition) in rules {
            let definitions = Rc::make_mut(self.dependency_macros.entry(*name).or_default());
            if !definitions.contains(definition) {
                definitions.push(definition.clone());
            }
        }
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
        let members = match manifest::workspace(snapshot, workspace) {
            Ok(members) => members,
            Err(Error::Manifest { .. }) if !head => {
                index.complete = false;
                return Ok(index);
            }
            Err(error) => return Err(error),
        };
        let dirs = snapshot.manifest_dirs(&mut self.manifest_dirs)?;
        let outside_members = manifest::packages(snapshot, &dirs, &members);
        let mut workspace_crates = HashSet::new();
        let mut outside = HashSet::new();
        let mut crates = Vec::new();
        let mut stack = Vec::new();
        let workspace_members = members.len();
        for (n, member) in members.into_iter().chain(outside_members).enumerate() {
            self.entries(&member, &mut index);
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
                    public_only: false,
                }));
            }
        }
        // A crate name a workspace member also has stays strict at head.
        outside.retain(|krate| !workspace_crates.contains(krate));

        let reached = self.reach(snapshot, stack, head, &outside, &mut index)?;
        let uses: Vec<&UseDecl> = reached.contributions().flat_map(|c| &c.uses).collect();
        let lends: Vec<&LendDecl> = reached.contributions().flat_map(|c| &c.lends).collect();
        let mut hidden = hidden_paths(
            reached
                .contributions()
                .map(|c| (c.hidden.as_slice(), c.exposed.as_slice())),
        );
        index.reexports =
            self.resolve_aliases(&mut index.paths, &mut hidden, &uses, &lends, &crates);
        if head {
            index.alive = macro_idents(&mut self.interner, &reached);
            index.unexpanded = reached
                .unexpanded()
                .filter(|call| call.expandable.is_some())
                .map(|call| call.name)
                .collect();
            index.params = reached
                .contributions()
                .flat_map(|c| c.params.iter().copied())
                .collect();
        }
        Ok(index)
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
        for len in (1..=segments.len()).rev() {
            let Some(enclosing) = self.interner.find_path(&segments[..len]) else {
                continue;
            };
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
        let tokens = &expandable.tokens;
        let text = match definition {
            Some(definition) => {
                let home = definition
                    .home
                    .map_or("crate", |home| self.interner.name(home));
                self.expander.expand(&definition.body, tokens, home)
            }
            None => Some(tokens.to_string()),
        };
        let text = match &expandable.impl_header {
            Some(header) => text.map(|text| format!("{header} {{ {text} }}")),
            None => text,
        };
        let items = text.and_then(|text| parse::parse(&text).ok());
        let built = items.map(|mut items| {
            items.nest(&expandable.inline);
            Rc::new(self.build(visit, &items))
        });
        self.expansions.insert(key, built.clone());
        built
    }

    /// The public items of the library whose root file is `root` in `snapshot`, rooted at
    /// `crate_name`.
    pub(crate) fn library(
        &mut self,
        snapshot: &dyn Snapshot,
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
        })
    }

    /// The paths of `libraries` that start with one of `roots`, lie below no private module and
    /// are not themselves hidden once the re-exports between all of them are resolved.
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
        let mut hidden = hidden_paths(
            libraries
                .iter()
                .map(|library| (library.hidden.as_slice(), library.exposed.as_slice())),
        );
        // What lies below a private module goes even where a `pub use` shares the module's path.
        let private = hidden.clone();
        self.resolve_aliases(&mut paths, &mut hidden, &uses, &lends, &crates);
        paths.retain(|&id| {
            let segments = self.interner.segments(id);
            roots.contains(&segments[0])
                && !hidden.contains(&id)
                && !(2..segments.len()).any(|len| {
                    self.interner
                        .find_path(&segments[..len])
                        .is_some_and(|prefix| private.contains(&prefix))
                })
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
                paths: Vec::new(),
                hidden: Vec::new(),
                exposed: Vec::new(),
                visits: Vec::new(),
                uses: Vec::new(),
                calls: Vec::new(),
                macro_rules: Vec::new(),
                lends: Vec::new(),
                params: Vec::new(),
                signatures: Vec::new(),
                failure: Some(message.clone()),
            },
        });
        self.memo.insert(key, Rc::clone(&contribution));
        Ok(contribution)
    }

    fn build(&mut self, visit: &Visit, items: &FileItems) -> Contribution {
        let module: Vec<Sym> = self.interner.segments(visit.module).to_vec();
        let crate_sym = module[0];
        let counts = |public: bool| public || !visit.public_only;
        let mut paths = Vec::new();
        let mut signatures = Vec::new();
        let mut hidden = Vec::new();
        let mut exposed = Vec::new();
        for decl in &items.items {
            // A private module stays, so that re-exports out of it resolve, and is dropped
            // with everything below it once they have.
            let private_module = decl.kind == ItemKind::Module && !counts(decl.public);
            if !counts(decl.public) && !private_module {
                continue;
            }
            let path = self.within(&module, &decl.path);
            let id = self.interner.path(&path, decl.kind);
            paths.push(id);
            if let Some(signature) = &decl.signature {
                signatures.push((id, Rc::from(signature.as_str())));
            }
            if decl.public {
                exposed.push(id);
            } else {
                hidden.push(id);
            }
        }
        let imports = self.imports(&module, items);
        for block in &items.impls {
            let within = self.within(&module, &block.module);
            let self_ty = imported(&imports, &within, self.syms(&block.self_ty));
            let Some(mut base) = self.resolve(&within, &self_ty, crate_sym) else {
                continue;
            };
            for (name, kind, public, signature) in &block.items {
                if counts(*public) {
                    base.push(self.interner.sym(name));
                    let id = self.interner.path(&base, *kind);
                    paths.push(id);
                    if *public {
                        exposed.push(id);
                    } else {
                        hidden.push(id);
                    }
                    if let Some(signature) = signature {
                        signatures.push((id, Rc::from(signature.as_str())));
                    }
                    base.pop();
                }
            }
        }
        for rules in items.macro_rules.iter().filter(|rules| rules.exported) {
            let name = self.interner.sym(&rules.name);
            paths.push(self.interner.path(&[crate_sym, name], ItemKind::Macro));
        }

        let visits = self.visits(visit, &module, items);

        let uses = items
            .uses
            .iter()
            .filter(|u| counts(u.public))
            .map(|u| {
                let name = u.name.as_deref().map(|n| self.interner.sym(n));
                let within = self.within(&module, &u.module);
                let source = imported(&imports, &within, self.syms(&u.source));
                (within, source, name, u.public)
            })
            .collect();
        let calls = self.calls(&module, items);
        let macro_rules = self.rules(&module, items);
        let lends = items
            .lends
            .iter()
            .map(|d| {
                let within = self.within(&module, &d.module);
                let owner = imported(&imports, &within, self.syms(&d.owner));
                let source = imported(&imports, &within, self.syms(&d.source));
                (within, owner, source)
            })
            .collect();
        Contribution {
            paths,
            hidden,
            exposed,
            visits,
            uses,
            calls,
            macro_rules,
            lends,
            params: self.syms(&items.params),
            signatures,
            failure: None,
        }
    }

    /// The named `use`s of `items` of any visibility, read as the module `module`, except those
    /// of a name their module declares.
    fn imports(&mut self, module: &[Sym], items: &FileItems) -> Vec<Import> {
        let declared: HashSet<Vec<Sym>> = items
            .items
            .iter()
            .map(|decl| self.within(module, &decl.path))
            .collect();
        items
            .uses
            .iter()
            .chain(&items.imports)
            .filter_map(|u| {
                let name = self.interner.sym(u.name.as_deref()?);
                let mut within = self.within(module, &u.module);
                within.push(name);
                // A declared name is what paths reach, so an import of it names another namespace.
                if declared.contains(&within) {
                    return None;
                }
                within.pop();
                Some((within, self.syms(&u.source), name))
            })
            .collect()
    }

    /// The macro invocations of `items`, read as the module `module`.
    fn calls(&mut self, module: &[Sym], items: &FileItems) -> Vec<Call> {
        items
            .invocations
            .iter()
            .map(|inv| {
                let within = self.within(module, &inv.module);
                Call {
                    module: self.interner.path(&within, ItemKind::Module),
                    name: self.interner.sym(&inv.name),
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
                        public_only: visit.public_only,
                    }));
                }
            }
        }
        for include in &items.includes {
            let within = self.within(module, &include.module);
            // As in rustc, an included file's `mod`s resolve against its own directory.
            visits.push(Rc::new(Visit {
                file: join(parent(&visit.file), &include.path),
                mod_rs: true,
                module: self.interner.path(&within, ItemKind::Module),
                public_only: visit.public_only,
            }));
        }
        visits
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
    fn resolve(&mut self, module: &[Sym], segments: &[Sym], crate_sym: Sym) -> Option<Vec<Sym>> {
        let crate_kw = self.interner.sym("crate");
        let self_kw = self.interner.sym("self");
        let super_kw = self.interner.sym("super");
        let self_ty = self.interner.sym("Self");
        let first = *segments.first()?;
        if first == self_ty {
            return None;
        }
        if first == crate_kw {
            let mut path = vec![crate_sym];
            path.extend_from_slice(&segments[1..]);
            return Some(path);
        }
        let mut base = module.to_vec();
        let mut rest = if first == self_kw {
            &segments[1..]
        } else {
            segments
        };
        while rest.first() == Some(&super_kw) {
            if base.len() <= 1 {
                return None;
            }
            base.pop();
            rest = &rest[1..];
        }
        base.extend_from_slice(rest);
        Some(base)
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
    ) -> Reexports {
        const ROUNDS: usize = 8;
        let mut origins = HashMap::new();
        let mut tree = self.tree(paths);
        // One path stands for every namespace, so a public re-export of a name keeps it public.
        for (module, _, name, public) in uses {
            if *public && let Some(name) = name {
                let mut alias = module.clone();
                alias.push(*name);
                hidden.remove(&self.interner.path(&alias, ItemKind::Reexport));
            }
        }
        for _ in 0..ROUNDS {
            let before = paths.len();
            let mut fresh = Vec::new();
            for (module, source, name, public) in uses {
                let target = self
                    .resolve_use(module, source, crates, paths)
                    .and_then(|resolved| self.interner.find_path(&resolved))
                    .filter(|id| paths.contains(id));
                if let Some(name) = name {
                    let mut alias = module.clone();
                    alias.push(*name);
                    let id = self.interner.path(&alias, ItemKind::Reexport);
                    if paths.insert(id) {
                        fresh.push(id);
                        if !public {
                            hidden.insert(id);
                        }
                    }
                    if let Some(target) = target {
                        let stands_for = origin(&origins, target);
                        origins.entry(id).or_insert(stands_for);
                        let copy = (target, alias, !public);
                        self.copy_below(copy, &tree, paths, &mut origins, hidden, &mut fresh);
                    }
                } else if let Some(target) = target {
                    let copy = (target, module.clone(), !public);
                    self.copy_below(copy, &tree, paths, &mut origins, hidden, &mut fresh);
                }
            }
            for (module, owner, source) in lends {
                let Some(owner) = self.resolve(module, owner, module[0]) else {
                    continue;
                };
                let target = self
                    .resolve(module, source, module[0])
                    .and_then(|resolved| self.interner.find_path(&resolved))
                    .filter(|id| paths.contains(id));
                let owner_known = self
                    .interner
                    .find_path(&owner)
                    .is_some_and(|id| paths.contains(&id));
                if let Some(target) = target
                    && owner_known
                {
                    let copy = (target, owner, false);
                    self.copy_below(copy, &tree, paths, &mut origins, hidden, &mut fresh);
                }
            }
            if paths.len() == before {
                break;
            }
            self.grow(&mut tree, &fresh);
        }
        // A source among the paths reaches mentions only through the copies above.
        let mut reexports = Reexports::new();
        for (module, source, name, _) in uses {
            let inside = self
                .resolve_use(module, source, crates, paths)
                .and_then(|resolved| self.interner.find_path(&resolved))
                .is_some_and(|id| paths.contains(&id));
            if inside {
                continue;
            }
            let mut alias = module.clone();
            alias.extend(*name);
            reexports.entry(alias).or_default().push(source.clone());
        }
        reexports
    }

    fn resolve_use(
        &mut self,
        module: &[Sym],
        source: &[Sym],
        crates: &[Sym],
        paths: &HashSet<PathId>,
    ) -> Option<Vec<Sym>> {
        let first = *source.first()?;
        let keyword = ["crate", "self", "super"]
            .iter()
            .any(|kw| self.interner.find_sym(kw) == Some(first));
        if keyword {
            return self.resolve(module, source, module[0]);
        }
        let mut local = module.to_vec();
        local.push(first);
        if self
            .interner
            .find_path(&local)
            .is_some_and(|id| paths.contains(&id))
        {
            local.extend_from_slice(&source[1..]);
            return Some(local);
        }
        crates.contains(&first).then(|| source.to_vec())
    }

    /// Re-roots every path strictly below `from` under `to`, never through a module the copy
    /// already passes, so that re-exports forming a cycle stay finite.
    ///
    /// `origins` maps every alias and copied path to the declared path it stands for, and
    /// `fresh` collects the paths the copy adds. A copy into another crate skips the `hidden`
    /// paths with everything below them, and a new copy of a hidden path, or any new copy when
    /// `hide` is set, joins them.
    fn copy_below(
        &mut self,
        (from, to, hide): (PathId, Vec<Sym>, bool),
        tree: &Tree,
        paths: &mut HashSet<PathId>,
        origins: &mut HashMap<PathId, PathId>,
        hidden: &mut HashSet<PathId>,
        fresh: &mut Vec<PathId>,
    ) {
        let across = self.interner.segments(from)[0] != to[0];
        let chain: Vec<PathId> = (1..=to.len())
            .filter_map(|len| self.interner.find_path(&to[..len]))
            .map(|id| origin(origins, id))
            .collect();
        let mut stack = vec![(from, to, chain)];
        let mut guard = HashSet::new();
        while let Some((source, prefix, chain)) = stack.pop() {
            if !guard.insert(source) {
                continue;
            }
            for &child in tree.children.get(&source).into_iter().flatten() {
                if across && hidden.contains(&child) {
                    continue;
                }
                let last = *self
                    .interner
                    .segments(child)
                    .last()
                    .expect("children are non-empty paths");
                let mut path = prefix.clone();
                path.push(last);
                let kind = self.interner.kind(child);
                let id = self.interner.path(&path, kind);
                if paths.insert(id) {
                    fresh.push(id);
                    if hide || hidden.contains(&child) {
                        hidden.insert(id);
                    }
                }
                let child_origin = origin(origins, child);
                origins.entry(id).or_insert(child_origin);
                if !chain.contains(&child_origin) {
                    let mut deeper = chain.clone();
                    deeper.push(child_origin);
                    stack.push((child, path, deeper));
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

    /// Adds the paths `fresh` to `tree`, adopting the paths whose parent one of them is and
    /// was not interned before.
    fn grow(&self, tree: &mut Tree, fresh: &[PathId]) {
        let mut touched = HashSet::new();
        for &id in fresh {
            if let Some(parent) = self.link(tree, id) {
                touched.insert(parent);
            }
            if let Some(adopted) = tree.orphans.remove(self.interner.segments(id)) {
                tree.children.entry(id).or_default().extend(adopted);
                touched.insert(id);
            }
        }
        for parent in touched {
            if let Some(list) = tree.children.get_mut(&parent) {
                list.sort_unstable();
            }
        }
    }

    /// Files `id` under its parent in `tree`, or among the orphans while the parent is not
    /// interned. A parent outside the set keeps its children unused until it joins. Returns
    /// the parent it was filed under.
    fn link(&self, tree: &mut Tree, id: PathId) -> Option<PathId> {
        let segments = self.interner.segments(id);
        let (_, parent) = segments.split_last()?;
        if parent.is_empty() {
            return None;
        }
        if let Some(found) = self.interner.find_path(parent) {
            tree.children.entry(found).or_default().push(id);
            return Some(found);
        }
        tree.orphans.entry(parent.into()).or_default().push(id);
        None
    }
}

/// The parent and child links among a set of paths.
#[derive(Default)]
struct Tree {
    /// The children of every path, sorted.
    children: HashMap<PathId, Vec<PathId>>,
    /// The paths whose parent was not interned yet, by the parent's segments.
    orphans: HashMap<Box<[Sym]>, Vec<PathId>>,
}

/// The declared path `id` stands for, given the alias and copy `origins`.
fn origin(origins: &HashMap<PathId, PathId>, id: PathId) -> PathId {
    origins.get(&id).copied().unwrap_or(id)
}

/// The paths some of the `(hidden, exposed)` declaration lists hide and none exposes, since
/// one path stands for every namespace and every `cfg` branch.
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

/// `path` as written in `module`, its first segment replaced while it names one of `imports`
/// there by that import's source.
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
