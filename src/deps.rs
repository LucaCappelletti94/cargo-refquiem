//! Dependency APIs resolved through a scratch Cargo manifest graph.
//!
//! Each local package has one identity. Repository manifests and API sources come from the selected snapshot, external packages from disk.
//! Manifests are serialized from the tables retained during discovery.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};

use cargo_metadata::{CargoOpt, Metadata, MetadataCommand, PackageId};
use gix::ObjectId;
use toml::{Table, Value};

use crate::git::{Snapshot, TreeSnapshot, WorkTreeSnapshot, relative};
use crate::index::Indexer;
use crate::intern::{PathId, Sym};
use crate::manifest::{self, Dependency, Manifests, Member, TargetKind, join};
use crate::{Error, HashMap, HashSet, Skipped};

/// The dependency tables of a manifest, each also under `[target.<cfg>]`.
const KINDS: [&str; 5] = [
    "dependencies",
    "dev-dependencies",
    "dev_dependencies",
    "build-dependencies",
    "build_dependencies",
];

/// The registries crates.io is known by in `Cargo.lock`.
const CRATES_IO: [&str; 2] = [
    "registry+https://github.com/rust-lang/crates.io-index",
    "sparse+https://index.crates.io/",
];

/// Where the manifests refquiem writes go, and where cargo reads its configuration from.
pub(crate) struct Scratch<'a> {
    /// A directory refquiem owns.
    pub(crate) dir: &'a Path,
    /// The workspace directory on disk, whose `.cargo/config.toml` applies.
    pub(crate) workspace_dir: &'a Path,
    /// The repository working tree on disk, against which path dependencies resolve.
    pub(crate) workdir: &'a Path,
    /// The workspace directory from the repository root.
    pub(crate) workspace: &'a str,
}

/// A dependency entry the range removed, with the tree closest to head that still has it.
pub(crate) struct Removal {
    /// The removed entry.
    pub(crate) entry: PathId,
    pub(crate) tree: ObjectId,
    pub(crate) member_dir: String,
    pub(crate) key: String,
}

/// What a set of dependencies contributes.
#[derive(Default)]
pub(crate) struct Layer {
    /// Public paths, rooted at the names code spells the dependencies by.
    pub(crate) paths: HashSet<PathId>,
    /// The names code spells the dependencies by.
    pub(crate) roots: HashSet<Sym>,
    /// Identifiers their item-position macros may turn into items.
    pub(crate) macro_idents: HashSet<Sym>,
}

/// The items of removed dependencies.
pub(crate) struct Removed {
    /// Public paths head lacks, each with the removal entry it comes from.
    pub(crate) items: HashMap<PathId, PathId>,
    /// Removed dependencies whose sources could not be resolved.
    pub(crate) skipped: Vec<Skipped>,
}

/// A package directory the graph export walks.
enum Local {
    /// From the repository root.
    Repo(String),
    /// On disk, outside the repository.
    Disk(PathBuf),
}

/// Where a mirrored package's sources are read from, keyed by mirror directory.
enum Source<'a> {
    /// The selected snapshot, the package's directory from the repository root.
    Repo(&'a dyn Snapshot, String),
    /// The package's original directory on disk.
    Disk(PathBuf),
}

