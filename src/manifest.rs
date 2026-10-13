//! Workspace members and their compilation targets, read from `Cargo.toml` files of a snapshot.

use toml::Table;

use crate::Error;
use crate::git::Snapshot;

/// One compilation target: a crate root file and the name its paths start with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    /// The crate name as written in paths, `-` replaced by `_`.
    pub(crate) crate_name: String,
    /// The root file, from the repository root.
    pub(crate) root: String,
    /// Whether this is a library target.
    pub(crate) lib: bool,
}

/// The targets of every workspace member of the workspace whose root manifest sits in `dir`.
///
/// A missing root manifest yields no targets. A manifest that is not valid TOML is an error.
pub(crate) fn targets(snapshot: &dyn Snapshot, dir: &str) -> Result<Vec<Target>, Error> {
    let Some(root) = read_manifest(snapshot, dir)? else {
        return Ok(Vec::new());
    };
    let mut members = Vec::new();
    if root.contains_key("package") {
        members.push(dir.to_owned());
    }
    if let Some(workspace) = root.get("workspace").and_then(toml::Value::as_table) {
        let excluded: Vec<String> = strings(workspace.get("exclude"))
            .into_iter()
            .map(|e| join(dir, &e))
            .collect();
        for pattern in strings(workspace.get("members")) {
            for member in expand(snapshot, dir, &pattern)? {
                if !excluded.contains(&member) && !members.contains(&member) {
                    members.push(member);
                }
            }
        }
    }
    let mut targets = Vec::new();
    for member in &members {
        if member == dir {
            package_targets(snapshot, member, &root, &mut targets)?;
        } else if let Some(manifest) = read_manifest(snapshot, member)? {
            package_targets(snapshot, member, &manifest, &mut targets)?;
        }
    }
    Ok(targets)
}

fn read_manifest(snapshot: &dyn Snapshot, dir: &str) -> Result<Option<Table>, Error> {
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

fn package_targets(
    snapshot: &dyn Snapshot,
    dir: &str,
    manifest: &Table,
    out: &mut Vec<Target>,
) -> Result<(), Error> {
    let Some(package) = manifest.get("package").and_then(toml::Value::as_table) else {
        return Ok(());
    };
    let Some(name) = package.get("name").and_then(toml::Value::as_str) else {
        return Ok(());
    };
    let auto = |key: &str| package.get(key).and_then(toml::Value::as_bool) != Some(false);
    let mut push = |crate_name: &str, root: String, lib: bool| {
        if !out.iter().any(|t| t.root == root) {
            out.push(Target {
                crate_name: crate_name.replace('-', "_"),
                root,
                lib,
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
        Some(path) => push(lib_name, join(dir, path), true),
        None if auto("autolib") => {
            let path = join(dir, "src/lib.rs");
            if snapshot.blob(&path)?.is_some() {
                push(lib_name, path, true);
            }
        }
        None => {}
    }

    let kinds = [
        ("bin", "autobins", "src/bin"),
        ("example", "autoexamples", "examples"),
        ("test", "autotests", "tests"),
        ("bench", "autobenches", "benches"),
    ];
    for (key, auto_key, default_dir) in kinds {
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
                push(target_name, path, false);
            }
        }
        if auto(auto_key) {
            if key == "bin" {
                let main = join(dir, "src/main.rs");
                if snapshot.blob(&main)?.is_some() {
                    push(name, main, false);
                }
            }
            for (target_name, path) in discovered(snapshot, &join(dir, default_dir))? {
                push(&target_name, path, false);
            }
        }
    }

    match package.get("build") {
        Some(toml::Value::String(path)) => push("build_script_build", join(dir, path), false),
        Some(toml::Value::Boolean(false)) => {}
        _ => {
            let path = join(dir, "build.rs");
            if snapshot.blob(&path)?.is_some() {
                push("build_script_build", path, false);
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
