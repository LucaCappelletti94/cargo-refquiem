//! Workspace members, their compilation targets and their manifest entries, read from the
//! `Cargo.toml` files of a snapshot.

use toml::Table;

use crate::Error;
use crate::git::Snapshot;

/// What a compilation target builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetKind {
    Lib,
    Bin,
    Example,
    Other,
}

/// One compilation target: a crate root file and the name its paths start with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    /// The target name as declared or discovered.
    pub(crate) name: String,
    /// The crate name as written in paths, `-` replaced by `_`.
    pub(crate) crate_name: String,
    /// The root file, from the repository root.
    pub(crate) root: String,
    pub(crate) kind: TargetKind,
}

/// A dependency's manifest key and explicit package identity.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Dependency {
    pub(crate) key: String,
    /// An explicit `package` also selects the dependency key as its crate name.
    pub(crate) package: Option<String>,
    /// The directory of a `path` dependency, from the repository root.
    pub(crate) path: Option<String>,
    /// The specification as a table, workspace inheritance applied.
    pub(crate) spec: Table,
    /// The tables it sits in, as [`NORMAL`], [`DEV`] and [`BUILD`] bits.
    pub(crate) tables: u8,
}

/// The bit of `[dependencies]` in [`Dependency::tables`].
pub(crate) const NORMAL: u8 = 1;
/// The bit of `[dev-dependencies]`.
pub(crate) const DEV: u8 = 2;
/// The bit of `[build-dependencies]`.
pub(crate) const BUILD: u8 = 4;

/// One workspace member package.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Member {
    /// The member directory, from the repository root.
    pub(crate) dir: String,
    pub(crate) package: String,
    pub(crate) dependencies: Vec<Dependency>,
    /// Declared features and the implicit features of optional dependencies.
    pub(crate) features: Vec<String>,
    pub(crate) targets: Vec<Target>,
}

/// Every member of the workspace whose root manifest sits in `dir`.
///
/// A missing root manifest yields no members. A manifest that is not valid TOML is an error.
pub(crate) fn workspace(snapshot: &dyn Snapshot, dir: &str) -> Result<Vec<Member>, Error> {
    let Some(root) = read_manifest(snapshot, dir)? else {
        return Ok(Vec::new());
    };
    let workspace = root.get("workspace").and_then(toml::Value::as_table);
    let mut dirs = Vec::new();
    if root.contains_key("package") {
        dirs.push(dir.to_owned());
    }
    if let Some(workspace) = workspace {
        let excluded: Vec<String> = strings(workspace.get("exclude"))
            .into_iter()
            .map(|e| join(dir, &e))
            .collect();
        for pattern in strings(workspace.get("members")) {
            for member in expand(snapshot, dir, &pattern)? {
                if !excluded.contains(&member) && !dirs.contains(&member) {
                    dirs.push(member);
                }
            }
        }
    }
    let inherited = workspace
        .and_then(|w| w.get("dependencies"))
        .and_then(toml::Value::as_table);
    let mut members = Vec::new();
    for member_dir in &dirs {
        let member = if member_dir == dir {
            read_member(snapshot, member_dir, &root, (dir, inherited))?
        } else if let Some(manifest) = read_manifest(snapshot, member_dir)? {
            read_member(snapshot, member_dir, &manifest, (dir, inherited))?
        } else {
            None
        };
        members.extend(member);
    }
    Ok(members)
}

/// The manifest in `dir`, `None` when there is none.
pub(crate) fn read_manifest(snapshot: &dyn Snapshot, dir: &str) -> Result<Option<Table>, Error> {
    let path = join(dir, "Cargo.toml");
    let Some(id) = snapshot.blob(&path)? else {
        return Ok(None);
    };
    let bytes = snapshot.read(&path, id)?;
    let text = String::from_utf8_lossy(&bytes);
    text.parse::<Table>()
        .map(Some)
        .map_err(|source| Error::Manifest {
            path,
            source: Box::new(source),
        })
}