/// The layer of head's dependencies.
pub(crate) fn head(
    indexer: &mut Indexer,
    snapshot: &dyn Snapshot,
    scratch: &Scratch<'_>,
) -> Result<Layer, Error> {
    let members = manifest::workspace(snapshot, scratch.workspace, &mut indexer.manifests)?;
    if members.iter().all(|member| member.dependencies.is_empty()) {
        return Ok(Layer::default());
    }
    let export = scratch.dir.join("metadata");
    let (root, sources) =
        export_graph(snapshot, &members, &export, scratch, &mut indexer.manifests)?;
    let metadata = metadata(&root, scratch.workspace_dir)?;
    // Cargo can enroll path packages that only the dependency index reads.
    let member_dirs: HashSet<&str> = members.iter().map(|member| member.dir.as_str()).collect();
    let workspace: HashSet<&PackageId> = metadata
        .packages
        .iter()
        .filter_map(|package| {
            let dir = package.manifest_path.parent()?;
            let Source::Repo(_, dir) = sources.get(dir.as_std_path())? else {
                return None;
            };
            member_dirs.contains(dir.as_str()).then_some(&package.id)
        })
        .collect();
    let mut roots = Vec::new();
    for node in metadata.resolve.iter().flat_map(|resolve| &resolve.nodes) {
        if workspace.contains(&node.id) {
            roots.extend(
                node.deps
                    .iter()
                    .filter(|dep| !workspace.contains(&dep.pkg))
                    .map(|dep| (dep.pkg.clone(), dep.name.clone())),
            );
        }
    }
    let crate_name = |id: &PackageId| {
        let package = metadata.packages.iter().find(|package| &package.id == id)?;
        Some(lib_target(package)?.name.replace('-', "_"))
    };
    let workspace_crates: HashSet<String> =
        workspace.iter().copied().filter_map(crate_name).collect();
    // A renamed dependency still reads by its crate name, as code written for it elsewhere does.
    let renamed: Vec<(PackageId, String)> = roots
        .iter()
        .filter_map(|(id, name)| {
            let crate_name = crate_name(id)?;
            (crate_name != *name && !workspace_crates.contains(&crate_name))
                .then(|| (id.clone(), crate_name))
        })
        .collect();
    roots.extend(renamed);
    libraries(indexer, &metadata, &roots, Some(&sources))
}

/// The public paths of the `removals` that neither set of `alive` paths has. A removal whose
/// sources cargo cannot resolve is skipped. Its paths are fetched once per run, for every
/// removal that resolves to the same manifest.
pub(crate) fn removed(
    indexer: &mut Indexer,
    repo: &gix::Repository,
    removals: &[Removal],
    alive: [&HashSet<PathId>; 2],
    scratch: &Scratch<'_>,
) -> Result<Removed, Error> {
    let mut removed = Removed {
        items: HashMap::new(),
        skipped: Vec::new(),
    };
    let mut memo: HashMap<ObjectId, HashSet<PathId>> = HashMap::new();
    for removal in removals {
        let tree = TreeSnapshot::new(repo, removal.tree, &indexer.trees);
        let Ok(members) = manifest::workspace(&tree, scratch.workspace, &mut indexer.manifests)
        else {
            continue;
        };
        let Some(member) = members.iter().find(|m| m.dir == removal.member_dir) else {
            continue;
        };
        let Some(dependency) = member.dependencies.iter().find(|d| d.key == removal.key) else {
            continue;
        };
        let paths = match &dependency.path {
            Some(dir) => path_library(indexer, &tree, dir, dependency),
            None => fetched(indexer, &tree, member, dependency, scratch, &mut memo),
        };
        let reason = match paths {
            Ok(paths) => {
                for path in paths {
                    if !alive.iter().any(|paths| paths.contains(&path)) {
                        removed.items.entry(path).or_insert(removal.entry);
                    }
                }
                continue;
            }
            Err(Error::Metadata { source, .. }) => source.to_string(),
            Err(error @ (Error::NoLibrary { .. } | Error::Manifest { .. })) => error.to_string(),
            Err(error) => return Err(error),
        };
        removed.skipped.push(Skipped {
            dependency: dependency.key.clone(),
            reason,
        });
    }
    Ok(removed)
}

/// Public paths of `dependency` rooted at its manifest key for an explicit `package`, or its library name otherwise.
fn path_library(
    indexer: &mut Indexer,
    tree: &TreeSnapshot<'_>,
    dir: &str,
    dependency: &Dependency,
) -> Result<HashSet<PathId>, Error> {
    let lib = manifest::workspace(tree, dir, &mut indexer.manifests)?
        .into_iter()
        .flat_map(|member| member.targets)
        .find(|target| target.kind == TargetKind::Lib)
        .ok_or_else(|| Error::NoLibrary {
            name: dependency.key.clone(),
        })?;
    let name = match &dependency.package {
        Some(_) => dependency.key.replace('-', "_"),
        None => lib.crate_name,
    };
    let library = indexer.library(tree, &lib.root, &name)?;
    let root = indexer.interner.sym(&name);
    Ok(indexer.public_paths(&[library], &[root]))
}

