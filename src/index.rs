//! Every item path a snapshot declares, assembled from its crates' module trees.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gix::ObjectId;

use crate::cache::FileCache;
use crate::git::Snapshot;
use crate::intern::{Interner, PathId, Sym};
use crate::manifest::{self, join};
use crate::parse::FileItems;
use crate::{Error, ItemKind};

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
}

/// A `use` creating a path: the module it sits in, the imported path, the created name or
/// `None` for a glob.
type UseDecl = (Vec<Sym>, Vec<Sym>, Option<Sym>);

/// A named import, its module, source and local name.
type Import = (Vec<Sym>, Vec<Sym>, Sym);

/// One file read as one module of one crate, reusable across snapshots.
struct Contribution {
    paths: Vec<PathId>,
    visits: Vec<Rc<Visit>>,
    uses: Vec<UseDecl>,
    invocations: Vec<(PathId, Sym, Vec<Sym>)>,
    macro_rules: Vec<(Sym, Vec<Sym>)>,
    failure: Option<String>,
}

/// A file to read as a module.
#[derive(PartialEq, Eq, Hash)]
struct Visit {
    /// The file read.
    file: String,
    /// The file `mod` declarations resolve against, differing from `file` for `include!`.
    origin: String,
    /// Whether `origin` is a `mod.rs`-style file owning its directory.
    mod_rs: bool,
    module: PathId,
}

/// Builds indexes, sharing parsed files and per-module contributions between snapshots.
pub(crate) struct Indexer {
    pub(crate) interner: Interner,
    cache: FileCache,
    memo: HashMap<(Rc<Visit>, ObjectId), Rc<Contribution>>,
}

impl Indexer {
    pub(crate) fn new(cache: FileCache) -> Self {
        Self {
            interner: Interner::default(),
            cache,
            memo: HashMap::new(),
        }
    }

    pub(crate) fn save(&self) {
        self.cache.save();
    }

    /// The index of `snapshot` for the workspace in `workspace`.
    ///
    /// At `head`, an unreadable manifest or Rust file is an error. Elsewhere it leaves the index
    /// incomplete.
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
        let targets = match manifest::targets(snapshot, workspace) {
            Ok(targets) => targets,
            Err(Error::Manifest { .. }) if !head => {
                index.complete = false;
                return Ok(index);
            }
            Err(error) => return Err(error),
        };
        let mut crates = Vec::new();
        let mut stack = Vec::new();
        for target in targets {
            let crate_sym = self.interner.sym(&target.crate_name);
            crates.push(crate_sym);
            if target.lib {
                index.libs.push(crate_sym);
            }
            let module = self.interner.path(&[crate_sym], ItemKind::Module);
            index.paths.insert(module);
            stack.push(Rc::new(Visit {
                origin: target.root.clone(),
                file: target.root,
                mod_rs: true,
                module,
            }));
        }

        let mut seen = HashSet::new();
        let mut reached = Vec::new();
        while let Some(visit) = stack.pop() {
            if !seen.insert(Rc::clone(&visit)) {
                continue;
            }
            let Some(id) = snapshot.blob(&visit.file)? else {
                continue;
            };
            let contribution = self.contribution(snapshot, &visit, id)?;
            if let Some(message) = &contribution.failure {
                if head {
                    return Err(Error::Parse {
                        path: visit.file.clone(),
                        message: message.clone(),
                    });
                }
                index.complete = false;
                continue;
            }
            index.paths.extend(contribution.paths.iter().copied());
            if head && !index.file_modules.contains_key(&visit.file) {
                let module = self.interner.segments(visit.module).to_vec();
                index.file_modules.insert(visit.file.clone(), module);
            }
            stack.extend(contribution.visits.iter().map(Rc::clone));
            reached.push(contribution);
        }