fn read_member(
    snapshot: &dyn Snapshot,
    dir: &str,
    manifest: &Table,
    inherited: Inherited<'_>,
) -> Result<Option<Member>, Error> {
    let Some(package) = manifest.get("package").and_then(toml::Value::as_table) else {
        return Ok(None);
    };
    let Some(name) = package.get("name").and_then(toml::Value::as_str) else {
        return Ok(None);
    };
    let mut targets = Vec::new();
    package_targets(snapshot, dir, manifest, package, name, &mut targets)?;
    let (dependencies, optional) = dependencies(manifest, dir, inherited);
    Ok(Some(Member {
        dir: dir.to_owned(),
        package: name.to_owned(),
        features: features(manifest, &optional),
        dependencies,
        targets,
    }))
}

/// The workspace directory and its `[workspace.dependencies]` table.
type Inherited<'a> = (&'a str, Option<&'a Table>);

/// Every dependency of every kind and platform of the manifest in `dir`, and the keys of the
/// optional ones.
fn dependencies(
    manifest: &Table,
    dir: &str,
    (workspace_dir, inherited): Inherited<'_>,
) -> (Vec<Dependency>, Vec<String>) {
    const KINDS: [(&str, u8); 5] = [
        ("dependencies", NORMAL),
        ("dev-dependencies", DEV),
        ("dev_dependencies", DEV),
        ("build-dependencies", BUILD),
        ("build_dependencies", BUILD),
    ];
    let mut tables: Vec<(u8, &Table)> = KINDS
        .iter()
        .filter_map(|&(kind, bit)| Some((bit, manifest.get(kind)?.as_table()?)))
        .collect();
    for platform in manifest
        .get("target")
        .and_then(toml::Value::as_table)
        .into_iter()
        .flat_map(Table::values)
        .filter_map(toml::Value::as_table)
    {
        tables.extend(
            KINDS
                .iter()
                .filter_map(|&(kind, bit)| Some((bit, platform.get(kind)?.as_table()?))),
        );
    }
    let mut dependencies: Vec<Dependency> = Vec::new();
    let mut optional = Vec::new();
    for (bit, table) in tables {
        for (key, own) in table {
            if let Some(known) = dependencies.iter_mut().find(|d| d.key == *key) {
                known.tables |= bit;
            }
            let own = as_spec(own);
            let inherits = own.get("workspace").and_then(toml::Value::as_bool) == Some(true);
            let (spec_dir, mut spec) = match inherited.and_then(|w| w.get(key)) {
                Some(shared) if inherits => (workspace_dir, as_spec(shared)),
                _ => (dir, own.clone()),
            };
            if inherits {
                for (name, value) in own {
                    if name != "workspace" {
                        spec.entry(name).or_insert(value);
                    }
                }
            }
            let package = spec
                .get("package")
                .and_then(toml::Value::as_str)
                .map(str::to_owned);
            let path = spec
                .get("path")
                .and_then(toml::Value::as_str)
                .map(|path| join(spec_dir, path));
            let is_optional = spec.get("optional").and_then(toml::Value::as_bool) == Some(true);
            if is_optional && !optional.contains(key) {
                optional.push(key.clone());
            }
            if !dependencies.iter().any(|d| d.key == *key) {
                dependencies.push(Dependency {
                    key: key.clone(),
                    package,
                    path,
                    spec,
                    tables: bit,
                });
            }
        }
    }
    (dependencies, optional)
}

/// A dependency specification as a table, a bare string being its version requirement.
fn as_spec(value: &toml::Value) -> Table {
    match value {
        toml::Value::Table(table) => table.clone(),
        other => Table::from_iter([("version".to_owned(), other.clone())]),
    }
}