/// The public paths of a registry or git `dependency` of `member`, fetched at the version the
/// `Cargo.lock` of `tree` records. The `memo` keeps the paths of every fetched manifest,
/// keyed by the hash of its bytes.
fn fetched(
    indexer: &mut Indexer,
    tree: &TreeSnapshot<'_>,
    member: &Member,
    dependency: &Dependency,
    scratch: &Scratch<'_>,
    memo: &mut HashMap<ObjectId, HashSet<PathId>>,
) -> Result<HashSet<PathId>, Error> {
    let package = dependency.package.as_deref().unwrap_or(&dependency.key);
    let lock = read_lock(tree, scratch.workspace)?;
    let mut spec = dependency.spec.clone();
    for key in ["optional", "path", "workspace", "public"] {
        spec.remove(key);
    }
    if let Some((version, source)) = lock
        .as_ref()
        .and_then(|l| locked(l, &member.package, package))
    {
        if let Some(git) = source.as_deref().and_then(|s| s.strip_prefix("git+")) {
            let (url, rev) = git.split_once('#').unwrap_or((git, ""));
            let url = url.split('?').next().unwrap_or(url);
            spec.remove("branch");
            spec.remove("tag");
            spec.insert("git".to_owned(), Value::from(url));
            spec.insert("rev".to_owned(), Value::from(rev));
        } else {
            spec.insert("version".to_owned(), Value::from(format!("={version}")));
            if let Some(source) = source.filter(|s| !CRATES_IO.contains(&s.as_str()))
                && !spec.contains_key("registry")
            {
                let index = source.strip_prefix("registry+").unwrap_or(&source);
                spec.insert("registry-index".to_owned(), Value::from(index));
            }
        }
    }
    let mut manifest: Table = "[package]\nname = \"refquiem-dependency\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[lib]\npath = \"lib.rs\"\n\n[workspace]\n"
        .parse()
        .expect("the dependency manifest skeleton is valid TOML");
    let mut dependencies = Table::new();
    dependencies.insert(dependency.key.clone(), Value::Table(spec));
    manifest.insert("dependencies".to_owned(), Value::Table(dependencies));
    let bytes = manifest.to_string().into_bytes();
    let id = gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::objs::Kind::Blob, &bytes)
        .map_err(|source| Error::Hash {
            path: "dependency manifest".to_owned(),
            source,
        })?;
    if let Some(paths) = memo.get(&id) {
        return Ok(paths.clone());
    }
    let dir = scratch.dir.join("deps").join(id.to_string());
    write(&dir.join("Cargo.toml"), &bytes)?;
    write(&dir.join("lib.rs"), b"")?;
    let metadata = metadata(&dir.join("Cargo.toml"), scratch.workspace_dir)?;
    // The manifest has exactly one dependency, named as its dependent spells it.
    let root = metadata
        .resolve
        .iter()
        .flat_map(|resolve| &resolve.nodes)
        .filter(|node| metadata.workspace_members.contains(&node.id))
        .flat_map(|node| &node.deps)
        .next()
        .ok_or_else(|| Error::NoLibrary {
            name: dependency.key.clone(),
        })?;
    let layer = libraries(
        indexer,
        &metadata,
        &[(root.pkg.clone(), root.name.clone())],
        None,
    )?;
    let paths = layer.paths;
    memo.insert(id, paths.clone());
    Ok(paths)
}

/// The `Cargo.lock` of the workspace in `tree`.
fn read_lock(tree: &TreeSnapshot<'_>, workspace: &str) -> Result<Option<Table>, Error> {
    let path = join(workspace, "Cargo.lock");
    let Some(id) = tree.blob(&path)? else {
        return Ok(None);
    };
    let bytes = tree.read(&path, id)?;
    Ok(String::from_utf8_lossy(&bytes).parse::<Table>().ok())
}