        let uses: Vec<&UseDecl> = reached.iter().flat_map(|c| &c.uses).collect();
        self.resolve_uses(&mut index.paths, &uses, &crates);
        if head {
            let mut bodies: HashMap<Sym, Vec<Sym>> = HashMap::new();
            for (name, idents) in reached.iter().flat_map(|c| &c.macro_rules) {
                bodies.entry(*name).or_default().extend(idents);
            }
            for (module, name, idents) in reached.iter().flat_map(|c| &c.invocations) {
                let alive = index.alive.entry(*module).or_default();
                alive.extend(idents);
                if let Some(body) = bodies.get(name) {
                    alive.extend(body);
                }
            }
        }
        Ok(index)
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
                visits: Vec::new(),
                uses: Vec::new(),
                invocations: Vec::new(),
                macro_rules: Vec::new(),
                failure: Some(message.clone()),
            },
        });
        self.memo.insert(key, Rc::clone(&contribution));
        Ok(contribution)
    }

    fn build(&mut self, visit: &Visit, items: &FileItems) -> Contribution {
        let module: Vec<Sym> = self.interner.segments(visit.module).to_vec();
        let crate_sym = module[0];
        let mut paths = Vec::new();
        for decl in &items.items {
            let path = self.within(&module, &decl.path);
            paths.push(self.interner.path(&path, decl.kind));
        }
        let imports = self.imports(&module, items, &paths);
        for block in &items.impls {
            let within = self.within(&module, &block.module);
            let self_ty = imported(&imports, &within, self.syms(&block.self_ty));
            let Some(mut base) = self.resolve(&within, &self_ty, crate_sym) else {
                continue;
            };
            for (name, kind) in &block.items {
                base.push(self.interner.sym(name));
                paths.push(self.interner.path(&base, *kind));
                base.pop();
            }
        }
        for rules in items.macro_rules.iter().filter(|rules| rules.exported) {
            let name = self.interner.sym(&rules.name);
            paths.push(self.interner.path(&[crate_sym, name], ItemKind::Macro));
        }

        let mut visits = Vec::new();
        for decl in &items.mods {
            let mut child = self.within(&module, &decl.module);
            child.push(self.interner.sym(&decl.name));
            let child = self.interner.path(&child, ItemKind::Module);
            let base = owned_dir(&visit.origin, visit.mod_rs, &decl.module);
            for path in decl.paths.iter().chain(&decl.conditional_paths) {
                let file = if decl.module.is_empty() {
                    join(parent(&visit.origin), path)
                } else {
                    join(&base, path)
                };
                visits.push(Rc::new(Visit {
                    origin: file.clone(),
                    file,
                    mod_rs: true,
                    module: child,
                }));
            }
            if decl.paths.is_empty() {
                for (file, mod_rs) in [
                    (join(&base, &format!("{}.rs", decl.name)), false),
                    (join(&base, &format!("{}/mod.rs", decl.name)), true),
                ] {
                    visits.push(Rc::new(Visit {
                        origin: file.clone(),
                        file,
                        mod_rs,
                        module: child,
                    }));
                }
            }
        }
        for include in &items.includes {
            let within = self.within(&module, &include.module);
            visits.push(Rc::new(Visit {
                file: join(parent(&visit.file), &include.path),
                origin: visit.origin.clone(),
                mod_rs: visit.mod_rs,
                module: self.interner.path(&within, ItemKind::Module),
            }));
        }

        let uses = items
            .uses
            .iter()
            .map(|u| {
                let name = u.name.as_deref().map(|n| self.interner.sym(n));
                let within = self.within(&module, &u.module);
                let source = imported(&imports, &within, self.syms(&u.source));
                (within, source, name)
            })
            .collect();
        let invocations = items
            .invocations
            .iter()
            .map(|inv| {
                let within = self.within(&module, &inv.module);
                let module = self.interner.path(&within, ItemKind::Module);
                (module, self.interner.sym(&inv.name), self.syms(&inv.idents))
            })
            .collect();
        let macro_rules = items
            .macro_rules
            .iter()
            .map(|rules| (self.interner.sym(&rules.name), self.syms(&rules.idents)))
            .collect();
        Contribution {
            paths,
            visits,
            uses,
            invocations,
            macro_rules,
            failure: None,
        }
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

    /// Adds re-export paths and, for sources inside the workspace, everything below them.
    fn resolve_uses(&mut self, paths: &mut HashSet<PathId>, uses: &[&UseDecl], crates: &[Sym]) {
        const ROUNDS: usize = 8;
        let mut origins = HashMap::new();
        for _ in 0..ROUNDS {
            let before = paths.len();
            let children = self.children(paths);
            for (module, source, name) in uses {
                let target = self
                    .resolve_use(module, source, crates, paths)
                    .and_then(|resolved| self.interner.find_path(&resolved))
                    .filter(|id| paths.contains(id));
                if let Some(name) = name {
                    let mut alias = module.clone();
                    alias.push(*name);
                    let id = self.interner.path(&alias, ItemKind::Reexport);
                    paths.insert(id);
                    if let Some(target) = target {
                        let stands_for = origin(&origins, target);
                        origins.entry(id).or_insert(stands_for);
                        self.copy_below(target, alias, &children, paths, &mut origins);
                    }
                } else if let Some(target) = target {
                    self.copy_below(target, module.clone(), &children, paths, &mut origins);
                }
            }
            if paths.len() == before {
                break;
            }
        }
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

    /// Copies descendants without visiting a declared origin twice on one path.
    fn copy_below(
        &mut self,
        from: PathId,
        to: Vec<Sym>,
        children: &HashMap<PathId, Vec<PathId>>,
        paths: &mut HashSet<PathId>,
        origins: &mut HashMap<PathId, PathId>,
    ) {
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
            for &child in children.get(&source).into_iter().flatten() {
                let last = *self
                    .interner
                    .segments(child)
                    .last()
                    .expect("children are non-empty paths");
                let mut path = prefix.clone();
                path.push(last);
                let kind = self.interner.kind(child);
                let id = self.interner.path(&path, kind);
                paths.insert(id);
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

    fn children(&self, paths: &HashSet<PathId>) -> HashMap<PathId, Vec<PathId>> {
        let mut children: HashMap<PathId, Vec<PathId>> = HashMap::new();
        for &id in paths {
            let segments = self.interner.segments(id);
            if segments.len() < 2 {
                continue;
            }
            if let Some(parent) = self.interner.find_path(&segments[..segments.len() - 1])
                && paths.contains(&parent)
            {
                children.entry(parent).or_default().push(id);
            }
        }
        for list in children.values_mut() {
            list.sort_unstable();
        }
        children
    }
}

/// The declared path represented by an alias or copied path.
fn origin(origins: &HashMap<PathId, PathId>, id: PathId) -> PathId {
    origins.get(&id).copied().unwrap_or(id)
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