/// The `[features]` keys and the implicit features of optional dependencies no `dep:` hides.
fn features(manifest: &Table, optional: &[String]) -> Vec<String> {
    let table = manifest.get("features").and_then(toml::Value::as_table);
    let mut features: Vec<String> = table.into_iter().flat_map(|t| t.keys().cloned()).collect();
    let hidden: Vec<&str> = table
        .into_iter()
        .flat_map(Table::values)
        .filter_map(toml::Value::as_array)
        .flatten()
        .filter_map(toml::Value::as_str)
        .filter_map(|value| value.strip_prefix("dep:"))
        .collect();
    for key in optional {
        if !hidden.contains(&key.as_str()) && !features.contains(key) {
            features.push(key.clone());
        }
    }
    features
}

fn package_targets(
    snapshot: &dyn Snapshot,
    dir: &str,
    manifest: &Table,
    package: &Table,
    name: &str,
    out: &mut Vec<Target>,
) -> Result<(), Error> {
    let auto = |key: &str| package.get(key).and_then(toml::Value::as_bool) != Some(false);
    let mut push = |target_name: &str, root: String, kind: TargetKind| {
        if !out.iter().any(|t| t.root == root) {
            out.push(Target {
                name: target_name.to_owned(),
                crate_name: target_name.replace('-', "_"),
                root,
                kind,
            });
        }
    };

    let lib = manifest.get("lib").and_then(toml::Value::as_table);
    let lib_name = lib
        .and_then(|lib| lib.get("name"))
        .and_then(toml::Value::as_str)
        .unwrap_or(name);
    match lib
        .and_then(|lib| lib.get("path"))
        .and_then(toml::Value::as_str)
    {
        Some(path) => push(lib_name, join(dir, path), TargetKind::Lib),
        None if auto("autolib") => {
            let path = join(dir, "src/lib.rs");
            if snapshot.blob(&path)?.is_some() {
                push(lib_name, path, TargetKind::Lib);
            }
        }
        None => {}
    }

    let kinds = [
        ("bin", "autobins", "src/bin", TargetKind::Bin),
        ("example", "autoexamples", "examples", TargetKind::Example),
        ("test", "autotests", "tests", TargetKind::Other),
        ("bench", "autobenches", "benches", TargetKind::Other),
    ];
    for (key, auto_key, default_dir, kind) in kinds {
        for table in manifest
            .get(key)
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(toml::Value::as_table)
        {
            let Some(target_name) = table.get("name").and_then(toml::Value::as_str) else {
                continue;
            };
            let path = match table.get("path").and_then(toml::Value::as_str) {
                Some(path) => Some(join(dir, path)),
                None => default_target_path(snapshot, dir, key, default_dir, target_name, name)?,
            };
            if let Some(path) = path {
                push(target_name, path, kind);
            }
        }
        if auto(auto_key) {
            if key == "bin" {
                let main = join(dir, "src/main.rs");
                if snapshot.blob(&main)?.is_some() {
                    push(name, main, kind);
                }
            }
            for (target_name, path) in discovered(snapshot, &join(dir, default_dir))? {
                push(&target_name, path, kind);
            }
        }
    }

    match package.get("build") {
        Some(toml::Value::String(path)) => {
            push("build_script_build", join(dir, path), TargetKind::Other);
        }
        Some(toml::Value::Boolean(false)) => {}
        _ => {
            let path = join(dir, "build.rs");
            if snapshot.blob(&path)?.is_some() {
                push("build_script_build", path, TargetKind::Other);
            }
        }
    }
    Ok(())
}