/// The version and source `lock` records for the `package` workspace member `member` depends
/// on.
fn locked(lock: &Table, member: &str, package: &str) -> Option<(String, Option<String>)> {
    let packages: Vec<&Table> = lock
        .get("package")?
        .as_array()?
        .iter()
        .filter_map(Value::as_table)
        .collect();
    let field =
        |table: &Table, key: &str| table.get(key).and_then(Value::as_str).map(str::to_owned);
    let requested = packages
        .iter()
        .find(|p| field(p, "name").as_deref() == Some(member) && !p.contains_key("source"))
        .and_then(|p| p.get("dependencies")?.as_array())
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .find(|spec| spec.split(' ').next() == Some(package))?;
    let mut parts = requested.split(' ').skip(1);
    let version = parts.next();
    let source = parts
        .next()
        .map(|s| s.trim_start_matches('(').trim_end_matches(')'));
    let found = packages.iter().find(|p| {
        field(p, "name").as_deref() == Some(package)
            && version.is_none_or(|v| field(p, "version").as_deref() == Some(v))
            && source.is_none_or(|s| field(p, "source").as_deref() == Some(s))
    })?;
    Some((field(found, "version")?, field(found, "source")))
}

/// The public paths of the `roots` packages, each by the name its dependent spells it, with
/// the libraries they re-export from indexed to resolve those re-exports. A package listed in
/// `sources` reads its library root from that source, every other from its manifest directory.
///
/// The `#[macro_export]` macros the indexed libraries define are registered, and the libraries
/// are indexed again while that gives an invocation a definition it lacked, as diesel's
/// `bitflags::bitflags!` needs the macros of `bitflags`. Head's layer and a removed
/// dependency's are then expanded alike.
fn libraries(
    indexer: &mut Indexer,
    metadata: &Metadata,
    roots: &[(PackageId, String)],
    sources: Option<&HashMap<PathBuf, Source<'_>>>,
) -> Result<Layer, Error> {
    let packages: HashMap<&PackageId, &cargo_metadata::Package> =
        metadata.packages.iter().map(|p| (&p.id, p)).collect();
    let nodes: HashMap<&PackageId, &cargo_metadata::Node> = metadata
        .resolve
        .iter()
        .flat_map(|resolve| &resolve.nodes)
        .map(|node| (&node.id, node))
        .collect();
    let root_syms: Vec<_> = roots
        .iter()
        .map(|(_, name)| indexer.interner.sym(name))
        .collect();
    let mut found;
    loop {
        let mut queue = roots.to_vec();
        let mut done = HashSet::new();
        found = Vec::new();
        while let Some((id, name)) = queue.pop() {
            if !done.insert((id.clone(), name.clone())) {
                continue;
            }
            let Some(package) = packages.get(&id) else {
                continue;
            };
            let Some(target) = lib_target(package) else {
                continue;
            };
            let Some(dir) = package.manifest_path.parent() else {
                continue;
            };
            let Some(root) = relative(dir.as_std_path(), target.src_path.as_std_path()) else {
                continue;
            };
            let library = match sources.and_then(|sources| sources.get(dir.as_std_path())) {
                Some(Source::Repo(snapshot, dir)) => {
                    indexer.library(*snapshot, &join(dir, &root), &name)?
                }
                Some(Source::Disk(dir)) => {
                    let snapshot = WorkTreeSnapshot::new(dir.clone(), Vec::new());
                    indexer.library(&snapshot, &root, &name)?
                }
                None => {
                    let snapshot =
                        WorkTreeSnapshot::new(dir.as_std_path().to_path_buf(), Vec::new());
                    indexer.library(&snapshot, &root, &name)?
                }
            };
            if let Some(node) = nodes.get(&id) {
                for first in library.use_roots() {
                    let first = indexer.interner.name(first);
                    if let Some(dep) = node.deps.iter().find(|dep| dep.name == first) {
                        queue.push((dep.pkg.clone(), dep.name.clone()));
                    }
                }
            }
            found.push(library);
        }
        let rules: Vec<_> = found
            .iter()
            .flat_map(|library| library.macro_rules.iter().cloned())
            .collect();
        let added = indexer.use_dependency_macros(&rules);
        if !added || found.iter().all(|library| library.unexpanded.is_empty()) {
            break;
        }
    }
    Ok(Layer {
        macro_idents: found
            .iter()
            .flat_map(|library| library.macro_idents.iter().copied())
            .collect(),
        paths: indexer.public_paths(&found, &root_syms),
        roots: root_syms.into_iter().collect(),
    })
}

