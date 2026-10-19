//! The public items of head's dependencies and of the dependencies the range removed, located
//! through `cargo metadata` on manifests refquiem writes, so the user's tree is never touched.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use cargo_metadata::{CargoOpt, Metadata, MetadataCommand, PackageId};
use gix::ObjectId;
use toml::{Table, Value};

use crate::git::{Snapshot, TreeSnapshot, WorkTreeSnapshot, relative};
use crate::index::Indexer;
use crate::intern::{PathId, Sym};
use crate::manifest::{self, Dependency, Member, TargetKind, join};
use crate::{Error, Skipped};

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

/// The layer of head's dependencies.
pub(crate) fn head(
    indexer: &mut Indexer,
    snapshot: &dyn Snapshot,
    scratch: &Scratch<'_>,
) -> Result<Layer, Error> {
    let members = manifest::workspace(snapshot, scratch.workspace)?;
    if members.iter().all(|member| member.dependencies.is_empty()) {
        return Ok(Layer::default());
    }
    let export = scratch.dir.join("metadata");
    export_workspace(snapshot, &members, &export, scratch)?;
    let metadata = metadata(&export.join("Cargo.toml"), scratch.workspace_dir)?;
    let mut roots = Vec::new();
    for node in metadata.resolve.iter().flat_map(|resolve| &resolve.nodes) {
        if metadata.workspace_members.contains(&node.id) {
            roots.extend(
                node.deps
                    .iter()
                    .filter(|dep| !metadata.workspace_members.contains(&dep.pkg))
                    .map(|dep| (dep.pkg.clone(), dep.name.clone())),
            );
        }
    }
    libraries(indexer, &metadata, roots)
}