fn default_target_path(
    snapshot: &dyn Snapshot,
    dir: &str,
    key: &str,
    default_dir: &str,
    target_name: &str,
    package_name: &str,
) -> Result<Option<String>, Error> {
    let mut candidates = vec![
        join(dir, &format!("{default_dir}/{target_name}.rs")),
        join(dir, &format!("{default_dir}/{target_name}/main.rs")),
    ];
    if key == "bin" && target_name == package_name {
        candidates.push(join(dir, "src/main.rs"));
    }
    for candidate in candidates {
        if snapshot.blob(&candidate)?.is_some() {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

/// `name.rs` and `name/main.rs` targets directly under `dir`.
fn discovered(snapshot: &dyn Snapshot, dir: &str) -> Result<Vec<(String, String)>, Error> {
    let mut found = Vec::new();
    for (entry, is_dir) in snapshot.list_dir(dir)? {
        if is_dir {
            let main = format!("{dir}/{entry}/main.rs");
            if snapshot.blob(&main)?.is_some() {
                found.push((entry, main));
            }
        } else if let Some(stem) = entry.strip_suffix(".rs") {
            found.push((stem.to_owned(), format!("{dir}/{entry}")));
        }
    }
    found.sort();
    Ok(found)
}

/// Member directories matching `pattern`, relative to the workspace directory `dir`.
fn expand(snapshot: &dyn Snapshot, dir: &str, pattern: &str) -> Result<Vec<String>, Error> {
    let mut current = vec![dir.to_owned()];
    for segment in pattern.split('/').filter(|s| !s.is_empty() && *s != ".") {
        let mut next = Vec::new();
        for base in &current {
            if segment == "**" {
                next.extend(descendants(snapshot, base)?);
            } else if segment.contains(['*', '?', '[']) {
                for (entry, is_dir) in snapshot.list_dir(base)? {
                    if is_dir && wildcard(segment, &entry) {
                        next.push(join(base, &entry));
                    }
                }
            } else {
                next.push(join(base, segment));
            }
        }
        current = next;
    }
    let mut members = Vec::new();
    for member in current {
        if snapshot.blob(&join(&member, "Cargo.toml"))?.is_some() {
            members.push(member);
        }
    }
    members.sort();
    Ok(members)
}

fn descendants(snapshot: &dyn Snapshot, base: &str) -> Result<Vec<String>, Error> {
    let mut out = vec![base.to_owned()];
    let mut i = 0;
    while i < out.len() {
        let dir = out[i].clone();
        for (entry, is_dir) in snapshot.list_dir(&dir)? {
            if is_dir && entry != ".git" && entry != "target" {
                out.push(join(&dir, &entry));
            }
        }
        i += 1;
    }
    Ok(out)
}

/// Shell-style matching of one path segment with `*`, `?` and `[...]` classes.
fn wildcard(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    matches(&pattern, &name)
}

fn matches(pattern: &[char], name: &[char]) -> bool {
    match pattern.first() {
        None => name.is_empty(),
        Some('*') => (0..=name.len()).any(|skip| matches(&pattern[1..], &name[skip..])),
        Some('?') => !name.is_empty() && matches(&pattern[1..], &name[1..]),
        Some('[') => {
            let Some(close) = pattern.iter().position(|&c| c == ']') else {
                return name.first() == Some(&'[') && matches(&pattern[1..], &name[1..]);
            };
            let Some(&c) = name.first() else {
                return false;
            };
            let class = &pattern[1..close];
            let (negated, class) = match class.first() {
                Some('!' | '^') => (true, &class[1..]),
                _ => (false, class),
            };
            let mut hit = false;
            let mut i = 0;
            while i < class.len() {
                if i + 2 < class.len() && class[i + 1] == '-' {
                    hit |= class[i] <= c && c <= class[i + 2];
                    i += 3;
                } else {
                    hit |= class[i] == c;
                    i += 1;
                }
            }
            hit != negated && matches(&pattern[close + 1..], &name[1..])
        }
        Some(&literal) => name.first() == Some(&literal) && matches(&pattern[1..], &name[1..]),
    }
}

fn strings(value: Option<&toml::Value>) -> Vec<String> {
    value
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// `dir/rest` with `.` and `..` segments resolved, `rest` alone when `dir` is empty.
pub(crate) fn join(dir: &str, rest: &str) -> String {
    let mut parts: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
    for segment in rest.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            segment => parts.push(segment),
        }
    }
    parts.join("/")
}