/// The library target of `package`, however its crate type is spelled.
fn lib_target(package: &cargo_metadata::Package) -> Option<&cargo_metadata::Target> {
    package
        .targets
        .iter()
        .find(|t| t.is_lib() || t.is_rlib() || t.is_dylib())
}

/// Runs `cargo metadata` on `manifest` from `cwd`, with every feature so optional dependencies
/// resolve too.
fn metadata(manifest: &Path, cwd: &Path) -> Result<Metadata, Error> {
    MetadataCommand::new()
        .manifest_path(manifest)
        .current_dir(cwd)
        .features(CargoOpt::AllFeatures)
        .exec()
        .map_err(|source| Error::Metadata {
            path: manifest.to_path_buf(),
            source: Box::new(source),
        })
}

/// Writes the local manifest graph and records the original API source of each mirror.
#[expect(
    clippy::too_many_lines,
    reason = "discovery precedes rewriting so every path has a settled package identity"
)]
fn export_graph<'a>(
    snapshot: &'a dyn Snapshot,
    members: &[Member],
    export: &Path,
    scratch: &Scratch<'_>,
    manifests: &mut Manifests,
) -> Result<(PathBuf, HashMap<PathBuf, Source<'a>>), Error> {
    let workdir = scratch.workdir;
    let workspace = scratch.workspace;
    let external = scratch.dir.join("external");
    for dir in [export, &external] {
        if dir.exists() {
            std::fs::remove_dir_all(dir).map_err(|source| Error::Io {
                path: dir.to_path_buf(),
                source,
            })?;
        }
    }
    let export = export.join("repo");
    let root = export.join(workspace);
    let mut repo = BTreeMap::new();
    let mut disk_tables = BTreeMap::new();
    let mut queue = VecDeque::new();
    let mut seen = HashSet::new();
    let mut claimed = HashMap::new();
    let mut roots = BTreeMap::new();
    for dir in
        std::iter::once(workspace.to_owned()).chain(members.iter().map(|member| member.dir.clone()))
    {
        queue_repo(dir, snapshot, manifests, &mut seen, &mut repo, &mut queue)?;
    }
    while let Some(dir) = queue.pop_front() {
        let (base, mut table, rel) = match dir {
            Local::Repo(rel) => {
                let table = repo.remove(&rel).expect("queued manifests are present");
                (workdir.join(&rel), table, Some(rel))
            }
            Local::Disk(base) => {
                let disk = WorkTreeSnapshot::new(base.clone(), Vec::new());
                let Some(table) = manifest::read_manifest(&disk, "", manifests)? else {
                    continue;
                };
                (base, table, None)
            }
        };
        each_path(&mut table, &mut |path| {
            let resolved = normalize(&base, path);
            if let Some(rel) = relative(workdir, &resolved) {
                queue_repo(rel, snapshot, manifests, &mut seen, &mut repo, &mut queue)
            } else {
                queue_external(&resolved, manifests, &mut claimed, &mut roots, &mut queue)
            }
        })?;
        if let Some(rel) = rel {
            if table.contains_key("workspace") && rel != workspace {
                for member in manifest::workspace(snapshot, &rel, manifests)? {
                    queue_repo(
                        member.dir, snapshot, manifests, &mut seen, &mut repo, &mut queue,
                    )?;
                }
            }
            repo.insert(rel, table);
        } else {
            disk_tables.insert(base, table);
        }
    }
    let mut sources = HashMap::new();
    for (rel, mut table) in repo {
        if let Some(member) = members.iter().find(|member| member.dir == rel) {
            for target in &member.targets {
                write(&export.join(&target.root), b"")?;
            }
        } else if let Some(package) = manifest::package(snapshot, &rel, &table)? {
            for target in package.targets {
                write(&export.join(&target.root), b"")?;
            }
        }
        mirror_paths(
            &mut table,
            &workdir.join(&rel),
            workdir,
            &export,
            &external,
            &claimed,
        )?;
        if rel == workspace {
            table
                .entry("workspace")
                .or_insert_with(|| Value::Table(Table::new()));
        }
        let dir = export.join(&rel);
        write(&dir.join("Cargo.toml"), table.to_string().as_bytes())?;
        sources.insert(dir, Source::Repo(snapshot, rel));
    }
    for (source, mut table) in disk_tables {
        let (owner, rel) = &claimed[&source];
        let destination = external.join(hex(owner, &source)?).join(rel);
        mirror_paths(&mut table, &source, workdir, &export, &external, &claimed)?;
        write(
            &destination.join("Cargo.toml"),
            table.to_string().as_bytes(),
        )?;
        sources.insert(destination, Source::Disk(source));
    }
    for (source, members) in roots {
        let mirror = external.join(hex(&source, &source)?);
        for member in members {
            for target in member.targets {
                write(&mirror.join(&target.root), b"")?;
            }
        }
    }
    let lock = join(workspace, "Cargo.lock");
    if let Some(id) = snapshot.blob(&lock)? {
        write(&root.join("Cargo.lock"), &snapshot.read(&lock, id)?)?;
    }
    Ok((root.join("Cargo.toml"), sources))
}