/// The public paths of the `removals` that neither set of `alive` paths has. A removal whose
/// sources cargo cannot resolve is skipped.
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
    for removal in removals {
        let tree = TreeSnapshot::new(repo, removal.tree);
        let Ok(members) = manifest::workspace(&tree, scratch.workspace) else {
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
            None => fetched(indexer, &tree, member, dependency, scratch).map(|layer| layer.paths),
        };
        let reason = match paths {
            Ok(paths) => {
                for path in paths {
                    let at_head = alive.iter().any(|paths| paths.contains(&path));
                    if indexer.interner.segments(path).len() >= 2 && !at_head {
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

/// The public paths of the `dependency` on the package in `dir` of `tree`, rooted at the name
/// code spells it by, which is the key when it renames the package and the library name
/// otherwise.
fn path_library(
    indexer: &mut Indexer,
    tree: &TreeSnapshot<'_>,
    dir: &str,
    dependency: &Dependency,
) -> Result<HashSet<PathId>, Error> {
    let lib = manifest::workspace(tree, dir)?
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
/// `Cargo.lock` of `tree` records.
fn fetched(
    indexer: &mut Indexer,
    tree: &TreeSnapshot<'_>,
    member: &Member,
    dependency: &Dependency,
    scratch: &Scratch<'_>,
) -> Result<Layer, Error> {
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
    libraries(
        indexer,
        &metadata,
        vec![(root.pkg.clone(), root.name.clone())],
    )
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
/// the libraries they re-export from indexed to resolve those re-exports.
fn libraries(
    indexer: &mut Indexer,
    metadata: &Metadata,
    roots: Vec<(PackageId, String)>,
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
    let mut queue = roots;
    let mut done = HashSet::new();
    let mut found = Vec::new();
    while let Some((id, name)) = queue.pop() {
        if !done.insert((id.clone(), name.clone())) {
            continue;
        }
        let Some(package) = packages.get(&id) else {
            continue;
        };
        let Some(target) = package
            .targets
            .iter()
            .find(|t| t.is_lib() || t.is_rlib() || t.is_dylib())
        else {
            continue;
        };
        let Some(dir) = package.manifest_path.parent() else {
            continue;
        };
        let Some(root) = relative(dir.as_std_path(), target.src_path.as_std_path()) else {
            continue;
        };
        let snapshot = WorkTreeSnapshot::new(dir.as_std_path().to_path_buf(), Vec::new());
        let library = indexer.library(&snapshot, &root, &name)?;
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
    Ok(Layer {
        macro_idents: found
            .iter()
            .flat_map(|library| library.macro_idents.iter().copied())
            .collect(),
        paths: indexer.public_paths(&found, &root_syms),
    })
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

/// Writes the root and member manifests, `Cargo.lock` and empty target roots of the workspace
/// in `snapshot` below `export`, with every `path` dependency outside them made absolute.
fn export_workspace(
    snapshot: &dyn Snapshot,
    members: &[Member],
    export: &Path,
    scratch: &Scratch<'_>,
) -> Result<(), Error> {
    if export.exists() {
        std::fs::remove_dir_all(export).map_err(|source| Error::Io {
            path: export.to_path_buf(),
            source,
        })?;
    }
    let workspace = scratch.workspace;
    let mut mirrored: Vec<&str> = vec![workspace];
    mirrored.extend(members.iter().map(|member| member.dir.as_str()));
    for (i, dir) in mirrored.iter().enumerate() {
        if i > 0 && *dir == workspace {
            continue;
        }
        let Some(mut table) = manifest::read_manifest(snapshot, dir)? else {
            continue;
        };
        each_dependency_table(&mut table, &mut |specs| {
            absolute_paths(specs, dir, &mirrored, scratch.workdir);
        });
        if i == 0 {
            table
                .entry("workspace")
                .or_insert_with(|| Value::Table(Table::new()));
        }
        let target = export.join(below(workspace, dir)).join("Cargo.toml");
        write(&target, table.to_string().as_bytes())?;
    }
    for target in members.iter().flat_map(|member| &member.targets) {
        write(&export.join(below(workspace, &target.root)), b"")?;
    }
    let lock = join(workspace, "Cargo.lock");
    if let Some(id) = snapshot.blob(&lock)? {
        write(&export.join("Cargo.lock"), &snapshot.read(&lock, id)?)?;
    }
    Ok(())
}

/// Calls `visit` on every table of dependency specifications in `manifest`.
fn each_dependency_table(manifest: &mut Table, visit: &mut impl FnMut(&mut Table)) {
    let kinds = |table: &mut Table, visit: &mut dyn FnMut(&mut Table)| {
        for kind in KINDS {
            if let Some(Value::Table(specs)) = table.get_mut(kind) {
                visit(specs);
            }
        }
    };
    kinds(manifest, visit);
    if let Some(Value::Table(targets)) = manifest.get_mut("target") {
        for (_, platform) in targets.iter_mut() {
            if let Value::Table(platform) = platform {
                kinds(platform, visit);
            }
        }
    }
    if let Some(Value::Table(workspace)) = manifest.get_mut("workspace")
        && let Some(Value::Table(specs)) = workspace.get_mut("dependencies")
    {
        visit(specs);
    }
    if let Some(Value::Table(patches)) = manifest.get_mut("patch") {
        for (_, specs) in patches.iter_mut() {
            if let Value::Table(specs) = specs {
                visit(specs);
            }
        }
    }
    if let Some(Value::Table(specs)) = manifest.get_mut("replace") {
        visit(specs);
    }
}

/// Makes absolute every relative `path` in `specs`, written in the manifest in `dir`, that
/// leaves the `mirrored` directories.
fn absolute_paths(specs: &mut Table, dir: &str, mirrored: &[&str], workdir: &Path) {
    for (_, spec) in specs.iter_mut() {
        if let Value::Table(spec) = spec
            && let Some(Value::String(path)) = spec.get_mut("path")
            && !Path::new(path.as_str()).is_absolute()
        {
            let resolved = join(dir, path);
            if !mirrored.contains(&resolved.as_str()) {
                *path = workdir.join(&resolved).to_string_lossy().into_owned();
            }
        }
    }
}

/// `path`, from the repository root, relative to the workspace directory `workspace`.
fn below<'p>(workspace: &str, path: &'p str) -> &'p str {
    if workspace.is_empty() {
        return path;
    }
    path.strip_prefix(workspace)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(if path == workspace { "" } else { path })
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