/// Reads the manifest of the repository directory `dir`, queuing it when new and present.
fn queue_repo(
    dir: String,
    snapshot: &dyn Snapshot,
    manifests: &mut Manifests,
    seen: &mut HashSet<String>,
    repo: &mut BTreeMap<String, Table>,
    queue: &mut VecDeque<Local>,
) -> Result<(), Error> {
    if !seen.insert(dir.clone()) {
        return Ok(());
    }
    let Some(table) = manifest::read_manifest(snapshot, &dir, manifests)? else {
        return Ok(());
    };
    let owns_workspace = table.contains_key("workspace");
    repo.insert(dir.clone(), table);
    if !owns_workspace {
        let mut ancestor = dir.as_str();
        while !ancestor.is_empty() {
            ancestor = ancestor.rsplit_once('/').map_or("", |(parent, _)| parent);
            if manifest::read_manifest(snapshot, ancestor, manifests)?
                .is_some_and(|table| table.contains_key("workspace"))
            {
                queue_repo(ancestor.to_owned(), snapshot, manifests, seen, repo, queue)?;
                break;
            }
        }
    }
    queue.push_back(Local::Repo(dir));
    Ok(())
}

/// Queues each external workspace once and records its original member locations.
fn queue_external(
    dir: &Path,
    manifests: &mut Manifests,
    claimed: &mut HashMap<PathBuf, (PathBuf, String)>,
    roots: &mut BTreeMap<PathBuf, Vec<Member>>,
    queue: &mut VecDeque<Local>,
) -> Result<(), Error> {
    if claimed.contains_key(dir) {
        return Ok(());
    }
    let (root, members) = external_members(dir, manifests)?;
    for member in &members {
        let abs = root.join(&member.dir);
        claimed.insert(abs.clone(), (root.clone(), member.dir.clone()));
        queue.push_back(Local::Disk(abs));
    }
    if !members.iter().any(|member| member.dir.is_empty()) {
        claimed.insert(root.clone(), (root.clone(), String::new()));
        queue.push_back(Local::Disk(root.clone()));
    }
    roots.insert(root, members);
    Ok(())
}

/// The workspace root of the external package at `dir` and its members, `dir` itself when it
/// owns a `[workspace]` table or no `[workspace]` ancestor lists it as a member.
fn external_members(
    dir: &Path,
    manifests: &mut Manifests,
) -> Result<(PathBuf, Vec<Member>), Error> {
    let read = |dir: &Path, manifests: &mut Manifests| {
        let disk = WorkTreeSnapshot::new(dir.to_path_buf(), Vec::new());
        manifest::read_manifest(&disk, "", manifests)
    };
    let Some(table) = read(dir, manifests)? else {
        return Ok((dir.to_path_buf(), Vec::new()));
    };
    if !table.contains_key("workspace") {
        for ancestor in dir.ancestors().skip(1) {
            let Some(table) = read(ancestor, manifests)? else {
                continue;
            };
            if !table.contains_key("workspace") {
                continue;
            }
            let disk = WorkTreeSnapshot::new(ancestor.to_path_buf(), Vec::new());
            let members = manifest::workspace(&disk, "", manifests)?;
            let rel = relative(ancestor, dir).unwrap_or_default();
            if members.iter().any(|member| member.dir == rel) {
                return Ok((ancestor.to_path_buf(), members));
            }
            break;
        }
    }
    let root = dir.to_path_buf();
    let disk = WorkTreeSnapshot::new(root.clone(), Vec::new());
    Ok((root, manifest::workspace(&disk, "", manifests)?))
}

/// Visits dependency paths and explicit workspace owners, including target and override tables.
fn each_path(
    manifest: &mut Table,
    visit: &mut impl FnMut(&mut String) -> Result<(), Error>,
) -> Result<(), Error> {
    if let Some(Value::Table(package)) = manifest.get_mut("package")
        && let Some(Value::String(path)) = package.get_mut("workspace")
    {
        visit(path)?;
    }
    let mut visit_specs = |specs: &mut Table| -> Result<(), Error> {
        for (_, spec) in specs.iter_mut() {
            if let Value::Table(spec) = spec
                && let Some(Value::String(path)) = spec.get_mut("path")
            {
                visit(path)?;
            }
        }
        Ok(())
    };
    for kind in KINDS {
        if let Some(Value::Table(specs)) = manifest.get_mut(kind) {
            visit_specs(specs)?;
        }
    }
    if let Some(Value::Table(targets)) = manifest.get_mut("target") {
        for (_, platform) in targets.iter_mut() {
            if let Value::Table(platform) = platform {
                for kind in KINDS {
                    if let Some(Value::Table(specs)) = platform.get_mut(kind) {
                        visit_specs(specs)?;
                    }
                }
            }
        }
    }
    if let Some(Value::Table(workspace)) = manifest.get_mut("workspace")
        && let Some(Value::Table(specs)) = workspace.get_mut("dependencies")
    {
        visit_specs(specs)?;
    }
    if let Some(Value::Table(patches)) = manifest.get_mut("patch") {
        for (_, specs) in patches.iter_mut() {
            if let Value::Table(specs) = specs {
                visit_specs(specs)?;
            }
        }
    }
    if let Some(Value::Table(specs)) = manifest.get_mut("replace") {
        visit_specs(specs)?;
    }
    Ok(())
}

/// Rewrites source paths to their settled mirror locations.
fn mirror_paths(
    manifest: &mut Table,
    base: &Path,
    workdir: &Path,
    export: &Path,
    external: &Path,
    claimed: &HashMap<PathBuf, (PathBuf, String)>,
) -> Result<(), Error> {
    each_path(manifest, &mut |path| {
        let resolved = normalize(base, path);
        let destination = if let Some(rel) = relative(workdir, &resolved) {
            export.join(rel)
        } else {
            let (root, rel) = &claimed[&resolved];
            external.join(hex(root, &resolved)?).join(rel)
        };
        *path = destination.to_string_lossy().into_owned();
        Ok(())
    })
}

/// The hex SHA-1 of `path`, naming a mirror directory.
fn hex(path: &Path, context: &Path) -> Result<String, Error> {
    let id = gix::objs::compute_hash(
        gix::hash::Kind::Sha1,
        gix::objs::Kind::Blob,
        path.to_string_lossy().as_bytes(),
    )
    .map_err(|source| Error::Hash {
        path: context.display().to_string(),
        source,
    })?;
    Ok(id.to_string())
}

/// Resolves a path against its manifest directory, clamping parent traversal at the filesystem root.
fn normalize(base: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    let mut out = if path.is_absolute() {
        PathBuf::new()
    } else {
        base.to_path_buf()
    };
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            component => out.push(component.as_os_str()),
        }
    }
    out
}

fn write(path: &Path, contents: &[u8]) -> Result<(), Error> {
    let io = |source| Error::Io {
        path: path.to_path_buf(),
        source,
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    std::fs::write(path, contents).map_err(io)
}
